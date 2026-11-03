//! 续传判定：给定"用户这次要什么"（`task::Task`）、"上一轮活下来的记录"（`state::Snapshot`，可能根本没有）、
//! "服务器此刻怎么说"（`ProbeFacts`），回答**续 / 重来 / 拒绝**三选一。
//!
//! 依据：spec §4 末段（凭据硬规约与 `needs_secret`）、spec §6.3（第 110–111 行「续传冲突」）、spec §6.5
//! （第 113 行「最终 URL 记进 `validator.final_url`，续传按它比对」）、spec §3 末段（第 67 行
//! 「续传请求带 `If-Range: <etag>`」—— 本模块的 ③ 就是这条的前提）、spec §2.2（POST 形状是一等公民，
//! ①b 因此存在）、plan 第 705–707 行的判定顺序 ①–⑥。
//! 计划勘误与本任务派发裁定：**E6**（模块声明进 `src/lib.rs`）、**E9/R6**（`SegView` 只有 `start`/`end`）、
//! **E10**（③ 的两步并列读法，已批准 ⇒ 本模块的 ③ 保持原样）、Task 5 评审 **F1** 的裁定（①b：判
//! "请求本身变没变"住在本模块，字段集见下）、以及"`ProbeFacts` 住在本文件"（见下）。
//!
//! ## 本模块不做 I/O
//!
//! 签名里没有任何路径、句柄或网络类型：`decide` 是纯函数 ⇒ 全表可以在单元层跑完，不需要 harness。
//! 读断点文件（`state::load`）与试探服务器（Task 6 的 `probe::run`）都在调用方（Task 8 的 engine）手里；
//! `--no-resume`（spec §4：无视已存在的断点文件）同样归调用方 —— 它只要把 `existing` 传成 `None`，
//! 本模块就给 `Fresh { reason: None }`。这里**不读** `task.flags` 里的 `FLAG_KEEP_STATE`，那是收尾的活。
//! ①b 比 `data_file` 时比的是**路径字符串**（`Option<PathBuf>` 整体相等），既不 `stat` 也不打开那个文件：
//! 所以"同名文件内容被改过"这一支本模块发现不了（要闭合它得把内容散列落盘，设计稿没给这个字段）。
//! 这与"不比 `password` 值"同一类：都是纯函数边界买不到的东西，登记给下游（见「①b 的字段集」）。
//!
//! ## `ProbeFacts` 为什么住在 `resume.rs` 而不是 `probe.rs`
//!
//! 它是"判定用的事实"，不是"试探这个动作"：`probe::ProbeResult`（Task 6）要 `use crate::resume::ProbeFacts`
//! 并提供 `facts()`。于是依赖边是 `probe → resume`，而 `resume` 向下碰 `task` 与 `state`
//! （`consts::FLAG_IGNORE_MTIME` 经 `task.ignore_mtime()` 间接生效，本文件只有测试直接点名它）。
//!
//! **T99-J1 §2 起本文件多一条 crate 内唯一的反向边：`resume → probe`。** 它是读侧那道闸接在
//! "打原因之前"那一步的结果，取舍写清楚：
//! - 判据**必须**与写侧同一个函数（`probe::carries_our_password`）。复制一份到本文件就是两本账，
//!   而"骗过写侧的那一份"正是读侧要挡的东西 —— 两份实现迟早分家，分家的那一格就是回显口令的那一格；
//! - 另一个可选位置是 `state::load` 之后（调用方 `engine::run`），但 `engine.rs` 不在这一发射程，
//!   而且在 load 处**改写**快照会破掉 ⑥"原样交回，一个字段都不改"这条既有契约（用例
//!   `resume_hands_the_snapshot_back_untouched` 钉着），还会让"读侧顺手改了账"变成引擎随后落盘的
//!   第二份真值；就地掩码只在**拼那句原因**这一步动，快照本体一个字不改。
//!   要消除这条反向边，判据应当搬到 `task`（唯一同时在下层的公共件）—— 本发按"不许动 `src/task.rs`"
//!   没有搬，登记给后续（报告"这一发仍看不见什么"）。
//!
//! Rust 允许 crate 内模块互相引用，这条边不成编译问题；不成环的那句旧话随此改准。
//! 计划第 46 行那条 `cli → task → probe → resume → scheduler` 说的是 main 的组装顺序，
//! 不是 crate 内唯一允许的边。
//!
//! ## 判定顺序与每一步的读法（逐条有用例钉着，见 `mod tests`）
//!
//! | 步 | 判据 | 结果 |
//! |---|---|---|
//! | — | `existing` 为 `None`（没有断点文件，含 `--no-resume`） | `Fresh { reason: None }` |
//! | ① | `snap.needs_secret && task.password.is_none()` | `Abort(原因)` |
//! | ①b | 断点里的请求视图 `snap.task` 与本次 `task` 相比，`url`/`referer`/`cookies`/`data_file`/`user` 任一处不符（**不比** `flags`、`dir`/`name`、`password`：见「①b 的字段集」） | `Fresh { reason }` |
//! | ② | `snap.validator.final_url != probe.final_url` | `Fresh { reason }` |
//! | ③ | 第一步：两边都有 ETag 且不等 ⇒ `Fresh`（`ignore_mtime` 救不了这一条）。第二步：**不论 ETag 是相等还是缺**，再按 `Option` 整体比一次 Last-Modified，不等 ⇒ `Fresh`；本次任务带 `ignore_mtime` 时整步跳过 | 任一步不符则 `Fresh { reason }` |
//! | ④ | `snap.validator.total_len != probe.total_len` | `Fresh { reason }` |
//! | ⑤ | `!probe.accept_ranges && snap.segments.len() > 1` | `Fresh { reason }` |
//! | ⑥ | 以上全过 | `Resume(snap)`（原样交回，不改一个字节） |
//!
//! 顺序是有意的：越靠前的判据越根本。口令缺失（①）时谈"资源是不是同一个"没有意义；请求本身换了（①b）
//! 时连"服务器那份文件变没变"都无从谈起 —— 两侧确认字节的**不是同一次请求**；重定向换了目标（②）
//! 意味着后面的比对对象都可能已经不是同一个资源；ETag/mtime（③）判"同一个资源"，长度（④）判"同一份内容"，
//! Range（⑤）只判"这份进度还接得上吗"。多处同时不符时，交出去的原因是**第一条**不符的那条 ——
//! 用例 `when_several_rules_disagree_the_first_one_wins` 钉住这一点（Task 9 只打一行，理由给错方向最误导人）。
//!
//! ### 读侧那道闸：②③ 那三条原因里的**值**（T99-J1 §2，复审 Q3-1）
//!
//! 写侧闸（`probe` 的两处装填口）管的是"本轮服务器交回的事实能不能收下"，**管不到磁盘上那份
//! 历史**：`resume::decide` 的 ② 与 ③ 两句把 `snap.validator.final_url`／`etag`／`last_modified`
//! **原样打进终端**（复审实测两格：`断点 http://127.0.0.1:56815/x/hunter2 → 现在 …`、
//! `断点 "\"hunter2\"" → 现在 "\"fresh-etag\""`）。于是 pre-H2 写进去的、或用**当时没覆盖到**的变形
//! 骗过写侧闸写进去的口令，下一轮就被回显 —— README 那句"断点文件里永远没有口令"的读者读的是文件，
//! 而文件是历史写的。
//!
//! 现行口径（三条，都有用例钉着）：
//! 1. 那三个字段各过**同一份**内容判据（`probe::carries_our_password`，含 T99-J1 §1 的第二组）；
//!    命中 ⇒ 那句原因换成**两个值都不回显**的版本，仍然给出"哪条判据不同意"（结论仍是 `Fresh`）。
//! 2. **未命中时那三条原因逐字不变**：诊断不许被这道闸吃掉（复审 I-3 同一条论证 —— 一句不回显的
//!    错话与一句吃掉原因的话是同一族缺陷）。用例
//!    `a_saved_validator_value_without_our_password_still_echoes_both_sides` 就是这一条的正向对照。
//! 3. 为什么是"照常决策"而不是"拒跑"：口令印到终端与任务能不能跑是两件事，这一格的用户形状是
//!    "重跑一次同一条命令"；把它变成 `Abort` 只是把外泄换成一次无谓的失败。
//!    ⚠ 反面照实登记：本次没给口令时（`task.password: None`）判据恒假 —— 那时 ① 的
//!    `needs_secret` 早就先 `Abort`（那份断点记着"当初带了口令"），这一族拿不到值可比；
//!    而旧文件里存的是**另一个**口令时读侧也不认识它（判据按本任务那份口令的形状比），
//!    那一格登记在报告"这一发仍看不见什么"，要闭合它得把口令或其散列落盘，与 spec §4 硬规约冲突。
//!
//! ### ①b 的字段集：为什么正好是这五个（评审 F1）
//!
//! ②③④⑤ 全部只在问"服务器那份文件变没变"，① 只问"口令给没给"。剩下第三件事没人管：**这次要发出的
//! 请求，是不是当初确认这些字节的那一个**。它不是纸上风险 —— spec §2.2 把 POST 形状列为一等公民：
//! 同 `--url`、同落地 URL，服务器对 POST 不回 `ETag`/`Last-Modified`（很常见），上一轮 `--data-file bodyA`、
//! 这一轮 `--data-file bodyB` ⇒ ③ 无事可做、④ 长度照样相等、⑤ 不触发 ⇒ 只能由 ①b 拦。放过去的后果就是
//! 设计稿第 67 行给 `If-Range` 写的那句话所指的**静默错误**，只不过这次"变"发生在客户端这一侧。
//!
//! - **比**：`url`、`referer`、`cookies`、`data_file`、`user` —— 五个都会进入请求、都能改变响应的字节。
//! - **不比 `flags`**：简报自己的分支 3 就是"用户重跑时加了 `--ignore-mtime`，而正是这个旗标让它续得起来"。
//!   断点里的 `TaskView` 存的是**上一轮**的旗标，不可能带着这一轮新加的那一位；让它参与比较，分支 3 会
//!   先变成 ①b 的 `Fresh` ⇒ `--ignore-mtime` 在续传路径上**永久失效**。用例
//!   `flags_are_deliberately_outside_the_request_comparison` 两个方向都钉住了。
//! - **不比 `dir` / `name`**：断点文件的路径由目标路径派生（`state::path_for` = `<目标>.dlm.json`，与目标
//!   同目录）⇒ 换落点意味着**读的是另一个断点文件**（多半读不到，走"没有断点"那一支），这里没有东西可
//!   对账。`a_different_output_location_is_not_a_request_change` 挡的是"后来人顺手把比较集扩成整个
//!   `TaskView`"。
//! - **不比 `password`**：`TaskView` 里没有这个字段（spec §4 硬规约：口令不落盘）⇒ 本模块**拿不到值可比**。
//!
//! **已知盲区（照实登记，不要以为 ①b 覆盖了全部凭据变更）**：上一轮 `needs_secret=true`、这一轮又给了
//! 一个**不同的**口令时，① 只看"给没给"、①b 看不到值，本模块**发现不了**口令换过。要闭合它只能把口令
//! （或其散列）落盘，那与 spec §4 的硬规约直接冲突 ⇒ 维持现状，把这一条记进 T8/T9 的账（同一位用户换
//! 口令通常伴随 401，那时由重试路径处理）。
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

