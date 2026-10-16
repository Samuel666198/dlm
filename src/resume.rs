//! 续传判定：给定"用户这次要什么"（`task::Task`）、"上一轮活下来的记录"（`state::Snapshot`，可能根本没有）、
//! "服务器此刻怎么说"（`ProbeFacts`），回答**续 / 重来 / 拒绝**三选一。
//!
//! 依据：spec §4 末段（凭据硬规约与 `needs_secret`）、spec §6.3（第 110–111 行「续传冲突」）、spec §6.5
//! （第 113 行「最终 URL 记进 `validator.final_url`，续传按它比对」）、spec §3 末段（第 67 行
//! 「续传请求带 `If-Range: <etag>`」—— 本模块的 ③ 就是这条的前提）、plan 第 705–707 行的判定顺序 ①–⑥。
//! 计划勘误与本任务派发裁定：**E6**（模块声明进 `src/lib.rs`）、**E9/R6**（`SegView` 只有 `start`/`end`）、
//! 以及"`ProbeFacts` 住在本文件"（见下）。
//!
//! ## 本模块不做 I/O
//!
//! 签名里没有任何路径、句柄或网络类型：`decide` 是纯函数 ⇒ 全表可以在单元层跑完，不需要 harness。
//! 读断点文件（`state::load`）与试探服务器（Task 6 的 `probe::run`）都在调用方（Task 8 的 engine）手里；
//! `--no-resume`（spec §4：无视已存在的断点文件）同样归调用方 —— 它只要把 `existing` 传成 `None`，
//! 本模块就给 `Fresh { reason: None }`。这里**不读** `task.flags` 里的 `FLAG_KEEP_STATE`，那是收尾的活。
//!
//! ## `ProbeFacts` 为什么住在 `resume.rs` 而不是 `probe.rs`
//!
//! 它是"判定用的事实"，不是"试探这个动作"：`probe::ProbeResult`（Task 6）要 `use crate::resume::ProbeFacts`
//! 并提供 `facts()`。于是依赖边是 `probe → resume`，而 `resume` 只向下碰 `task` 与 `state`
//! （`consts::FLAG_IGNORE_MTIME` 经 `task.ignore_mtime()` 间接生效，本文件只有测试直接点名它），不成环。
//! 计划第 46 行那条 `cli → task → probe → resume → scheduler` 说的是 main 的组装顺序，
//! 不是 crate 内唯一允许的边。
//!
//! ## 判定顺序与每一步的读法（逐条有用例钉着，见 `mod tests`）
//!
//! | 步 | 判据 | 结果 |
//! |---|---|---|
//! | — | `existing` 为 `None`（没有断点文件，含 `--no-resume`） | `Fresh { reason: None }` |
//! | ① | `snap.needs_secret && task.password.is_none()` | `Abort(原因)` |
//! | ② | `snap.validator.final_url != probe.final_url` | `Fresh { reason }` |
//! | ③ | 第一步：两边都有 ETag 且不等 ⇒ `Fresh`（`ignore_mtime` 救不了这一条）。第二步：**不论 ETag 是相等还是缺**，再按 `Option` 整体比一次 Last-Modified，不等 ⇒ `Fresh`；本次任务带 `ignore_mtime` 时整步跳过 | 任一步不符则 `Fresh { reason }` |
//! | ④ | `snap.validator.total_len != probe.total_len` | `Fresh { reason }` |
//! | ⑤ | `!probe.accept_ranges && snap.segments.len() > 1` | `Fresh { reason }` |
//! | ⑥ | 以上全过 | `Resume(snap)`（原样交回，不改一个字节） |
//!
//! 顺序是有意的：越靠前的判据越根本。口令缺失（①）时谈"资源是不是同一个"没有意义；重定向换了目标（②）
//! 意味着后面的比对对象都可能已经不是同一个资源；ETag/mtime（③）判"同一个资源"，长度（④）判"同一份内容"，
//! Range（⑤）只判"这份进度还接得上吗"。多处同时不符时，交出去的原因是**第一条**不符的那条 ——
//! 用例 `when_several_rules_disagree_the_first_one_wins` 钉住这一点（Task 9 只打一行，理由给错方向最误导人）。
//!
//! ### ③ 的四种存在组合（规则原文只写了"两者都有 / 任一缺失"两支，这里必须补齐）
//!
//! 记断点里的值为 `S`、服务器现在给的是 `P`。两步，**顺序执行、不互斥**：
//!
//! **第一步（ETag，只在 `S.etag` 与 `P.etag` 都有时可比）**
//! - 都有且不等 ⇒ `Fresh`，且 `ignore_mtime` **救不了**这一条（spec §6.3 括号那句「ETag 变了仍重来」）。
//! - 都有且相等 ⇒ 记下"ETag 这关过了"，继续第二步。
//! - 任一边缺 ETag ⇒ 无从比对，`If-Range` 也就没有强验证子可用，继续第二步。
//!
//! **第二步（Last-Modified，`Option<String>` 整体比；本次任务带 `FLAG_IGNORE_MTIME` 时整步跳过）**
//! - `Some(x) == Some(x)` ⇒ 过，落到 ④。
//! - `Some(x) != Some(y)` ⇒ `Fresh{原因}`。
//! - **一边有、一边没有** ⇒ 也算不符，`Fresh`。理由：`--ignore-mtime` 之外没有任何旗标能解释
//!   "服务器开始发 / 不再发 Last-Modified"，而断点当时是按有（或没有）这个值才敢确认字节的 ——
//!   验证前提本身变了就是不符。
//! - `None == None`（两边都没有任何验证子）⇒ ③ 无事可做，落到 ④ 用长度兜底 —— 见「裁定 2」。
//!
//! **裁定 1（ETag 相等时还要不要比 Last-Modified）**：要。计划第 706 行那句「ETag 两者都有则只比 ETag」
//! 的字面读法与两处更权威的文本**直接冲突**，同一组输入给出相反结论：
//! - spec §6.3（设计稿第 110–111 行）：「服务器 `ETag`/`Last-Modified` 与断点不符 → 默认从头重来」——
//!   两个验证子是并列的，不符任一个就重来；`--ignore-mtime` 只豁免 Last-Modified 那一个。
//! - 简报自己的分支 3 与分支 4：断点 `val("E","L2")` 对上 `base`（`etag: Some("E")`，两边 ETag **相等**），
//!   分支 4 要求 `task_plain` ⇒ `Fresh`、分支 3 要求带 `FLAG_IGNORE_MTIME` ⇒ `Resume`。
//!   按"只比 ETag"读，分支 4 必红（那支断点 ETag 相等，根本没有可拦的东西）。
//!
//! 取 spec + 测试，不取那句计划散文。**代价如实记下**：ETag 已是强验证子时，一次无害的 mtime 抖动
//! （重新发布、CDN 刷新、`touch`）仍会换来整份重下；用户可以用 `--ignore-mtime` 明确豁免，
//! 而豁免不影响 ETag 那一步。计划散文建议改为「ETag 两者都有则**以 ETag 为准（不等即重来，ignore_mtime 无效）**，
//! 之后再按 `ignore_mtime` 决定是否比 Last-Modified」—— 见报告里的勘误候选 E10。
//!
//! **裁定 2（两边都没有任何验证子）**：`S`、`P` 的 ETag 与 Last-Modified 全缺 ⇒ 只用 ④ 的长度判。
//! 简报的七支没覆盖这一支。取"放行到 ④"而不是"一律重来"，理由：这种服务器上一轮也拿不到任何可比对的值，
//! 拒了就没有任何旗标能让它续传（`--ignore-mtime` 帮不上，因为压根没有 mtime 可比），而用户的代价是
//! 一整份重下且没有解释；长度 + 最终 URL 是这条链路上仅剩的、也确实是 spec 让 ④ 承担的那道兜底。
//!
//! **裁定 3（`ignore_mtime` 取自哪里）**：只取**本次** `task.flags` 的 `FLAG_IGNORE_MTIME`
//! （即 `task.ignore_mtime()`）。`snap.validator.ignore_mtime` 是"上一轮用没用这个判据"的**记录**，
//! 归展示与排障，不参与判定 —— 用户这次加不加 `--ignore-mtime` 是他此刻的决定，不该被旧文件反向绑架，
//! 反过来也不能让旧文件替他做一个他没要求的宽松判定。两个方向都有用例。
//!
//! **裁定 4（⑤ 的边界是"段数 > 1"，单行照续）**：不认 Range 的服务器每次只能从 0 交整份 body，
//! 一轮进度因此最多是一条连续区间。**单行**的断点在这种服务器上接得上：E7③ 明写
//! 「对端不认 Range 时 `iv.start > 0` 的行是真会出现的」，并把"先丢掉 body 前 `iv.start` 字节"定为
//! Task 7 的义务 ⇒ 起点不是 0 的单行照样能续。**多行**意味着断点里有互不相连的洞，而不认 Range 的对端
//! 没法只填某个洞：每一枪都要从 0 重交一遍，已确认的洞两侧都会被覆写。故取 ⑤ 的字面边界：`> 1` 重来，
//! `0` 或 `1` 行放行到 ⑥。
//!
//! ### ① 的措辞要求
//!
//! spec §4：「续传时必须重新给口令，**否则拒绝续传并说明原因**」。原因要说清"缺什么、怎么补"，
//! 且**绝不引用 `task.password` 的值**（`Task` 的手写 `Debug` 掩码的是另一条通道；这里根本不需要值，
//! 断点文件里也没有：`TaskView` 少这个字段，E9）。

