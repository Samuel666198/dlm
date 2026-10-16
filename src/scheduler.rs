//! 分段调度器：**零 I/O 的纯状态机** —— "哪个 worker 下哪段字节"的唯一权威。
//!
//! 见 spec §2.1 / §2.2 / §7.1 与 plan 第 7 行。行为由下面的表驱动测试钉死。
//!
//! 三条规则：
//! 1. `Connected(w)`（worker `w` 手上有连接、等活）领活：优先派**无人认领的最长区间**；
//!    只有当某个在途 worker 的**剩余尾**更长时，才动它 —— 在剩余尾的中点劈开，上半截（含中点）
//!    交给新 worker（`Assign`），下半截把 incumbent 收紧（`Retighten`，携带该 worker 新的**完整**区间，
//!    worker 后续要按它改发**带显式终点**的请求）。
//! 2. 绝对下限：劈完任半截 < `min_seg` 就**不劈**。`min_seg` 由构造方注入（Task 8 传
//!    `consts::MIN_SEGMENT_BYTES`），本模块不读 `consts`，为的是边界能在任意尺寸上用表钉死。
//!    劈不动时不空等：退回去派无人认领的区间；没有 ⇒ `Idle`。
//! 3. `NoRanges`（spec §2.2：对端不认 Range，或首响应是 200）之后退化为单段顺序下载：
//!    不再有新段被派出，直到在途段全部交回；此后一次只派一段、永不劈分（不产生 `Retighten`）。
//!    已在途的段**不被打断** —— v1 不发 `Halt`，让它们按各自（已收紧的）终点自然收尾，
//!    所以退化点落在多段在途时，短暂会有"多段并行收尾"，但绝不会新增并行段。
//!    已写的字节不会被重下（剩余区间从已写偏移之后起算）。
//!
//! 记账不变量（Task 4 断点文件的底）：`done` 只收**已确认落盘**的字节 —— worker 交回区间时，
//! 已写前缀并进 `done`（相邻段自动合并、按 `start` 升序），未写部分回队列；
//! `pending_bytes()` = 队列区间 + 各在途区间的未写后缀，与 `done` 互不重叠。
//! ⇒ `is_done()` 为真时 `done` 必然整段覆盖文件。
//!
//! `Action::Halt` 是 Task 8 的收尾通道（Ctrl-C / 任务失败时停发新请求），本模块 v1 **不产出**它。
//! spec §6.4 的 416 由 Task 7 折成 `Outcome::Complete` ⇒ 到本模块就是 `WindowDone`；
//! 若该段一个字节都没写过，`WindowDone` 会把整段交回队列（不标记完成），
//! 所以"反复 416"的退避归 Task 8 的重试逻辑管，不在这里发明规则。

use std::cmp::Reverse;

/// 闭区间 `[start, end]`（两端都是绝对字节偏移）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub start: u64,
    pub end: u64,
}

impl Interval {
    /// 区间内的字节数；`end < start` 视为空区间返回 0。
    pub fn len(&self) -> u64 {
        if self.is_empty() { 0 } else { self.end - self.start + 1 }
    }
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }
}

/// 进：来自 worker 的事实。`Connected` 是"就绪等活"，其余都带 worker 序号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// worker 已握有一条可用连接，来领活。
    Connected(usize),
    /// 该 worker 已写到**绝对偏移** `abs_end`（含）。只前进、且被夹到它自己的区间内。
    Progress { worker: usize, abs_end: u64 },
    /// 该 worker 的窗口读满了（或整段下完了）：区间交回调度器。
    WindowDone { worker: usize },
    /// 该 worker 这一段失败/被掐断：未写的尾巴交回队列，已写的字节不丢。
    Failed { worker: usize },
    /// 对端不支持 Range ⇒ 整个任务退化为单段顺序下载（spec §2.2）。
    NoRanges,
}