use crate::probe::carries_our_password;
use crate::state::{Snapshot, TaskView, Validator};
use crate::task::Task;

/// 试探（Task 6）之后关于远端文件**此刻**的事实，即 ②③④⑤ 的比对面。
///
/// 字段逐字按 plan 第 673 行的 `Produces`。值都来自 HTTP 响应头，语义由 Task 6 负责（本模块只比字符串）：
/// - `etag` / `last_modified`：原样保留响应头的值（含引号），**不做归一化** —— 弱 ETag `W/"x"` 与 `"x"`
///   在这里是两个不同的值。要放宽请在 probe 那侧决定并写进它的文档，别在这里悄悄改。
/// - `total_len`：`Content-Range` 的总长或 `Content-Length`（退化路径）。
/// - `final_url`：跟随重定向之后的落地 URL（spec §6.5：续传按它比对）。
/// - `accept_ranges`：服务器**认不认 `Range`** —— ⚠ **不是**"`Accept-Ranges` 头写了 `bytes` 吗"。
///   **E11③/T6 评审 I1** 把它定成**行为加权的合取**：头说 `bytes` **且**这一次真按我们发的 `Range` 答了 206
///   才算真（判据住在 `probe::peer_accepts_ranges`，读法住在 `probe` 那份唯一的"响应头 → `ProbeFacts`"里）。
///   只看头会把"头说支持、实际发整条 200"的对端误判成支持 ⇒ 引擎会开着窗口化形状去打一条不认 Range 的线。
///   ⇒ **Task 9 与任何后来读者都不许把这行读成"抄一个响应头"**；E11 把这条转派给 Task 8 时漏的就是这一句（评审 E 第 3 条）。
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