use crate::state::Snapshot;
use crate::task::Task;

/// 试探（Task 6）之后关于远端文件**此刻**的事实，即 ②③④⑤ 的比对面。
///
/// 字段逐字按 plan 第 673 行的 `Produces`。值都来自 HTTP 响应头，语义由 Task 6 负责（本模块只比字符串）：
/// - `etag` / `last_modified`：原样保留响应头的值（含引号），**不做归一化** —— 弱 ETag `W/"x"` 与 `"x"`
///   在这里是两个不同的值。要放宽请在 probe 那侧决定并写进它的文档，别在这里悄悄改。
/// - `total_len`：`Content-Range` 的总长或 `Content-Length`（退化路径）。
/// - `final_url`：跟随重定向之后的落地 URL（spec §6.5：续传按它比对）。
/// - `accept_ranges`：服务器是否认 `Range`（`Accept-Ranges: bytes`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFacts {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub total_len: u64,
    pub final_url: String,
    pub accept_ranges: bool,
}

/// `decide` 的三选一结论。
///
/// `Fresh` 带**可选**原因：只有"本来就没有断点"这一种情况是 `None`（没有需要解释的冲突），
/// 其余每一处冲突都必须给出人话 —— Task 9 会在中途重来时把这句直接打给用户。
///
/// `#[allow(clippy::large_enum_variant)]`：它量出 360 字节并建议 `Resume(Box<Snapshot>)`，但那要改的正是
/// 简报 `Produces` 写死的对外形状（plan 第 673 行 `Resume(Snapshot)`），而且 Task 8 拿到结论后本来就要
/// **接管**整份快照去改 `segments`/`stats` —— 那时它必须 `*box` 出来，堆上那一跳只是把一次 memcpy
/// 换成一次 allocation。一个任务只判一次（不是热循环），取契约与调用方的简单，不取这个微优化。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 无视断点、从 0 开始。`reason` 见 `Decision` 的文档。
    Fresh { reason: Option<String> },
    /// 按这份断点继续。快照**原样交回**（本模块不改它任何字段，也不去碰 `segments`）。
    Resume(Snapshot),
    /// 拒绝开工：断点需要口令而本次没给。调用方应报错退出，**不要**退化成"从头下载"
    /// （从头下也要同一个口令才拿得到资源，悄悄重下等于把用户的凭据要求吞掉）。
    Abort(String),
}

