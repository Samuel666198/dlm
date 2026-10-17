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
//!    `consts::MIN_SEGMENT_BYTES`），**本模块的产品代码**不读 `consts`，为的是边界能在任意尺寸上
//!    用表钉死；只有本文件的测试模块读它（**复审 B-Minor 8** 把措辞改成事实：
//!    `consts_default_splits_exactly_one_round` 与 `resume_accepts_the_state_segment_shape_verbatim`
//!    各读一次 `crate::consts::MIN_SEGMENT_BYTES`，为的是把简报那条 `Consumes` 钉在真常数上）。
//!    劈不动时不空等：退回去派无人认领的区间；没有 ⇒ `Idle`。
//! 3. `NoRanges`（spec §2.2：对端不认 Range，或首响应是 200）之后退化为单段顺序下载：
//!    到达那一刻就把**无人认领的区间表合并成一段**（`collapse_pending`，只合并相邻/重叠的区间；
//!    中间隔着已确认或在途字节的不合并 ⇒ 不会把已落盘字节重算成待下载），此后每次派发取
//!    **起点最低**的那段（不是"最长"：对端只会从字节 0 起交出整个 body，派靠后的区间会把内容写错位置）。
//!    不再有新段被派出，直到在途段全部交回；此后一次只派一段、永不劈分（不产生 `Retighten`）。
//!    已在途的段**不被打断**，让它们按各自（已收紧的）终点自然收尾，
//!    所以退化点落在多段在途时，短暂会有"多段并行收尾"，但绝不会新增并行段；
//!    交回的尾巴并进那唯一的一段，段表不会因失败重新变多。
//!    已写的字节不会被重下（剩余区间从已写偏移之后起算）。
//!
//! 记账不变量（Task 4 断点文件的底）：`done` 只收**已确认落盘**的字节 —— worker 交回区间时，
//! 已写前缀并进 `done`（相邻段自动合并、按 `start` 升序），未写部分回队列；
//! `pending_bytes()` = 队列区间 + 各在途区间的未写后缀，与 `done` 互不重叠。
//! ⇒ `is_done()` 为真时 `done` 必然整段覆盖文件。
//!
//! 一条例外：`Event::SegmentComplete`（spec §6.4 的 416 ⇒「该段标完成」）封存整段时**不看 `written`**，
//! 所以那段里可能有从未落盘的字节 —— 那是设计裁定（Range 越界的字节本来就拿不到），
//! `snapshot_intervals()` 如实把它报为"归属已定、不再下载"的已确认覆盖集，Task 4 按这个措辞落盘。
//! 416 的上游契约随之改为 Task 7 报 `SegmentComplete`（不再折成 `WindowDone`）：
//! 走 `WindowDone` 会把整段推回队列 ⇒ "再派→再 416"的无界活锁，且因为它不是 `Failed`，
//! spec §6 的快速/慢速重试封顶也拦不住；其余失败/截断仍走 `Failed`。
//! `Action` 只有三种：`Assign`/`Retighten`/`Idle` —— v1 不产出"让 worker 停手"的命令
//! （spec §6.6 的 Ctrl-C 在计划里已降级为"任意时刻被杀都能续"，没有生产者就不留这个变体）。

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
    /// 该 worker 的区间**就此收尾，不需要交回任何字节**：spec §6.4 的 416（Range 越界 ⇒ 该段标完成）。
    /// 与 `WindowDone`/`Failed` 的区别只在记账：整段直接并进 `done`，**不看 `written`**、不回队列。
    /// 这是本模块唯一一条"字节没落盘也记 done"的通道，理由见模块文档的记账不变量。
    SegmentComplete { worker: usize },
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

    /// **续传播种入口**（计划勘误 **E13①**）：把"上一轮已经确认的字节"当成既成事实交给调度器。
    ///
    /// 为什么必须有它：`new(total, min_seg)` 只会把整份文件报成待下载，于是
    /// `resume::Decision::Resume(snap)` 到了引擎手里只剩两条路 —— 整份重下，或者在引擎里另记一本
    /// 覆盖账（**E7** 明令禁止第二本账）。这个入口让断点文件里那份段表直接进调度器唯一的那本账。
    ///
    /// **不新增状态、不新增字段**：每个种子区间夹进 `[0, total-1]` 后挂到一个只用于播种的临时槽位上，
    /// 再走 `seal()` 这条**既有的**生产路径（`Event::SegmentComplete` 用的就是它）⇒ 归一化由
    /// `push_done` 完成，交回的覆盖集与 `snapshot_intervals()` 的措辞逐字一致（升序、相邻/重叠已合并、
    /// 两两不重叠）。唯一的新逻辑是收尾那句"把队列重算成覆盖集在 `[0, total-1]` 里的补集"：播种之后
    /// "还没归属的连续空位"就是这些缝。**这一步不是可选的** —— 少了它，队列里那条 `[0, total-1]`
    /// 会把已确认字节重新算成待下载（`done + pending <= total` 当场红），也就等于整份重下。
    ///
    /// 与"真跑出来的进度"等价 —— 参照物完全由公开 API 造出来。**等价判据按勘误 E15 的三条钉**，
    /// 缺一不算过（原句"同一事件流下动作序列逐条相同"按字面**做不到**，已作废）：
    /// ① **洞的集合相等**（按字节集比，不是比条数、也不是比总长 —— 只比条数会同时放过"漏一个洞"
    ///    与"多并一个洞"）；② **种子里的字节一次都不许被重新派出**（`dispatched ∩ seed = ∅`，
    ///    这条才是"不重下已完成段"的正身）；③ 每一步的 `pending_bytes()` / `is_done()` /
    ///    `snapshot_intervals()` 两边一致，且两臂各自跑 `check_invariants`。
    /// 用例：前缀种子在 `resume_prefix_seed_is_indistinguishable_from_the_run_that_earned_it`
    /// （那种种子的补集天然只有一条，所以①②③之外**还多**拿到动作逐条相同 —— 那是特例不是通则；
    /// **逐步**的③ —— 每一步两臂一致且两臂各跑 `check_invariants` —— **也只有这一条覆盖**）；
    /// 多段空洞种子在 `resume_multi_gap_seed_matches_the_earned_state_byte_for_byte`
    /// （**复审 B-Minor 3 把措辞收成实话**：那条钉的是①、②，加上**播种那一步**的③
    /// 与"剩下要下的字节集合"，它的 drain 段按单 worker 干完、只在收尾跑一次 `check_invariants`，
    /// 不逐步比 ③ —— 为什么动作分岔是**对的**，写在那条用例的文档里）。
    ///
    /// 输入允许是**任意形状**：乱序、重叠、相邻、越过 `total_len`、空区间都会先夹进文件再归一化
    /// （调用方不该被要求先整理 —— 落盘的本来就是 `snapshot_intervals()`，但这里的独立归一化让
    /// "外部改过的断点文件"也走同一条路）。`total == 0` ⇒ 什么都不接（空文件没有可确认的字节，
    /// 与 `state::check_segments` 同一立场）。
    ///
    /// 退化模式（**E7**③）不属于本入口：`no_ranges` 只由 `Event::NoRanges` 打开，
    /// 引擎在播种之后自己决定要不要发那一枪。播种留下的槽位一律是 `None`，
    /// 与"从来没有 worker 报过就绪"在 `is_done()`/`longest_tail()` 上读作同一件事。
    /// 这一对**真的**撞在一起时（先播种再收到 `NoRanges`）的读法由
    /// `no_ranges_on_a_multi_gap_seed_dispatches_lowest_start_and_never_collapses_across_done` 钉：
    /// 段表不因已确认字节而合并、派发改按起点最低、一次只一段在途、永不 `Retighten`。
    pub fn resume(total: u64, min_seg: u64, done: &[Interval]) -> Self {
        let mut sc = Self::new(total, min_seg);
        for iv in done {
            let Some(clamped) = clamp_to_file(*iv, total) else { continue };
            sc.active.push(Some(Active { iv: clamped, written: None }));
            let slot = sc.active.len() - 1;
            sc.seal(slot);
        }
        sc.active.clear();
        sc.queue = uncovered(&sc.done, total);
        sc
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

    /// **已确认字节的极小覆盖集**：按 `start` 升序、相邻/重叠已合并、两两不重叠。
    /// 它不是"段表边界列表"（IDM 的断点文件按分段边界记，这里按字节覆盖记）：一段被劈过再交回，
    /// 这里可能只有合并后的一条；`SegmentComplete` 封存的段也照样出现在这里。
    /// ⇒ Task 4 的 `SegView` 得按"已确认覆盖区间"措辞，不要按"分段"措辞。
    pub fn snapshot_intervals(&self) -> Vec<Interval> {
        self.done.clone()
    }

    /// 唯一的状态迁移入口。`Connected` 至少返回一个动作（无事可做就是 `Idle`），其余事件返回空。
    pub fn on(&mut self, ev: Event) -> Vec<Action> {
        match ev {
            Event::NoRanges => {
                self.no_ranges = true;
                self.collapse_pending();
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
            // 416（spec §6.4）：整段就此封存，字节不必落盘、也不得再回队列。
            Event::SegmentComplete { worker } => {
                self.seal(worker);
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

    /// 封存 worker 的整段：**不查 `written`、不产生队列项**（spec §6.4 的 416 ⇒「该段标完成」）。
    /// 为什么不复用 `release()`：那条路径会把未写部分推回队列 ⇒ 再派→再 416 的无界活锁，
    /// 而它不是 `Failed`，spec §6 的快速/慢速重试封顶拦不住；
    /// 也不让 Task 7 用 `Progress{abs_end: iv.end}` + `WindowDone` 绕：那等于把从未写入的字节
    /// 记成"已确认落盘" ⇒ Task 4 的断点文件与 spec §7.2 的 sha256 门槛都被污染。
    /// 手上没活（或序号越界）⇒ 幂等空操作，`done` 靠 `push_done` 的合并保证同一段只记一次。
    fn seal(&mut self, worker: usize) {
        let Some(Some(a)) = self.active.get(worker).cloned() else { return };
        self.active[worker] = None;
        self.push_done(a.iv);
    }

    /// 最长的无人认领区间；等长时取起点最低的（保证测试可复现、Task 8 的回放一致）。
    /// **只在正常模式使用**：退化模式下对端不认 Range，派发顺序必须跟着文件顺序走（`first_pending`）。
    fn longest_pending(&self) -> Option<Interval> {
        self.queue.iter().min_by_key(|i| (Reverse(i.len()), i.start)).copied()
    }

    /// 起点最低的无人认领区间 —— 退化模式（spec §2.2）唯一的派法。
    /// 对端只会从字节 0 开始把整个 body 交出来，而 `fetch::run_window` 按 `iv.start` 落盘；
    /// 派一个起点靠后的区间等于把 0.. 的内容写到文件中段 ⇒ 字节级错位。
    fn first_pending(&self) -> Option<Interval> {
        self.queue.iter().min_by_key(|i| i.start).copied()
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
            // 派发的判据是**起点最低**，不是"最长" —— 见 `first_pending`。
            if self.active.iter().any(|a| a.is_some()) {
                return vec![Action::Idle];
            }
            return match self.first_pending() {
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
        // 退化模式下交回的尾巴要和表里其余区间并回一段，否则段表会随每一次失败重新变多。
        if self.no_ranges {
            self.collapse_pending();
        }
    }

    /// spec §2.2「段表退化为单段」：**把无人认领的区间合并成尽可能少的连续段**。
    /// 只合并相邻（`cur.start <= last.end + 1`）或重叠的区间 —— 被 `done` 或被在途区间挡在中间的字节
    /// **不会**被并进来：那等于把已确认落盘的字节重新算成待下载，`done + pending <= total` 会当场红。
    /// 于是"一段"的含义是"一段连续的空位"，而派发顺序（`first_pending`）保证它是文件里最靠前的那段。
    fn collapse_pending(&mut self) {
        if self.queue.len() < 2 {
            return;
        }
        self.queue.sort_by_key(|i| i.start);
        let mut merged: Vec<Interval> = Vec::with_capacity(self.queue.len());
        for cur in std::mem::take(&mut self.queue) {
            match merged.last_mut() {
                Some(last) if cur.start <= last.end.saturating_add(1) => last.end = last.end.max(cur.end),
                _ => merged.push(cur),
            }
        }
        self.queue = merged;
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

/// 把区间夹进 `[0, total-1]`；空区间（以及 `total == 0` 的任何区间）给 `None`。
/// **整条越过 EOF 的行也给 `None`**（T8 评审 A1 修的那一格）：起点已经不在文件里，这条种子
/// 一个已确认字节都不许造出来。把 `start` 也跟着 `min(last)` 夹一次会凭空造出一个
/// **从没被下载过的"幽灵已确认字节"**（`total = 1000` 上的 `[1500, 2000]` 变成 `{999, 999}`）——
/// `is_done()` 会因此为真而那个字节谁都没派过，`check_invariants` 还看不见（done + pending 照样平）。
/// 正确读法与 `engine::clamp_seed_to_disk` 同一句话：那里用 `.filter(|iv| iv.start <= last)`
/// 把整条越界的种子**丢掉**，不是夹回来。
/// 起点不挪 ⇒ 夹完必然仍非空（`end` 只会降到 `last`，而 `start <= last` 是上面那道闸）。
fn clamp_to_file(iv: Interval, total: u64) -> Option<Interval> {
    if total == 0 || iv.is_empty() {
        return None;
    }
    let last = total - 1;
    if iv.start > last {
        return None;
    }
    Some(Interval { start: iv.start, end: iv.end.min(last) })
}

/// 覆盖集 `done` 在 `[0, total-1]` 里的**补集** —— 播种后队列该有的那些"还没归属的连续空位"。
/// `done` 必须是 `push_done` 维持的那个形状（升序、两两不重叠且不相邻），补集因此自动满足
/// `push_pending` 的"同一起点不重复登记"。
fn uncovered(done: &[Interval], total: u64) -> Vec<Interval> {
    let mut gaps = vec![];
    let mut cursor = 0u64;
    for d in done {
        if d.start > cursor {
            gaps.push(Interval { start: cursor, end: d.start - 1 });
        }
        cursor = cursor.max(d.end.saturating_add(1));
    }
    if total > 0 && cursor < total {
        gaps.push(Interval { start: cursor, end: total - 1 });
    }
    gaps
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
        assert!(acts2.iter().all(|a| matches!(a, Action::Idle)), "{acts2:?}");
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
        step(&mut sc, Event::Connected(3), "C3");                 // 在途尾 [500,999] 比空闲 [301,499] 长 ⇒ 劈在途尾（空闲那段原样留着）
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
        assert!(sc.on(Event::Connected(2)).iter().all(|a| matches!(a, Action::Idle)),
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

    /// spec §2.2「段表退化为单段」的第一条判据：**派发的顺序必须是文件顺序**。
    /// 对端不认 Range ⇒ 它只会从 0 开始把整个 body 交出来，而 `fetch::run_window` 按
    /// `iv.start` 落盘；把 [700,999] 排在 [201,499] 前面就等于把 0.. 的字节写到 700 之后 ⇒ 文件错位。
    /// 所以退化模式一律取**起点最低**的无人认领区间，"最长优先"只在正常模式成立。
    #[test]
    fn degraded_dispatch_is_start_ascending() {
        let mut sc = s();
        sc.on(Event::Connected(0));                                   // [0,999]
        sc.on(Event::Connected(1));                                   // ⇒ w0 [0,499] + w1 [500,999]
        sc.on(Event::NoRanges);
        sc.on(Event::Progress { worker: 0, abs_end: 200 });
        sc.on(Event::WindowDone { worker: 0 });                       // done [0,200]，队列 [201,499]
        sc.on(Event::Progress { worker: 1, abs_end: 699 });
        sc.on(Event::WindowDone { worker: 1 });                       // done 再多 [500,699]，队列再多 [700,999]
        assert_eq!(sc.pending_bytes(), 299 + 300, "两段无人认领：[201,499] 与 [700,999]");
        let acts = sc.on(Event::Connected(2));
        assert_eq!(acts, vec![Action::Assign { worker: 2, iv: Interval { start: 201, end: 499 } }],
            "退化模式必须按起点升序派，不得因为 [700,999] 更长就先派它: {acts:?}");
        assert_eq!(sc.on(Event::Connected(3)), vec![Action::Idle], "退化期一次只允许一段在途");
        sc.on(Event::Progress { worker: 2, abs_end: 499 });
        sc.on(Event::WindowDone { worker: 2 });                       // done 并进 [0,499]
        let acts = sc.on(Event::Connected(3));
        assert_eq!(acts, vec![Action::Assign { worker: 3, iv: Interval { start: 700, end: 999 } }],
            "轮到 [700,999] 时前一段必须已经交回: {acts:?}");
        sc.on(Event::Progress { worker: 3, abs_end: 999 });
        sc.on(Event::WindowDone { worker: 3 });
        assert!(sc.is_done());
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }]);
        check_invariants(&sc, 1000, "degraded-start-ascending");
    }

    /// spec §2.2「段表退化为单段」的第二条判据：`NoRanges` 到达时，**队列里并存的区间必须塌成一段**。
    /// 今天三条无人认领的区间（[0,249]/[250,499]/[500,999]）会原样留在表里、被按"最长"逐条派出去
    /// ⇒ 既不是一整段，也不是文件顺序。相邻的无人认领区间合并成一段，
    /// 中间隔着已确认/在途字节的段**不合并**（那会把已确认字节重新算成待下载）。
    #[test]
    fn no_ranges_collapses_the_pending_table_into_one_segment() {
        let mut sc = s();
        sc.on(Event::Connected(0));                                   // [0,999]
        sc.on(Event::Connected(1));                                   // ⇒ w0 [0,499] / w1 [500,999]
        sc.on(Event::Connected(2));                                   // ⇒ w0 [0,249] / w2 [250,499] / w1 [500,999]
        sc.on(Event::Failed { worker: 0 });                           // 队列 [0,249]
        sc.on(Event::Failed { worker: 2 });                           // 队列 [0,249] + [250,499]
        sc.on(Event::WindowDone { worker: 1 });                       // 队列再多 [500,999]（今天表里三段并存）
        assert_eq!(sc.pending_bytes(), 1000, "一个字节都没写 ⇒ 全部仍是待下载");
        sc.on(Event::NoRanges);                                       // ⇒ 段表塌成一段
        let acts = sc.on(Event::Connected(3));
        assert_eq!(acts, vec![Action::Assign { worker: 3, iv: Interval { start: 0, end: 999 } }],
            "退化后的段表必须是**一段**、且从最低的未确认偏移起: {acts:?}");
        assert!(!acts.iter().any(|a| matches!(a, Action::Retighten { .. })), "退化期永不劈分");
        assert_eq!(sc.on(Event::Connected(4)), vec![Action::Idle], "退化期一次只允许一段在途");
        sc.on(Event::Failed { worker: 3 });                           // 交回 [0,999]
        assert_eq!(sc.on(Event::Connected(4)), vec![Action::Assign { worker: 4, iv: Interval { start: 0, end: 999 } }],
            "退化期交回的尾巴并回同一段，段表不得重新变多");
        assert_eq!(sc.pending_bytes(), 1000);
        check_invariants(&sc, 1000, "noranges-collapse");
    }

    /// spec §7.1 表里此前缺失的第六行：「416 后该段标完成」。Task 7 把 416 映射成
    /// `Event::SegmentComplete { worker }` —— "这个 worker 的区间已经收尾，不需要交回任何字节"：
    /// release 路径的**显式分支**，整段并入 `done`，**不看 `written`**、不回队列。
    /// 为什么不能用旧形状表达：唯一捷径是 `Progress{abs_end: iv.end}` + `WindowDone`，
    /// 那会把**从未落盘的字节**记成已确认 ⇒ 污染 Task 4 的断点文件与 spec §7.2 的 sha256 门槛；
    /// 而零字节的 `WindowDone` 把整段推回队列 ⇒ "再派→再 416→再回队"的无界 HTTP 活锁，
    /// 且那条路径不是 `Failed`，spec §6 的快速/慢速重试封顶永远不会生效。
    #[test]
    fn segment_complete_marks_a_zero_progress_interval_done_and_never_requeues() {
        let mut sc = s();
        assert_eq!(sc.on(Event::Connected(0)), vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: 999 } }]);
        sc.on(Event::SegmentComplete { worker: 0 });                // 一个字节都没写 ⇒ 416
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }], "整段恰好入 done 一次");
        assert_eq!(sc.pending_bytes(), 0, "该段不得回队列（活锁的根）");
        assert!(sc.is_done());
        assert_eq!(sc.on(Event::Connected(1)), vec![Action::Idle], "没有第二段可派 ⇒ 不会再打第三次 416");
        assert_eq!(sc.on(Event::SegmentComplete { worker: 0 }), Vec::<Action>::new(), "手上没活 ⇒ 幂等空动作");
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }], "重复上报不得把 done 记两次");
        check_invariants(&sc, 1000, "segment-complete");
    }

    /// 封存的段**只入 done 一次**，且永不回队列；与已写字节相邻时按相邻合并。
    #[test]
    fn segment_complete_seals_its_interval_exactly_once_and_is_not_redispatched() {
        let mut sc = s();
        sc.on(Event::Connected(0));                                 // [0,999]
        sc.on(Event::Connected(1));                                 // ⇒ w0 [0,499] / w1 [500,999]
        sc.on(Event::Progress { worker: 1, abs_end: 600 });
        sc.on(Event::SegmentComplete { worker: 1 });                // 写到 600 之后对端回 416
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 500, end: 999 }]);
        assert_eq!(sc.pending_bytes(), 500, "只剩 w0 的 [0,499]");
        sc.on(Event::Progress { worker: 0, abs_end: 499 });
        sc.on(Event::WindowDone { worker: 0 });                     // 相邻 ⇒ 并进 [0,999]
        let acts = sc.on(Event::Connected(1));
        assert_eq!(acts, vec![Action::Idle], "被封存的 [500,999] 不得再被派出: {acts:?}");
        assert!(sc.is_done());
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }], "done 只记一次并相邻合并");
        check_invariants(&sc, 1000, "segment-complete-partial");
    }

    /// 对照行：零字节的 `WindowDone` **仍然**把整段回队列 —— 那是"窗口读满/连接断了但字节没到手"的形状。
    /// 两个事件不能合并，否则 416 与普通截断无从区分；也正因此 416 必须走 `SegmentComplete`。
    #[test]
    fn zero_progress_window_done_still_requeues_the_whole_interval() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::WindowDone { worker: 0 });
        assert_eq!(sc.pending_bytes(), 1000, "WindowDone 不封存任何字节");
        assert!(sc.snapshot_intervals().is_empty());
        assert_eq!(sc.on(Event::Connected(1)), vec![Action::Assign { worker: 1, iv: Interval { start: 0, end: 999 } }],
            "回队列的段照旧可再派 ⇒ 416 若折成 WindowDone 就是活锁");
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
    /// 事件流里四种 worker 事件（`Progress`/`WindowDone`/`SegmentComplete`/`Failed`）都可能落在任何时刻。
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
                            match rng.next(4) {
                                0 => {
                                    let advance = rng.next(iv.len());
                                    sc.on(Event::Progress { worker: w, abs_end: iv.start + advance })
                                }
                                1 => {
                                    held.retain(|(hw, _)| *hw != w);
                                    sc.on(Event::WindowDone { worker: w })
                                }
                                2 => {
                                    // spec §6.4 的 416 落在任意时刻、任意 written 状态上
                                    held.retain(|(hw, _)| *hw != w);
                                    sc.on(Event::SegmentComplete { worker: w })
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
                Action::Idle => {}
            }
        }
        let mut in_flight: Vec<Interval> = held.iter().map(|(_, iv)| *iv).collect();
        in_flight.sort_by_key(|iv| iv.start);
        for pair in in_flight.windows(2) {
            assert!(pair[1].start > pair[0].end, "两段在途区间重叠 ⇒ 同一字节被派给两个 worker: {in_flight:?}");
        }
    }

    // ---- **E13①**：`Scheduler::resume` 的播种门（Task 8 的续传入口） ----

    /// 独立实现的"这批种子的极小覆盖集"：**逐字节成员判定**再把命中串成游程 ——
    /// 对 `[0, total)` 里每个字节问一句"有没有哪条种子含它"，连续的命中合成一条区间。
    ///
    /// 为什么长这样（**E15 末尾的方法教训**，T8 评审 A1 的成因）：这一版之前抄了被测的那条夹法
    /// （`iv.start.min(total - 1)`）⇒ 参照与 `clamp_to_file` 在**正被检验的那条轴**上不独立，
    /// "整条越过 EOF 的种子被夹成一个幽灵已确认字节"这一类缺陷当场看不见（两边一起错、一起绿）。
    /// 现在这个参照**不共享**被测的任何一条规则：不夹边界（文件之外的字节根本不在扫描范围里）、
    /// 不排序、不做"插入 + 相邻合并"，只回答"这个字节在不在种子里"。空区间与越界行因此自然消失。
    ///
    /// 可比轴只有一条：**字节集合**。动作序列、队列粒度、夹法规则一概不可比，也不许拿来当
    /// E15 那三条性质里 ②③ 的参照（那两个函数测的是别的轴）。
    /// 代价是 `O(total × seeds)`，测试用的 `total` 只有几千，慢一点没关系（刻意的笨换独立性）。
    fn cover(seeds: &[Interval], total: u64) -> Vec<Interval> {
        let held = |b: u64| seeds.iter().any(|iv| iv.start <= b && b <= iv.end);
        let mut out: Vec<Interval> = vec![];
        let mut run: Option<(u64, u64)> = None;   // 当前这段连续命中的 (起点, 含端点)
        for b in 0..total {
            if held(b) {
                match run {
                    Some((s, e)) => run = Some((s, e.max(b))),
                    None => run = Some((b, b)),
                }
            } else if let Some((s, e)) = run.take() {
                out.push(Interval { start: s, end: e });
            }
        }
        if let Some((s, e)) = run {
            out.push(Interval { start: s, end: e });
        }
        out
    }

    /// **E13① 点名的等价性用例（严格版，逐条比动作）**：种子是"从头连续下到的前缀"时，
    /// `resume` 之后与"真把这段前缀跑出来"的调度器在同一串随机事件下**动作逐字相同**。
    ///
    /// 参照物完全由公开 API 造：`new()` 之后只用 worker 0（场上永远只有一条在途区间 ⇒ 无从劈分），
    /// 反复 `Connected → Progress → WindowDone` 把前缀一段一段下到 `k` 为止。这样得到的
    /// `done = [[0,k-1]]`、`queue = [[k,total-1]]` —— **补集天然只有一条**，与 `resume` 造的极小补集
    /// 同形，于是两台可以从第一步起逐条比动作。多段空洞那种种子为什么不能逐条比，见
    /// `resume_multi_gap_seed_matches_the_earned_state_byte_for_byte` 的文档（实测分岔形状就记在那里）。
    #[test]
    fn resume_prefix_seed_is_indistinguishable_from_the_run_that_earned_it() {
        let mut rng = Lcg(0x5EED_1234_ABCD_0001);
        let mut compared = 0usize;   // 真正跑了动作比对的用例数
        for case in 0..300usize {
            let total = 2 + rng.next(4095);
            let min_seg = 1 + rng.next(200);
            let workers = 1 + rng.next(6) as usize;

            // ① 参照物：把前缀真跑出来。
            let mut earned = Scheduler::new(total, min_seg);
            let k = 1 + rng.next(total - 1);
            let mut covered = 0u64;
            while covered < k {
                let acts = earned.on(Event::Connected(0));
                let iv = match acts.as_slice() {
                    [Action::Assign { iv, .. }] => *iv,
                    other => panic!("case {case}: 前缀参照物每轮该只领到一整条区间，实际 {other:?}"),
                };
                assert_eq!(iv.start, covered, "case {case}: 前缀之外不该有别的东西可派");
                let stop = (k - 1).min(total - 1);
                earned.on(Event::Progress { worker: 0, abs_end: stop });
                earned.on(Event::WindowDone { worker: 0 });
                covered = stop + 1;
                check_invariants(&earned, total, &format!("case {case} 参照物"));
            }
            let seed = earned.snapshot_intervals();
            if seed.iter().map(Interval::len).sum::<u64>() == total {
                continue;   // 前缀正好盖满整份 ⇒ 没有可比对的剩余工作
            }
            compared += 1;

            // ② 种子打乱 + 加"不改并集"的噪声（重复行、子区间、空区间、越过 EOF 的行）。
            let mut noisy: Vec<Interval> = vec![];
            for iv in &seed {
                noisy.push(*iv);
                noisy.push(*iv);                                    // 重复行 ⇒ 同一段不许算两次
                noisy.push(Interval { start: iv.end + 1, end: iv.end });   // 空区间 ⇒ 丢弃
                if iv.len() >= 3 {
                    noisy.push(Interval { start: iv.start + 1, end: iv.end - 1 });   // 子区间
                }
                if iv.end + 1 == total {
                    noisy.push(Interval { start: iv.start, end: total + 4_000 });    // 越界 ⇒ 夹回来
                }
            }
            for i in 1..noisy.len() {
                let j = rng.next((i + 1) as u64) as usize;
                noisy.swap(i, j);
            }
            assert_eq!(cover(&noisy, total), seed, "case {case}: 夹具自己得先自洽（噪声不许改并集）");

            let mut seeded = Scheduler::resume(total, min_seg, &noisy);
            assert_eq!(seeded.snapshot_intervals(), seed, "case {case}: 播种后的覆盖集");
            assert_eq!(seeded.pending_bytes(), earned.pending_bytes(), "case {case}: 待下载字节数");

            // ③ 同一串随机事件喂给两台，逐步比对动作与全部可观测量。
            let mut held: Vec<(usize, Interval)> = vec![];
            for step in 0..140 {
                let w = rng.next(workers as u64) as usize;
                let ev = match (rng.next(4), held.iter().find(|(hw, _)| *hw == w).copied()) {
                    (0, _) | (_, None) => Event::Connected(w),
                    (_, Some((_, iv))) if rng.next(3) == 0 => {
                        Event::Progress { worker: w, abs_end: iv.start + rng.next(iv.len()) }
                    }
                    (_, Some((_, _))) => {
                        held.retain(|(hw, _)| *hw != w);
                        if rng.next(2) == 0 { Event::WindowDone { worker: w } } else { Event::Failed { worker: w } }
                    }
                };
                let a = earned.on(ev);
                let b = seeded.on(ev);
                assert_eq!(a, b, "case {case} step {step} 事件 {ev:?} 上动作分岔：earned={a:?} resume={b:?}");
                track(&mut held, &a);
                check_invariants(&earned, total, &format!("case {case} 参照物 step {step}"));
                check_invariants(&seeded, total, &format!("case {case} 播种 step {step}"));
                assert_eq!(earned.pending_bytes(), seeded.pending_bytes(), "case {case} step {step} pending 背离");
                assert_eq!(earned.snapshot_intervals(), seeded.snapshot_intervals(), "case {case} step {step} 覆盖集背离");
                assert_eq!(earned.is_done(), seeded.is_done(), "case {case} step {step} is_done 背离");
            }
        }
        assert!(compared >= 200, "300 例里只有 {compared} 例可比对 ⇒ 夹具退化了，不是在测播种");
    }

    /// 多段空洞（断点文件的常态形状）的等价性只能按**要下的字节集合**比，不能按动作序列比 ——
    /// 这个区别是实测出来的，不是推理出来的，所以把它写在这里：
    ///
    /// `new()` 一路走到"中间有空洞"的进度时，队列里留下的是**按历史切碎**的区间
    /// （`push_pending` 只按起点去重，合并相邻段只发生在 `push_done` 与退化模式的 `collapse_pending`
    /// 里）。一份种子**没有历史**，`resume` 只能造出极小补集（相邻的缝合成一条）。同一段字节、
    /// 两台的**队列粒度**天然不同 ⇒ `longest_pending` 挑中的第一段就可能不一样，动作序列从第一步就
    /// 分岔（原样复现的形状：earned 队里是 `[560,591] [592,624] [625,686] [687,749]` 四条，
    /// 播种方是 `[560,749]` 一条，于是同一个 `Connected(3)` 一边派 `[687,749]` 一边派 `[560,749]`）。
    ///
    /// 粒度不同不是缺陷，而且取"合并"这一边是有理由的：调度器该看见"最长的一段还没归属的字节"，
    /// 这与 `collapse_pending` 的动机同一句话；反过来照历史切分则根本没有信息可照。
    /// 所以这里钉的是**不因粒度而变**的那批性质：覆盖集、pending、`is_done` 完全相同，
    /// 剩下要下的**字节集合**完全相同（不多不少、一个已确认字节都不重下），且都收工在整段覆盖上。
    #[test]
    fn resume_multi_gap_seed_matches_the_earned_state_byte_for_byte() {
        let mut rng = Lcg(0xC0FF_EE12_3456_789A);
        let mut compared = 0usize;
        for case in 0..300usize {
            let total = 2 + rng.next(4095);
            let min_seg = 1 + rng.next(200);
            let workers = 2 + rng.next(5) as usize;

            // ① 参照物：多 worker 随机跑一段，中途把在途段零进度交回 ⇒ 停在"有洞、也有待下载"的静止形状。
            let mut earned = Scheduler::new(total, min_seg);
            let mut held: Vec<(usize, Interval)> = vec![];
            for _ in 0..60 {
                let w = rng.next(workers as u64) as usize;
                let acts = match (rng.next(4), held.iter().find(|(hw, _)| *hw == w).copied()) {
                    (0, _) | (_, None) => earned.on(Event::Connected(w)),
                    (_, Some((_, iv))) => {
                        held.retain(|(hw, _)| *hw != w);
                        match rng.next(3) {
                            0 => {
                                earned.on(Event::Progress { worker: w, abs_end: iv.start + rng.next(iv.len()) });
                                earned.on(Event::WindowDone { worker: w })
                            }
                            1 => earned.on(Event::SegmentComplete { worker: w }),
                            _ => earned.on(Event::Failed { worker: w }),
                        }
                    }
                };
                track(&mut held, &acts);
                check_invariants(&earned, total, &format!("case {case} 参照物"));
            }
            for (w, _) in std::mem::take(&mut held) {
                earned.on(Event::WindowDone { worker: w });
            }
            // **收口复审 B2 第 4 条（形状 a）**：上面循环里那句 `check_invariants` 断的一直是
            // **"还在途"**的状态，而真正要拿去播种的是**在途全部交回之后**的这一格 —— 它原先**没人审**。
            // 于是这一句先补上审计，再取种子。（这不是把 `cover` 那句搬个位置：见下面替换掉的注释。）
            check_invariants(&earned, total, &format!("case {case} 参照物（在途全部交回之后）"));
            let seed = earned.snapshot_intervals();
            let covered = seed.iter().map(Interval::len).sum::<u64>();
            if covered == 0 || covered == total || seed.len() < 2 {
                continue;   // 只要"至少两个互不相邻的空洞"这一格，那才是断点文件的常态
            }
            compared += 1;
            // **原来这一句是** `assert_eq!(cover(&seed, total), seed, "播种的输入自己得先过独立参照")`，
            // 删掉了（**收口复审 B2 第 4 条**）。它不对的地方有两层，写在这里免得下轮再把它当成"少了
            // 一道参照"补回来：
            // ① 右边那个 `seed` 出自被测自己的 `snapshot_intervals()`，而 `cover` 的输出形状与
            //    `push_done` 维持的形状是同一条（升序、两两不重叠且不相邻）⇒ 这一句审的是**形状**，
            //    审不到本条用例的正题（`resume` 怎么读这批区间），却在注释里被叫作"独立参照"，
            //    把夹具与被测各自成立的同一件事说成了证据。
            // ② 它也不是**字面上**永远绿（把 `push_done` 的合并弄坏、让它交出未归一化的形状才会红 ——
            //    这是照 `cover` 与 `push_done` 两边的算式推出来的，本轮没有为它跑变异），
            //    但那件事**上一句** `check_invariants` 在同一条状态上已经断过，而且断得更宽
            //    （还查 `done + pending <= total` 与 `is_done` 的蕴含）。
            //    ⇒ 按派发的 (a) 支：把状态审计补到**交回在途之后**，删掉这句复读。
            //
            // 补一句**手算**的独立期望（按派发的 (b) 支，用的是本用例自己已有的两个数）：场上已经
            // 没有在途段 ⇒ `earned` 欠的字节只能等于"`total` - 种子里那些已确认字节"，右边只由
            // `total` 与 `seed` 每条区间的长度相加得到，不读被测的任何一本账。
            // 这一句红得起来，而且红在**被测的账 vs 手算**这一对上。变异自证（8F-4b 报告）：
            // 把 `release` 回队列的尾巴**少算一格**（那一格既不在 done 也不在 queue），并把本用例
            // 那两句 `check_invariants` 暂时关掉以隔离这一句 ⇒ 第一红落在这一句（`left: 0 / right: 1`）。
            // 同一发变异在那两句审计没关时**先**红在 `check_invariants` 的 `is_done()` 分支
            // ⇒ 这一句的独立价值是把那条 `<=` **收成 `==`**：尾巴少一格、而 `is_done()` 恰好仍然是假
            // 的那一片形状，只有这一句看得见（这半句是按两条判据的算式推出来的，不是实测）。
            assert_eq!(
                earned.pending_bytes(),
                total - covered,
                "case {case}: 在途全部交回后，参照物欠的字节不等于「全长 - 种子已确认字节」（种子 {seed:?}）"
            );
            let mut seeded = Scheduler::resume(total, min_seg, &seed);

            // ② 记账必须一字不差。
            assert_eq!(seeded.snapshot_intervals(), seed, "case {case}: 覆盖集不同");
            assert_eq!(seeded.pending_bytes(), earned.pending_bytes(), "case {case}: pending 不同");
            assert_eq!(seeded.pending_bytes(), total - covered, "case {case}: 播种后仍要满足「待下载 = 全长 - 已确认」");
            assert!(!seeded.is_done() && !earned.is_done(), "case {case}: 还有洞时两边都不该收工");

            // ③ 剩下要下的字节集合：用单 worker 干完（场上永远只有一条在途区间 ⇒ 不劈分，
            //    每次领到的都是一整条队列区间），于是"派出去过的区间"直接就是那张字节集合。
            let earned_got = drain_with_one_worker(&mut earned);
            let seeded_got = drain_with_one_worker(&mut seeded);
            let bytes = |v: &[Interval]| v.iter().map(Interval::len).sum::<u64>();
            assert_eq!(bytes(&seeded_got), total - covered, "case {case}: 播种方下的字节数不等于它欠的字节数");
            assert_eq!(bytes(&earned_got), bytes(&seeded_got), "case {case}: 两台欠的字节数不同");
            assert_eq!(cover(&earned_got, total), cover(&seeded_got, total), "case {case}: 两台要下的字节集合不同");
            for iv in &seeded_got {
                assert!(
                    seed.iter().all(|d| iv.end < d.start || iv.start > d.end),
                    "case {case}: 播种后仍重下了已确认字节 {iv:?}（种子 {seed:?}）"
                );
            }
            assert!(seeded.is_done() && earned.is_done(), "case {case}: 干完活没收工");
            assert_eq!(
                seeded.snapshot_intervals(),
                vec![Interval { start: 0, end: total - 1 }],
                "case {case}: 补完洞之后覆盖集必须是整份文件，一条不重不漏"
            );
            check_invariants(&seeded, total, &format!("case {case} 播种收尾"));
        }
        assert!(compared >= 50, "300 例里只有 {compared} 例跑出多段空洞的种子 ⇒ 夹具退化了");
    }

    /// 用**一个** worker 把剩下的活儿干完，返回派出去过的区间（按派出顺序）。
    /// 单 worker 是刻意的：`longest_tail()` 在那种场上永远为空 ⇒ 永不产生 `Retighten`，
    /// 每次 `Connected` 领到的都是一整条队列区间，"派出去过的区间"因此就是那张字节集合本身。
    fn drain_with_one_worker(sc: &mut Scheduler) -> Vec<Interval> {
        let mut dispatched = vec![];
        let mut guard = 0;
        while !sc.is_done() {
            guard += 1;
            assert!(guard < 20_000, "drain 收不了工 ⇒ 有区间凭空消失了");
            for act in sc.on(Event::Connected(0)) {
                match act {
                    Action::Assign { iv, .. } => {
                        sc.on(Event::Progress { worker: 0, abs_end: iv.end });
                        sc.on(Event::WindowDone { worker: 0 });
                        dispatched.push(iv);
                    }
                    Action::Retighten { iv, .. } => panic!("单 worker 的场上不该有劈分: {iv:?}"),
                    Action::Idle => {}
                }
            }
        }
        dispatched
    }

    /// 播种门自身的三条边界：`total == 0`、空种子、种子盖满整份文件。
    #[test]
    fn resume_seeding_edges_match_the_accounting_rules() {
        // 空文件：没有任何可确认的字节（与 `state::check_segments` 同一立场），也不许 panic。
        let mut empty = Scheduler::resume(0, 100, &[Interval { start: 0, end: 0 }]);
        assert!(empty.snapshot_intervals().is_empty(), "空文件不接受任何种子");
        assert_eq!(empty.pending_bytes(), 0);
        assert!(empty.is_done());
        assert_eq!(empty.on(Event::Connected(0)), vec![Action::Idle]);

        // 空种子 == `new`：整份仍是待下载。
        let total = 1000;
        let mut plain = Scheduler::resume(total, 100, &[]);
        assert_eq!(plain.snapshot_intervals(), vec![]);
        assert_eq!(plain.pending_bytes(), total);
        assert!(!plain.is_done());
        assert_eq!(plain.on(Event::Connected(0)), vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: total - 1 } }],
            "空种子的第一枪必须与 `new` 给出同一个动作");

        // 种子盖满整份 ⇒ 直接收工，一段都不派。
        let mut full = Scheduler::resume(total, 100, &[Interval { start: 0, end: total - 1 }]);
        assert!(full.is_done(), "已确认字节覆盖整份文件 ⇒ 没有待下载工作");
        assert_eq!(full.pending_bytes(), 0);
        assert_eq!(full.on(Event::Connected(0)), vec![Action::Idle]);
        check_invariants(&full, total, "seed-full");
    }

    /// 乱序 / 重叠 / 相邻 / 空 / 越过 EOF 的种子必须归一化成 `snapshot_intervals()` 那个措辞，
    /// 并且**派出去的区间一个字节都不落在种子里**（播种的全部意义所在）。
    #[test]
    fn resume_normalizes_malformed_seeds_and_never_redispatches_seeded_bytes() {
        const TOTAL: u64 = 1000;
        let seeds = [
            Interval { start: 600, end: 699 },
            Interval { start: 0, end: 99 },
            Interval { start: 100, end: 149 },    // 与上一行相邻 ⇒ 并成 [0,149]
            Interval { start: 50, end: 700 },     // 横跨一切 ⇒ 全部并进 [0,700]
            Interval { start: 900, end: 50_000 }, // 终点越过 EOF ⇒ 夹到 999
            Interval { start: 800, end: 799 },    // 空区间 ⇒ 丢掉
        ];
        let want = vec![Interval { start: 0, end: 700 }, Interval { start: 900, end: 999 }];
        let mut sc = Scheduler::resume(TOTAL, 100, &seeds);
        assert_eq!(sc.snapshot_intervals(), want, "归一化结果要与 literal 期望和独立实现都相同");
        assert_eq!(sc.snapshot_intervals(), cover(&seeds, TOTAL));
        assert_eq!(sc.pending_bytes(), 199, "只剩 [701,899] 这一条缝");
        check_invariants(&sc, TOTAL, "seed-normalized");

        // 唯一可派的就是那条缝：起点、终点都必须绕开种子。
        let acts = sc.on(Event::Connected(0));
        assert_eq!(acts, vec![Action::Assign { worker: 0, iv: Interval { start: 701, end: 899 } }],
            "播种后派出去的区间只能是没归属的那条缝: {acts:?}");
        sc.on(Event::Progress { worker: 0, abs_end: 899 });
        sc.on(Event::WindowDone { worker: 0 });
        assert!(sc.is_done());
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }], "补完缝后整段覆盖，不重复计字节");
    }

    /// **T8 评审 A1（真缺陷）**：**整条越过 EOF 的种子必须整条丢掉，一个已确认字节都不许造出来**。
    ///
    /// 修之前的写法把 `start` 和 `end` **一起**夹到 `last`（`iv.start.min(last)`）⇒ `total = 1000`
    /// 上的 `[1500, 2000]` 变成 `{999, 999}`：一个**从没被下载过**的"幽灵已确认字节"。它能骗过两样东西：
    /// `is_done()` 会因为补集空了而为真，`check_invariants` 也看不见（done 与 pending 照样把文件分完，
    /// 平的）。而隔壁 `engine::clamp_seed_to_disk` 早就给了正确读法（`.filter(|iv| iv.start <= last)`
    /// 整条丢掉）—— 门的夹法与调用方的夹法各说各话，就是这一格。
    ///
    /// 期望值**全部写死**（不问被测函数要），`cover` 只作为"字节集合"这条轴上的第二意见
    /// （它刻意不共享夹法，见它自己的文档）。
    ///
    /// **判别力要说清（复审 B-I-3 / 控制员亲跑 R-1）**：退回旧夹法（把 `start` 也 `min(last)`）之后，
    /// 本用例**只有 ① 段的两条**红（`snapshot_intervals` 那条等式与 `pending_bytes`）——
    /// ② 段那四条种子在旧夹法下并进同一个 `{900,999}`（`{1500,2000}`、`{1000,1000}`、`{4000,9999}`
    /// 都被夹成 `{999,999}`，与 `{900,50_000}` 夹出的 `[900,999]` 相邻合并 ⇒ 结果一模一样），
    /// ③ 段是 `total == 0` 那一支，旧夹法也一样什么都不接 ⇒ **两段全绿**。
    /// "幽灵字节伪装成已完成"那一格由 `a_ghost_byte_next_to_a_near_full_seed_is_the_shape_that_fakes_completion`
    /// 钉；`start == last` 这一格（R-2：把 `>` 写成 `>=`）由
    /// `a_seed_that_ends_exactly_at_the_last_byte_is_not_treated_as_past_eof` 钉。
    #[test]
    fn a_seed_that_starts_past_eof_confirms_no_bytes_at_all() {
        const TOTAL: u64 = 1_000;
        let past = [Interval { start: 1_500, end: 2_000 }];

        // ① 只有整条越界的种子 ⇒ 等价于"没有种子"：覆盖集空、全长还欠着、没收工。
        let empty_handed = Scheduler::resume(TOTAL, 100, &past);
        assert_eq!(empty_handed.snapshot_intervals(), vec![], "起点在文件之外的种子不许留下任何已确认字节");
        assert_eq!(empty_handed.pending_bytes(), 1_000, "1000 个字节一个都没归属");
        assert!(
            !empty_handed.is_done(),
            "整条越界的种子不许让进度看起来前进一步：1000 个字节都还欠着 ⇒ 不收工。\
             （**复审 B-I-3**：这一句原先写「这一条就是那场事故的正面」，是 over-claim —— \
             旧夹法在这一格留下最后一个字节那份幽灵确认之后，补集仍交回剩下 999 个字节那一条缝 \
             ⇒ 本句在缺陷代码下照样绿；事故的正面在 \
             `a_ghost_byte_next_to_a_near_full_seed_is_the_shape_that_fakes_completion`）"
        );
        assert_eq!(empty_handed.snapshot_intervals(), cover(&past, TOTAL), "字节集合这条轴上与独立参照一致");
        check_invariants(&empty_handed, TOTAL, "seed-past-eof-only");

        // ② 混着给：夹回来的行（起点在文件内）算数，三条整条越界的一律丢掉。
        let mixed = [
            Interval { start: 900, end: 50_000 }, // 起点在内、终点越界 ⇒ 夹到 999（唯一的合法行）
            Interval { start: 1_500, end: 2_000 }, // 整条越界
            Interval { start: 1_000, end: 1_000 }, // 单字节，正好落在 EOF 之外第一格
            Interval { start: 4_000, end: 9_999 }, // 整条越界，且很长
        ];
        let mut sc = Scheduler::resume(TOTAL, 100, &mixed);
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 900, end: 999 }], "只有 [900,999] 这一条进了覆盖集");
        assert_eq!(sc.pending_bytes(), 900, "还欠 [0,899] 这 900 个字节");
        assert!(!sc.is_done());
        check_invariants(&sc, TOTAL, "seed-past-eof-mixed");
        // 第一枪只能派没归属的那条缝，而且终点必须停在种子之前。
        assert_eq!(
            sc.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: 899 } }],
            "派出去的区间只能是 [0,899]：不许从幽灵字节 999 之外凭空多出字节"
        );

        // ③ `total == 0` 那一支本来就什么都不接，与"整条越界"同一立场。
        let nothing = Scheduler::resume(0, 100, &[Interval { start: 0, end: 9 }]);
        assert_eq!(nothing.snapshot_intervals(), vec![], "空文件没有可确认的字节");
        assert_eq!(nothing.pending_bytes(), 0);
        assert!(nothing.is_done());
    }

    /// **复审 B-I-3 的 R-2 那一格**：`clamp_to_file` 里 `start > last` 那道闸在 `start == last`
    /// 时必须**放行**。"文件最后一个字节已确认"是完全合法的种子形状（`{total-1, total-1}`），
    /// 把它当成越界丢掉 ⇒ 每一轮续传都重下那一个字节，而且**无声**（覆盖集只是少一条，账面平）。
    /// 全树原先没有任何用例打到这一格：把 `>` 改成 `>=` 之后 163 条单测全绿（控制员亲跑 R-2）。
    #[test]
    fn a_seed_that_ends_exactly_at_the_last_byte_is_not_treated_as_past_eof() {
        const TOTAL: u64 = 1_000;
        let last = TOTAL - 1;

        // ① 单字节种子正好落在最后一个字节上 ⇒ 必须进覆盖集，一个字节都不许被当成越界丢掉。
        let seed_exact = [Interval { start: last, end: last }];
        let mut only_last = Scheduler::resume(TOTAL, 100, &seed_exact);
        assert_eq!(
            only_last.snapshot_intervals(),
            vec![Interval { start: last, end: last }],
            "`start == last` 不是越界：最后一个字节的种子必须原样留下（丢掉它 = 每轮续传重下它一个字节）"
        );
        assert_eq!(only_last.pending_bytes(), 999, "还欠 [0,998] 这 999 个字节");
        assert!(!only_last.is_done(), "[0,998] 还没归属 ⇒ 不许收工");
        assert_eq!(
            only_last.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: last - 1 } }],
            "派出去的只能是剩下那条缝，终点停在种子之前"
        );
        check_invariants(&only_last, TOTAL, "seed-last-byte-only");

        // ② 同一个字节换个写法：终点报得比 EOF 更远 ⇒ 夹完仍然算数（不许因为 `end` 越界
        //    就把合法的 `start` 一起丢掉 —— 那正是旧夹法与 `start >= last` 两种错法的分界）。
        let seed_over_eof = [Interval { start: last, end: last + 50 }];
        let clipped = Scheduler::resume(TOTAL, 100, &seed_over_eof);
        assert_eq!(
            clipped.snapshot_intervals(),
            vec![Interval { start: last, end: last }],
            "夹终点不丢种子：第 {last} 个字节确认了，与它后面那些并不存在的字节是两件事"
        );
        check_invariants(&clipped, TOTAL, "seed-last-byte-clipped");

        // ③ **独立参照比的是"被测报回来的覆盖集"**，不是夹具对一个字面量的自答
        //    （**收口复审 B2 第 3 条**）。上一轮这里写的是
        //    `cover(&[{last,last}], TOTAL) == {last,last}`，头顶注释叫"独立参照在这一格给同一个答案
        //    （`cover` 不共享 `clamp_to_file` 的写法）"—— 那句把它说成了证据，其实它是**恒真**：
        //    `cover` 按 `[0, total)` 逐字节扫命中，喂给它的又是一条**已在范围内、已归一化**的单区间，
        //    它只能原样吐回来；右边那个字面量与左边同源 ⇒ `clamp_to_file` 怎么改这一句都绿。
        //    现在左边仍是 `cover` 对**原始种子**扫出来的字节集合，右边换成**两台被测各自报回的覆盖集**
        //    ⇒ 这一句红得起来。⚠ **但它不是那一格的第一红**（控制员在 `0d847f0` 上亲跑：把
        //    `clamp_to_file` 的 `start > last` 写成 `start >= last` ⇒ 165 passed / **1 failed**，
        //    首红落在上面的 ①（`left: []`），本用例在那里就中止了，这一句轮不到说话）。
        //    所以它的独立价值不在"看得见 `clamp_to_file` 的错"—— 那一维 ① 已经占了；
        //    在于**把 ① 右边那个手写的字面量与 `cover` 扫出来的字节集合对上账**：
        //    只有当有人把 ① 放宽成只看条数/只看形状那一类判据时，这一句才是还站着的那根针。
        //    要单独验它自己有没有牙，得先把 ① 摘掉再跑同一发变异（本轮没跑这一发，
        //    因为摘掉 ① 会同时拆掉本条用例的主判据 ⇒ 记为**未单独自证**）。
        //    `cover` 与被测的独立性只在**字节集合**这一条轴上（它不夹边界、不排序、不做插入合并，
        //    见 `cover` 自己的文档与 **E15** 末尾的方法教训）；动作序列与队列粒度这两条轴不可比，
        //    这一句也就不声称能看见它们。
        assert_eq!(
            cover(&seed_exact, TOTAL),
            only_last.snapshot_intervals(),
            "①那一格的字节集合：被测报回的覆盖集必须等于参照扫出来的那一个字节（种子 {seed_exact:?}）"
        );
        assert_eq!(
            cover(&seed_over_eof, TOTAL),
            clipped.snapshot_intervals(),
            "②那一格的字节集合：参照只扫 `[0, TOTAL)`，所以越过 EOF 的终点在它眼里不存在，\
             被测夹出来的也必须正好是第 {last} 个字节这一个（种子 {seed_over_eof:?}）"
        );
    }

    /// **那场事故的正面形状**（复审 B-I-3：隔壁那条用例的 ① 段拦不住旧夹法，理由写在它的文档里）：
    /// 幽灵字节能伪装成"已完成"要**两个条件同时成立** —— 一条合法种子把文件几乎盖满（`{0,998}`），
    /// 外加一条整条越界的种子被旧夹法折成 `{999,999}`。两者相邻合并 ⇒ 补集空 ⇒ `is_done()` 为真，
    /// 而 `999` 那个字节谁都没派过、盘上没有它。
    #[test]
    fn a_ghost_byte_next_to_a_near_full_seed_is_the_shape_that_fakes_completion() {
        const TOTAL: u64 = 1_000;
        let mut sc = Scheduler::resume(
            TOTAL,
            100,
            &[
                Interval { start: 0, end: 998 },       // 合法：前 999 个字节真的确认了
                Interval { start: 1_500, end: 2_000 }, // 整条越界：必须整条丢掉
            ],
        );
        assert_eq!(sc.pending_bytes(), 1, "只剩最后一个字节还欠着 —— 旧夹法在这里算成 0");
        assert!(
            !sc.is_done(),
            "那一个字节从没被派出 ⇒ 这一格才是「幽灵字节伪装成已完成」的正面形状：旧夹法在这里收工"
        );
        assert_eq!(
            sc.snapshot_intervals(),
            vec![Interval { start: 0, end: 998 }],
            "越界那条不许并进覆盖集：并进来就是把最后那一格说成已确认"
        );
        assert_eq!(
            sc.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 999, end: 999 } }],
            "剩下的活儿必须被派出去，不许因为覆盖集看起来满了就 `Idle`"
        );
        check_invariants(&sc, TOTAL, "seed-ghost-vs-near-full");
        sc.on(Event::Progress { worker: 0, abs_end: 999 });
        sc.on(Event::WindowDone { worker: 0 });
        assert!(sc.is_done(), "补上最后一个字节之后才真的收工");
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: 999 }], "相邻合并成整份文件");
        check_invariants(&sc, TOTAL, "seed-ghost-closed");
    }

    /// 种子在**中间**（两侧都有待下载的缝）时，补集必须是两条缝，且派发按正常的"最长优先"走。
    #[test]
    fn resume_with_a_middle_seed_leaves_both_gaps_and_dispatches_the_longest() {
        const TOTAL: u64 = 1000;
        let mut sc = Scheduler::resume(TOTAL, 600, &[Interval { start: 200, end: 299 }]);
        assert_eq!(sc.pending_bytes(), 900, "[0,199] 与 [300,999] 两条缝都还在");
        assert_eq!(
            sc.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 300, end: 999 } }],
            "两条缝里派最长的那条；已确认的 [200,299] 一个字节都不许再派"
        );
        check_invariants(&sc, TOTAL, "seed-middle");
        // 第二段派剩下的 [0,199]（长度 200 < min_seg 600 ⇒ 不许劈在途尾）。
        assert_eq!(
            sc.on(Event::Connected(1)),
            vec![Action::Assign { worker: 1, iv: Interval { start: 0, end: 199 } }],
            "剩下的起点缝必须原样派出，不被并回已确认字节"
        );
        check_invariants(&sc, TOTAL, "seed-middle-2");
    }

    /// **复审 B-Minor 8 补的那一格**：`resume` 与 `Event::NoRanges` 的组合原先**没有任何用例**打到，
    /// 而 `resume` 的文档（"退化模式（**E7**③）不属于本入口"）只说了这一对不冲突、没说清撞在一起
    /// 时读作什么。这条把它钉住三件事：
    /// 1. **播种留下的多条缝不会因为 `NoRanges` 并成一段** —— `collapse_pending` 只合并相邻/重叠的
    ///    区间，而 `resume` 造出来的补集两两之间都隔着已确认字节 ⇒ 段表**必然**还是多条。
    ///    （派发里那句"塌成一段"只对 `no_ranges_collapses_the_pending_table_into_one_segment`
    ///    那种"队列里的段彼此相邻"的形状成立；控制员给的"多洞种子 ⇒ 塌成一段"在真多洞形状上
    ///    是做不到的，那正是 `push_done`/`collapse_pending` 两条合并规则合起来保护的东西：
    ///    并过去就等于把已确认字节重新算成待下载。）
    /// 2. 退化期的派发按**起点最低**走，不是正常期的"最长优先" —— 这一格两种读法给出的答案不同，
    ///    所以这条用例真能分辨（下面第一枪：最低 `[0,199]`，最长会是 `[300,799]`）。
    /// 3. 退化期一次只一段在途、永不 `Retighten`、已确认字节一个都不重下。
    #[test]
    fn no_ranges_on_a_multi_gap_seed_dispatches_lowest_start_and_never_collapses_across_done() {
        const TOTAL: u64 = 1_000;
        let seed = [Interval { start: 200, end: 299 }, Interval { start: 800, end: 899 }];
        let mut sc = Scheduler::resume(TOTAL, 100, &seed);
        // 播种后的正常读法：三条缝 [0,199] / [300,799] / [900,999]，最长优先会挑中间那条。
        assert_eq!(sc.snapshot_intervals(), seed.to_vec(), "两行不相邻 ⇒ 原样进");
        assert_eq!(sc.pending_bytes(), 800, "200 + 500 + 100 三条缝都还欠着");
        check_invariants(&sc, TOTAL, "seed-multi-gap-before-noranges");

        sc.on(Event::NoRanges);
        assert_eq!(sc.pending_bytes(), 800, "`NoRanges` 只改派法，不许凭空造或吞掉待下载字节");
        assert_eq!(sc.snapshot_intervals(), seed.to_vec(), "已确认字节不许被并进队列（那正是合并规则拦的事）");
        assert!(!sc.is_done(), "三条缝一条都没补 ⇒ 不收工");

        // 判据 2：起点最低，不是最长。
        let acts = sc.on(Event::Connected(0));
        assert_eq!(
            acts,
            vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: 199 } }],
            "退化期派发的判据是起点最低：最长优先会给出 [300,799]，那会把 body 中段写到文件中段"
        );
        // 这一句**被上面那条等式全包**（等式已经钉死 `acts` 只有一条 `Assign` ⇒ 不可能有 `Retighten`）。
        // 留着是有意的（收口复审 B2 第 8 条裁定"不删"）：它是判据 3 里"永不劈分"那一半**唯一有名有姓**的针。
        // 上面那条等式日后若被放宽成 `acts.len()` 或 `any(matches!(Assign))` 这类形状，
        // "退化期会不会劈分"就没人看守了 ⇒ **冗余而无害，但不许撤**；要撤它必须先把上面那条等式钉牢。
        assert!(!acts.iter().any(|a| matches!(a, Action::Retighten { .. })), "退化期永不劈分: {acts:?}");
        assert_eq!(sc.on(Event::Connected(1)), vec![Action::Idle], "退化期一次只允许一段在途");
        check_invariants(&sc, TOTAL, "seed-multi-gap-degraded-1");

        // 三条缝必须按文件顺序一条不漏地派完，而且都不碰已确认字节。
        sc.on(Event::Progress { worker: 0, abs_end: 199 });
        sc.on(Event::WindowDone { worker: 0 });
        assert_eq!(
            sc.snapshot_intervals(),
            vec![Interval { start: 0, end: 299 }, Interval { start: 800, end: 899 }],
            "补完 [0,199] 后与种子首行相邻合并"
        );
        assert_eq!(
            sc.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 300, end: 799 } }],
            "第二条缝：剩下的里起点最低的仍是 [300,799]"
        );
        sc.on(Event::Progress { worker: 0, abs_end: 799 });
        sc.on(Event::WindowDone { worker: 0 });
        assert_eq!(sc.pending_bytes(), 100, "只剩 [900,999]");
        check_invariants(&sc, TOTAL, "seed-multi-gap-degraded-2");
        assert_eq!(
            sc.on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 900, end: 999 } }],
            "第三条缝：已确认的 [200,299] 与 [800,899] 一个字节都没重下"
        );
        sc.on(Event::Progress { worker: 0, abs_end: 999 });
        sc.on(Event::WindowDone { worker: 0 });
        assert!(sc.is_done(), "三条缝补完 ⇒ 收工");
        assert_eq!(sc.snapshot_intervals(), vec![Interval { start: 0, end: TOTAL - 1 }], "相邻合并成整份文件");
        check_invariants(&sc, TOTAL, "seed-multi-gap-degraded-done");
    }

    /// `state::SegView::interval()` 的注释（"Task 8 若要把断点喂回调度器"）假设的门确实通了：
    /// 断点里的段经过 `resume` 之后，`snapshot_intervals()` 报回来的就是它自己（不动一个字段）。
    #[test]
    fn resume_accepts_the_state_segment_shape_verbatim() {
        let segs = [Interval { start: 0, end: 236_031 }, Interval { start: 1_000_000, end: 1_048_575 }];
        let sc = Scheduler::resume(4_194_304, crate::consts::MIN_SEGMENT_BYTES, &segs);
        assert_eq!(sc.snapshot_intervals(), segs.to_vec(), "两行本来就不相邻 ⇒ 原样进、原样出");
        assert_eq!(
            sc.pending_bytes(),
            4_194_304 - segs.iter().map(Interval::len).sum::<u64>(),
            "待下载 = 全长 - 已确认，一条缝都不许多算"
        );
        check_invariants(&sc, 4_194_304, "seed-state-shape");
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