/// 这一轮续传该发的 `If-Range` 值；`None` = 不发（退回探针先比、窗口照常的旧路径）。
///
/// 强 ETag 优先：RFC 9110 的 If-Range 只接受强验证器，`W/"..."` 与不带引号的串都不发。
/// Last-Modified 只在本次**没开** `--ignore-mtime` 时用 —— 开了还发它，等于让对端按时间戳
/// 把"用户明确说可以忽略"的变化重新判成重来。
pub fn if_range_value(validator: &Validator, ignore_mtime: bool) -> Option<String> {
    if let Some(etag) = validator.etag.as_deref() {
        let etag = etag.trim();
        if etag.len() >= 2 && etag.starts_with('"') && etag.ends_with('"') {
            return Some(etag.to_string());
        }
    }
    if !ignore_mtime && let Some(lm) = validator.last_modified.as_deref() {
        let lm = lm.trim();
        if !lm.is_empty() {
            return Some(lm.to_string());
        }
    }
    None
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

    // ①b 评审 F1：**请求本身**变了。① 只问"口令给没给"，②③④⑤ 全都只问"服务器那份文件变没变"，
    //    而续传还依赖第三件事：**这次要发出的请求，和当初确认这些字节的那个请求，是同一个**。
    //    具体可达形状（spec §2.2 把 POST 列为一等公民）：同 `url`、同 `final_url`，服务器不发
    //    `ETag`/`Last-Modified`（POST 响应非常常见），上一轮 `--data-file bodyA`、这一轮 `bodyB`
    //    （或换了 `--cookie`/`--referer`）⇒ ③ 整步无事可做、④ 的长度照样可能相等、⑤ 不触发，
    //    没有这一步就会返回 `Resume`：把**另一个请求确认过的字节**接着用 —— 正是设计稿第 67 行
    //    「文件已变但仍按旧段续 会变成静默错误」要挡的那件事，只是这次"变"在客户端这一侧。
    //    比哪些字段（以及**刻意不比**哪些）见 `changed_request` 的文档。
    if let Some(part) = changed_request(&snap.task, task) {
        return fresh(format!(
            "请求与断点不符：换了 {part} —— 已确认的字节出自另一个请求，续传会把两次请求的结果拼在一起，从头下载"
        ));
    }

    // ② spec §6.5：跟随重定向后把最终 URL 记进 `validator.final_url`，续传按它比对。
    //    换了落地地址 = 后面每一条比对的对象都可能不是同一个资源，所以排在验证子之前。
    //    ⚠ **读侧那道闸**（T99-J1 §2）：`snap.validator` 里那三份值不是本轮服务器交的，是**上一轮
    //    写进文件的历史**（pre-H2 写的那份、或用当时没覆盖到的变形骗过写侧闸写进去的那份都可能
    //    带着口令）。命中 ⇒ 结论照旧 `Fresh`（读侧的目的就是"别把口令印到终端"，不是"别让任务跑"），
    //    被换掉的只有句子里的**值**；未命中 ⇒ 下面那三条原因逐字不变，**诊断一口都不许吃掉**。
    if snap.validator.final_url != probe.final_url {
        if carries_our_password(&snap.validator.final_url, task.password.as_deref()) {
            return fresh(
                "最终 URL 与断点不符（断点里那一份带着本任务的凭据形状，两个值都不回显）：\
                 断点记的落地地址与现在这条不同，已确认的字节不再属于同一个资源，从头下载"
                    .into(),
            );
        }
        return fresh(format!(
            "最终 URL 与断点不符（断点 {} → 现在 {}）：重定向把请求带到了别处，已确认的字节不再属于同一个资源，从头下载",
            snap.validator.final_url, probe.final_url
        ));
    }

    // ③ 验证子。spec §6.3 是两步并列的：**先**用 ETag 定"是不是同一个资源"（不等即重来，`ignore_mtime`
    //    在这一条上说话不算），**再**按 `ignore_mtime` 决定是否比 Last-Modified。
    //    计划第 706 行「ETag 两者都有则只比 ETag」的字面读法与 spec §6.3 和简报自己的分支 3/4 冲突，
    //    取 spec + 测试（裁定 1，展开的四种存在组合见模块文档）。
    //    同一份读侧闸：`saved`（断点里那份 ETag）命中 ⇒ 两个值都不回显（③ 那句 `{saved:?} → {now:?}`
    //    正是 T99-H2 §3 给验证子立闸时点名的外泄口 —— 那边管住的是**本轮**装填，这一句管住的是**旧文件**）。
    if let (Some(saved), Some(now)) = (snap.validator.etag.as_deref(), probe.etag.as_deref())
        && saved != now
    {
        if carries_our_password(saved, task.password.as_deref()) {
            return fresh(
                "ETag 与断点不符（断点里那一份带着本任务的凭据形状，两个值都不回显）：服务器说这是另一个资源，\
                 已确认的字节不再算数，从头下载（--ignore-mtime 只豁免 Last-Modified，不豁免这一条）"
                    .into(),
            );
        }
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
        if snap
            .validator
            .last_modified
            .as_deref()
            .is_some_and(|saved| carries_our_password(saved, task.password.as_deref()))
        {
            return fresh(
                "Last-Modified 与断点不符（断点里那一份带着本任务的凭据形状，两个值都不回显）：文件被改过，\
                 从头下载；确认那只是无害的时间戳抖动可以加 --ignore-mtime"
                    .into(),
            );
        }
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

/// ①b 的判据：断点里那份请求视图（`snap.task`）与本次 `task` 相比，**第一条**不符的字段。
///
/// 只比这五个，因为判据要的是"对端交出的字节是不是同一批"，而这五个字段都会进入请求、都能改变响应：
/// `url`、`referer`、`cookies`、`data_file`、`user`。
///
/// 刻意**不**比的三处（每一处都有用例挡"以后有人顺手把比较集扩大"）：
/// - `flags`：简报自己的分支 3 就是"用户重跑时加了 `--ignore-mtime`，而正是这个旗标让它续得起来"。
///   断点里的 `TaskView` 存的是**上一轮**的旗标，不可能带着这一轮新加的那一位 ⇒ 一旦参与比较，
///   加分支 3 会变成 ①b 的 `Fresh`，`--ignore-mtime` 在续传路径上就**永久失效**。其余两位
///   （`--keep-state`、`--no-start`）同样不改变请求内容（用例
///   `flags_are_deliberately_outside_the_request_comparison`）。
/// - `dir` / `name`：断点文件的路径是**由目标路径派生**的（`state::path_for`：`<目标>.dlm.json`，与目标
///   同目录），所以换输出目录/换名意味着读的是**另一个**断点文件（通常读不到 ⇒ 走"没有断点"那一支），
///   这里没有需要对账的东西（用例 `a_different_output_location_is_not_a_request_change`）。
/// - `password`：`TaskView` 里**根本没有**这个字段（spec §4 的硬规约：口令不落盘），本模块拿不到值可比。
///   ① 管的是"要不要给"，管不到"给的是不是同一个口令" ⇒ 这是本模块的**已知盲区**（见模块文档）。
///
/// 只报第一条：五处不符给的是同一个结论（`Fresh`），Task 9 只打得下一行。
/// 报的是**字段名 + 对应参数**而不是字段值：`cookies`/`referer` 都可能带着凭据（`state.rs` 第 2 条把
/// 这个风险登记给了 Task 9），把值抄进原因等于替它再多开一条外传通道。
fn changed_request(saved: &TaskView, now: &Task) -> Option<&'static str> {
    if saved.url != now.url {
        Some("URL（--url）")
    } else if saved.referer != now.referer {
        Some("Referer（--referer）")
    } else if saved.cookies != now.cookies {
        Some("Cookie（--cookie）")
    } else if saved.data_file != now.data_file {
        Some("POST 体（--data-file）")
    } else if saved.user != now.user {
        Some("用户名（--user）")
    } else {
        None
    }
}

/// `Fresh` 但一定带原因：①b②③④⑤ 每一条都必须说得出"哪条判据不同意"（Task 9 会打这句）。
fn fresh(reason: String) -> Decision {
    Decision::Fresh {
        reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{FLAG_ADD_ONLY, FLAG_IGNORE_MTIME, FLAG_KEEP_STATE};
    use crate::scheduler::Interval;
    use crate::state::{SegView, TaskView, Validator};

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

    fn facts(
        etag: Option<&str>,
        last_modified: Option<&str>,
        total_len: u64,
        final_url: &str,
        accept_ranges: bool,
    ) -> ProbeFacts {
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

    fn val_opt(
        etag: Option<&str>,
        last_modified: Option<&str>,
        total_len: u64,
        final_url: &str,
    ) -> Validator {
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
        assert!(
            span > 10,
            "夹具：total_len={} 撑不出 {n} 行带洞的表",
            v.total_len
        );
        let rows = (0..n)
            .map(|i| {
                SegView::from(Interval {
                    start: (i * span) as u64,
                    end: (i * span + 9) as u64,
                })
            })
            .collect::<Vec<_>>();
        let got = Snapshot::stub_for_test(v, rows);
        assert_eq!(
            got.segments.len(),
            n,
            "夹具：段数必须真的是 {n}（⑤ 判的就是这个数）"
        );
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
            matches!(
                decide(&task(), Some(snap(val("E", "L2", 100, URL))), &p),
                Decision::Resume(_)
            ),
            "task() 需带 FLAG_IGNORE_MTIME"
        );
        // 4 只有 Last-Modified 变、未开 ignore ⇒ Fresh
        assert!(
            matches!(
                decide(&task_plain(), Some(snap(val("E", "L2", 100, URL))), &p),
                Decision::Fresh { .. }
            ),
            "没开 ignore_mtime 时 Last-Modified 变了必须重来"
        );
        // 5 总长变 ⇒ Fresh
        assert!(
            matches!(
                decide(&task_plain(), Some(snap(val("E", "L", 99, URL))), &p),
                Decision::Fresh { .. }
            ),
            "长度变了必须重来"
        );
        // 6 needs_secret 且本次没给口令 ⇒ Abort
        let mut s = snap(val("E", "L", 100, URL));
        s.needs_secret = true;
        assert!(
            matches!(
                decide(&task_plain(), Some(s.clone()), &p),
                Decision::Abort(_)
            ),
            "缺口令要拒绝续传，而不是悄悄从头下"
        );
        // 7 服务器不支持 Range 但断点多段 ⇒ Fresh
        let mut b = base();
        b.accept_ranges = false;
        assert!(
            matches!(
                decide(
                    &task_plain(),
                    Some(snap_multi(val("E", "L", 100, URL), 2)),
                    &b
                ),
                Decision::Fresh { .. }
            ),
            "不认 Range 却留着多段进度 ⇒ 重来"
        );
        // 同一份多段断点在认 Range 的服务器下必须**能**续（挡掉"⑤ 写成无条件拒绝"的伪装修法）
        assert!(
            matches!(
                decide(
                    &task_plain(),
                    Some(snap_multi(val("E", "L", 100, URL), 2)),
                    &base()
                ),
                Decision::Resume(_)
            ),
            "{:?}：Range 可用时多段进度是本工具的正常形状",
            decide(
                &task_plain(),
                Some(snap_multi(val("E", "L", 100, URL), 2)),
                &base()
            )
        );
    }

    // ---- ③ 的完整组合表（含简报没写的"一边缺失"与"两边全缺"）----

    /// (断点 etag, 断点 last_modified, 服务器 etag, 服务器 last_modified, 本次 ignore_mtime, 期望)
    /// 期望：`"RESUME"`，或 `Fresh` 原因里必须出现的判据名。
    type Matrix = (
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        bool,
        &'static str,
    );

    #[test]
    fn every_etag_and_last_modified_presence_combination_is_ruled() {
        let cases: [Matrix; 18] = [
            // 两边都有 ETag：ETag 是**决定性**的那一步（不等即 Fresh，ignore_mtime 无效）
            (Some("E"), Some("L"), Some("E"), Some("L"), false, "RESUME"),
            (Some("E2"), Some("L"), Some("E"), Some("L"), false, "ETag"),
            (Some("E2"), Some("L"), Some("E"), Some("L"), true, "ETag"), // 裁定 1：旗标救不了 ETag
            (Some("E2"), Some("L2"), Some("E"), Some("L"), true, "ETag"), // 同上，mtime 也变了仍先说 ETag
            // ETag 相等之后**仍比** Last-Modified（裁定 1 / spec §6.3 / 简报分支 3+4）
            (
                Some("E"),
                Some("L2"),
                Some("E"),
                Some("L"),
                false,
                "Last-Modified",
            ), // = 简报分支 4
            (Some("E"), Some("L2"), Some("E"), Some("L"), true, "RESUME"), // = 简报分支 3
            (
                Some("E"),
                Some("L"),
                Some("E"),
                None,
                false,
                "Last-Modified",
            ), // 一边有一边没有，ETag 相等也拦
            (Some("E"), Some("L"), Some("E"), None, true, "RESUME"),
            (Some("E"), None, Some("E"), None, false, "RESUME"), // 都有 etag、都没有 mtime ⇒ None==None 放行
            // 任一 ETag 缺失 ⇒ 只剩 Last-Modified 可比
            (None, Some("L"), Some("E"), Some("L"), false, "RESUME"), // 断点没记 etag，mtime 相等即放行
            (
                None,
                Some("L"),
                Some("E"),
                Some("L2"),
                false,
                "Last-Modified",
            ),
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
                        panic!(
                            "断点({s_etag:?},{s_lm:?}) vs 服务器({p_etag:?},{p_lm:?}) ignore={ignore} 该 Fresh 并给原因，实际 {d:?}"
                        );
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
        assert!(matches!(
            decide(&task(), Some(s), &p),
            Decision::Fresh { .. }
        ));
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
            assert!(
                why.contains("断点"),
                "原因要说清是这份断点记录在提要求，而不是一句\"需要口令\"：{why}"
            );
            assert!(
                !why.contains('\n'),
                "原因是一行话（Task 9 直接打在进度行里）：{why}"
            );
        }
        // 反方向：补了口令就正常续传（本模块拿不到旧口令，**无法**比对，只做"有没有给"这一件事）。
        let with_secret = task_with(0, Some(SECRET));
        assert!(
            matches!(
                decide(&with_secret, Some(s.clone()), &base()),
                Decision::Resume(_)
            ),
            "重新给了口令就该继续，别再拦"
        );
        // 断点没记 needs_secret 时，带不带口令都照续（不能反过来要求一个并不需要的口令）。
        let plain = snap(val("E", "L", 100, URL));
        assert!(matches!(
            decide(&task_plain(), Some(plain.clone()), &base()),
            Decision::Resume(_)
        ));
        assert!(matches!(
            decide(&with_secret, Some(plain), &base()),
            Decision::Resume(_)
        ));
    }

    #[test]
    fn needs_secret_without_a_state_file_is_not_an_abort() {
        // ① 只在"有断点可谈"时才可能触发；没有断点就是全新任务（Fresh，无原因）。
        let d = decide(&task_plain(), None, &base());
        assert!(matches!(d, Decision::Fresh { reason: None }), "{d:?}");
    }

    // ---- ①b 请求本身变了（评审 F1：这一条原先没人管）----

    /// 把快照里的 `task` 视图换成**上一轮那次真实的请求**（其余字段取自 `stub_for_test`）。
    ///
    /// `stub_for_test` 落的是 `url = validator.final_url` 的占位值，把"用户请求的 URL"与"重定向落地的
    /// URL"混成一件事 —— ①b 比的就是这个视图，所以凡涉及"请求/落地地址不一致"的夹具都必须由本函数
    /// 显式给出上一轮的请求形状，否则用例是在对一个盘上不可能的形状成立（同 `state.rs` 的 F2 理由）。
    fn snap_after(v: Validator, prev: &Task) -> Snapshot {
        let mut s = Snapshot::stub_for_test(v, vec![]);
        s.task = TaskView::from(prev);
        s
    }

    /// 上一轮那次请求的"最小形状"：不带任何旗标，除要比的那一个字段外全空（= `task_plain()`）。
    /// 名字叫出来是为了让 ①b 的每组用例读起来是"上一轮 vs 这一轮"，而不是两个不相干的夹具。
    fn plain_prev_run() -> Task {
        task_plain()
    }

    #[test]
    fn a_changed_request_field_is_a_fresh_even_when_every_validator_agrees() {
        // 这是 F1 的具体可达形状：同 url、同 final_url、服务器不发 ETag/Last-Modified（POST 的常见形状），
        // 于是 ③ 无事可做、④ 长度照样相等、⑤ 空表放行 —— 除了 ①b，没有任何判据能发现"确认字节的那个
        // 请求已经换了"。§3 第 67 行要挡的静默错误正是这一支。
        let p = facts(None, None, 100, URL, true);
        let cases: Vec<(Task, Task, &str)> = vec![
            // (上一轮的请求, 本次的请求, 原因里必须点名的那一部分)
            (
                plain_prev_run(),
                Task {
                    url: "https://x/a?retry=2".into(),
                    ..plain_prev_run()
                },
                "URL",
            ),
            (
                Task {
                    referer: Some("https://ref/a".into()),
                    ..plain_prev_run()
                },
                plain_prev_run(),
                "Referer",
            ),
            (
                Task {
                    cookies: Some("sid=42".into()),
                    ..plain_prev_run()
                },
                plain_prev_run(),
                "Cookie",
            ),
            (
                Task {
                    data_file: Some("bodyA".into()),
                    ..plain_prev_run()
                },
                Task {
                    data_file: Some("bodyB".into()),
                    ..plain_prev_run()
                },
                "POST",
            ),
            (
                Task {
                    data_file: Some("bodyA".into()),
                    ..plain_prev_run()
                },
                plain_prev_run(),
                "POST",
            ),
            (
                Task {
                    user: Some("alice".into()),
                    ..plain_prev_run()
                },
                plain_prev_run(),
                "用户",
            ),
        ];
        for (prev, now, keyword) in cases {
            // 对照：请求字段没变的同一份夹具必须能续 —— 少了这条，上面的断言可能在测夹具本身。
            let same = decide(
                &prev,
                Some(snap_after(val_opt(None, None, 100, URL), &prev)),
                &p,
            );
            assert!(
                matches!(same, Decision::Resume(_)),
                "请求没变时这一支该续（否则下面的断言测的是别的东西）：{same:?}"
            );

            let d = decide(
                &now,
                Some(snap_after(val_opt(None, None, 100, URL), &prev)),
                &p,
            );
            let text =
                reason(&d).unwrap_or_else(|| panic!("请求变了也要给原因（{keyword}）：{d:?}"));
            assert!(
                matches!(d, Decision::Fresh { .. }),
                "换了请求字段必须 Fresh，实际 {d:?}"
            );
            assert!(
                text.contains(keyword),
                "原因要说出是哪一部分的请求变了（{keyword}）：{text}"
            );
            assert!(!text.contains('\n'), "原因必须是一行：{text}");
            assert!(
                text.chars().count() <= 200,
                "原因太长了，进度行放不下：{text}"
            );
        }
    }

    #[test]
    fn the_request_change_reason_names_the_part_and_never_echoes_the_credential() {
        // ①b 的判据是"哪个字段变了"，字段值不需要给用户看：`cookies`/`referer` 都可能带着凭据
        // （cookie 是凭据这条风险登记在 `state.rs` 第 2 条，处置归 Task 9），把它们抄进原因等于
        // 替它再多开一条外传通道；`--url` 那一支用户自己刚敲过，更没必要复述。
        // 每支**只改一个字段**：①b 报的是"第一条不符"，同时改两处会把原因归到别的头上。
        let cases: Vec<(Task, Task, &str)> = vec![
            (
                Task {
                    referer: Some("https://ref/?t=OLDTOKEN".into()),
                    ..plain_prev_run()
                },
                plain_prev_run(),
                "Referer",
            ),
            (
                Task {
                    cookies: Some("sid=OLDTOKEN".into()),
                    ..plain_prev_run()
                },
                Task {
                    cookies: Some("sid=NEWTOKEN".into()),
                    ..plain_prev_run()
                },
                "Cookie",
            ),
            (
                Task {
                    user: Some("alice".into()),
                    ..plain_prev_run()
                },
                Task {
                    user: Some("bob".into()),
                    ..plain_prev_run()
                },
                "用户",
            ),
            (
                Task {
                    data_file: Some("bodyA".into()),
                    ..plain_prev_run()
                },
                Task {
                    data_file: Some("bodyB".into()),
                    ..plain_prev_run()
                },
                "POST",
            ),
        ];
        for (prev, now, keyword) in cases {
            let d = decide(
                &now,
                Some(snap_after(val("E", "L", 100, URL), &prev)),
                &base(),
            );
            let text = reason(&d).unwrap_or_else(|| panic!("{keyword} 变了要给原因：{d:?}"));
            assert!(text.contains(keyword), "原因该点名 {keyword}：{text}");
            for value in [
                "OLDTOKEN", "NEWTOKEN", "alice", "bob", "bodyA", "bodyB", "sid=",
            ] {
                assert!(
                    !text.contains(value),
                    "原因里把字段的值抄出去了（{value}）：{text}"
                );
                assert!(
                    !format!("{d:?}").contains(value),
                    "Debug 里也不该有（{value}）：{d:?}"
                );
            }
        }
    }

    #[test]
    fn the_two_headline_shapes_from_the_finding_are_caught_and_name_their_flag() {
        // 派发点名的两条具体路径各钉一支，并要求原因把**参数名**写出来（用户要能照着改）。
        let p = facts(None, None, 100, URL, true); // 没有验证子、长度还照样相等
        let saved_post = Task {
            data_file: Some("bodyA".into()),
            ..plain_prev_run()
        };
        let now_post = Task {
            data_file: Some("bodyB".into()),
            ..plain_prev_run()
        };
        let d = decide(
            &now_post,
            Some(snap_after(val_opt(None, None, 100, URL), &saved_post)),
            &p,
        );
        let text = reason(&d).expect("换了 POST 体也要给原因");
        assert!(
            matches!(d, Decision::Fresh { .. }),
            "F1 的正主：bodyA 确认的字节不能接着给 bodyB 用，{d:?}"
        );
        assert!(
            text.contains("--data-file"),
            "原因要指名是哪个参数变了：{text}"
        );

        let saved_cookie = Task {
            cookies: Some("sid=42".into()),
            ..plain_prev_run()
        };
        let now_cookie = plain_prev_run(); // 本次没带 cookie
        let d = decide(
            &now_cookie,
            Some(snap_after(val_opt(None, None, 100, URL), &saved_cookie)),
            &p,
        );
        let text = reason(&d).expect("换了 Cookie 也要给原因");
        assert!(
            matches!(d, Decision::Fresh { .. }),
            "同一份文件在另一个身份下可能是另一个资源：{d:?}"
        );
        assert!(
            text.contains("--cookie"),
            "原因要指名是哪个参数变了：{text}"
        );
    }

    #[test]
    fn flags_are_deliberately_outside_the_request_comparison() {
        // 裁定：**不比 `flags`**，这不是风格问题。简报分支 3 就是"用户重跑时加了 `--ignore-mtime`，
        // 而正是这个旗标让它能续"：断点里存的 `flags` 不可能带着这个旗标，一旦参与比较，加旗标就会先
        // 撞上"请求变了"⇒ 该旗标在续传路径上永久失效。
        let prev = plain_prev_run(); // flags = 0
        let now = task(); // flags = FLAG_IGNORE_MTIME
        assert_eq!(now.flags, FLAG_IGNORE_MTIME, "夹具：两个任务只差那一个旗标");
        // 请求字段全同、mtime 真的变了 ⇒ 说话的是 ③b 的旗标豁免，不是 ①b。
        assert!(
            matches!(
                decide(
                    &now,
                    Some(snap_after(val("E", "L2", 100, URL), &prev)),
                    &base()
                ),
                Decision::Resume(_)
            ),
            "加了 --ignore-mtime 反而变成 Fresh，就是这个旗标被 ①b 吃掉了"
        );
        // 反方向：上一轮开了旗标、这一轮没开，其余全等（mtime 也相等）⇒ 同样不是"请求变了"。
        assert!(
            matches!(
                decide(
                    &prev,
                    Some(snap_after(val("E", "L", 100, URL), &now)),
                    &base()
                ),
                Decision::Resume(_)
            ),
            "旧文件带过的旗标不该在本次反向绑架判定（这里没有任何不符可言）"
        );
        // 其余旗标（完成后保留断点 / 只入队不下）都不影响对端返回什么字节 ⇒ 照样不参与比较。
        let others = task_with(FLAG_KEEP_STATE | FLAG_ADD_ONLY, None);
        assert_eq!(
            others.flags,
            prev.flags ^ (FLAG_KEEP_STATE | FLAG_ADD_ONLY),
            "夹具：只动了旗标"
        );
        assert!(
            matches!(
                decide(
                    &others,
                    Some(snap_after(val("E", "L", 100, URL), &prev)),
                    &base()
                ),
                Decision::Resume(_)
            ),
            "旗标差异一律不构成 Fresh"
        );
    }

    #[test]
    fn a_different_output_location_is_not_a_request_change() {
        // 裁定：**不比 `dir` / `name`**。断点文件的路径是由目标路径派生的（`state::path_for`：
        // `<目标>.dlm.json`，与目标同目录），所以换输出目录/换名意味着**读的是另一个断点文件**（多半
        // 读不到），这里根本没有需要对账的东西；把它们纳入比较只会让"换个目录继续下"变成整份重下。
        // 这条用例是给后来人挡"顺手把比较集扩成整个 `TaskView`"的。
        let prev = plain_prev_run();
        let moved = Task {
            dir: "D:/other/place".into(),
            ..plain_prev_run()
        };
        assert_ne!(
            moved.target_path(),
            prev.target_path(),
            "夹具：落点确实换了"
        );
        assert!(
            matches!(
                decide(
                    &moved,
                    Some(snap_after(val("E", "L", 100, URL), &prev)),
                    &base()
                ),
                Decision::Resume(_)
            ),
            "只换输出目录不构成 Fresh"
        );
        let renamed = Task {
            name: Some("renamed.bin".into()),
            ..plain_prev_run()
        };
        assert_ne!(
            renamed.target_path(),
            prev.target_path(),
            "夹具：目标文件名确实换了"
        );
        assert!(
            matches!(
                decide(
                    &renamed,
                    Some(snap_after(val("E", "L", 100, URL), &prev)),
                    &base()
                ),
                Decision::Resume(_)
            ),
            "只换目标文件名不构成 Fresh"
        );
    }

    #[test]
    fn the_request_check_sits_after_the_secret_abort_and_before_the_final_url() {
        // 位置由裁定写死：① 仍然最前（缺口令时连"请求变没变"都不谈），①b 排在 ② 之前
        // （请求换了比落地地址换了更早、更可行动 —— 后者是服务器做的事）。
        let mut secret = snap_after(val("E", "L", 100, URL), &plain_prev_run());
        secret.needs_secret = true;
        let new_request = Task {
            cookies: Some("sid=9".into()),
            ..plain_prev_run()
        };
        assert!(
            matches!(
                decide(&new_request, Some(secret), &base()),
                Decision::Abort(_)
            ),
            "① 必须仍排在 ①b 之前"
        );
        let redirected = facts(Some("E"), Some("L"), 100, "https://cdn.example/b.bin", true);
        let d = decide(
            &new_request,
            Some(snap_after(
                val("E", "L", 100, URL),
                &Task {
                    cookies: Some("sid=1".into()),
                    ..plain_prev_run()
                },
            )),
            &redirected,
        );
        let text = reason(&d).expect("两处不符也要给原因");
        assert!(text.contains("Cookie"), "该先说请求变了：{text}");
        assert!(!text.contains("最终 URL"), "原因只能有一条：{text}");
    }

    // ---- ⑤ 的边界：单行必须能续（派发点名的可达组合）----

    #[test]
    fn one_row_progress_resumes_even_when_the_server_refuses_ranges() {
        let mut no_ranges = base();
        no_ranges.accept_ranges = false;
        // 起点为 0 的一条连续前缀：不认 Range 的服务器本来就只能产出这种形状。
        let prefix = snap_with_rows(100, &[(0, 59)]);
        assert!(matches!(
            decide(&task_plain(), Some(prefix), &no_ranges),
            Decision::Resume(_)
        ));
        // 起点不是 0 的一条：E7③ 明写这种行在退化模式里真会出现（Task 7 负责丢前缀），必须照样能续。
        let tail = snap_with_rows(100, &[(40, 99)]);
        assert!(
            matches!(
                decide(&task_plain(), Some(tail), &no_ranges),
                Decision::Resume(_)
            ),
            "裁定 4：单行不以 0 开头也要放行给 Task 7 的退化路径处理"
        );
        // 空表（什么都没确认过）不是"多段"。
        assert!(matches!(
            decide(
                &task_plain(),
                Some(snap(val("E", "L", 100, URL))),
                &no_ranges
            ),
            Decision::Resume(_)
        ));
        // 三行带洞才是 ⑤ 要拦的形状；边界取 >1，两行就该拦。
        let two = snap_with_rows(100, &[(0, 9), (50, 59)]);
        let d = decide(&task_plain(), Some(two), &no_ranges);
        let text = reason(&d).expect("⑤ 重来也要给原因");
        assert!(matches!(d, Decision::Fresh { .. }), "{d:?}");
        assert!(
            text.contains("Range"),
            "原因要说出是 Range/段数这条判据：{text}"
        );
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
            matches!(
                decide(&task_plain(), Some(recorded_true), &base()),
                Decision::Fresh { .. }
            ),
            "旧文件的宽松记录不能替用户做一次他没要求的放行"
        );
        // 旧文件记 false，本次开了 --ignore-mtime ⇒ 按本次的宽松判据续。
        assert!(
            !changed_mtime.validator.ignore_mtime,
            "夹具：简报的 val(...) 落盘的 ignore_mtime 是 false"
        );
        assert!(
            matches!(
                decide(&task(), Some(changed_mtime), &base()),
                Decision::Resume(_)
            ),
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
        assert!(
            !text.contains("ETag") && !text.contains("长度"),
            "原因只能有一条：{text}"
        );
        // ③+④ 同时不符 ⇒ 说 ETag。
        let p2 = facts(Some("E"), Some("L"), 999, URL, true);
        let d2 = decide(&task_plain(), Some(snap(val("E2", "L9", 100, URL))), &p2);
        let text2 = reason(&d2).expect("ETag 与长度同时不符也要给原因");
        assert!(
            text2.contains("ETag") && !text2.contains("长度"),
            "该先说 ETag：{text2}"
        );
        // ① 排在最前：缺口令时连比对都不做。
        let mut s = snap(val("E2", "L9", 100, URL));
        s.needs_secret = true;
        assert!(
            matches!(decide(&task_plain(), Some(s), &p), Decision::Abort(_)),
            "① 必须最先"
        );
    }

    // ---- 交回给 Task 8 的东西：快照原样、原因能读 ----

    #[test]
    fn resume_hands_the_snapshot_back_untouched() {
        let original = snap_multi(val("E", "L", 100, URL), 3);
        let d = decide(&task(), Some(original.clone()), &base());
        let Decision::Resume(got) = d else {
            panic!("这一支该续：{d:?}")
        };
        assert_eq!(
            got, original,
            "判定模块不许改快照（stats/segments/validator 都得原样）"
        );
        assert_eq!(got.segments.len(), 3);
        assert_eq!(
            got.stats.bytes_duplicated, 0,
            "计数归 Task 8 累加，本模块不动"
        );
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
            (
                task_plain(),
                snap(val("E", "L2", 100, URL)),
                base(),
                "Last-Modified",
            ),
            (task_plain(), snap(val("E", "L", 99, URL)), base(), "长度"),
            // 夹具修复（F1 的比对让这里不能再图省事）：`stub_for_test` 把 `task.url` 落成了
            // `validator.final_url`，于是这份夹具读起来像"上一轮请求的就是 /elsewhere"⇒ 会先撞上 ①b
            // （换了 URL）而不是本用例要测的 ②（最终 URL 与断点不符）。讲得通的上一轮是
            // "请求 /a、被重定向带到 /elsewhere"，所以请求视图取 `task_plain()` 自己。
            (
                task_plain(),
                snap_after(val("E", "L", 100, "https://x/elsewhere"), &task_plain()),
                base(),
                "URL",
            ),
            (
                task_plain(),
                snap_multi(val("E", "L", 100, URL), 2),
                no_ranges,
                "Range",
            ),
            (task_plain(), secret, base(), "口令"),
            (
                task_plain(),
                s_len,
                facts(Some("E"), Some("L"), 100, URL, true),
                "Last-Modified",
            ),
        ];
        for (t, s, p, keyword) in cases {
            let d = decide(&t, Some(s), &p);
            let text =
                reason(&d).unwrap_or_else(|| panic!("{keyword} 的冲突必须给出一句话原因：{d:?}"));
            assert!(!text.is_empty(), "原因不能是空串：{d:?}");
            assert!(!text.contains('\n'), "原因必须是一行：{text}");
            assert!(
                text.chars().count() <= 200,
                "原因太长了，进度行放不下：{} 字：{text}",
                text.chars().count()
            );
            assert!(
                text.contains(keyword),
                "原因要说出是哪条判据不同意（{keyword}）：{text}"
            );
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
            (
                given.clone(),
                Some(secret),
                facts(Some("E2"), Some("L"), 9, "https://x/z", false),
            ),
            (
                given.clone(),
                // 夹具修复（同上）：这份断点记的落地地址是 /z，而本次请求的是 URL ⇒ 旧写法把
                // `task.url` 也落成 /z，看起来像"用户换了 --url"，会被 ①b 抢先。上一轮请求 /a 被带到 /z
                // 才是讲得通的形状，于是请求视图取 `given` 自己（口令照样不落盘：`TaskView` 没这个字段）。
                Some(snap_after(val("E2", "L2", 9, "https://x/z"), &given)),
                facts(Some("E"), Some("L"), 100, URL, true),
            ),
            (
                task_with(0, Some(SECRET)),
                Some(snap(val("E", "L2", 100, URL))),
                base(),
            ),
            (task_with(0, Some(SECRET)), None, base()),
        ];
        for (t, s, p) in battery {
            let d = decide(&t, s, &p);
            if let Some(text) = reason(&d) {
                assert!(!text.contains(SECRET), "决策原因里出现了口令本尊：{text}");
            }
            assert!(
                !format!("{d:?}").contains(SECRET),
                "Debug 里也不该有（{d:?}）"
            );
        }
    }

    // ---- T99-J1 §2：读侧那道闸（断点文件里那三份值也可能带着口令）----

    /// 读侧那道闸的**命中臂**（T99-J1 §2，复审 Q3-1 的实测两格）：断点文件里那三份值带着本任务
    /// 口令的形状时，②③ 那两句结论照旧给出（`Fresh`，任务照常跑），但**两个值都不许进终端**。
    ///
    /// 四臂分别钉：② 的字面形、③ 第一句的字面形、③ 第二句的 base64 形（去 padding 那一支），
    /// 以及**线上那份凭据对** `base64("<user>:<口令>")` 的 mod 1 那一档 —— 最后一臂是 §1 与 §2
    /// 的合流：读侧用的就是写侧那同一份判据，所以新形状在读侧同样开火。
    /// 外加一臂挡住"本次没给口令"那一族：那时 ① 的 `needs_secret` 早已先 `Abort`（顺序是既有的），
    /// 这一格不回显不是判据的功劳，是 ① 的 —— 所以它必须**仍然**不回显。
    #[test]
    fn a_saved_validator_value_carrying_our_password_is_never_echoed_in_the_reason() {
        let tainted_url = "http://127.0.0.1:56815/x/hunter2";
        let pair_blob = "http://127.0.0.1:56815/x/YWxpY2VYOmh1bnRlcjI="; // base64("aliceX:hunter2")
        // 本轮服务器交回的事实：`final_url` 是本次那条、ETag 换了、Last-Modified 换了。
        let moved = facts(Some("\"fresh-etag\""), Some("L"), 100, URL, true);
        let only_mtime = facts(Some("E"), Some("L"), 100, URL, true);

        // 本次给了同一份口令 ⇒ 判据拿得到形状（`needs_secret` 为假，① 不接管）。
        let given = task_with(0, Some(SECRET));
        let cases: Vec<(Validator, Task, &ProbeFacts, &str, &str)> = vec![
            // (断点里的 validator, 本次任务, 本轮事实, 期望点名的判据, 原因里不许出现的串)
            (
                val("E", "L", 100, tainted_url),
                given.clone(),
                &moved,
                "最终 URL",
                SECRET,
            ),
            (
                val("\"hunter2\"", "L", 100, URL),
                given.clone(),
                &moved,
                "ETag",
                SECRET,
            ),
            // 这一支 ETag 两侧相等（否则 ③ 第一步抢先），只有 Last-Modified 不符 ⇒ 钉的是第二句。
            (
                val("E", "\"aHVudGVyMg\"", 100, URL),
                given.clone(),
                &only_mtime,
                "Last-Modified",
                "aHVudGVyMg",
            ),
            // §1 的第二组在读侧同样开火：这一格的串里**没有**口令的字面，只有那份线上凭据对。
            (
                val("E", "L", 100, pair_blob),
                Task {
                    user: Some("aliceX".into()),
                    ..given.clone()
                },
                &moved,
                "最终 URL",
                "YWxpY2VYOmh1bnRlcjI=",
            ),
        ];
        for (v, t, p, keyword, forbidden) in cases {
            let s = snap_after(v, &t);
            assert!(!s.needs_secret, "夹具前提：这一格要走 ②③ 而不是 ①");
            let d = decide(&t, Some(s), p);
            assert!(
                matches!(d, Decision::Fresh { .. }),
                "命中也要照常决策，不许变成 Abort/Resume：{d:?}"
            );
            let text = reason(&d).expect("冲突必须给出一句话原因");
            assert!(
                text.contains(keyword),
                "原因要说清是哪条判据不同意（{keyword}）：{text}"
            );
            assert!(
                !text.contains(forbidden),
                "断点里那份值被回显了（{forbidden}）：{text}"
            );
            assert!(!text.contains(SECRET), "原因里出现了口令本尊：{text}");
            assert!(
                !text.contains("127.0.0.1"),
                "落地地址的片段被抄出来了：{text}"
            );
            assert!(
                !text.contains('\n') && text.chars().count() <= 200,
                "原因必须是一行、进度行放得下：{text}"
            );
        }

        // 本次没给口令那一族：① 先 `Abort`（那份断点记着"当初带了口令"），同样一个字都不回显。
        let mut legacy = snap_after(val("E", "L", 100, tainted_url), &task_plain());
        legacy.needs_secret = true;
        let d = decide(&task_plain(), Some(legacy), &base());
        let Decision::Abort(why) = d else {
            panic!("缺口令必须是 Abort，不能退化成回显旧值的 Fresh：{d:?}")
        };
        assert!(
            !why.contains(SECRET) && !why.contains("127.0.0.1"),
            "① 那句把旧值抄出来了：{why}"
        );
        assert!(
            why.contains("口令") && why.contains("--password"),
            "① 要可行动：{why}"
        );
    }

    /// 读侧那道闸的**未命中臂**（复审 I-3 同一条论证的另一半）：串里没有本任务口令的形状时，
    /// 那三条原因**逐字照打**、两个值都写出来 —— 这道闸一口都不许吃掉诊断。
    /// 夹具与上面那一发同一批，只把口令换成串里根本没有的那份（`wxyz9021`）⇒ 判据恒假。
    #[test]
    fn a_saved_validator_value_without_our_password_still_echoes_both_sides() {
        let unrelated = task_with(0, Some("wxyz9021"));
        let tainted_url = "http://127.0.0.1:56815/x/hunter2";
        let now = facts(Some("\"fresh-etag\""), Some("L"), 100, URL, true);

        // ② 那句：断点里那个 URL 与现在这条都要出现在原因里（旧写法一字未动）。
        let d = decide(
            &unrelated,
            Some(snap_after(val("E", "L", 100, tainted_url), &unrelated)),
            &now,
        );
        let text = reason(&d).expect("② 的冲突必须给原因");
        assert!(
            text.contains(tainted_url)
                && text.contains(URL)
                && text.contains("重定向把请求带到了别处"),
            "未命中时 ② 那三条原因之一被这道闸吃掉了：{text}"
        );

        // ③ 第一句（ETag）：两侧值都出现，含 `{saved:?}` 那一层 Debug 引号。
        let d = decide(
            &unrelated,
            Some(snap_after(val("\"hunter2\"", "L", 100, URL), &unrelated)),
            &now,
        );
        let text = reason(&d).expect("③ 的 ETag 冲突必须给原因");
        assert!(
            text.contains("\"\\\"hunter2\\\"\"")
                && text.contains("fresh-etag")
                && text.contains("--ignore-mtime"),
            "未命中时 ETag 那句的值被抹掉了：{text}"
        );

        // ③ 第二句（Last-Modified）：base64 形在这一臂里**不是**本任务口令的形状（口令是 wxyz9021），
        // 于是照原样回显 ⇒ 这一格同时钉住"判据按本任务那份口令比"这件事（不是"看见 base64 就抹"）。
        // 这一支的 ETag 两侧相等，否则 ③ 第一步抢先、测不到第二句。
        let only_mtime = facts(Some("E"), Some("L"), 100, URL, true);
        let d = decide(
            &unrelated,
            Some(snap_after(val("E", "\"aHVudGVyMg\"", 100, URL), &unrelated)),
            &only_mtime,
        );
        let text = reason(&d).expect("③ 的 Last-Modified 冲突必须给原因");
        assert!(
            text.contains("aHVudGVyMg")
                && text.contains("\"L\"")
                && text.contains("--ignore-mtime"),
            "未命中时 Last-Modified 那句的值被抹掉了：{text}"
        );
    }

    /// 读侧那道闸的**接线针**（写法照 `src/probe.rs` 那根源码扫描针）：
    /// `src/resume.rs` 产品区里 `carries_our_password(` 的调用**恰好 3 次**（② 一句 ＋ ③ 两句），
    /// 每一处的第二个实参逐字是 `task.password.as_deref()`（不许裸 `None`），而本文件**没有**第二份
    /// 判据实现（定义 0 次）—— 这条挡的就是"读侧顺手复制一份"那种分家。
    ///
    /// ⚠ 同步代价（复审 M-4 那条口径）：这一根要求调用点**写在同一行**、且第二实参的**文本**里出现
    /// `password`。把口令改名（`let pw = task.password.as_deref()`）或折进局部变量、或把调用拆成多行，
    /// 都要连同这根针一起改，并且要说清为什么值得。
    #[test]
    fn the_read_side_gate_calls_the_shared_criterion_three_times_and_reimplements_nothing() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("resume.rs");
        let all = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "读 {} 的源码文本失败：{err}（本发是源码扫描针，读不到就是没有判据）",
                path.display()
            )
        });
        let src = match all.find("#[cfg(test)]") {
            Some(at) => &all[..at],
            None => panic!(
                "src/resume.rs 里找不到 `#[cfg(test)]`——扫描区靠它切掉测试文本，不许退回整文件（针会扫到自己）"
            ),
        };
        let lines: Vec<&str> = src
            .lines()
            .map(str::trim_start)
            .filter(|l| !l.starts_with("//") && !l.starts_with("//!"))
            .collect();
        let calls: Vec<String> = lines
            .iter()
            .filter(|l| l.contains("carries_our_password("))
            .map(|l| (*l).to_string())
            .collect();
        assert_eq!(
            calls.len(),
            3,
            "读侧那道闸应当调用共享判据恰好 3 次（② 的最终 URL ＋ ③ 的 ETag ＋ ③ 的 Last-Modified），\
             实得 {} 次：少了＝某条原因退回无脑回显（复审 Q3-1 那一格），多了＝读侧新开了判据入口。逐行：{calls:?}",
            calls.len()
        );
        for site in &calls {
            assert!(
                site.contains(", task.password.as_deref())"),
                "调用点的第二实参不是逐字的 `task.password.as_deref()`（读侧拿不到本任务口令 ⇒ 那道闸恒假）：{site}"
            );
            assert!(!site.contains("(None"), "调用点交回裸 None：{site}");
        }
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("fn carries_our_password"))
                .count(),
            0,
            "src/resume.rs 里不许有第二份判据实现（两本账）——判据只住在 `src/probe.rs`"
        );
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
        assert_eq!(
            p,
            base(),
            "字段名/类型即 plan 第 673 行的 Produces，且逐字段可比"
        );
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
        let ok = decide(
            &task_plain(),
            Some(s.clone()),
            &facts(None, None, 100, URL, true),
        );
        assert!(matches!(ok, Decision::Resume(_)), "{ok:?}");
        let short = decide(&task_plain(), Some(s), &facts(None, None, 101, URL, true));
        let text = reason(&short).expect("长度不符要给原因");
        assert!(matches!(short, Decision::Fresh { .. }), "{short:?}");
        assert!(
            text.contains("长度"),
            "原因要说 100 → 101 是长度不符：{text}"
        );
        // 边界：0 字节的资源 + 空段表也能续（别让"长度为 0"退化成"肯定不符"）。
        let empty = snap(val_opt(None, None, 0, URL));
        assert!(matches!(
            decide(
                &task_plain(),
                Some(empty),
                &facts(None, None, 0, URL, false)
            ),
            Decision::Resume(_)
        ));
    }

    // ---- A1：If-Range 取值（R17）----

    #[test]
    fn if_range_prefers_a_strong_etag() {
        let v = val_opt(
            Some("\"abc\""),
            Some("Wed, 21 Oct 2015 07:28:00 GMT"),
            100,
            URL,
        );
        assert_eq!(if_range_value(&v, false).as_deref(), Some("\"abc\""));
        assert_eq!(
            if_range_value(&v, true).as_deref(),
            Some("\"abc\""),
            "ETag 不受 ignore_mtime 豁免"
        );
    }

    #[test]
    fn if_range_falls_back_to_last_modified_only_without_ignore_mtime() {
        let v = val_opt(None, Some("Wed, 21 Oct 2015 07:28:00 GMT"), 100, URL);
        assert_eq!(
            if_range_value(&v, false).as_deref(),
            Some("Wed, 21 Oct 2015 07:28:00 GMT")
        );
        assert_eq!(
            if_range_value(&v, true),
            None,
            "开了 --ignore-mtime 就不许再按时间戳问对端"
        );
    }

    #[test]
    fn if_range_rejects_weak_or_unquoted_etags_and_uses_the_fallback() {
        let v = val_opt(Some("W/\"weak\""), Some("LM"), 100, URL);
        assert_eq!(
            if_range_value(&v, false).as_deref(),
            Some("LM"),
            "弱 ETag 不能用，退 Last-Modified"
        );
        let no_lm = val_opt(Some("W/\"weak\""), None, 100, URL);
        assert_eq!(
            if_range_value(&no_lm, false),
            None,
            "弱 ETag 不能单独当 If-Range"
        );
        let unquoted = val_opt(Some("abc"), None, 100, URL);
        assert_eq!(
            if_range_value(&unquoted, false),
            None,
            "不带引号的 ETag 不是强验证器"
        );
    }

    #[test]
    fn if_range_is_none_without_a_usable_validator() {
        let v = val_opt(None, None, 100, URL);
        assert_eq!(if_range_value(&v, false), None);
        assert_eq!(if_range_value(&v, true), None);
    }
}