/// 纯函数判定。`existing` = `state::load(state::path_for(&task.target_path()))?` 的结果，
/// `probe` = `probe::run(..)?.facts()`；`--no-resume` 由调用方折成 `existing = None`（spec §4）。
pub fn decide(task: &Task, existing: Option<Snapshot>, probe: &ProbeFacts) -> Decision {
    // 没有断点文件（或 `--no-resume` 把它折成了"没有"）⇒ 全新任务。这里**没有**冲突可言，
    // 所以 `reason: None`：Task 9 不该为一件没发生的事打印解释。
    let Some(snap) = existing else {
        return Decision::Fresh { reason: None };
    };

    // ① spec §4 末段：口令不落盘 ⇒ 续传必须重新给。给不出就**拒绝**，不替用户退化成"从头下载"
    //    （从头下同样要过认证，悄悄重下只会让服务器再吃一次匿名请求，然后报一个更难懂的错）。
    if snap.needs_secret && task.password.is_none() {
        return Decision::Abort(
            "这份断点记录写着当初带了口令才拿得到资源，而口令不写进断点文件：请用 --password 重新给出同一口令\
             （不想续这份断点就删掉它的 .dlm.json）".into(),
        );
    }

    // ② spec §6.5：跟随重定向后把最终 URL 记进 `validator.final_url`，续传按它比对。
    //    换了落地地址 = 后面每一条比对的对象都可能不是同一个资源，所以排在验证子之前。
    if snap.validator.final_url != probe.final_url {
        return fresh(format!(
            "最终 URL 与断点不符（断点 {} → 现在 {}）：重定向把请求带到了别处，已确认的字节不再属于同一个资源，从头下载",
            snap.validator.final_url, probe.final_url
        ));
    }

    // ③ 验证子。spec §6.3 是两步并列的：**先**用 ETag 定"是不是同一个资源"（不等即重来，`ignore_mtime`
    //    在这一条上说话不算），**再**按 `ignore_mtime` 决定是否比 Last-Modified。
    //    计划第 706 行「ETag 两者都有则只比 ETag」的字面读法与 spec §6.3 和简报自己的分支 3/4 冲突，
    //    取 spec + 测试（裁定 1，展开的四种存在组合见模块文档）。
    if let (Some(saved), Some(now)) = (snap.validator.etag.as_deref(), probe.etag.as_deref())
        && saved != now
    {
        return fresh(format!(
            "ETag 与断点不符（断点 {saved:?} → 现在 {now:?}）：服务器说这是另一个资源，已确认的字节不再算数，从头下载\
             （--ignore-mtime 只豁免 Last-Modified，不豁免这一条）"
        ));
    }
    // Last-Modified 按 `Option<String>` 整体比：`None == None`（两把验证子都没有 ⇒ ③ 无事可做，交给 ④
    // 兜底，裁定 2）、`Some(x) == Some(x)` 放行、不等或**一边有一边没有**都算不符。
    // `ignore_mtime` 取**本次**任务的旗标（`FLAG_IGNORE_MTIME`），不取 `snap.validator.ignore_mtime`
    // 那份历史记录 —— 裁定 3。
    if !task.ignore_mtime() && snap.validator.last_modified != probe.last_modified {
        return fresh(format!(
            "Last-Modified 与断点不符（断点 {:?} → 现在 {:?}）：文件被改过，从头下载；\
             确认那只是无害的时间戳抖动可以加 --ignore-mtime",
            snap.validator.last_modified, probe.last_modified
        ));
    }

    // ④ 长度。③ 放行的任何组合最后都由它兜底（③ 通过后 `segments` 的边界仍在这份长度之内：
    //    `state::check_segments` 已保证每行 `end < 断点长度`，而这一条要求两个长度相等）。
    if snap.validator.total_len != probe.total_len {
        return fresh(format!(
            "文件长度与断点不符（断点 {} 字节 → 现在 {} 字节）：服务器上的文件变了，从头下载",
            snap.validator.total_len, probe.total_len
        ));
    }

    // ⑤ 不认 Range 的对端每次只能从 0 交整份 body ⇒ 它填不了断点里的洞。边界取"段数 > 1"：
    //    单行（哪怕起点不是 0）在退化模式下接得上，那是 E7③ 交给 Task 7 的义务（丢前缀），裁定 4。
    if !probe.accept_ranges && snap.segments.len() > 1 {
        return fresh(format!(
            "服务器现在不支持 Range，而断点里有 {} 段互不相连的进度：这种进度接不上，从头下载",
            snap.segments.len()
        ));
    }

    // ⑥ 全部判据都同意：原样交回，一个字段都不改（账归 Task 8 记）。
    Decision::Resume(snap)
}