/// 出：调度器对 worker 下的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 整段新区间交给该 worker（新 worker 接手，或空闲区间被派出去）。
    Assign { worker: usize, iv: Interval },
    /// 该 worker 的区间被收紧（终点提前）：它必须改用带显式终点的新请求。
    Retighten { worker: usize, iv: Interval },
    /// 这一轮没活可派。
    Idle,
    /// 让该 worker 停手（v1 不由调度器产出，留给 Task 8 的收尾）。
    Halt,
}

/// 一个 worker 手上压着的区间。`written` = 已确认落盘的最高绝对偏移，`None` = 一个字节都没写。
/// 用 `Option` 而不是 `start - 1` 当哨兵：`start == 0` 时 `start - 1` 会下溢（debug 直接 panic），
/// 而且会把"字节 0 还没写"和"字节 0 已写完"混成同一个值。
#[derive(Debug, Clone, Copy)]
struct Active {
    iv: Interval,
    written: Option<u64>,
}

impl Active {
    /// 还需要下载的尾巴；已写完 ⇒ `None`。
    fn remaining(&self) -> Option<Interval> {
        let start = match self.written {
            None => self.iv.start,
            Some(w) => (w + 1).max(self.iv.start),
        };
        if start > self.iv.end { None } else { Some(Interval { start, end: self.iv.end }) }
    }
    /// 已确认落盘的前缀；一个字节都没写 ⇒ `None`。
    fn completed(&self) -> Option<Interval> {
        let w = self.written?;
        if w < self.iv.start { None } else { Some(Interval { start: self.iv.start, end: w.min(self.iv.end) }) }
    }
}

#[derive(Debug)]
pub struct Scheduler {
    total: u64,
    min_seg: u64,
    /// 无人认领、待派发的区间。
    queue: Vec<Interval>,
    /// 下标 = worker 序号；`None` = 该 worker 手上没活。长度按 `Connected(w)` 的 w 增长 ——
    /// 简报里硬编码的 64 槽位会在 w ≥ 64 时 panic（`self.active[w] = ...` 越界），
    /// 改成按需增长：worker 序号由 Task 8 的线程池给出，本模块不该因为上游序号而崩。
    active: Vec<Option<Active>>,
    /// 已确认落盘的区间：按 `start` 升序、互不重叠且不相邻。
    done: Vec<Interval>,
    no_ranges: bool,
}

impl Scheduler {
    pub fn new(total: u64, min_seg: u64) -> Self {
        let queue = if total == 0 { vec![] } else { vec![Interval { start: 0, end: total - 1 }] };
        Self { total, min_seg, queue, active: vec![], done: vec![], no_ranges: false }
    }

    /// 没有任何在途工作、也没有待派工作。此时 `done` 必然覆盖整个文件。
    pub fn is_done(&self) -> bool {
        self.queue.is_empty() && self.active.iter().all(|a| a.is_none())
    }

    /// 还差多少字节：队列里的整段 + 各在途段没写完的尾巴。不含已确认字节。
    pub fn pending_bytes(&self) -> u64 {
        let queued = self.queue.iter().map(Interval::len).sum::<u64>();
        let in_flight = self.active.iter().flatten().filter_map(|a| a.remaining()).map(|iv| iv.len()).sum::<u64>();
        queued.saturating_add(in_flight)
    }

    /// 已完成区间（升序、相邻段已合并），供 `state` 落盘。
    pub fn snapshot_intervals(&self) -> Vec<Interval> {
        self.done.clone()
    }

    /// 唯一的状态迁移入口。`Connected` 至少返回一个动作（无事可做就是 `Idle`），其余事件返回空。
    pub fn on(&mut self, ev: Event) -> Vec<Action> {
        match ev {
            Event::NoRanges => {
                self.no_ranges = true;
                vec![]
            }
            Event::Progress { worker, abs_end } => {
                self.note_progress(worker, abs_end);
                vec![]
            }
            // 窗口读满与失败在记账上是同一件事：写完的进 done，没写完的回队列。
            Event::WindowDone { worker } | Event::Failed { worker } => {
                self.release(worker);
                vec![]
            }
            Event::Connected(worker) => self.dispatch(worker),
        }
    }