/// `Fresh` 但一定带原因：②③④⑤ 每一条都必须说得出"哪条判据不同意"（Task 9 会打这句）。
fn fresh(reason: String) -> Decision {
    Decision::Fresh { reason: Some(reason) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::FLAG_IGNORE_MTIME;
    use crate::scheduler::Interval;
    use crate::state::{SegView, Validator};

    const URL: &str = "https://x/a";
    const SECRET: &str = "hunter2";

    fn task_with(flags: u32, password: Option<&str>) -> Task {
        Task {
            url: URL.into(),
            referer: None,
            cookies: None,
            data_file: None,
            user: None,
            password: password.map(str::to_string),
            dir: ".".into(),
            name: None,
            flags,
        }
    }

    /// 简报的 `task()`：**带** `FLAG_IGNORE_MTIME`（简报分支 3 的注释点名要求）。
    fn task() -> Task {
        task_with(FLAG_IGNORE_MTIME, None)
    }

    /// 简报的 `task_plain()`：一个 flag 都不带。
    fn task_plain() -> Task {
        task_with(0, None)
    }

    fn facts(etag: Option<&str>, last_modified: Option<&str>, total_len: u64, final_url: &str, accept_ranges: bool) -> ProbeFacts {
        ProbeFacts {
            etag: etag.map(str::to_string),
            last_modified: last_modified.map(str::to_string),
            total_len,
            final_url: final_url.into(),
            accept_ranges,
        }
    }

    /// 简报的 `base`：两把验证子都有、长度 100、认 Range。
    fn base() -> ProbeFacts {
        facts(Some("E"), Some("L"), 100, URL, true)
    }

    fn val_opt(etag: Option<&str>, last_modified: Option<&str>, total_len: u64, final_url: &str) -> Validator {
        Validator {
            etag: etag.map(str::to_string),
            last_modified: last_modified.map(str::to_string),
            total_len,
            final_url: final_url.into(),
            ignore_mtime: false,
        }
    }

    /// 简报的 `val(...)`：两把验证子都给的形状（`ignore_mtime` 落盘为 `false`，取值见 `val_opt`）。
    fn val(etag: &str, last_modified: &str, total_len: u64, final_url: &str) -> Validator {
        val_opt(Some(etag), Some(last_modified), total_len, final_url)
    }

    fn snap(v: Validator) -> Snapshot {
        Snapshot::stub_for_test(v, vec![])
    }

    /// `n` 行**互不相邻**的覆盖集：行间留 10 字节以上的洞，因为相邻两行是 `state::check_segments`
    /// 直接拒掉的形状（磁盘上不可能存在），而 `stub_for_test` 与它同一条闸 ⇒ 写相邻行会当场 panic。
    fn snap_multi(v: Validator, n: usize) -> Snapshot {
        let span = (v.total_len as usize) / n.max(1);
        assert!(span > 10, "夹具：total_len={} 撑不出 {n} 行带洞的表", v.total_len);
        let rows = (0..n)
            .map(|i| SegView::from(Interval { start: (i * span) as u64, end: (i * span + 9) as u64 }))
            .collect::<Vec<_>>();
        let got = Snapshot::stub_for_test(v, rows);
        assert_eq!(got.segments.len(), n, "夹具：段数必须真的是 {n}（⑤ 判的就是这个数）");
        got
    }

    /// 取结论里的那句话（`Resume` 没有理由 ⇒ `None`）。
    fn reason(d: &Decision) -> Option<&str> {
        match d {
            Decision::Fresh { reason } => reason.as_deref(),
            Decision::Abort(why) => Some(why.as_str()),
            Decision::Resume(_) => None,
        }
    }

    // ---- brief Step 1：七支全表（分支编号与注释逐条对应简报第 683–700 行）----

    #[test]
    fn decides_across_all_validator_combinations() {
        let p = base();
        // 1 无断点 ⇒ Fresh
        assert!(matches!(decide(&task(), None, &p), Decision::Fresh { .. }));
        // 2 ETag 变 ⇒ Fresh 且带原因
        let d = decide(&task(), Some(snap(val("E2", "L", 100, URL))), &p);
        assert!(matches!(d, Decision::Fresh { reason: Some(_) }), "{d:?}");
        // 3 只有 Last-Modified 变 + ignore_mtime ⇒ Resume
        assert!(
            matches!(decide(&task(), Some(snap(val("E", "L2", 100, URL))), &p), Decision::Resume(_)),
            "task() 需带 FLAG_IGNORE_MTIME"
        );
        // 4 只有 Last-Modified 变、未开 ignore ⇒ Fresh
        assert!(
            matches!(decide(&task_plain(), Some(snap(val("E", "L2", 100, URL))), &p), Decision::Fresh { .. }),
            "没开 ignore_mtime 时 Last-Modified 变了必须重来"
        );
        // 5 总长变 ⇒ Fresh
        assert!(
            matches!(decide(&task_plain(), Some(snap(val("E", "L", 99, URL))), &p), Decision::Fresh { .. }),
            "长度变了必须重来"
        );
        // 6 needs_secret 且本次没给口令 ⇒ Abort
        let mut s = snap(val("E", "L", 100, URL));
        s.needs_secret = true;
        assert!(
            matches!(decide(&task_plain(), Some(s.clone()), &p), Decision::Abort(_)),
            "缺口令要拒绝续传，而不是悄悄从头下"
        );
        // 7 服务器不支持 Range 但断点多段 ⇒ Fresh
        let mut b = base();
        b.accept_ranges = false;
        assert!(
            matches!(decide(&task_plain(), Some(snap_multi(val("E", "L", 100, URL), 2)), &b), Decision::Fresh { .. }),
            "不认 Range 却留着多段进度 ⇒ 重来"
        );
        // 同一份多段断点在认 Range 的服务器下必须**能**续（挡掉"⑤ 写成无条件拒绝"的伪装修法）
        assert!(
            matches!(decide(&task_plain(), Some(snap_multi(val("E", "L", 100, URL), 2)), &base()), Decision::Resume(_)),
            "{:?}：Range 可用时多段进度是本工具的正常形状",
            decide(&task_plain(), Some(snap_multi(val("E", "L", 100, URL), 2)), &base())
        );
    }

    // ---- ③ 的完整组合表（含简报没写的"一边缺失"与"两边全缺"）----

    /// (断点 etag, 断点 last_modified, 服务器 etag, 服务器 last_modified, 本次 ignore_mtime, 期望)
    /// 期望：`"RESUME"`，或 `Fresh` 原因里必须出现的判据名。
    type Matrix = (Option<&'static str>, Option<&'static str>, Option<&'static str>, Option<&'static str>, bool, &'static str);

    #[test]
    fn every_etag_and_last_modified_presence_combination_is_ruled() {
        let cases: [Matrix; 18] = [
            // 两边都有 ETag：ETag 是**决定性**的那一步（不等即 Fresh，ignore_mtime 无效）
            (Some("E"), Some("L"), Some("E"), Some("L"), false, "RESUME"),
            (Some("E2"), Some("L"), Some("E"), Some("L"), false, "ETag"),
            (Some("E2"), Some("L"), Some("E"), Some("L"), true, "ETag"), // 裁定 1：旗标救不了 ETag
            (Some("E2"), Some("L2"), Some("E"), Some("L"), true, "ETag"), // 同上，mtime 也变了仍先说 ETag
            // ETag 相等之后**仍比** Last-Modified（裁定 1 / spec §6.3 / 简报分支 3+4）
            (Some("E"), Some("L2"), Some("E"), Some("L"), false, "Last-Modified"), // = 简报分支 4
            (Some("E"), Some("L2"), Some("E"), Some("L"), true, "RESUME"), // = 简报分支 3
            (Some("E"), Some("L"), Some("E"), None, false, "Last-Modified"), // 一边有一边没有，ETag 相等也拦
            (Some("E"), Some("L"), Some("E"), None, true, "RESUME"),
            (Some("E"), None, Some("E"), None, false, "RESUME"), // 都有 etag、都没有 mtime ⇒ None==None 放行
            // 任一 ETag 缺失 ⇒ 只剩 Last-Modified 可比
            (None, Some("L"), Some("E"), Some("L"), false, "RESUME"), // 断点没记 etag，mtime 相等即放行
            (None, Some("L"), Some("E"), Some("L2"), false, "Last-Modified"),
            (None, Some("L"), Some("E"), Some("L2"), true, "RESUME"), // 整支跳过
            (Some("E"), Some("L"), None, Some("L"), false, "RESUME"), // 服务器不再发 etag
            (Some("E"), Some("L"), None, None, false, "Last-Modified"), // 一边有一边没有 ⇒ 不符
            (Some("E"), Some("L"), None, None, true, "RESUME"),
            (None, None, Some("E"), Some("L"), false, "Last-Modified"), // 断点没记 mtime、服务器现在有
            // 两把验证子全缺 ⇒ ③ 无事可做，交给 ④ 的长度兜底（裁定 2）
            (None, None, None, None, false, "RESUME"),
            (None, None, None, None, true, "RESUME"),
        ];
        for (s_etag, s_lm, p_etag, p_lm, ignore, want) in cases {
            let t = if ignore { task() } else { task_plain() };
            let s = snap(val_opt(s_etag, s_lm, 100, URL));
            let pr = facts(p_etag, p_lm, 100, URL, true);
            let d = decide(&t, Some(s.clone()), &pr);
            match want {
                "RESUME" => assert!(
                    matches!(d, Decision::Resume(_)),
                    "断点({s_etag:?},{s_lm:?}) vs 服务器({p_etag:?},{p_lm:?}) ignore={ignore} 该续，实际 {d:?}"
                ),
                keyword => {
                    let Some(text) = reason(&d) else {
                        panic!("断点({s_etag:?},{s_lm:?}) vs 服务器({p_etag:?},{p_lm:?}) ignore={ignore} 该 Fresh 并给原因，实际 {d:?}");
                    };
                    assert!(
                        matches!(d, Decision::Fresh { .. }),
                        "这一支必须是 Fresh（不是 Abort/Resume）：{d:?}"
                    );
                    assert!(
                        text.contains(keyword),
                        "断点({s_etag:?},{s_lm:?}) vs 服务器({p_etag:?},{p_lm:?}) ignore={ignore} 的原因要说出是 {keyword} 不对：{text}"
                    );
                }
            }
        }
    }

    // ---- ② 最终 URL ----

    #[test]
    fn a_different_final_url_is_a_fresh_with_the_url_in_the_reason() {
        let p = facts(Some("E"), Some("L"), 100, "https://cdn.example/b.bin", true);
        let d = decide(&task_plain(), Some(snap(val("E", "L", 100, URL))), &p);
        let text = reason(&d).expect("重定向换了目标也要给原因");
        assert!(matches!(d, Decision::Fresh { .. }), "{d:?}");
        assert!(text.contains("URL"), "原因要说出是最终 URL 不对：{text}");
        assert!(
            text.contains(URL) && text.contains("https://cdn.example/b.bin"),
            "原因要把两个 URL 都写出来，用户才知道服务器把他带去了哪：{text}"
        );
        // ignore_mtime 与 ETag 相等都救不了这一支（② 排在 ③ 之前）
        let mut s = snap(val("E", "L", 100, URL));
        s.validator.ignore_mtime = true;
        assert!(matches!(decide(&task(), Some(s), &p), Decision::Fresh { .. }));
    }

    // ---- ① needs_secret 的两个方向 ----

    #[test]
    fn a_stored_secret_must_be_given_again_and_the_message_says_how() {
        let mut s = snap(val("E", "L", 100, URL));
        s.needs_secret = true;
        for t in [task_plain(), task()] {
            let d = decide(&t, Some(s.clone()), &base());
            let Decision::Abort(why) = d else {
                panic!("缺口令必须是 Abort，不能退化成 Fresh/Resume：{d:?}");
            };
            assert!(why.contains("口令"), "原因要说明缺的是口令：{why}");
            assert!(why.contains("--password"), "原因要告诉用户怎么补：{why}");
            assert!(why.contains("断点"), "原因要说清是这份断点记录在提要求，而不是一句\"需要口令\"：{why}");
            assert!(!why.contains('\n'), "原因是一行话（Task 9 直接打在进度行里）：{why}");
        }
        // 反方向：补了口令就正常续传（本模块拿不到旧口令，**无法**比对，只做"有没有给"这一件事）。
        let with_secret = task_with(0, Some(SECRET));
        assert!(
            matches!(decide(&with_secret, Some(s.clone()), &base()), Decision::Resume(_)),
            "重新给了口令就该继续，别再拦"
        );
        // 断点没记 needs_secret 时，带不带口令都照续（不能反过来要求一个并不需要的口令）。
        let plain = snap(val("E", "L", 100, URL));
        assert!(matches!(decide(&task_plain(), Some(plain.clone()), &base()), Decision::Resume(_)));
        assert!(matches!(decide(&with_secret, Some(plain), &base()), Decision::Resume(_)));
    }

    #[test]
    fn needs_secret_without_a_state_file_is_not_an_abort() {
        // ① 只在"有断点可谈"时才可能触发；没有断点就是全新任务（Fresh，无原因）。
        let d = decide(&task_plain(), None, &base());
        assert!(matches!(d, Decision::Fresh { reason: None }), "{d:?}");
    }

    // ---- ⑤ 的边界：单行必须能续（派发点名的可达组合）----

    #[test]
    fn one_row_progress_resumes_even_when_the_server_refuses_ranges() {
        let mut no_ranges = base();
        no_ranges.accept_ranges = false;
        // 起点为 0 的一条连续前缀：不认 Range 的服务器本来就只能产出这种形状。
        let prefix = snap_with_rows(100, &[(0, 59)]);
        assert!(matches!(decide(&task_plain(), Some(prefix), &no_ranges), Decision::Resume(_)));
        // 起点不是 0 的一条：E7③ 明写这种行在退化模式里真会出现（Task 7 负责丢前缀），必须照样能续。
        let tail = snap_with_rows(100, &[(40, 99)]);
        assert!(
            matches!(decide(&task_plain(), Some(tail), &no_ranges), Decision::Resume(_)),
            "裁定 4：单行不以 0 开头也要放行给 Task 7 的退化路径处理"
        );
        // 空表（什么都没确认过）不是"多段"。
        assert!(matches!(decide(&task_plain(), Some(snap(val("E", "L", 100, URL))), &no_ranges), Decision::Resume(_)));
        // 三行带洞才是 ⑤ 要拦的形状；边界取 >1，两行就该拦。
        let two = snap_with_rows(100, &[(0, 9), (50, 59)]);
        let d = decide(&task_plain(), Some(two), &no_ranges);
        let text = reason(&d).expect("⑤ 重来也要给原因");
        assert!(matches!(d, Decision::Fresh { .. }), "{d:?}");
        assert!(text.contains("Range"), "原因要说出是 Range/段数这条判据：{text}");
    }

    fn snap_with_rows(total_len: u64, spec: &[(u64, u64)]) -> Snapshot {
        let rows = spec
            .iter()
            .map(|&(start, end)| SegView::from(Interval { start, end }))
            .collect::<Vec<_>>();
        Snapshot::stub_for_test(val("E", "L", total_len, URL), rows)
    }

    // ---- 裁定 3：只有本次任务的旗标说话，旧文件里记的那份不参与判定 ----

    #[test]
    fn only_this_runs_ignore_mtime_flag_decides_not_the_recorded_one() {
        let changed_mtime = snap(val("E", "L2", 100, URL));
        // 旧文件记了 ignore_mtime=true，本次没开 ⇒ 仍按本次的严格判据重来。
        let mut recorded_true = changed_mtime.clone();
        recorded_true.validator.ignore_mtime = true;
        assert!(
            matches!(decide(&task_plain(), Some(recorded_true), &base()), Decision::Fresh { .. }),
            "旧文件的宽松记录不能替用户做一次他没要求的放行"
        );
        // 旧文件记 false，本次开了 --ignore-mtime ⇒ 按本次的宽松判据续。
        assert!(
            !changed_mtime.validator.ignore_mtime,
            "夹具：简报的 val(...) 落盘的 ignore_mtime 是 false"
        );
        assert!(
            matches!(decide(&task(), Some(changed_mtime), &base()), Decision::Resume(_)),
            "本次的旗标才说话（简报分支 3 正是这一支）"
        );
    }

    // ---- 冲突的多处不符：最先那条说话 ----

    #[test]
    fn when_several_rules_disagree_the_first_one_wins() {
        // ②+③+④ 同时不符 ⇒ 说最终 URL（最根本的那条）。
        let p = facts(Some("E"), Some("L"), 999, "https://x/other.bin", false);
        let d = decide(&task_plain(), Some(snap(val("E2", "L9", 100, URL))), &p);
        let text = reason(&d).expect("多处不符也要给原因");
        assert!(text.contains("URL"), "该先说最终 URL：{text}");
        assert!(!text.contains("ETag") && !text.contains("长度"), "原因只能有一条：{text}");
        // ③+④ 同时不符 ⇒ 说 ETag。
        let p2 = facts(Some("E"), Some("L"), 999, URL, true);
        let d2 = decide(&task_plain(), Some(snap(val("E2", "L9", 100, URL))), &p2);
        let text2 = reason(&d2).expect("ETag 与长度同时不符也要给原因");
        assert!(text2.contains("ETag") && !text2.contains("长度"), "该先说 ETag：{text2}");
        // ① 排在最前：缺口令时连比对都不做。
        let mut s = snap(val("E2", "L9", 100, URL));
        s.needs_secret = true;
        assert!(matches!(decide(&task_plain(), Some(s), &p), Decision::Abort(_)), "① 必须最先");
    }

    // ---- 交回给 Task 8 的东西：快照原样、原因能读 ----

    #[test]
    fn resume_hands_the_snapshot_back_untouched() {
        let original = snap_multi(val("E", "L", 100, URL), 3);
        let d = decide(&task(), Some(original.clone()), &base());
        let Decision::Resume(got) = d else { panic!("这一支该续：{d:?}") };
        assert_eq!(got, original, "判定模块不许改快照（stats/segments/validator 都得原样）");
        assert_eq!(got.segments.len(), 3);
        assert_eq!(got.stats.bytes_duplicated, 0, "计数归 Task 8 累加，本模块不动");
    }

    #[test]
    fn every_conflict_reason_is_a_single_human_sentence() {
        let mut no_ranges = base();
        no_ranges.accept_ranges = false;
        // 旧文件记了 ignore_mtime=true、本次没开、mtime 又变了 ⇒ 仍然 Fresh（裁定 3 的那一支）。
        let mut s_len = snap(val("E", "L2", 100, URL));
        s_len.validator.ignore_mtime = true;
        let mut secret = snap(val("E", "L", 100, URL));
        secret.needs_secret = true;
        let cases: Vec<(Task, Snapshot, ProbeFacts, &str)> = vec![
            (task_plain(), snap(val("E2", "L", 100, URL)), base(), "ETag"),
            (task_plain(), snap(val("E", "L2", 100, URL)), base(), "Last-Modified"),
            (task_plain(), snap(val("E", "L", 99, URL)), base(), "长度"),
            (task_plain(), snap(val("E", "L", 100, "https://x/elsewhere")), base(), "URL"),
            (task_plain(), snap_multi(val("E", "L", 100, URL), 2), no_ranges, "Range"),
            (task_plain(), secret, base(), "口令"),
            (task_plain(), s_len, facts(Some("E"), Some("L"), 100, URL, true), "Last-Modified"),
        ];
        for (t, s, p, keyword) in cases {
            let d = decide(&t, Some(s), &p);
            let text = reason(&d).unwrap_or_else(|| panic!("{keyword} 的冲突必须给出一句话原因：{d:?}"));
            assert!(!text.is_empty(), "原因不能是空串：{d:?}");
            assert!(!text.contains('\n'), "原因必须是一行：{text}");
            assert!(
                text.chars().count() <= 200,
                "原因太长了，进度行放不下：{} 字：{text}",
                text.chars().count()
            );
            assert!(text.contains(keyword), "原因要说出是哪条判据不同意（{keyword}）：{text}");
        }
    }

    #[test]
    fn no_decision_message_ever_carries_the_password_value() {
        // 派发第 5 条：消息只说"缺什么"，绝不把口令带出去。`Task` 的 `Debug` 是掩码的，
        // 所以就算哪天有人把 `{task:?}` 抄进原因里也不会漏 —— 这条挡的是**明文**那条路。
        let mut secret = snap(val("E", "L", 100, URL));
        secret.needs_secret = true;
        let given = task_with(FLAG_IGNORE_MTIME, Some(SECRET));
        let battery: Vec<(Task, Option<Snapshot>, ProbeFacts)> = vec![
            (given.clone(), Some(secret.clone()), base()),
            (given.clone(), Some(secret), facts(Some("E2"), Some("L"), 9, "https://x/z", false)),
            (
                given.clone(),
                Some(snap(val("E2", "L2", 9, "https://x/z"))),
                facts(Some("E"), Some("L"), 100, URL, true),
            ),
            (task_with(0, Some(SECRET)), Some(snap(val("E", "L2", 100, URL))), base()),
            (task_with(0, Some(SECRET)), None, base()),
        ];
        for (t, s, p) in battery {
            let d = decide(&t, s, &p);
            if let Some(text) = reason(&d) {
                assert!(!text.contains(SECRET), "决策原因里出现了口令本尊：{text}");
            }
            assert!(!format!("{d:?}").contains(SECRET), "Debug 里也不该有（{d:?}）");
        }
    }

    // ---- 类型契约：Task 6 要能按这个名字拼出 ProbeFacts ----

    #[test]
    fn probe_facts_and_decision_shapes_are_the_ones_task_6_and_9_can_use() {
        let p = ProbeFacts {
            etag: Some("E".into()),
            last_modified: Some("L".into()),
            total_len: 100,
            final_url: URL.into(),
            accept_ranges: true,
        };
        assert_eq!(p, base(), "字段名/类型即 plan 第 673 行的 Produces，且逐字段可比");
        let fresh = Decision::Fresh { reason: None };
        assert!(matches!(fresh, Decision::Fresh { reason: None }));
        let abort = Decision::Abort("x".into());
        assert_eq!(abort.clone(), Decision::Abort("x".into()));
        assert_ne!(fresh, abort);
    }

    // ---- 长度兜底真的在（④ 与 ⑤/③ 都不冲突时的独立一支）----

    #[test]
    fn total_len_is_the_last_line_of_defence_when_no_validator_exists() {
        // 裁定 2 的另一半：两把验证子全缺时，长度仍然拦得住 —— 不是"没验证子就无条件续"。
        let s = snap(val_opt(None, None, 100, URL));
        let ok = decide(&task_plain(), Some(s.clone()), &facts(None, None, 100, URL, true));
        assert!(matches!(ok, Decision::Resume(_)), "{ok:?}");
        let short = decide(&task_plain(), Some(s), &facts(None, None, 101, URL, true));
        let text = reason(&short).expect("长度不符要给原因");
        assert!(matches!(short, Decision::Fresh { .. }), "{short:?}");
        assert!(text.contains("长度"), "原因要说 100 → 101 是长度不符：{text}");
        // 边界：0 字节的资源 + 空段表也能续（别让"长度为 0"退化成"肯定不符"）。
        let empty = snap(val_opt(None, None, 0, URL));
        assert!(matches!(decide(&task_plain(), Some(empty), &facts(None, None, 0, URL, false)), Decision::Resume(_)));
    }
}