    /// 记下"写到哪了"：只前进、且夹到自己区间终点（越界上报会把在途尾算成负数）。
    fn note_progress(&mut self, worker: usize, abs_end: u64) {
        let Some(Some(a)) = self.active.get_mut(worker) else { return };
        let capped = abs_end.min(a.iv.end).min(self.total.saturating_sub(1));
        a.written = Some(a.written.map_or(capped, |prev| prev.max(capped)));
    }

    /// worker 交回它的区间。
    fn release(&mut self, worker: usize) {
        let Some(Some(a)) = self.active.get(worker).cloned() else { return };
        self.active[worker] = None;
        if let Some(prefix) = a.completed() {
            self.push_done(prefix);
        }
        if let Some(rest) = a.remaining() {
            self.push_pending(rest);
        }
    }

    /// 最长的无人认领区间；等长时取起点最低的（保证测试可复现、Task 8 的回放一致）。
    fn longest_pending(&self) -> Option<Interval> {
        self.queue.iter().min_by_key(|i| (Reverse(i.len()), i.start)).copied()
    }

    /// 最长的在途尾巴，连同它的 incumbent worker 序号。
    fn longest_tail(&self) -> Option<(usize, Interval)> {
        self.active
            .iter()
            .enumerate()
            .filter_map(|(w, a)| a.as_ref().and_then(Active::remaining).map(|tail| (w, tail)))
            .min_by_key(|(_, tail)| (Reverse(tail.len()), tail.start))
    }

    fn dispatch(&mut self, worker: usize) -> Vec<Action> {
        self.grow_slots(worker);
        // 同一个 worker 重复报就绪：它手上那段还有偏移，覆盖就等于丢掉已写字节 ⇒ 拒派。
        if self.active[worker].is_some() {
            return vec![Action::Idle];
        }
        if self.no_ranges {
            // 退化：一次只允许一段在途，且永不劈分（不产生 Retighten）。
            if self.active.iter().any(|a| a.is_some()) {
                return vec![Action::Idle];
            }
            return match self.longest_pending() {
                Some(iv) => self.assign_whole(worker, iv),
                None => vec![Action::Idle],
            };
        }
        let free = self.longest_pending();
        let free_len = free.map_or(0, |iv| iv.len());
        // 动已在途的请求要多发一轮 HTTP，只有在它确实更长时才值得。
        if let Some((incumbent, tail)) = self.longest_tail()
            && tail.len() > free_len
        {
            if let Some(acts) = self.split_tail(worker, incumbent, tail) {
                return acts;
            }
            // 劈不动：退回派无人认领的区间；没有才 Idle。
            if free.is_none() {
                return vec![Action::Idle];
            }
        }
        match free {
            Some(iv) => self.assign_whole(worker, iv),
            None => vec![Action::Idle],
        }
    }

    /// 在剩余尾的中点劈开。任半截低于 `min_seg`（或只剩一个字节）⇒ 返回 `None`，不劈。
    fn split_tail(&mut self, worker: usize, incumbent: usize, tail: Interval) -> Option<Vec<Action>> {
        let half = tail.len() / 2;
        if half == 0 || half < self.min_seg {
            return None;
        }
        let mid = tail.start + half;
        let upper = Interval { start: mid, end: tail.end };
        let head = {
            let a = self.active[incumbent].as_mut()?;
            a.iv.end = mid - 1;
            a.iv
        };
        self.active[worker] = Some(Active { iv: upper, written: None });
        // 顺序固定：先收紧 incumbent（它在途的请求要改终点），再派新段。
        Some(vec![Action::Retighten { worker: incumbent, iv: head }, Action::Assign { worker, iv: upper }])
    }

    fn assign_whole(&mut self, worker: usize, iv: Interval) -> Vec<Action> {
        self.take_pending(iv);
        self.active[worker] = Some(Active { iv, written: None });
        vec![Action::Assign { worker, iv }]
    }

    fn take_pending(&mut self, iv: Interval) {
        if let Some(pos) = self.queue.iter().position(|i| *i == iv) {
            self.queue.remove(pos);
        }
    }

    fn push_pending(&mut self, iv: Interval) {
        if iv.is_empty() {
            return;
        }
        // 同一起点已在队里 ⇒ 跳过：区间终点只会变短，重复入队会把一段活算成两段。
        if self.queue.iter().any(|i| i.start == iv.start) {
            return;
        }
        self.queue.push(iv);
    }

    /// 插入并合并相邻/重叠的已确认段，维持 `done` 升序且无相邻段。
    fn push_done(&mut self, iv: Interval) {
        if iv.is_empty() {
            return;
        }
        let mut merged = vec![iv];
        merged.extend(self.done.iter().copied());
        merged.sort_by_key(|i| i.start);
        self.done.clear();
        for cur in merged {
            match self.done.last_mut() {
                Some(last) if cur.start <= last.end.saturating_add(1) => last.end = last.end.max(cur.end),
                _ => self.done.push(cur),
            }
        }
    }

    fn grow_slots(&mut self, worker: usize) {
        if self.active.len() <= worker {
            self.active.resize(worker + 1, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> Scheduler { Scheduler::new(1000, 100) }   // total=1000, min_seg=100

    /// 记账不变量：`done` 里的段必须升序、互不重叠、互不相邻；
    /// `done` 与 `pending_bytes()` 不得重复计入同一字节；`is_done()` 为真时 `done` 整段覆盖文件。
    fn check_invariants(sc: &Scheduler, total: u64, label: &str) {
        let snap = sc.snapshot_intervals();
        let mut prev_end: Option<u64> = None;
        for iv in &snap {
            assert!(!iv.is_empty(), "{label}: done 里出现空区间 {snap:?}");
            if let Some(p) = prev_end {
                assert!(iv.start > p + 1, "{label}: done 段未排序或未合并相邻段 {snap:?}");
            }
            prev_end = Some(iv.end);
        }
        let done: u64 = snap.iter().map(Interval::len).sum();
        let pending = sc.pending_bytes();
        assert!(
            done.saturating_add(pending) <= total,
            "{label}: 已确认 {done} + 待下载 {pending} 超过全长 {total} ⇒ 有字节被重复计入"
        );
        if sc.is_done() {
            assert_eq!(pending, 0, "{label}: is_done() 为真却仍有待下载字节");
            assert_eq!(done, total, "{label}: is_done() 为真但 done 没有覆盖整个文件 {snap:?}");
        }
    }

    #[test]
    fn first_connection_gets_whole_file() {
        assert_eq!(s().on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: 999 } }]);
    }
    #[test]
    fn second_connection_splits_longest_at_midpoint() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        let acts = sc.on(Event::Connected(1));
        // 老段收紧到上半截之前，新连接从中点起
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { worker: 1, iv }
            if iv.start == 500 && iv.end == 999)), "{acts:?}");
        assert!(acts.iter().any(|a| matches!(a, Action::Retighten { worker: 0, iv }
            if iv.start == 0 && iv.end == 499)), "{acts:?}");
    }
    #[test]
    fn split_uses_remaining_part_of_active_segment() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Connected(1));               // 中点 500
        sc.on(Event::Progress { worker: 1, abs_end: 600 });
        let acts = sc.on(Event::Connected(2));    // 最大区间是 [601,999] 还是 [0,499]？取最长
        let assign = acts.iter().find_map(|a| if let Action::Assign { iv, .. } = a { Some(*iv) } else { None })
            .expect("应有新分配");
        // 简报原文写作 `499 - 0` 与 `... - ((...) / 2 + 1) % 1`：`x - 0` 触发 clippy::identity_op、
        // `% 1` 触发 deny-by-default 的 clippy::modulo_one，本仓库门禁是 `-D warnings`，
        // 所以把这两处恒等运算摘掉 —— 期望值仍是同一个数（(0+499)/2 + 1 = 250），断言方向没动。
        let longest = if 999 - 601 > 499 { Interval { start: 601, end: 999 } } else { Interval { start: 0, end: 499 } };
        assert_eq!(assign.start, (longest.start + longest.end) / 2 + 1);
        let mid = (longest.start + longest.end) / 2;
        assert!(assign.start == mid || assign.start == mid + 1, "{assign:?} mid={mid}");
    }
    #[test]
    fn stops_splitting_below_min_segment() {
        let mut sc = Scheduler::new(1000, 400);
        sc.on(Event::Connected(0));               // [0,999]
        let acts = sc.on(Event::Connected(1));    // 半长 500 >= 400 可以劈
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { .. })));
        let acts2 = sc.on(Event::Connected(2));   // 再劈会得到 <400 的段 ⇒ 不劈
        assert!(acts2.iter().all(|a| matches!(a, Action::Idle | Action::Halt)), "{acts2:?}");
    }
    #[test]
    fn window_done_returns_remainder_and_reassigns_without_new_connection() {
        let mut sc = s();
        sc.on(Event::Connected(0));               // [0,999]
        sc.on(Event::Progress { worker: 0, abs_end: 300 });
        sc.on(Event::WindowDone { worker: 0 });
        let acts = sc.on(Event::Connected(0));    // 同一 worker 复用，不再需要新连接
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { worker: 0, iv } if iv.start == 301)), "{acts:?}");
    }
    #[test]
    fn no_ranges_forces_single_segment() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::NoRanges);
        let acts = sc.on(Event::Connected(1));
        assert!(acts.iter().all(|a| matches!(a, Action::Idle)), "退化后不得再派第二段: {acts:?}");
    }
    #[test]
    fn failed_returns_interval_for_requeue() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 200 });
        sc.on(Event::Failed { worker: 0 });
        assert_eq!(sc.pending_bytes(), 799);
    }
    #[test]
    fn done_when_nothing_pending() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 999 });
        sc.on(Event::WindowDone { worker: 0 });
        assert!(sc.is_done());
    }

    // ---- 以下钉的是"记账"与"劈不动时别空等"，表里没写但 Task 4/8 依赖它们 ----

    #[test]
    fn done_snapshot_tracks_every_released_byte() {
        const TOTAL: u64 = 1000;
        let mut sc = Scheduler::new(TOTAL, 100);
        let step = |sc: &mut Scheduler, ev: Event, label: &str| {
            sc.on(ev);
            check_invariants(sc, TOTAL, label);
        };
        step(&mut sc, Event::Connected(0), "C0");                 // [0,999]
        step(&mut sc, Event::Connected(1), "C1");                 // ⇒ [0,499] + [500,999]
        step(&mut sc, Event::Connected(2), "C2");                 // [0,499] 最长 ⇒ [0,249] + [250,499]
        step(&mut sc, Event::Progress { worker: 0, abs_end: 249 }, "P0");
        step(&mut sc, Event::WindowDone { worker: 0 }, "WD0");    // [0,249] 整段确认
        step(&mut sc, Event::Progress { worker: 2, abs_end: 300 }, "P2");
        step(&mut sc, Event::Failed { worker: 2 }, "F2");         // 已写的 [250,300] 进 done，[301,499] 回队列
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 300 }], "相邻的已确认段必须合并");
        assert_eq!(sc.pending_bytes(), 199 + 500, "回队列 199 + 在途 [500,999] 500");
        step(&mut sc, Event::Connected(3), "C3");                 // 在途尾 [500,999] 比空闲 [301,499] 长 ⇒ 劈后者
        step(&mut sc, Event::Progress { worker: 3, abs_end: 999 }, "P3");
        step(&mut sc, Event::WindowDone { worker: 3 }, "WD3");
        step(&mut sc, Event::Progress { worker: 1, abs_end: 749 }, "P1");
        step(&mut sc, Event::WindowDone { worker: 1 }, "WD1");
        step(&mut sc, Event::Connected(4), "C4");                 // 接手无人认领的 [301,499]
        step(&mut sc, Event::Progress { worker: 4, abs_end: 499 }, "P4");
        step(&mut sc, Event::WindowDone { worker: 4 }, "WD4");
        assert!(sc.is_done());
        assert_eq!(sc.pending_bytes(), 0);
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }], "全部落盘后 done 必须整段覆盖文件");
    }

    #[test]
    fn no_ranges_never_splits_and_never_runs_two_segments() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 400 });
        sc.on(Event::NoRanges);
        for w in 1..5 {
            assert_eq!(sc.on(Event::Connected(w)), vec![Action::Idle], "退化期间 w{w} 不得领到第二段");
        }
        sc.on(Event::WindowDone { worker: 0 });                   // 交回剩余 [401,999]
        let acts = sc.on(Event::Connected(1));
        assert_eq!(acts, vec![Action::Assign { worker: 1, iv: Interval { start: 401, end: 999 } }],
            "退化后一次只派一段、整段交出剩余区间：不劈、不重下已写的 0..400");
        assert_eq!(sc.on(Event::Connected(2)), vec![Action::Idle], "已有一段在途时不得再派");
        assert_eq!(sc.pending_bytes(), 599);
        check_invariants(&sc, 1000, "noranges");
    }

    #[test]
    fn queued_work_is_dispatched_when_longest_tail_is_too_short_to_split() {
        let mut sc = Scheduler::new(1000, 400);
        sc.on(Event::Connected(0));                               // [0,999]
        sc.on(Event::Connected(1));                               // ⇒ [0,499] + [500,999]
        assert_eq!(sc.on(Event::Connected(2)), vec![Action::Idle], "250 < 400 ⇒ 不劈，且无空闲区间可派");
        sc.on(Event::Failed { worker: 1 });                       // [500,999] 整体回队列
        let acts = sc.on(Event::Connected(3));                    // 在途尾 [0,499] 劈不动 ⇒ 接手无人认领的段
        assert_eq!(acts, vec![Action::Assign { worker: 3, iv: Interval { start: 500, end: 999 } }],
            "最长段劈不动时必须让新 worker 接手无人认领的区间，而不是让它空等: {acts:?}");
    }

    /// 简报的 `Consumes: consts::MIN_SEGMENT_BYTES` 落在调用方：调度器只认注入参数，
    /// 真默认值由 Task 8 从 `consts` 传进来。下面两条一起钉的是"地板归参数"这条裁定。
    #[test]
    fn split_threshold_is_the_injected_min_seg() {
        let mut loose = Scheduler::new(1000, 100);
        loose.on(Event::Connected(0));
        assert!(loose.on(Event::Connected(1)).iter().any(|a| matches!(a, Action::Assign { .. })),
            "地板 100 拦不住半长 500 的第一刀");

        let mut blocked = Scheduler::new(1000, 501);
        blocked.on(Event::Connected(0));
        assert_eq!(blocked.on(Event::Connected(1)), vec![Action::Idle],
            "总长一模一样，地板抬到 501（半长 500 触底）⇒ 一刀都不许劈");
    }

    #[test]
    fn consts_default_splits_exactly_one_round() {
        use crate::consts::MIN_SEGMENT_BYTES as M;
        let mut sc = Scheduler::new(M * 2, M);
        assert_eq!(sc.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: M * 2 - 1 } }]);
        let acts = sc.on(Event::Connected(1));                    // 半长正好 = M ⇒ 不触底，允许一刀
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { worker: 1, iv }
            if iv.start == M && iv.end == M * 2 - 1)), "{acts:?}");
        assert!(acts.iter().any(|a| matches!(a, Action::Retighten { worker: 0, iv }
            if iv.start == 0 && iv.end == M - 1)), "{acts:?}");
        assert!(sc.on(Event::Connected(2)).iter().all(|a| matches!(a, Action::Idle | Action::Halt)),
            "第二刀会得到 M/2 的半段 ⇒ 必须触底停劈");
    }

    #[test]
    fn no_ranges_after_several_splits_still_never_splits() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Connected(1));                       // ⇒ w0 [0,499] + w1 [500,999]
        sc.on(Event::NoRanges);
        sc.on(Event::Progress { worker: 0, abs_end: 200 });
        sc.on(Event::WindowDone { worker: 0 });           // 已写前缀进 done，[201,499] 回队列
        let acts = sc.on(Event::Connected(2));            // w1 还在途 ⇒ 退化期一律 Idle
        assert_eq!(acts, vec![Action::Idle], "还有段在途时退化期不得派新段: {acts:?}");
        sc.on(Event::Progress { worker: 1, abs_end: 999 });
        sc.on(Event::WindowDone { worker: 1 });
        let acts = sc.on(Event::Connected(2));
        assert_eq!(acts, vec![Action::Assign { worker: 2, iv: Interval { start: 201, end: 499 } }],
            "退化期派出的必须是整段无人认领区间，不含 Retighten");
        assert!(!acts.iter().any(|a| matches!(a, Action::Retighten { .. })));
        assert_eq!(sc.pending_bytes(), 299);
        check_invariants(&sc, 1000, "noranges-after-splits");
    }

    #[test]
    fn empty_total_is_done_without_any_work() {
        let mut sc = Scheduler::new(0, 100);
        assert!(sc.is_done());
        assert_eq!(sc.pending_bytes(), 0);
        assert!(sc.snapshot_intervals().is_empty());
        assert_eq!(sc.on(Event::Connected(0)), vec![Action::Idle], "零字节任务不得造出一个 1 字节的伪区间");
        assert!(sc.is_done());
    }

    #[test]
    fn second_connected_for_a_busy_worker_does_not_overwrite_its_progress() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 500 });
        assert_eq!(sc.pending_bytes(), 499);
        let acts = sc.on(Event::Connected(0));                    // 同一 worker 又报"就绪"，手上还压着 [0,999]
        assert_eq!(acts, vec![Action::Idle], "重复 Connected 覆盖在途段会丢掉已写偏移 ⇒ 必须拒派: {acts:?}");
        assert_eq!(sc.pending_bytes(), 499, "重复 Connected 不得改动已确认的偏移");
    }

    #[test]
    fn failed_after_every_byte_was_written_still_marks_the_segment_done() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 999 });
        sc.on(Event::Failed { worker: 0 });                       // 写满之后连接才断（416 / EOF 竞态的形状）
        assert!(sc.is_done(), "字节都已写满，Failed 不该让这段凭空消失：否则 is_done 与 done 会背离");
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }]);
        assert_eq!(sc.pending_bytes(), 0);
    }

    #[test]
    fn progress_is_clamped_to_the_segment_and_never_regresses() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Connected(1));                               // w0 [0,499], w1 [500,999]
        sc.on(Event::Progress { worker: 1, abs_end: u64::MAX });  // 越界上报（对端多给了 / 记错）
        assert_eq!(sc.pending_bytes(), 500, "越界上报必须夹到区间终点，否则在途尾算成负数");
        sc.on(Event::Progress { worker: 0, abs_end: 300 });
        sc.on(Event::Progress { worker: 0, abs_end: 100 });       // 迟到的旧上报
        assert_eq!(sc.pending_bytes(), 199, "偏移只进不退");
        check_invariants(&sc, 1000, "clamp");
    }

    /// 表里没写的最后一道闸：任意顺序的事件流（Task 8 的劈分时机由真实时序决定，所以"任意顺序"才是主判据）
    /// 必须 ① 不丢字节、不重复计字节，② 派出去的在途区间互不重叠，③ 一直派活总能收工。
    #[test]
    fn arbitrary_event_orders_never_lose_or_double_count_bytes() {
        let mut rng = Lcg(0x9E37_79B9_7F4A_7C15);
        for case in 0..300 {
            let total = 1 + rng.next(4096);
            let min_seg = 1 + rng.next(200);
            let workers = 1 + rng.next(6) as usize;
            let mut sc = Scheduler::new(total, min_seg);
            // 只由 Action 反推出来的"谁手上压着哪段"，不碰实现内部字段
            let mut held: Vec<(usize, Interval)> = vec![];

            // 约 1/3 的用例在中途退化（spec §2.2 的"首响应是 200"可能落在任何时刻）
            let degrade = rng.next(3) == 0;
            for step in 0..80 {
                if degrade && step == 40 {
                    sc.on(Event::NoRanges);
                }
                let w = rng.next(workers as u64) as usize;
                let acts = if rng.next(4) == 0 {
                    sc.on(Event::Connected(w))
                } else {
                    match held.iter().find(|(hw, _)| *hw == w).copied() {
                        Some((_, iv)) => {
                            match rng.next(3) {
                                0 => {
                                    let advance = rng.next(iv.len());
                                    sc.on(Event::Progress { worker: w, abs_end: iv.start + advance })
                                }
                                1 => {
                                    held.retain(|(hw, _)| *hw != w);
                                    sc.on(Event::WindowDone { worker: w })
                                }
                                _ => {
                                    held.retain(|(hw, _)| *hw != w);
                                    sc.on(Event::Failed { worker: w })
                                }
                            }
                        }
                        None => vec![],
                    }
                };
                track(&mut held, &acts);
                check_invariants(&sc, total, &format!("case {case}"));
            }

            // 收尾：把在途段写满并持续报"就绪"。收不了工就说明有区间凭空消失了。
            let mut guard = 0;
            while !sc.is_done() {
                guard += 1;
                assert!(guard < 500, "case {case}: total={total} min_seg={min_seg} 再怎么派也收不了工 ⇒ 有区间凭空消失");
                for w in 0..workers {
                    match held.iter().position(|(hw, _)| *hw == w) {
                        Some(pos) => {
                            let iv = held[pos].1;
                            sc.on(Event::Progress { worker: w, abs_end: iv.end });
                            sc.on(Event::WindowDone { worker: w });
                            held.remove(pos);
                        }
                        None => track(&mut held, &sc.on(Event::Connected(w))),
                    }
                }
            }
            assert_eq!(sc.pending_bytes(), 0, "case {case}: is_done() 为真但 pending 非零");
            let covered: u64 = sc.snapshot_intervals().iter().map(Interval::len).sum();
            assert_eq!(covered, total, "case {case}: done 没有整段覆盖文件");
        }
    }

    /// 把调度器发出的动作记进测试自己的"在途"表，并检查它派得合法。
    fn track(held: &mut Vec<(usize, Interval)>, acts: &[Action]) {
        for act in acts {
            match *act {
                Action::Assign { worker, iv } => {
                    assert!(!held.iter().any(|(hw, _)| *hw == worker), "给正忙的 w{worker} 又派了一段 ⇒ 覆盖它的偏移");
                    held.push((worker, iv));
                }
                Action::Retighten { worker, iv } => {
                    let (_, cur) = held.iter_mut().find(|(hw, _)| *hw == worker).expect("Retighten 打给了没活的 worker");
                    assert_eq!(cur.start, iv.start, "收紧只能提前终点，不能挪起点: {cur:?} -> {iv:?}");
                    assert!(iv.end >= cur.start && iv.end < cur.end, "收紧后的区间得是原区间的前缀: {cur:?} -> {iv:?}");
                    *cur = iv;
                }
                Action::Idle | Action::Halt => {}
            }
        }
        let mut in_flight: Vec<Interval> = held.iter().map(|(_, iv)| *iv).collect();
        in_flight.sort_by_key(|iv| iv.start);
        for pair in in_flight.windows(2) {
            assert!(pair[1].start > pair[0].end, "两段在途区间重叠 ⇒ 同一字节被派给两个 worker: {in_flight:?}");
        }
    }

    /// 手写 LCG：spec §9 只允许那 6 个依赖（不加 rand），且种子固定 ⇒ 失败可复现（spec §2.1 的可复现要求）。
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound.max(1)
        }
    }
}
