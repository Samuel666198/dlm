//! 主循环：把七个模块接成真线程、真定时器、真文件的下载引擎。
//!
//! 依据：spec §2（模块表、§2.1 的事件/动作契约、§2.2 两条一致性约束）、§3（常量与来源强度）、
//! §4（断点文件、"写者唯一"、`--no-resume`）、§6（重试、416、完成校验）、§7（测试策略），
//! 以及计划勘误 **E7**（两本账必须分开）、**E8**（`flush` = `sync_all`：只在收尾调一次，
//! 且不许在 writer 还活着时调）、**E11①**/**E12⑧**/**R7**（带 `--data-file` 的任务不试探、
//! 那一枪只发一次、该形状既不读也不写断点文件）、**E13**（本任务要改的三处断接口与配套裁定）。
//!
//! ## 分工：线程在这里，决策不在这里
//!
//! 本模块只做三件事：开线程、把 `fetch` 的结论折成 `scheduler::Event`、按纯模块给出的判定落盘。
//! "哪个 worker 下哪段字节"永远由 `Scheduler` 说了算（全工程唯一的段归属权威）；
//! "续 / 重来 / 拒绝"由 `resume::decide` 说了算；"这一枪成没成"由 `fetch` 说了算。
//! 引擎一处都不重写这些判据。spec §2 那句"不共享可变状态（除 `Mutex<Scheduler>` 这一处）"
//! 在这里还更严格些：调度器住在主线程的栈上，连那把锁都不需要 —— worker 只交回事实，不查表。
//!
//! ## 两本账（E7 的追加段）
//!
//! | 问题 | 唯一的权威 |
//! |---|---|
//! | 哪些字节的**归属已定** | `scheduler::snapshot_intervals()`（416 封存的段可能从没落盘） |
//! | 磁盘上**真有多少字节** | `store::Store::len()` |
//!
//! 完成判定是**两个**条件：`sched.is_done()` **且** `store.len() == total_len`（见 `completion_verdict`）。
//! 拿覆盖集的字节数去过 spec §6 末条那道"已写字节数 == total_len"的闸，正是 E7 点名不许做的事；
//! `completion_verdict` 的签名里根本没有覆盖集那个量 —— 想犯错也没有入口。单元用例
//! `completion_verdict_needs_both_books_and_never_the_covering_set` 钉这一格。
//!
//! ## 一条派发公式的更正（写在这里是因为它会咬人）
//!
//! 派发给本任务的说法是 `Advanced{bytes}` → `Progress{abs_end: iv.start + bytes}`，
//! `fetch` 模块文档里 `Outcome` 那一节**曾经也这么写**（修复轮 8F-4a 已按本节改成正式，现在那一段
//! 与本节一致）。**这个式子差一字节**：`Advanced` 的 `bytes` 是
//! "经 `store.write_at` 确认落盘的字节数"（`fetch::Outcome::Advanced` 自己定义，实现是
//! `written(iv, cursor) = cursor - iv.start`，见 `fetch::written`），而 `Event::Progress` 的
//! `abs_end` 是**含端点**的"已写到的绝对偏移"（同一句话就写在 `Event::Progress` 那一行的文档上；用例
//! `window_done_returns_remainder_and_reassigns_without_new_connection` 报 300 之后下一段起点是 301）。
//! 所以正确的换算是 `abs_end = iv.start + bytes - 1`（见 `progress_abs_end`）。
//!
//! 照字面用 `iv.start + bytes` 会把**从没写过的那一个字节**报成已确认：调度器随即把
//! `[iv.start+bytes+1, iv.end]` 当成待下载，那一个字节永远没人要 ⇒ 文件里留一个洞，
//! 而覆盖集与断点文件都说它已经落盘（`is_done()` 照样为真、`store.len()` 照样等于全长，
//! 于是**两本账都对得上，文件却是坏的** —— 只有 `--sha256` 那道闸看得见它）。
//! 受影响的是所有**跨窗口的段**（区间长过 `WINDOW_BYTES` 或对端交得比区间短），
//! 因为 `Complete` 那一支不带字节。钉它的是单元用例
//! `the_advanced_mapping_is_inclusive_and_bounded_to_the_interval`；
//! 集成用例 `full_download_is_byte_exact` 顺手也走这一支（8 MiB 的第一条区间长过 4 MiB 的窗口），
//! 但它**不是**这条差一的可靠判据 —— 洞落在 blob 的块首（非零）上的概率只有 4096 分之一。
//!
//! ## `Retighten` 不是一枪，但它有一条通道（阶段 5A 按 **E14**① 改过这一节）
//!
//! `scheduler::Action::Retighten{worker, iv}` 的文档说"它必须改用带显式终点的新请求"。在
//! `fetch::run_window` 的形状下这句话**执行不了**：那个 worker 此刻正阻塞在一次 `read()` 里
//! （一发请求一个结论，中间没有任何通道能递话给它），而"再发一枪"要么排在当前这枪之后
//! （起点是 `iv.start` ⇒ 把已写的前缀重下一遍），要么并发第二条连接（`workers` 的意义就变了）。
//! 所以引擎的做法是：**不发新的一枪**。阶段 4 到这里就停了（只记 `Tightened`），于是那一枪照旧
//! 把自己那一截读完，多出来的字节全进 `bytes_duplicated`。
//! ⚠ **"读完的是自己那个窗口（最多 `WINDOW_BYTES` = 4 MiB）"只对认 `Range` 那一支成立**：窗口夹
//! 住在 `fetch::plan_shot` 的 `BodyShape::Windowed` 那一支。退化那一支（**E16** 的 `BodyShape::ToEof`）
//! 一枪读的是**整条剩余区间**、没有窗口夹，所以"最多 4 MiB"在它的形状下是假的。这一支也走不到本节
//! 这条收紧路径 —— 前提是**退化时一次只有一段在途**，因而根本没有第二把刀可落：判据住在
//! `scheduler::dispatch` 的 `no_ranges` 那一支（只要还有人在途就一律 `Action::Idle`），它产不出
//! `Action::Retighten`（钉它的是 `no_ranges_never_splits_and_never_runs_two_segments`）。
//! 这个前提原先只写在调度器那一侧，本节之前没人把它和这句"照旧读完"钉在一起。
//! 阶段 4 那一版的实测线上多打（**唯一诚实的口径** `Σ conn_close_side.sent`，字节数与倍数都照计划勘误
//! **E14 附记**那张表；那张表"线上多打"那一列给的是**减过文件长之后**的净额，倍数却是
//! **线上交出 ÷ 文件长**）：
//! 8 MiB×4 worker 多打 8,465,356 个字节 ⇒ 线上交出 `8,465,356 + 8,388,608 = 16,853,964` 个字节
//! = 文件长 8,388,608 的 **×2.009**（⚠ 别把它读成 `8,465,356 / 8,388,608`，那一商只有 ×1.009，
//! 而且它不是这张表里任何一个数）、
//! 8 MiB×8 是 ×2.618、48 MiB×4 是 ×1.418、48 MiB×8 是 ×1.694（四组表见计划勘误 **E14 附记**）。
//! ⚠ 别用 `Σ response_done.bytes` 当线上量具：它**不含半路被丢弃的响应**（这一形状就漏 1 条，48 MiB 上漏 8 条），
//! 而把 `response_done` 与 `conn_close_side` 相加又**重复计数**（正常收尾的响应两条都记）。
//!
//! **E14**①（乙）给的就是那条唯一存在的通道：**终点帽**。每个 worker 一格 `Arc<fetch::EndCap>`
//! （`AtomicU64`，装"这一枪最多还允许写到哪个**含端点**的偏移"）：
//! - `dispatch` 在**发 Job 之前**把它重置成 `NO_END_CAP`（顺序是判据：反过来 worker 可能把上一枪的
//!   旧帽读进这一枪）；
//! - `run_window` 一进门把自己的**读上界** `stop - 1` **降**进去（`lower_end_cap` ⇒ 只降不升）：
//!   `Windowed` 那一支那个上界就是窗口终点，`ToEof`（退化）那一支是 `iv.end` —— "窗口终点"那句
//!   只对着认 `Range` 的那一支（**E16** 划的界，见 `fetch` 模块文档开头那条）；
//! - 主线程处理 `Shot::Tighten{worker, iv}` 时把 `iv.end` 降进去（`ping_free_workers` 里，与
//!   调度器做决定的那一刻同一次遍历 ⇒ 帽不会晚于决定本身）；
//! - 那一枪在每块 64 KiB 之前查一次帽，越过帽就**停手**，交回的还是已有的 `Advanced { bytes }`。
//!   没有 sleep、没有第二条读 socket 的线程、没有 `tokio`、没有轮询定时器（E14① 的形状约束）。
//!
//! **一处对 E14 字面的偏离（必须说清）**：E14① 的括号里写的是 `fetch_max`，但同一条要求"帽**只降不升**
//! （更大的陈旧值不许把已经收紧的枪放宽）"—— `fetch_max` 是单调**升**，按字面写正好把那条性质做反。
//! 落地按**性质**走：`fetch_min`（等价写法是对 `u64::MAX - end` 用 `fetch_max`，读起来更绕，不取）。
//! 钉它的用例：单元 `end_cap_lowering_is_monotonic_downward_only`（`src/fetch.rs`）与集成
//! `an_end_cap_already_lowered_before_the_shot_starts_survives_the_shots_own_window`。
//!
//! 代价要照实说，帽管不到的那一截还在：帽只能拦**它落下之后**还要读的那些字节（一块之内），
//! 拦不到"调度器决定收紧时那一枪已经读过去的字节"—— 调度器只在一枪结束时才从 `Progress` 知道
//! 它读到哪儿（`scheduler.rs` 的 `written` 是收枪才更新的），所以一刀可能落在 incumbent 早已写过的
//! 偏移之下。同一形状（8 MiB / 4 worker）改帽前后各自的读数与**口径**都记在
//! [`crate::state::Stats::bytes_duplicated`] 的字段文档里（那张表的线上量具是
//! `Σ conn_close_side.sent`，字段读数与线上多打是两个不同的数，引的时候别混着写、也别把字节数
//! 换成十进制的 "MiB"）；要到"每笔 ≤ 一块"得让**调度器**别反复劈同一条在途尾巴
//! （派发明确禁止改那条规则），已登记给控制员。
//!
//! `TraceStep::Tightened` 与 `Dispatched` 仍然**分列**：前者说"调度器把谁的目标提前了"（以及帽降到了
//! 哪儿），后者说"引擎真打出去的一枪"。阶段 5 那道"与线上 `Range` 对齐"的链接只拿 `Dispatched` 比。
//!
//! ## 保存断点的节奏（brief 那句"`STATE_SAVE_MAX_INTERVAL_SECS` + 关键事件"的读法）
//!
//! 覆盖集只在 `WindowDone`/`Failed`/`SegmentComplete` 这三条事件上增长，而那三条**都**是关键事件，
//! 所以"关键事件触发 + 至多每 5 秒"如果读成两把独立的闸，5 秒那把永远是空转（事件驱动的那把先过了）。
//! 落地取唯一自洽的读法：**关键事件（覆盖集变小了 pending）且距上次已不小于
//! `STATE_SAVE_MAX_INTERVAL_SECS`** ⇒ 至多每 5 秒一份，一份都不许多写；另外两处**无条件**落盘：
//! 开工一次（让"任意时刻被杀都能续"从一开始就成立）与收尾一次（成败都落，成功再按 `--keep-state` 决定删不删）。
//!
//! ## 线程的账
//!
//! - worker 只做 `recv → 一枪 → send 结论`，不认识调度器、不认识断点文件（spec §4"写者唯一"）。
//! - 每个 worker 挂一枚 `AtomicBool` 心跳，`Drop` 时置假（**含 unwind**）：主线程每次醒来查一次
//!   "标着 Busy 但心跳已没了"的 worker ⇒ 崩掉的线程手上那一枪不会被永远等下去。
//!   `JoinHandle::join()` 报错同样进结论（`finish` 里那条判据兑现）：
//!   **worker 崩了绝不许交回 `ok = true`**。
//! - `recv_timeout` 的期限取 `min(最近的退避到点, 上一拍 + LOOP_POLL)`：没有忙等，也没有第二个计时线程。
//!
//! ## 进度行的数据面：`RunSnapshot` + `Config.on_tick`（勘误 **E21** 甲，9A 的全部内容）
//!
//! spec §5 那一行 `\r 45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4 dup 5.5%` 与 §6.2 的 `slow-retry xN`
//! 在 9A 之前**没有数据来源**：`run` 一直阻塞到结束才交回 `Report`。E21 裁的是"给引擎加运行态
//! 只读快照 + 回调出口"（走甲），落点就是 [`RunSnapshot`] 那八格与 [`Config::on_tick`]。
//! 渲染不在这里（9B：`cli.rs`/`report.rs`/`main.rs`），本模块只负责**交得出、按拍交、不乱交**。
//!
//! 三条形状约束各自挡住一种 tempting 的写法：
//! - **零新计时器、零新线程、零新依赖**（判据⑦）：快照挂在主循环已有的那一拍上，
//!   每拍至多一次 + 收尾一次。调用点只有两处（`tick_if_due` 与 `drive` 的出口），
//!   **绝不进 `translate`/结论处理那一圈** —— 那里一块 64 KiB 走一次，挂上去就是每块打一行
//!   （与 **E8①** 给 `flush` 立的那条"绝不进每块循环"同族）。钉它的是
//!   `a_sink_gets_one_tick_per_beat_and_never_one_per_block`。
//! - **速率那一格是两拍之差，不是全程平均**（判据④）：[`running_rate_bytes_per_sec`] 那个数
//!   服务的是 `fetch::window_budget` 的分子，两个数不许共用、不许并成一条式子。钉它的是
//!   `the_rate_cell_is_the_difference_between_two_beats_not_the_whole_run_average`。
//! - **`--data-file` 那一支一次都不交**（判据⑧）：[`run_whole_once`] 整程阻塞在一发 POST 里、
//!   按 **E8②** 不开线程，中途没有任何引擎侧读得到的量（为它造一个正是 **E21③** 裁"不做"的
//!   那件事）。⇒ `on_tick` 不是"必然存在的进度源"，那条路的权威只有 `Report`。钉它的是
//!   `the_data_file_route_emits_no_ticks_while_the_segment_route_emits_some`。
//!
//! 快照**不是第三本账**（**E7**）：八格全部由 `sched` / `stats` / `run_delivered` / `workers` / `cfg`
//! 现读现算，引擎不为它另记任何进度；覆盖那一格也只是 `total_len − pending_bytes()` 一次减法，
//! 调度器一个新 API 都没加（判据①）。`covered` 与 `run_delivered`/`run_bytes_duplicated` 的寿命
//! **故意不同**（前者含上一轮续传来的字节，后两格只看本轮），与 **E20⑧** 那两本跨轮账也不许
//! 互相"对齐" —— 三处各自的口径写在字段文档上。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use crate::consts::{FAST_RETRY_MAX, FLAG_KEEP_STATE, PROBE_READ_BYTES, SLOW_RETRY_INTERVAL_SECS, STATE_SAVE_MAX_INTERVAL_SECS};
use crate::fetch::{self, FailKind, Outcome};
use crate::probe;
use crate::rate::RateLimiter;
use crate::resume::{self, Decision};
use crate::scheduler::{Action, Event, Interval, Scheduler};
use crate::state::{self, SegView, Snapshot, Stats, Validator};
use crate::store::Store;
use crate::task::Task;

/// 引擎级请求的**连接与响应**期限。与 `probe`/`fetch` 的每请求期限是两回事：
/// `run_window` 给每一枪单独设 `.timeout(窗口预算)`，那个值会**覆盖**这里这一条
/// （R7 末段"全局期限不能指望给单个窗口兜底"），所以这里只服务两件事：`probe::run` 那条
/// 没设预算的试探，以及"某条新路径忘了设预算"的兜底。**不许**据此写"每个窗口最多等 30 秒"。
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);
/// 建连期限。与 `tests/harness/mod.rs` 的 `client()` 同一形状（那边是简报 Step 2 定的）。
/// 刻意**没有** `Connection: close`，也**没有** `pool_max_idle_per_host(0)`（R5 / E2 / STATUS 第 3 条）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 主循环的醒来节拍，三条用途，都要求它**不为零**：查 worker 心跳、评估保存时机、
/// 让 `recv_timeout` 永远拿得到一个有限期限 ⇒ 循环不可能空转。
/// 取 1 秒的依据是 spec §3 常量表那行的 `STATE_SAVE_MIN_INTERVAL`（"1 s，事件驱动 + 每 5 s 兜底"）；
/// `consts` 里没有这个常量，而 **E2** 把常量表钉死在 12 条（不许加第 13 条），所以它住在引擎里。
const LOOP_POLL: Duration = Duration::from_secs(1);
/// `run_whole` 那一枪交回的速率：这一支**没有**任何测量可用（一发请求读完全部，读完才知道快慢），
/// 交 0 给 `fetch::window_budget` 就是"速率未知 ⇒ 给最宽的预算"（`fetch::budget_for` 的除零裁定）。
const RATE_UNKNOWN: u64 = 0;

/// 一次重试要等多久（spec §6.1 的快速退避 + §6.2 的慢速无限重试 + `--max-retries` 封顶）。
///
/// 纯函数、零睡眠 —— 表可以在单元层钉死，真睡眠只在集成层跑一条（**E13④**："退避不许造假时钟"，
/// 也不许为此引入 `Sleep` trait 或可注入的调度器：v1 保持真实时间，这是裁定不是疏漏）。
///
/// `attempt` 是**这一条链上已经重试过几次**（0 ⇒ 马上要等的是第一次重试）。
/// 三条规则按 spec 的原文排：
/// 1. §6.1 快速退避 `1, 2, 4, 8, 16` 秒（`FAST_RETRY_MAX = 5` 次）。
/// 2. §6.2 超过 5 次**不判整任务失败**，转入 `SLOW_RETRY_INTERVAL_SECS` 的无限慢重试
///    （对齐 IDM 队列行为 A15）。
/// 3. `--max-retries` 封顶的是**整条链**（快速 + 慢速一起数），所以它排在最前面：
///    `Some(n)` 且已经重试满 `n` 次 ⇒ `None` = "别再重试了"，调用方据此让整任务失败。
///    `None`（没给 `--max-retries`）⇒ 无限慢重试。
pub fn next_retry_delay(attempt: u32, max_retries: Option<u32>) -> Option<Duration> {
    if max_retries.is_some_and(|cap| attempt >= cap) {
        return None;
    }
    Some(Duration::from_secs(if attempt < FAST_RETRY_MAX {
        // `attempt < FAST_RETRY_MAX = 5` ⇒ 位移最多 4，不会溢出 u64 的秒数。
        1u64 << attempt
    } else {
        SLOW_RETRY_INTERVAL_SECS
    }))
}

/// 一次运行的旋钮。**`keep_state` 不在这里**（**E13②** 的自纠）：它已经是 `Task::flags` 里的
/// `FLAG_KEEP_STATE`，而 `flags` 会随 `TaskView` 进断点文件 ⇒ 两处都能读就是两本账。
/// `force_fresh` 反过来**刻意不进** `flags`：`--no-resume` 说的是"这一次怎么办"，
/// 不是"这个任务是什么"，写进断点文件会污染下一轮的比对（`resume` 的 ①b 明写不比 `flags`）。
#[derive(Clone)]
pub struct Config {
    /// worker 线程数（每个 worker 一条阻塞连接，spec §1）。
    pub workers: usize,
    /// 劈分的绝对下限，交给调度器（`consts::MIN_SEGMENT_BYTES` 是它的默认值）。
    pub min_seg: u64,
    /// `--max-retries`：封顶整条重试链（快速 + 慢速一起数）。`None` ⇒ 无限慢重试（spec §6.2）。
    pub max_retries: Option<u32>,
    /// `--sha256`：给了就读真文件算一遍（spec §6.7）。
    pub verify_sha256: Option<String>,
    /// `--no-resume`（spec §4）：无视已存在的断点文件，从 0 开始并覆盖它。
    pub force_fresh: bool,
    /// R18：单任务所有连接合计的速率上限（字节/秒）；0 = 不限。
    pub max_speed: u64,
    /// A16：本次运行最多新交付的字节数；0 = 不限。到量只停当前运行，断点保留。
    pub max_bytes: u64,
    /// 运行态快照的出口（勘误 **E21** 甲，spec §5 那一行进度缺的就是这个数据面）。
    ///
    /// **默认 `None`** ⇒ 一个观察者都不挂，默认路径与 9A 之前逐字节相同（这是 9A 的验收下限）。
    /// 类型是 `Arc` 而不是 `Box`：`Config` 派生 `Clone`，每条任务复制一份配置时不能把观察者
    /// 复制成两个所有者（判据⑨）。快照本身是 `Copy`、按引用交，**不跨线程**。
    ///
    /// ## 哪些路有 tick、哪些没有（判据⑧要求写明）
    /// - **可分段那一条**（`probe` ⇒ `resume` ⇒ `Engine` ⇒ 主循环）：每拍至多一次
    ///   （`LOOP_POLL = 1 s`）+ 收尾那一拍一次终值。
    /// - **`--data-file` 那一条**（`run_whole_once`）：**一次都没有**。那一支整程阻塞在一发 POST 里，
    ///   本函数按 **E8②** 不开线程 ⇒ 中途没有任何引擎侧读得到的量；为它造一个观测量正是 **E21③**
    ///   刚裁"不做"的那件事（`T8-R6`：收紧过程不打开每块上报）。
    ///   ⇒ **CLI 不许把 `on_tick` 当"必然存在的进度源"**：那条路的权威只有 `Report`。
    ///
    /// ## 实现者的义务（判据⑨）
    /// 回调在**引擎线程里同步执行**：它慢了，`recv_timeout` 的期限、退避到点、保存节奏全都跟着慢
    /// ⇒ 不许在回调里做长阻塞 I/O（网络、读大文件、等人），**打印一行是预期用法**。
    pub on_tick: Option<OnTick>,
}

/// 手写 `Debug`（立场与 `task.rs` 里那条 **E9** 给 `Task` 的那一份一致：某个字段不可打印
/// 不是把整条 `Debug` 摘掉的理由）。`on_tick` 只打印"有没有挂观察者"：闭包本体没有可读的表示，
/// 而"这一份 `Config` 挂了观察者"这件事本身正是调试时要看的那一格。
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("workers", &self.workers)
            .field("min_seg", &self.min_seg)
            .field("max_retries", &self.max_retries)
            .field("verify_sha256", &self.verify_sha256)
            .field("force_fresh", &self.force_fresh)
            .field("max_speed", &self.max_speed)
            .field("max_bytes", &self.max_bytes)
            .field("on_tick", &self.on_tick.as_ref().map(|_| "«观察者»"))
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workers: crate::consts::DEFAULT_CONNECTIONS,
            min_seg: crate::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: None,
            force_fresh: false,
            max_speed: 0,
            max_bytes: 0,
            on_tick: None,
        }
    }
}

/// 一次运行的**只读快照**：渲染 spec §5 那一行进度（`\r 45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4
/// dup 5.5%`）与 §6.2 那个 `slow-retry xN` 所需的**最小闭包**（勘误 **E21** 甲）。
///
/// 命名说明（判据⑩）：spec §2 模块表那行 `report | tick(&Snapshot)` 里的 `Snapshot` 与已实现的
/// [`crate::state::Snapshot`]（磁盘断点体）撞名 ⇒ 实现叫 `RunSnapshot`、`report::tick(&RunSnapshot)`。
/// 这是命名落地选择，不改 spec 承诺的形状。
///
/// ## 它不是什么
/// - **不是**第二本账（**E7**）：八格里没有任何"覆盖账之外另记的进度"，覆盖集那两格只是
///   `total_len − sched.pending_bytes()` 这一个既有量的另一种读法。
/// - **不是**跨线程对象：`Copy`、按引用交，回调之后这份值就没了（判据⑨）。
/// - **不是**"本次运行一共多少字节落盘"的最终答案：那是 [`Report`]，收尾那一拍只是同一批量的
///   中途读数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSnapshot {
    /// 全长（`ProbeFacts::total_len`）。判据②：**不是 `Option`，也不许拿 0 当"未知"哨兵** ——
    /// `probe` 那侧拒绝"206 说不出可用总长"，所以多 worker 那一支恒已知；`total_len == 0` 是
    /// **真的空文件**。⇒ 渲染侧唯一的义务是不许除零、不许印 `NaN%`，那是活分支不是防御性代码。
    pub total_len: u64,
    /// 已确认进覆盖集的字节。判据①：`total_len.saturating_sub(sched.pending_bytes())`，
    /// **不给调度器加新 API**（`pending_bytes()` 的定义已经是"没进覆盖集的字节"，
    /// `scheduler.rs` 里那条 `pending_bytes() == total − covered` 的等式已经把它钉住了）。
    /// 寿命：**含上一轮续传来的字节**（文件级百分比本来就该按整文件算）。
    pub covered_bytes: u64,
    /// **本次运行**交出去落盘的字节：就是引擎已有的那一格 `Engine::run_delivered`（判据③）。
    /// 与 `Report::duplicated`/`Report::retries` 那两本**跨断点文件**的账**故意不同寿命**（**E20⑧**），
    /// 两处不许互相"对齐"。
    pub run_delivered: u64,
    /// **本次运行**的 dup 增量 = `stats.bytes_duplicated − 起点`，起点是 `plan_from_decision`
    /// 交回的那份跨轮账（判据③）。与上一格**同一拍取** —— 分子分母不同源就是假数。
    /// 口径照 [`crate::state::Stats::bytes_duplicated`]：它是"落在归属范围之外、**可能**被别人
    /// 重下的字节"，**不是流量账单**，`dup %` 的分子只能按这个强度说。
    pub run_bytes_duplicated: u64,
    /// **本次运行**这一拍的瞬时速率。判据④：`(run_delivered − 上一拍的 run_delivered) × 1e6 ÷
    /// 两拍间隔微秒`，间隔下限 1 ms。⚠ **不是**全程平均
    /// （`running_rate_bytes_per_sec` 那个数服务的是 `fetch::window_budget` 的分子，两个数不许共用）。
    /// 第一拍没有"上一拍"，锚点取**主循环开工的那一刻**（不是 `run` 的 `started`：那里面还夹着
    /// 试探与写预读的时间，把它算进分母正是判据④点名的"慢启动把速率算低"）。
    pub instant_bytes_per_sec: u64,
    /// 这一拍 `WorkerState::Busy` 的条数（真在途）。判据⑤：**不是** `any_in_flight()` —— 那个把
    /// `Backoff` 也算进去，它回答的是"还有没有活没干完"。慢重试期间这一格是 0 而
    /// [`RunSnapshot::slow_retry_rounds`] ≥ 1，这一对组合是诚实的，不许为好看把 Backoff 算进来。
    pub active_workers: usize,
    /// `cfg.workers`（进度行里那个 `x4` 的分母）。
    pub workers_total: usize,
    /// 判据⑥：`workers.map(|w| w.attempt.saturating_sub(FAST_RETRY_MAX)).max()`。
    /// ⚠ 取的是**跨 worker 最大值**：既不是"全场总轮数"，也不是"每条链各几轮"。
    /// 0 ⇒ 还没排下慢重试；≥1 ⇒ 已排下第 N 次 30 s 慢重试。每 worker 各自的 attempt 不在 v1
    /// 的交付面里（§6.2 只承诺一个 `slow-retry xN`，YAGNI）。
    pub slow_retry_rounds: u32,
}

/// 快照出口的类型别名。`Send + Sync` 只加在**闭包**上（判据⑨）：`Config` 要能被克隆、要跟着
/// 任务走，而快照只在引擎线程里活一瞬间，不需要跨线程。
pub type OnTick = Arc<dyn Fn(&RunSnapshot) + Send + Sync>;

/// 本次运行"喂给调度器的事件"与"引擎真打出去的那一枪"，按发生的顺序录下来（**E13③**）。
///
/// 为什么不按简报那条 `rep.event_log_path` 落一份日志文件：引擎在生产里不写日志，
/// 也不许为测试造一条（E13③）。为什么录的是**事实**而不是"预测序列"：spec §7.2 明令
/// "不能直接拿『预测序列』对比，因为劈分时机取决于真实时序"。这里录的正是真实时序里真实
/// 发生过的事件，回放给一台全新调度器才比得出"引擎有没有照调度器办事"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceStep {
    /// 第 0 条：本次运行播种给调度器的那批已确认区间（回放要先重现这个初始状态才对得上）。
    /// **永远是 `trace` 的第一条**，即便没有种子（`vec![]` = 从 `Scheduler::new` 起步）。
    Seeded(Vec<Interval>),
    /// 引擎喂给调度器的那一步事件。全部按顺序录，包括每条 `Connected` ——
    /// 调度器只有在这一步上交回动作，漏掉它回放就复现不出派发序列。
    Fed(Event),
    /// 引擎照某个 `Assign` **真打出去的一枪**（区间与线上那条 `Range` 一一对应，但**发的是哪一段
    /// 由 `fetch::BodyShape` 决定**（**E16** 之后这一句有两个读法，别再当成只有一条）：
    /// - 对端认 `Range`（`Windowed`）⇒ 发的是这一枪的**窗口** `[iv.start, min(iv.end, iv.start + WINDOW_BYTES - 1)]`，
    ///   只有窗口够到文件尾才写成开放终点 —— 见 **E14**② 与 `src/fetch.rs` 模块文档；
    /// - 对端不认（`ToEof`）⇒ 发的是**整条区间** `[iv.start, iv.end]`，退化模式下就是问到 EOF。
    ///
    /// 按字面只写第一条是**说谎**：那正是 E16 撤掉的那个形状。）
    Dispatched { worker: usize, iv: Interval },
    /// 调度器把某个在途 worker 的终点提前了。**引擎不为此发新的一枪**（理由见模块文档
    /// "`Retighten` 不是一枪"那节），所以它绝不能混进 `dispatched()` —— 那道链接要比的是线上真收到的 Range。
    Tightened { worker: usize, iv: Interval },
}

/// 一次运行的结论。
///
/// 在简报那五个字段之上**刻意加了四个**，每个都有出处：
/// - `delivered` 与 `confirmed`：**E7/T2-D** 要求"落盘量"与"归属已定的字节"分开算、**分开报**，
///   只有 `bytes_total` 与 `duplicated` 报不出这个区别（两者不等是常态，不是缺陷）。
/// - `fresh_reason`：`Decision::Fresh{reason}` 的原因串必须活着走到用户眼前
///   （`src/resume.rs` 的文档明写"Task 9 会在中途重来时把这句直接打给用户"）。
/// - `message`：契约是**双向**的，只读一半会把一次成功报成失败：
///   ① `ok = false` 必须带着人话（退避封顶 / worker 崩了 / 校验不符 / POST 那一枪没拿全）；
///   ② **反方向不成立** —— `ok = true` 时 `message` 也可以有值：收尾删断点删不掉那一支就是先
///   把 `ok` 判成真的（`run` 里"删不掉不改写 `ok`，只留一句话"那条），语义是"下载已完成，但…"。
///   ⇒ **退出码的判据只有 `!ok`**；`message.is_some()` 只是"有一句要打印"，不是失败信号，
///   写成 `if report.message.is_some() { 非零退出 }` 会踩上面那一支。
///   ⚠ `ok = true` + `message = Some(..)` 这一格**今天没有用例钉住**：要造它得让断点文件先被
///   `save_state` 成功写出、随后 `fs::remove_file` 以一个不是 `NotFound` 的错误失败 —— 现成的形状
///   只有"别的句柄占着那个文件"（Windows 共享冲突）这类竞态，没有确定形状可铺，宁可按实写"无判据"
///   也不造只说服自己的夹具。
///   `FailKind::Transport` 的载荷在生产侧已是单行、≤200 字符、不含 body 字节 ⇒
///   这里**不再截断、不再拼接**（E12⑦：两处截断会互相吃掉信息）。
#[derive(Debug, Clone)]
pub struct Report {
    /// 服务器此刻说的文件总长（`ProbeFacts::total_len`）。
    pub bytes_total: u64,
    /// 磁盘上真有多少字节（收尾时问的 `store.len()`）。
    pub delivered: u64,
    /// 归属已定的字节（覆盖集）。**与 `delivered` 不是一回事**，见模块文档那两本账。
    /// 带 `--data-file` 的那一支**没有覆盖账**（E13④：该形状不落断点），所以那一支恒 0。
    pub confirmed: u64,
    /// **落在归属范围之外、可能被别人重下的字节**（不是"线上流量账单"，勘误 **E14**③；强度只有
    /// "可能"这一档，措辞按 E14③ 批准的那一句，不许写成"必然"。字段口径与
    /// 它为什么不是账单都写在 `state::Stats::bytes_duplicated` 的文档里，两处是同一个数，
    /// 那边那句"**既不是账单的下界也不是上界**"在这里同样成立 —— 别把它打印成"本次多打了多少字节"）。
    /// ⚠ **寿命：跨断点文件累积，不是本次运行的量**。`Decision::Resume` 那一支把快照里的 `stats`
    /// 原样读回来接着加（`plan_from_decision`），所以这个数是"这个断点文件从建立起一共记下的量"；
    /// `Fresh`（含 `--no-resume`）那一支才从 0 起。判据见
    /// `engine::tests::fresh_resume_and_abort_route_to_their_own_plan`（同一夹具里 `retries`/`bytes_delivered`
    /// 被断成"跨轮接着数"；`bytes_duplicated` 那一格由 `state::tests::stats_start_at_zero_and_survive_a_roundtrip`
    /// 钉"存多少读回多少"）。
    /// 例外：带 `--data-file` 的那一支恒 0（那一支既不读也不写断点，E13④，根本没有归属账）。
    pub duplicated: u64,
    /// 快速 + 慢速重试的总次数（spec §6.1/§6.2）。
    /// ⚠ **寿命同上：跨断点文件累积**。续传时这一支从快照把 `stats` 读回来接着加
    /// （`plan_from_decision`），所以报出来的是"这个断点文件一共重试过多少次"，**不是"本次运行"**；
    /// Task 9 打印时要照后一句说，别写成这一轮的次数。已由
    /// `engine::tests::fresh_resume_and_abort_route_to_their_own_plan` 钉住
    /// （`assert_eq!(plan.stats.retries, 2)`，断言消息就是"重试计数是跨轮的账，不能从 0 重数"）。
    /// 例外：带 `--data-file` 的那一支恒 0（E13④：那一支不落断点 ⇒ 没有跨轮的载体，
    /// `run_whole_once` 里 `retries` 与 `duplicated` 都是硬编码的 0）。
    pub retries: u32,
    pub elapsed: Duration,
    /// 本次运行有没有把活干完。**非零退出码的判据就是这个布尔取反（`!ok`）**，不是别的字段是不是 `Some`。
    pub ok: bool,
    /// A16：本次运行是不是因为 `--max-bytes` 到量而停（且没下完）。装配层据此映射退出码 3；
    /// 预算内正常下完时它保持假、`ok` 为真。
    pub budget_reached: bool,
    pub trace: Vec<TraceStep>,
    pub fresh_reason: Option<String>,
    /// 一句给人看的话。**它不是失败信号**：`ok = true` 时它也可以是 `Some`（"下载已完成，但…"），
    /// 所以退出码只由 `!ok` 决定。双向契约、以及"这一格今天没有用例钉住"的实底，都写在结构体文档
    /// 那条 `message` 上。
    pub message: Option<String>,
}

impl Report {
    /// 把 `trace` 回放给一台**全新**的调度器，交回它按序给出的动作。
    ///
    /// 这是 **E13③** 第一道链接的工具：回放给出的 `Assign{iv}` 必须与 `dispatched()` 一一相等，
    /// 回放给出的 `Retighten{iv}` 必须与 `tightened()` 一一相等（`Idle` 两边都滤掉）。
    /// 测试用它，而不是自己另写一个"预测器"—— 后者正是 §7.2 禁止的读法。
    pub fn replay(&self, total_len: u64, min_seg: u64) -> Vec<Action> {
        let seed = match self.trace.first() {
            Some(TraceStep::Seeded(iv)) => iv.clone(),
            _ => vec![],
        };
        let mut sched = if seed.is_empty() {
            Scheduler::new(total_len, min_seg)
        } else {
            Scheduler::resume(total_len, min_seg, &seed)
        };
        let mut actions = vec![];
        for step in &self.trace {
            if let TraceStep::Fed(ev) = step {
                actions.extend(sched.on(*ev));
            }
        }
        actions
    }

    /// 本次运行喂给调度器的全部事件（按顺序）。
    pub fn fed(&self) -> Vec<Event> {
        self.trace
            .iter()
            .filter_map(|s| match s {
                TraceStep::Fed(ev) => Some(*ev),
                _ => None,
            })
            .collect()
    }

    /// 引擎真打出去的那些区间（= 线上应当出现的 `Range`，按派出顺序）。
    pub fn dispatched(&self) -> Vec<Interval> {
        self.trace
            .iter()
            .filter_map(|s| match s {
                TraceStep::Dispatched { iv, .. } => Some(*iv),
                _ => None,
            })
            .collect()
    }

    /// 调度器收紧过的那些区间（**不**对应任何一枪，见模块文档）。
    pub fn tightened(&self) -> Vec<Interval> {
        self.trace
            .iter()
            .filter_map(|s| match s {
                TraceStep::Tightened { iv, .. } => Some(*iv),
                _ => None,
            })
            .collect()
    }
}

/// 主循环为什么停下来。只有 `Done` 有可能是成功，而且要等收尾那两本账才算数。
/// `Debug` 只为 9A 的用例服务（断言消息里要摆出"循环是从哪一条出口停的"），不参与任何判据。
#[derive(Debug)]
enum Stop {
    /// 调度器说没活可派了、也没有任何一枪在途或退避中。
    Done,
    /// `--max-bytes` 到量：本次运行不再派新枪，断点保留，下次运行继续。
    BudgetReached,
    /// `--max-retries` 封顶 ⇒ 整任务失败（**E13④**：`next_retry_delay` 给 `None` 就是这一支）。
    RetriesExhausted { worker: usize, attempted: u32 },
    /// 有 worker 线程已经不在了（崩了，或者全部崩了导致通道断开）⇒ 它手上那一枪永远不会交回
    /// 结论，继续等只会挂死。`None` = 整条回报通道都断了。
    WorkerGone { worker: Option<usize> },
    /// worker 交回的不是网络结论而是**落盘失败**（磁盘满一类的真故障，重试也换不来一块盘）。
    StoreFailed(String),
    /// 断点文件写不进去。与上一条同一地位（"跑到这里是因为盘用不了"），只是坏的是 `.dlm.json`。
    StateFailed(String),
}

/// 主线程发给 worker 的一件事。
enum Job {
    /// 打这一枪。`iv` 是**这一枪**要取的区间，起点已经绕开这一枪自己写过的前缀
    /// （所以"失败段的重下不会重下已写前缀"，见集成用例
    /// `a_failed_segments_written_prefix_survives_and_is_not_downloaded_twice`）。
    Fetch { iv: Interval, rate_bps: u64 },
    /// 收工。worker 只有在阻塞于 `recv` 时才收得到，所以在途的那一枪会跑完（有窗口预算封顶）。
    Stop,
}

/// worker 交回的一件事：结论 + 它**当时被派的那个区间**。
///
/// 为什么把区间带回来而不是让主线程记一本"每个 worker 手上是什么"的表：
/// `Advanced{bytes}` 要折成含端点的 `Progress{abs_end}` 就必须知道这一枪的起点，
/// 而"我让你下的是什么"本来就是主线程发出去的那条消息 —— 让它跟着结论一起回来，
/// 主线程就**不需要**任何自己的段表。E7 禁止的是第二本**覆盖账**（"哪些字节算数了"），
/// 那个问题在这里只有一个答案：`snapshot_intervals()`。
struct WorkerReport {
    worker: usize,
    iv: Interval,
    outcome: Result<Outcome>,
}

/// 每个 worker 的连接状态。**不是段表** —— 段的归属只由调度器决定，
/// 这里只记"这条连接此刻在不在忙"与"它自己这条失败链已经重试了几次"。
#[derive(Debug, Clone, Copy)]
struct WorkerCtl {
    state: WorkerState,
    /// 这条连接**连续**失败过几次（一次成功就归零）。spec §6.1 的退避挂在失败链上，
    /// 而"哪段字节归谁"归调度器，所以这个数按 worker 记，不按区间记。
    ///
    /// 代价要说清楚（这是设计取舍，不是漏）：一个区间失败后它的尾巴回到全局队列，
    /// 可能被**另一个** worker 接手，那条 worker 的 `attempt` 从头数 ⇒ 一个坏区间最多被
    /// `workers` 条链各自退避一轮，请求速率上界是 `workers / 退避时长`，仍然有界。
    /// 反过来按区间记账要的就是那本被 E7 禁掉的第二账。
    attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerState {
    /// 手上没活，等着要。
    Idle,
    /// 一发请求在途。
    Busy,
    /// 失败后退避中；到点才再要活（spec §6.1/§6.2 的退避因此**真的**接进了循环）。
    Backoff { until: Instant },
}

/// 引擎跑一次"可分段形状"的全部家当。所有跨线程的东西都从这里出发。
struct Launch {
    task: Arc<Task>,
    cfg: Config,
    client: Arc<Client>,
    store: Arc<Store>,
    sched: Scheduler,
    /// 服务器此刻说的全长（`ProbeFacts::total_len`）：调度器的边界、`run_window` 的第 6 个参数、
    /// 收尾那道闸，三处都取这一个值。
    total_len: u64,
    /// 对端认不认 Range（`accept_ranges` 是**行为加权**的读法，见 E11③；注释里那句"广告头"已作废）。
    accept_ranges: bool,
    validator: Validator,
    state_path: PathBuf,
    stats: Stats,
    seed: Vec<Interval>,
    fresh_reason: Option<String>,
    /// R17：这一轮每一条窗口请求要带的 `If-Range`；`None` = 不发。
    if_range: Option<String>,
    /// R18：探针与所有 worker 共用的漏桶。
    limiter: Arc<RateLimiter>,
    started: Instant,
}

/// 跑一个任务。**`Err` = 根本跑不起来**（`task.validate()` 不过、workers 为 0、断点文件损坏、
/// 开文件/建目录失败、`resume` 判 `Abort`、落盘或写断点失败）；
/// **`Ok(Report{ok:false})` = 跑到一个结论但没下完**（退避封顶、worker 崩了、校验不符、
/// POST 那一枪没拿全）。Task 9 把 `!ok` 折成非零退出码，这里不发明信号协议
/// （spec §6.6 的 Ctrl-C 在本计划里已降级为"任意时刻被杀都能续"）。
///
/// ⚠ **本函数不看 `consts::FLAG_ADD_ONLY`（spec §5 的 bit0"只入队不下"）**：全树生产代码里那个位
/// 没有任何读取点，它必须由**调用方在进入 `run` 之前**短路（打印任务摘要然后收工）。
/// **一旦进了 `run` 就一定会上网取字节** —— 别以为引擎会替你留着不发那一枪。
///
/// 进度快照（**E21** 甲）在这两条分岔上给得不一样，别把它当成两条路共用的进度源：
/// 可分段那一条每拍至多一发、收尾再发一发终值；带 `--data-file` 的那一条（`run_whole_once`）
/// **一发都没有**，那条路的权威只有这里交回的 [`Report`]。
pub fn run(task: &Task, cfg: Config) -> Result<Report> {
    task.validate()?;
    let started = Instant::now();
    // B3a：HLS 是顺序拼接，不进分段引擎（CLI 已把 --data-file/--max-bytes 与 --hls 设为互斥）。
    if task.hls() {
        return crate::hls::run(task, &cfg, started);
    }
    ensure!(cfg.workers >= 1, "workers 至少要是 1（给了 {}）：没有 worker 就下不动任何东西", cfg.workers);
    let client = Arc::new(build_client()?);
    let task = Arc::new(task.clone());

    // **E11① + E13④**：带 `--data-file` 的形状**不试探、不读断点、不写断点**。
    // 不试探：`probe::run` 自己在上网前硬拒（那不是要绕的 bug，是裁定）。
    // 不读写断点：POST 不可续（E12⑧），给它存一份段表等于凭空造一条
    //           "下一轮会判 `Resume` 却永远执行不了"的假路径。
    if task.data_file.is_some() {
        return run_whole_once(&task, &cfg, &client, started);
    }

    // R18：一只漏桶被探针与所有 worker 共用；`0` = 不限（consume 立即返回）。
    let limiter = Arc::new(RateLimiter::new(cfg.max_speed));

    let target = task.target_path();
    let state_path = state::path_for(&target);
    ensure_parent_dir(&target)?;

    // ① 断点文件。**损坏（`Err`）与不存在（`Ok(None)`）绝不合并**（`state::load` 的决策 4）：
    //    把损坏读成"没有断点"会把已验证过的字节重新算成待下载，还会骗过 `--no-resume` 的判据。
    //    `force_fresh` 折成 `existing = None`（`resume` 的文档就把这一步交给调用方），
    //    且此时**不去读**那个文件 —— 读它只会把"这次不管断点"变成"这次因为断点坏了跑不起来"。
    let loaded = if cfg.force_fresh { None } else { state::load(&state_path)? };

    // ② 试探。`total_len` / `accept_ranges` / 验证子只有这里拿得到 —— **E13②** 正是因此判定
    //    简报那个 `Config{resume: Decision}` 站不住：`Fresh` 里一个事实都没有。
    let prelude_max = PROBE_READ_BYTES;
    let probed = probe::run_with_limiter(&task, &client, prelude_max, Some(&limiter))?;
    let facts = probed.facts();

    // ③ 判定 ⇒ 执行计划（`Abort` 在这里变成 `Err`，**一个字节都还没碰过**）。
    let plan = plan_from_decision(resume::decide(&task, loaded, &facts))?;
    // R17：只有续传、只有对端认 Range 时才发 If-Range。值取自断点验证器；强 ETag 优先，
    // Last-Modified 只在本次没开 `--ignore-mtime` 时用（`resume::if_range_value` 是唯一取值处）。
    let if_range = if_range_for(plan.resume_validator.as_ref(), facts.accept_ranges, task.ignore_mtime());
    let mut seed = plan.seed;
    let mut stats = plan.stats;

    let store = Arc::new(Store::open(&target, plan.truncate)?);

    // 预读的字节：`ProbeResult::prelude` 的文档把"engine 把它写在偏移 0"这条义务交给本模块。
    // 只在**新建**那一支写。续传那一支不写（段表已经说明了文件头部该有什么，再覆写一遍是把
    // 本来就在覆盖集里的字节又记一笔重复账），代价要照实说：**`probe` 已经把那批字节从线上取回来了，
    // 这一支既不落盘也不进任何账** ⇒ 它是**真线上代价却没被报出**的第二处已知少报
    // （第一处 = 在途窗口已落盘却没进段表的那批，两处都写在 `state::Stats::bytes_duplicated` 的字段文档上）。
    //
    // 为什么不给它开一笔账（**E14③ / E17** 的口径，不是偷懒）：引擎手里只有"覆盖集 + `store.len()`"
    // 两个量，从它们倒推"这 `min(prelude_max, total_len)` 个字节是白取的"正是 E14③ 明令禁止的第二本账
    // —— 唯一诚实的量具是对端日志（计划勘误 **E14 附记**：`Σ conn_close_side.sent`），引擎在运行时拿不到。
    // 也不许为它新增状态字段或第三本账（**E7** 的两本账裁定）。
    // 量级是**推出来的**（不是实测）：每一轮 `Resume` 至多 `min(prelude_max, total_len)` 个字节，
    // 其中 `prelude_max = consts::PROBE_READ_BYTES`（引擎直接取常数，`Task` 上没有这个旋钮）；
    // 反复续传会按轮累积，
    // 而 `stats` 本身也跨轮读回来接着加（见字段文档末段）。
    let mut prelude_written = 0u64;
    if plan.truncate && !probed.prelude.is_empty() {
        let take = probed.prelude.len().min(facts.total_len as usize);
        if take > 0 {
            let wrote = store.write_at(0, &probed.prelude[..take]).context("把预读字节写到偏移 0 失败")?;
            if wrote > 0 {
                stats.bytes_delivered = stats.bytes_delivered.saturating_add(wrote as u64);
                seed.push(Interval { start: 0, end: wrote as u64 - 1 });
                prelude_written = wrote as u64;
            }
        }
    }

    let on_disk = store.len()?;
    // 先夹种子：断点说"这些字节已经归我了"，而盘上可能根本没有（用户删了目标文件、
    // 或上一轮在 `save` 之后被杀）。不夹就会造出一个**永远续不上**的任务，理由见 `clamp_seed_to_disk`。
    //
    // 这里**不记**"盘上比覆盖集多出来的那一段"的账（**E17** 撤掉了那一笔）：那一段绝大多数是
    // **从没下载过的洞**（`Store::write_at` 一延长文件，中间没写的洞读起来是零、照样算进 `len()`；
    // 多 worker 的中点劈分天生造洞），拿它倒推重复字节正是 **E14③** 明令禁止的第二本账。
    // 因此"上一轮在途窗口已落盘、没进段表、本轮会重下"那批字节**不在任何账里** ——
    // 少报的口径与其算术上界写在 `state::Stats::bytes_duplicated` 的字段文档上。
    let seed = clamp_seed_to_disk(&seed, on_disk);

    // 播种门（E13①）：断点里那些已确认区间进调度器唯一的那本账。
    let mut sched = Scheduler::resume(facts.total_len, cfg.min_seg, &seed);
    if !facts.accept_ranges {
        // spec §2.2 第二条：对端不认 Range ⇒ 退化。这个开关**只由 `NoRanges` 事件打开**，
        // 播种门不碰它（E13① 末句）。注意"降级那一支不回连接池"（`src/probe.rs` 模块文档里
        // "对端无视那条 `Range` ⇒ 那条连接被**拆掉**、不回池复用"那一条）是
        // **推理不是实测** ⇒ 这里当约束用，不许写成断言。
        sched.on(Event::NoRanges);
    }

    let validator = Validator {
        etag: facts.etag.clone(),
        last_modified: facts.last_modified.clone(),
        total_len: facts.total_len,
        final_url: facts.final_url.clone(),
        // `ignore_mtime` 由调用方传进去（`state`/`resume` 刻意不读 `task.flags`）。
        ignore_mtime: task.ignore_mtime(),
    };

    let mut engine = Engine::new(Launch {
        task,
        cfg,
        client,
        store,
        sched,
        total_len: facts.total_len,
        accept_ranges: facts.accept_ranges,
        validator,
        state_path,
        stats,
        seed,
        fresh_reason: plan.fresh_reason,
        if_range,
        limiter,
        started,
    })?;
    // A16：新建任务首轮预读确实落盘并进覆盖集 ⇒ 它要计入本次运行的字节预算。续传的预读不落盘、
    // 不进账（见上面那段长注释），所以 `budget_base` 只在 `Fresh` 那一支取非零。
    if plan.truncate {
        engine.budget_base = prelude_written;
    }
    // 退化那一枪（`NoRanges`）排在本轮**所有** `Connected` 之前 ⇒ 回放按同一顺序喂才复现得出派发表。
    if !engine.accept_ranges {
        engine.record(TraceStep::Fed(Event::NoRanges));
    }
    // 开工先落一份断点：从这一刻起"任意时刻被杀都能续"才真的成立（spec §4）。
    engine.initial_save()?;
    let stop = engine.drive();
    engine.finish(stop)
}

/// 引擎对一份 `Decision` 的执行计划。**`Abort` 变成 `Err(原因)` 且调用方还没碰过任何文件**
/// （T5 转派第 ④ 条）；`Fresh` ⇒ 覆盖目标文件、没有种子；`Resume` ⇒ 不动文件、段表进调度器。
struct Plan {
    truncate: bool,
    seed: Vec<Interval>,
    fresh_reason: Option<String>,
    stats: Stats,
    /// `Resume` 那一支把断点的验证器带进引擎；`Fresh` 为 `None`。
    /// R17 的 `If-Range` 值从这一份算，不从本轮探针算 —— 探针已经先做过 ③–⑤ 的比对。
    resume_validator: Option<Validator>,
}

fn plan_from_decision(decision: Decision) -> Result<Plan> {
    match decision {
        Decision::Abort(why) => bail!("{why}"),
        Decision::Fresh { reason } => Ok(Plan {
            truncate: true,
            seed: vec![],
            fresh_reason: reason,
            stats: Stats::default(),
            resume_validator: None,
        }),
        Decision::Resume(snap) => {
            let Snapshot { validator, segments, stats, .. } = snap;
            Ok(Plan {
                truncate: false,
                seed: segments.iter().map(SegView::interval).collect(),
                fresh_reason: None,
                stats,
                resume_validator: Some(validator),
            })
        }
    }
}

/// R17 的取值入口：Fresh（没有断点验证器）、不认 Range 的对端、没有可用验证器 ⇒ 都不发。
/// 单独成函数是为了让这三条否定条件各有一格单测，而不是埋在 `run` 的分支里。
fn if_range_for(resume_validator: Option<&Validator>, accept_ranges: bool, ignore_mtime: bool) -> Option<String> {
    if !accept_ranges {
        return None;
    }
    resume_validator.and_then(|v| resume::if_range_value(v, ignore_mtime))
}

/// 完成判定：`is_done()` **且** 落盘量 == 全长。**签名里没有覆盖集那个量**（E7）。
///
/// `confirmed == total_len` 而 `on_disk < total_len` 时（416 封存过一段从没落盘的字节）
/// 这里必须给假 —— 那条闸问的是"磁盘上到底有多少字节"，答案只有 `store.len()` 给得起。
fn completion_verdict(sched_done: bool, on_disk: u64, total_len: u64) -> bool {
    sched_done && on_disk == total_len
}

/// `Advanced{bytes}` → `Event::Progress` 的 `abs_end`（**含端点**，见模块文档那条公式更正）。
/// `bytes == 0` ⇒ `None`：一个字节都没落地就没有"写到哪儿"可报（`fetch` 的 `Advanced` 恒 ≥ 1，
/// 这里守的是调用方，不是对端）。
fn progress_abs_end(shot_start: u64, bytes: u64) -> Option<u64> {
    bytes.checked_sub(1).map(|d| shot_start.saturating_add(d))
}

/// 这一枪**落到盘上**的字节数。`Complete` 不带字节，但它兑现的承诺是"`iv.end` 那个字节也写了"
/// （`fetch::run_window` 末尾那条 `cursor > iv.end` 判据），所以它就是区间全长；
/// 三条失败支在返回之前**一个字节都没写过**（状态码/偏移核对都在读 body 之前）。
fn shot_bytes(outcome: &Outcome, shot: Interval) -> u64 {
    match outcome {
        Outcome::Advanced { bytes } => *bytes,
        Outcome::Complete => shot.len(),
        Outcome::Stalled | Outcome::Failed(_) => 0,
    }
}

/// 这一枪写下的字节里，落在"调度器此刻认给这个 worker 的区间"之内的部分。
/// 超出那一段（**E14**① 乙之后只剩帽追不回的那两类）：收紧落在这枪**结束之后**（字节已经写了），
/// 或者调度器按上次上报的进度判出的新终点落在这枪**早已读过的位置之下** —— 帽只能往下压、不能往回退。
/// 越过帽本身最多一个 64 KiB 分块。这些字节真在盘上、但永远不会进覆盖集 ⇒ **可能**由另一枪重下
/// （"可能"这一档是 E14③ 批准的口径，不许写成"必然"）
/// ⇒ 正是 `bytes_duplicated` 该计的账（口径见 `state::Stats::bytes_duplicated`）。
fn in_scope_bytes(shot: Interval, bytes: u64, owned: Interval) -> u64 {
    if bytes == 0 {
        return 0;
    }
    // 交集体取两端都在内：可达形状里 `owned.start == shot.start`（`split_tail` 只动终点），
    // 下界写出来是为了让这个函数的名字与它算的东西一致，不是为了处理那条不可能。
    let lo = shot.start.max(owned.start);
    let hi = shot.start.saturating_add(bytes - 1).min(owned.end);
    if lo > hi { 0 } else { hi - lo + 1 }
}

/// 一条 `fetch` 结论折成"喂给调度器的事件 + 落盘量 + 重复量"。**引擎里唯一的事件翻译处**。
///
/// 映射表（`fetch::Outcome` 的文档与派发给的三条，加上那条差一字的更正）：
/// | 结论 | 事件（按序） | 落盘 | 重复 |
/// |---|---|---|---|
/// | `Advanced{bytes}` | `Progress{abs_end: start+bytes-1}` → `WindowDone` | `bytes` | 出界的那截 |
/// | `Complete` | `Progress{abs_end: iv.end}` → `SegmentComplete` | `iv.len()` | 出界的那截 |
/// | `Stalled` / `Failed` | `Failed` | 0 | 0 |
///
/// `WindowDone` 而不是"再来一枪"：窗口读满与 body 被掐断在调度器看来是同一件事
/// （`Scheduler::on` 里 `Event::WindowDone | Event::Failed` 那一支两者都走 `release()`），
/// 剩下的尾巴由它重新决定给谁。
fn translate(outcome: &Outcome, shot: Interval, owned: Interval, worker: usize) -> (Vec<Event>, u64, u64) {
    let landed = shot_bytes(outcome, shot);
    let duplicated = landed - in_scope_bytes(shot, landed, owned);
    let mut events = Vec::with_capacity(2);
    if let Some(abs_end) = progress_abs_end(shot.start, landed) {
        events.push(Event::Progress { worker, abs_end });
    }
    events.push(match outcome {
        Outcome::Advanced { .. } => Event::WindowDone { worker },
        Outcome::Complete => Event::SegmentComplete { worker },
        Outcome::Stalled | Outcome::Failed(_) => Event::Failed { worker },
    });
    (events, landed, duplicated)
}

/// 一条动作对这个 worker 意味着什么：**发一枪**，还是**只把在途那一枪的目标提前**。
///
/// 单独做成一个纯函数就是为了能被表钉死：`Retighten` 落在 `Tighten` 而不是 `Fire`，
/// 是模块文档那一节的全部内容（发一枪会把已写前缀重下一遍）。
/// 序号取自动作本身而不是"当前正在处理的那个 worker"：调度器可以把 `Retighten` 指向
/// 另一个正忙的 worker（`split_tail` 正是如此），拿错对象就会把收紧记到别人头上。
enum Shot {
    Fire { worker: usize, iv: Interval },
    Tighten { worker: usize, iv: Interval },
    Nothing,
}

fn action_for_worker(action: Action) -> Shot {
    match action {
        Action::Assign { worker, iv } => Shot::Fire { worker, iv },
        Action::Retighten { worker, iv } => Shot::Tighten { worker, iv },
        Action::Idle => Shot::Nothing,
    }
}

/// 把种子夹到"盘上真有的那一段"里：越过 EOF 的尾巴丢掉，整条越界的行也丢掉。
///
/// 为什么引擎必须做这一步：`Store::open(truncate=false)` 的文档明写"断点文件说有进度、目标却已被
/// 用户删掉 ⇒ 空手起步（长度 0），不报错"。可 `resume::decide` 只看任务与验证子，**看不见文件**，
/// 它会照样判 `Resume`。那时如果把"盘上根本不存在"的区间照播给调度器，调度器就把它们算成已确认
/// （`is_done()` 真、覆盖集 == 全长），而 `store.len()` 永远补不齐 ⇒ 每一轮都只能报 `ok = false`
/// 且**永远续不上**（失败那一支按裁定留着断点文件，下一轮还是同一副样子）。
/// 夹一道就是把 `Store` 那句承诺兑现到调度器的输入上：那些字节重新变成待下载。
///
/// 不破坏 `state::check_segments` 的闸：只把终点往前挪或整行删掉，行与行之间只会**更远**，
/// 不会新产生重叠或相邻。`on_disk == 0` ⇒ 交回空表（与 `Scheduler::new` 同一形状）。
fn clamp_seed_to_disk(seed: &[Interval], on_disk: u64) -> Vec<Interval> {
    if on_disk == 0 {
        return vec![];
    }
    let last = on_disk - 1;
    seed.iter()
        .filter(|iv| iv.start <= last)
        .map(|iv| Interval { start: iv.start, end: iv.end.min(last) })
        .collect()
}

/// 运行至今的平均交付速率（**字节/秒** —— `fetch::window_budget` 的除数就是字节/秒，
/// 那里 `WINDOW_BYTES / rate` 用的都是字节）。
///
/// 用"累计字节 / 累计墙钟"而不是滑动窗口：v1 要的是"别把预算定得比实际吞吐还小"，
/// 平均值够稳且不引入第二个状态。两处除零都要挡：`elapsed == 0`（同一瞬间）⇒ 0 = 未知 ⇒
/// `window_budget` 给最宽的 120 s；这一支绝不能 panic 或在 debug 里炸。
fn running_rate_bytes_per_sec(delivered: u64, elapsed: Duration) -> u64 {
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    if micros == 0 {
        return 0;
    }
    delivered.saturating_mul(1_000_000) / micros
}

/// 快照里那一格"瞬时速率"的式子（勘误 **E21** 判据④）。
///
/// 与上面那条 [`running_rate_bytes_per_sec`] 是**两条式子、两个用途**，不许并成一条：
/// - 这条的分子是**两拍之差**（`delta`），问的是"这一拍在动多快" ⇒ 进度行 `3.1 MiB/s`；
/// - 那条的分子是**累计字节 / 累计墙钟**，问的是"这轮平均交得出多少" ⇒ `fetch::window_budget`
///   的分子（那里要的是"别把预算定得比实际吞吐还小"，平均值才是它要的稳）。
///
/// 判据④点名的是坑：拿平均去显示，慢启动段会把它算低（一整段 15 s 的爬坡把后面稳定的一秒
/// 也一起摊平）。用例 `the_rate_cell_is_the_difference_between_two_beats_not_the_whole_run_average`
/// 就是这一条的活体针（把 `run_delivered / started.elapsed()` 交出去当显示速率，它当场红）。
///
/// 间隔下限 **1 ms**：`LOOP_POLL` 之下两次取时钟可能落在同一微秒里，不设下限就是除以 0
/// （debug 构建 panic）或报出一个 5e8 字节的假高速。`saturating_mul` 与那条同一个立场：
/// 速率这一格绝不能在加法上回绕。
fn instant_rate_bytes_per_sec(delta_bytes: u64, gap: Duration) -> u64 {
    let micros = u64::try_from(gap.as_micros()).unwrap_or(u64::MAX).max(1_000);
    delta_bytes.saturating_mul(1_000_000) / micros
}

/// 快照里那一格"在途 worker 数"（判据⑤）：**只数 `Busy`**。
///
/// 与 [`Engine::any_in_flight`] 的区别是判据本身：那一条回答"还有没有活没干完"（退避中的人手里
/// 还有一段没交回的区间，算活），这一条回答"此刻真有几条连接在收字节"（进度行的 `x4`）。
/// 慢重试那一档最见区别：全场都在 30 s 退避 ⇒ 这一格是 **0**，而 `slow_retry_rounds ≥ 1`；
/// 把 Backoff 算进来就是让进度行说谎（"4 条连接在跑"，实际一条都没有）。
fn busy_worker_count(workers: &[WorkerCtl]) -> usize {
    workers.iter().filter(|ctl| ctl.state == WorkerState::Busy).count()
}

/// 快照里那一格"慢重试轮数"（判据⑥）：跨 worker 取**最大值**。
///
/// `WorkerCtl::attempt` 的语义是"这条失败链已经排下来几次重试、一次成功就归零"，
/// 减去快速那一档的 `FAST_RETRY_MAX = 5` 就是"已经排下的 30 s 慢重试有几次"：
/// attempt 5 ⇒ 0（第 6 次才排下头一发），attempt 9 ⇒ 4。
/// 为什么是 max 而不是求和或逐条报：§6.2 只承诺一个 `slow-retry xN`，而求和会把
/// "四条链各排下 1 次"印成 `x4`（读起来像同一条链重试了四轮，那是假象）；
/// 逐 worker 报不在 v1 的交付面里。
fn slow_retry_rounds(workers: &[WorkerCtl]) -> u32 {
    workers.iter().map(|ctl| ctl.attempt.saturating_sub(FAST_RETRY_MAX)).max().unwrap_or(0)
}

/// `FailKind` → 一句能给人看的话。**不截断、不拼接**（E12⑦：载荷已经是单行 ≤200 字符）。
fn failure_note(kind: &FailKind) -> String {
    match kind {
        FailKind::Http(code) => format!("服务器对这一枪回了 {code}"),
        FailKind::Transport(note) => note.clone(),
        FailKind::ValidatorChanged => "服务端说这份文件已经变了（带 If-Range 的请求回了 200，或 416/206 的范围对不上）".to_string(),
    }
}

/// `--sha256` 那道闸的比对（spec §6.7）：**大小写不敏感、允许首尾空白**。
/// 期望串为空（用户给了 `--sha256 ""`）时**永不放行**：一条恒真的闸比没有闸更坏。
pub(crate) fn hash_matches(got: &str, want: &str) -> bool {
    let want = want.trim();
    !want.is_empty() && got.eq_ignore_ascii_case(want)
}

/// `--sha256` 不符那一句**用户可见**文案的唯一出口（T99-G5 第 2 条）。
///
/// 为什么要有这个函数：`run_whole_once` 与 `Engine::finish` 以前**各抄了一份同一个字面量**，
/// 于是那半句 `（spec §6.7）` 出现在两处、一次真跑的 stdout 里就带着一个内部文档坐标
/// （E28③ 那一族的另一半表面：那根针只管 `--help`，失败路径不在射程里）。
/// 两处各写一遍就是"下一次改口只改一半"的成因 ⇒ 收成一只出口。
///
/// **信息量一个字不许少**：算得什么／期望什么／两份文件都保留不自动删，这三件事照旧要说清
/// （`tests/integration.rs` 里那根扩过的针的正臂断的就是这句话真的到达用户）。
/// 被删掉的只有 `（spec §6.7）` 那半句括注 —— 用户不需要去处，出处在文档里（spec 归 spec）。
/// 这句走"结论"那一档（`ok = false` ⇒ 退出码 1），**不是**"参数写错了"那一族。
pub(crate) fn sha256_mismatch_message(got: &str, want: &str) -> String {
    format!("sha256 不符：算得 {got}，期望 {want}。目标文件与断点文件都保留，不自动删")
}

/// 字节 → 小写十六进制（`hex` crate 不在 spec §9 的白名单里，两行手写）。
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 对任意 `Read` 算 sha256（分块，不把整个文件吸进内存）。与 `harness::sha256_of` 同一形状，
/// 两边各算一遍正是"引擎说的对 ≠ 引擎自己给自己打分"的那道分离。
fn sha256_of_reader(reader: &mut impl Read) -> Result<String> {
    const CHUNK: usize = 64 * 1024;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).context("读目标文件算 sha256 失败")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

pub(crate) fn sha256_of_path(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("打开目标文件算 sha256 失败：{}", path.display()))?;
    sha256_of_reader(&mut file)
}

/// 建引擎级客户端。形状与 `tests/harness/mod.rs` 的 `client()` 一致，且**刻意**没有
/// `Connection: close` / `pool_max_idle_per_host(0)`（R5 / E2 / STATUS 第 3 条）。
pub(crate) fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(CLIENT_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .tcp_nodelay(true)
        .build()
        .context("建 HTTP 客户端失败")
}

/// 目标文件的父目录。`Store::open` 与 `state::save` 都不建目录（模块文档各自写明"归 cli/engine"）。
fn ensure_parent_dir(target: &Path) -> Result<()> {
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("建目标目录失败：{}", dir.display()))
}

/// `--data-file` 那一支：**一发请求、只发一次**（spec R7 / E11① / E12⑧）。
///
/// 不试探、不读断点、不写断点、不开 worker 线程（该形状不可分段，spec §2.2 第一条）。
/// 事实取自这一枪**自己的**响应头（`fetch::run_whole` 的 F2 那半）。
/// 被窗口预算掐在 body 中间 ⇒ 如实报"未完成"。**绝不重发** —— 重发就是把用户的表单交第二次。
///
/// ⚠ **这一支一发 `on_tick` 都不交**（勘误 **E21** 判据⑧）：整程阻塞在一发请求里、按 **E8②**
/// 不开线程，中途没有任何引擎侧读得到的量 —— 这里连 `Engine` 都没有构造。
/// 所以 `cfg.on_tick` 在本函数里**一次都不被读**，那条路的进度权威只有下面这份 [`Report`]。用例
/// `the_data_file_route_emits_no_ticks_while_the_segment_route_emits_some` 钉这一格。
fn run_whole_once(task: &Arc<Task>, cfg: &Config, client: &Arc<Client>, started: Instant) -> Result<Report> {
    let target = task.target_path();
    ensure_parent_dir(&target)?;
    // 恒 truncate：这一支没有可读的断点，残留的半截文件不属于本轮任何账。
    let store = Store::open(&target, true)?;
    // R18：整份那一枪只有一条连接；`0` = 不限。
    let limiter = RateLimiter::new(cfg.max_speed);
    let (outcome, facts) = fetch::run_whole_with_limiter(client, task, &store, RATE_UNKNOWN, Some(&limiter))?;
    // 收尾一次 flush。**E8① 的判据是"收尾调一次、绝不进 `run_window` 的每块循环"，不是"全二进制只有一个调用点"**
    // —— 本文件有两条互斥的出口路线（`data_file` 走这里，多 worker 走 `Engine::run` 收尾那一处），
    // 所以 flush 有两个调用点，每条路线上至多一次（T8 评审 E 第 2 条：原先这两句各写"全场只有这一处"，互为反例）。
    // 此刻没有任何 writer 活着 —— 本函数不开线程（E8②）。
    store.flush()?;
    let delivered = store.len()?;
    let bytes_total = facts.as_ref().map_or(0, |f| f.total_len);

    let mut ok = matches!(outcome, Outcome::Complete) && delivered == bytes_total;
    let mut message = match &outcome {
        Outcome::Complete if !ok => Some(format!(
            "整份那一枪说完成了，盘上却只有 {delivered}/{bytes_total} 字节"
        )),
        Outcome::Complete => None,
        Outcome::Advanced { bytes } => Some(format!(
            "POST 那一枪被窗口预算掐在 body 中间（已写 {bytes} 字节，服务器说全长 {bytes_total}）。\
             这一枪只许发一次：重发就是把表单交第二次（spec R7/E12⑧），所以如实报未完成"
        )),
        Outcome::Stalled => Some("POST 那一枪在预算内一个字节都没到（连接没答或答得太慢）；这一枪不重发".to_string()),
        Outcome::Failed(kind) => Some(format!(
            "POST 那一枪没成：{}；这一枪不重发（重发＝二次提交表单）",
            failure_note(kind)
        )),
    };
    if ok && let Some(want) = &cfg.verify_sha256 {
        match sha256_of_path(&target) {
            Err(e) => {
                ok = false;
                message = Some(format!("{e:#}"));
            }
            Ok(got) if !hash_matches(&got, want) => {
                ok = false;
                message = Some(sha256_mismatch_message(&got, want));
            }
            Ok(_) => {}
        }
    }
    Ok(Report {
        bytes_total,
        delivered,
        // 该形状**没有覆盖账**（E13④）：不读不写断点，也就没有"归属已定的字节"可报。
        confirmed: 0,
        duplicated: 0,
        retries: 0,
        elapsed: started.elapsed(),
        ok,
        budget_reached: false,
        // `trace` 留空：这条链接里没有调度器，没有任何事件被喂出去；硬造一条 `Dispatched`
        // 会让阶段 5 拿一个**没带 Range 的 POST** 去比对区间。
        trace: vec![],
        fresh_reason: None,
        message,
    })
}

/// worker 线程的入口参数（打包成一个结构是为了躲 `clippy::too_many_arguments`，
/// 不是为了好看：八条通道/句柄少一个都编译不过）。
struct WorkerSpawn {
    worker: usize,
    jobs: Receiver<Job>,
    reports: Sender<WorkerReport>,
    alive: Arc<AtomicBool>,
    /// 这一枪的**终点帽**（勘误 **E14**①）：每个 worker 一格，主线程在 `Retighten` 上往下调，
    /// `fetch::run_window` 每读一块查一次。与 `alive` 同一地位（跨线程那一侧的东西都从这里出发）。
    cap: Arc<fetch::EndCap>,
    client: Arc<Client>,
    task: Arc<Task>,
    store: Arc<Store>,
    total_len: u64,
    /// 这一台 worker 的每一枪取多少 body（勘误 **E16** R-B2）：对端认 `Range` ⇒ `Windowed`，
    /// 不认 ⇒ `ToEof`。**整个 worker 生命周期里恒定** —— 它由 `ProbeFacts.accept_ranges` 决定，
    /// 而那个事实一次运行之内不变（变了就是 `ValidatorChanged`，走的是另一条路）。
    body: fetch::BodyShape,
    /// R17：这一轮每一条窗口请求要带的 `If-Range`；`None` = 不发。一次运行内不变。
    if_range: Option<Arc<str>>,
    /// R18：所有 worker 共用的漏桶。
    limiter: Arc<RateLimiter>,
    /// A4/R21：这一枪写到哪（含端点）。worker 每写一块更新，engine 在下次 `Connected` 前读它。
    progress: Arc<AtomicU64>,
}

/// 线程心跳：`Drop` 时置假，**包括 panic 展开时**（std 保证 unwind 期间析构）。
/// 主线程靠它分辨"这一枪还在读"与"这个线程已经不在了"。
struct Heartbeat(Arc<AtomicBool>);

impl Heartbeat {
    fn new(flag: Arc<AtomicBool>) -> Heartbeat {
        flag.store(true, Ordering::SeqCst);
        Heartbeat(flag)
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// worker 的全部职责：`recv 一个区间 → 打那一枪 → 交回结论`。不认识调度器，不认识断点文件。
///
/// `cap` 是**这一枪**的终点帽（勘误 **E14**①）：主线程在 `Retighten` 上把它往下调，
/// `run_window` 每读一块查一次。这一格与这个 worker 同生同死（`Engine::new` 建、随 `dispatch`
/// 重置），所以收紧永远递得给"此刻在途的那一枪"—— 这是 `Retighten` 唯一能兑现的通道：
/// 那个线程正阻塞在 `resp.read()` 里，除了它自己每块看一眼的格子，外面没有任何东西能插话。
fn worker_loop(spawn: WorkerSpawn) {
    let WorkerSpawn { worker, jobs, reports, alive, cap, client, task, store, total_len, body, if_range, limiter, progress } = spawn;
    let _heartbeat = Heartbeat::new(alive);
    // `recv` Err = 主线程已经交回发送端（引擎退出）⇒ 收工。`Job::Stop` 是正常出口。
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Stop => break,
            Job::Fetch { iv, rate_bps } => {
                // `run_window` 里的 `Err` 只有一种来源：`store.write_at` 失败（网络失败它自己折成
                // `Outcome::Failed`）。所以上抛的就是"落盘坏了"这一件事，主线程据此停整个任务。
                let outcome = fetch::run_window_with_limits(
                    &client,
                    &task,
                    iv,
                    &store,
                    rate_bps,
                    total_len,
                    &cap,
                    body,
                    if_range.as_deref(),
                    Some(&limiter),
                    Some(&progress),
                );
                if reports.send(WorkerReport { worker, iv, outcome }).is_err() {
                    break; // 主线程不在了，交回给谁都没意义
                }
            }
        }
    }
}

/// 主循环。持有 `Scheduler`、`Store`、退避时钟与保存节奏，是**唯一**写断点文件的人（spec §4）。
struct Engine {
    task: Arc<Task>,
    cfg: Config,
    /// `client` **不在这里**：客户端只在 `Engine::new` 里被克隆给每条 worker 线程，
    /// 主线程自己一条请求都不发（试探在起线程之前就做完了）。留着它就是一具没人读的尸体。
    store: Arc<Store>,
    sched: Scheduler,
    total_len: u64,
    accept_ranges: bool,
    validator: Validator,
    state_path: PathBuf,
    stats: Stats,
    /// 主线程发给每个 worker 的那一端。
    jobs: Vec<Sender<Job>>,
    /// worker 交回结论的那一端。**主线程不留任何 `Sender` 克隆**，否则"全部线程都退了"
    /// 永远不会变成 `Disconnected`，那一路挂死就没人报。
    reports: Receiver<WorkerReport>,
    handles: Vec<JoinHandle<()>>,
    workers: Vec<WorkerCtl>,
    alive: Vec<Arc<AtomicBool>>,
    /// 每个 worker 一格**终点帽**（勘误 **E14**①）：主线程处理 `Action::Retighten{worker, iv}` 时
    /// 把 `iv.end` **往下**调，那一枪的读循环每块查一次 ⇒ 收紧终于递得进正阻塞在 `read()` 里的那一枪。
    /// 与 `alive` 同一地位（每 worker 一格、随 `Engine` 生灭）；`dispatch` 在发下一枪之前重置它，
    /// 所以"帽只降不升"这条纪律的作用域是**一枪之内**（跨枪重置是主线程自己写的，不会与 worker 抢）。
    caps: Vec<Arc<fetch::EndCap>>,
    /// A4/R21：每 worker 一格块级光标（含端点）；worker 每写一块更新。
    progress: Vec<Arc<AtomicU64>>,
    /// 已经喂给调度器的光标值；避免同一条 `Progress` 重复喂。
    synced_progress: Vec<u64>,
    /// 调度器此刻认给每个 worker 的区间（`Retighten` 会就地改它）。**不是第二本覆盖账**：
    /// 它只回答"这一枪的字节该不算作重复的那条边界在哪"，覆盖集永远只有 `snapshot_intervals()`。
    current: Vec<Option<Interval>>,
    trace: Vec<TraceStep>,
    fresh_reason: Option<String>,
    /// 本轮自己交出去的字节（速率用它算，不把上一轮的 `stats.bytes_delivered` 混进来）。
    run_delivered: u64,
    /// A16：每个 worker 当前那一枪**预占**的字节预算；枪收尾即归零。与 `run_delivered` 分开算，
    /// 否则并发时后面的 worker 会把同一份额度再借一遍。
    budget_share: Vec<u64>,
    /// A16：新建任务时预读落盘的那批字节（续传的预读不计数）。它不在 `run_delivered` 里 ——
    /// 后者继续只服务速率计算（`RunSnapshot::instant_bytes_per_sec` 的分母）。
    budget_base: u64,
    /// A16：额度已经分不出来（或 `run_delivered + budget_base` 已到顶）。置真后只收在途结论，
    /// 不再向调度器要 Assign/Retighten。
    budget_exhausted: bool,
    last_save: Instant,
    last_tick: Instant,
    /// 上一**拍**交快照的时刻（勘误 **E21** 判据④的分母来源）。
    ///
    /// 为什么不复用隔壁那格 `last_tick`：那一记的是"上一次循环迭代结束"，报告来得勤时它每来一条
    /// 就被往前挪一次，用它算间隔会把"两拍之差"读成"两块之差"。锚点初值取主循环开工的那一刻
    /// （不是 `run` 的 `started`，理由写在 [`RunSnapshot::instant_bytes_per_sec`] 上）。
    tick_at: Instant,
    /// 上一拍交快照时的 `run_delivered`（判据④的分子来源；与 `tick_at` **同一拍取**）。
    tick_delivered: u64,
    /// 本轮开工时从断点读回的那份 `bytes_duplicated`（判据③：快照交的是**本次运行**的增量，
    /// 要减的就是这个起点）。`run_delivered` 不需要对应的起点 —— 它本来就是从零起的本轮量。
    dup_base: u64,
    /// 上次落盘时的 `pending_bytes()`。它只可能因为覆盖集增长而**变小**，是"关键事件发生过"的记号。
    saved_pending: u64,
    started: Instant,
    /// 最近一次失败的原文（`RetriesExhausted` 时报给用户）。
    last_note: Option<String>,
}

impl Engine {
    /// 建通道、起 `cfg.workers` 条线程。起不来就把已经起的那几条收回去再报错（"根本跑不起来"）。
    fn new(launch: Launch) -> Result<Engine> {
        let Launch {
            task, cfg, client, store, sched, total_len, accept_ranges, validator, state_path, stats, seed,
            fresh_reason, if_range, limiter, started,
        } = launch;
        let count = cfg.workers;
        // 交快照时要减掉的那个起点必须在 `stats` 被搬进结构体**之前**取（判据③）。
        let dup_base_from_stats = stats.bytes_duplicated;

        // **E16** R-B2：对端不认 `Range` ⇒ 每一枪取整条剩余 body（`ToEof`），**不许**按窗重请求。
        // 信号只有这一处（`ProbeFacts.accept_ranges`，**E11③** 的行为加权读法）；`run_window` 自己
        // 要到响应头到手才看得见退化，而预算（整请求期限，R7）必须在那之前定 —— 次序见 `fetch::BodyShape`。
        let body = if accept_ranges { fetch::BodyShape::Windowed } else { fetch::BodyShape::ToEof };
        // R17 的值一次运行内不变；每个 worker 拿一份引用计数克隆，别把 String 复制 N 份。
        let if_range: Option<Arc<str>> = if_range.map(Arc::<str>::from);

        let (reports_tx, reports) = channel::<WorkerReport>();
        let mut jobs = Vec::with_capacity(count);
        let mut handles = Vec::with_capacity(count);
        let mut alive = Vec::with_capacity(count);
        // 终点帽一格一个 worker（**E14**①）：初值就是"外面没有帽"，`dispatch` 每次发枪前重置。
        let mut caps = Vec::with_capacity(count);
        let mut progresses = Vec::with_capacity(count);
        for worker in 0..count {
            let (tx, rx) = channel::<Job>();
            let flag = Arc::new(AtomicBool::new(true));
            let cap = Arc::new(fetch::EndCap::new(fetch::NO_END_CAP));
            let progress = Arc::new(AtomicU64::new(fetch::NO_PROGRESS));
            let spawn = WorkerSpawn {
                worker,
                jobs: rx,
                reports: reports_tx.clone(),
                alive: Arc::clone(&flag),
                cap: Arc::clone(&cap),
                client: Arc::clone(&client),
                task: Arc::clone(&task),
                store: Arc::clone(&store),
                total_len,
                body,
                if_range: if_range.clone(),
                limiter: Arc::clone(&limiter),
                progress: Arc::clone(&progress),
            };
            match thread::Builder::new().name(format!("dlm-worker-{worker}")).spawn(move || worker_loop(spawn)) {
                Ok(handle) => {
                    jobs.push(tx);
                    handles.push(handle);
                    alive.push(flag);
                    caps.push(cap);
                    progresses.push(progress);
                }
                Err(e) => {
                    // 收回已起的线程：它们要么阻塞在 `recv`（`Stop` 立刻生效），要么在跑一枪（有窗口预算封顶）。
                    for tx in &jobs {
                        let _ = tx.send(Job::Stop);
                    }
                    for handle in handles.drain(..) {
                        let _ = handle.join();
                    }
                    return Err(anyhow::Error::from(e).context(format!("开不出第 {worker} 个 worker 线程")));
                }
            }
        }
        // 主线程那份克隆必须交还：`Stop::WorkerGone{None}` 靠 `Disconnected` 才认得出"全没了"。
        drop(reports_tx);

        let idle = WorkerCtl { state: WorkerState::Idle, attempt: 0 };
        let now = Instant::now();
        let mut engine = Engine {
            task,
            cfg,
            store,
            sched,
            total_len,
            accept_ranges,
            validator,
            state_path,
            stats,
            jobs,
            reports,
            handles,
            workers: vec![idle; count],
            alive,
            caps,
            progress: progresses,
            synced_progress: vec![fetch::NO_PROGRESS; count],
            current: vec![None; count],
            trace: Vec::new(),
            fresh_reason,
            run_delivered: 0,
            budget_share: vec![0; count],
            budget_base: 0,
            budget_exhausted: false,
            last_save: now,
            last_tick: now,
            // 快照的两个锚点都从"主循环开工"起算（判据④的第一拍没有上一拍，就用这个锚点），
            // dup 的起点是断点带回来的那份跨轮账（判据③）。
            tick_at: now,
            tick_delivered: 0,
            dup_base: dup_base_from_stats,
            saved_pending: 0,
            started,
            last_note: None,
        };
        engine.saved_pending = engine.sched.pending_bytes();
        // 种子排在 trace 第一条：回放要先重现"这一轮开工时已经确认了什么"（E13③）。
        engine.record(TraceStep::Seeded(seed));
        Ok(engine)
    }

    fn record(&mut self, step: TraceStep) {
        self.trace.push(step);
    }

    fn keep_state(&self) -> bool {
        self.task.flags & FLAG_KEEP_STATE != 0
    }

    /// 开工那一份断点。失败 ⇒ `Err`（"这次跑不起来"，不是"跑完没跑全"）。
    fn initial_save(&mut self) -> Result<()> {
        self.save_state()
    }

    fn save_state(&mut self) -> Result<()> {
        let segments: Vec<SegView> = self.sched.snapshot_intervals().into_iter().map(SegView::from).collect();
        let mut snap = Snapshot::from_task(&self.task, self.validator.clone(), segments)?;
        // `from_task` 会把 stats 归零（它只认任务与段表，不认运行中的账），这里把本轮的账填回去；
        // `saved_at` 也按派发那句重新盖一次 `utc_now()`，与 `stats` 同一时刻。
        snap.stats = self.stats;
        snap.saved_at = state::utc_now();
        state::save(&self.state_path, &snap)?;
        self.last_save = Instant::now();
        self.saved_pending = self.sched.pending_bytes();
        Ok(())
    }

    /// 把到期退避的 worker 放回可派发行列（spec §6.1/§6.2 的睡眠因此**真的**接进了循环）。
    fn expire_backoffs(&mut self) {
        let now = Instant::now();
        for ctl in &mut self.workers {
            if let WorkerState::Backoff { until } = ctl.state
                && now >= until
            {
                ctl.state = WorkerState::Idle;
            }
        }
    }

    /// 每个空闲 worker 报一次 `Connected`。**只有这一支事件会交回动作**（`Scheduler::on` 里只有
    /// `Event::Connected` 那一支把 `dispatch` 的结果交回来，其余各支一律返回空表），
    /// 所以"worker 空出来了"必须重发这个事件，而不是等调度器主动派。
    fn ping_free_workers(&mut self) -> Option<Stop> {
        // A4/R21：先把在途 worker 的块级光标同步给调度器，再让 Connected 决定劈分。
        self.sync_in_flight_progress();
        // A16：额度分不出来时**连 `Connected` 都不喂** —— 调度器一旦 `Assign` 就会把区间记在
        // 某个 worker 名下，而我们不发枪 ⇒ 那段会永远"在途"。宁可这一拍不派，等在途收尾。
        if self.cfg.max_bytes > 0 && self.budget_available() == 0 {
            self.budget_exhausted = true;
            return None;
        }
        for worker in 0..self.workers.len() {
            if self.workers[worker].state != WorkerState::Idle {
                continue;
            }
            let actions = self.sched.on(Event::Connected(worker));
            self.record(TraceStep::Fed(Event::Connected(worker)));
            for action in actions {
                match action_for_worker(action) {
                    Shot::Fire { worker: w, iv } => {
                        if let Some(stop) = self.dispatch(w, iv) {
                            return Some(stop);
                        }
                    }
                    // 收紧在途那一枪的目标：**不发新的一枪**（模块文档那节），而是把终点帽递下去。
                    // 序号可能不是 `worker` 而是另一个正忙的 worker（`split_tail` 正是这样），
                    // 所以这里按动作里的那个序号记账。区间不可能为空：`split_tail` 只在两半截
                    // 都不低于 `min_seg` 时才收紧。
                    Shot::Tighten { worker: w, iv } => {
                        // **E14**① 乙：这一句就是"Retighten 兑现不了"那笔账的还债处。
                        // 只往下调（`fetch::lower_end_cap`），所以一个来路不明的更大值
                        // 不会把已经收紧的枪重新放宽。
                        fetch::lower_end_cap(&self.caps[w], iv.end);
                        self.current[w] = Some(iv);
                        self.record(TraceStep::Tightened { worker: w, iv });
                    }
                    Shot::Nothing => {}
                }
            }
        }
        None
    }

    /// A4/R21：把在途 worker 的块级光标喂给调度器。只喂新值 —— `Progress` 只前进，
    /// 同一个值再喂一次没有信息，还会把 trace 撑大。
    fn sync_in_flight_progress(&mut self) {
        for w in 0..self.progress.len() {
            if self.current[w].is_none() {
                continue;
            }
            let abs_end = self.progress[w].load(Ordering::Acquire);
            if abs_end == fetch::NO_PROGRESS || abs_end == self.synced_progress[w] {
                continue;
            }
            self.synced_progress[w] = abs_end;
            self.record(TraceStep::Fed(Event::Progress { worker: w, abs_end }));
            self.sched.on(Event::Progress { worker: w, abs_end });
        }
    }

    /// 把一枪交给 worker。发不出去 = 那个线程已经不在了。
    ///
    /// **顺序是判据的一部分**：先把终点帽重置成"外面没有帽"，再 `send`。
    /// 反过来的话 worker 可能在重置之前就把上一枪的旧帽读进去 —— 那一枪会被一个不属于它的
    /// 终点提前掐掉（少写不致错，但那是"上一段的账管到这一段"，正是 E7 禁的第二本账的形状）。
    /// `mpsc::send` 与 worker 侧的 `recv` 之间的同步把这次重置带到那一枪眼前（原子格子自己
    /// 用 `Release`/`Acquire` 这一对，不靠"大概看得见"）。
    fn dispatch(&mut self, worker: usize, iv: Interval) -> Option<Stop> {
        let rate = running_rate_bytes_per_sec(self.run_delivered, self.started.elapsed());
        let mut cap = fetch::NO_END_CAP;
        if self.cfg.max_bytes > 0 {
            if self.budget_exhausted {
                return None;
            }
            let available = self.budget_available();
            if available == 0 {
                self.budget_exhausted = true;
                return None;
            }
            let free_slots = self
                .workers
                .iter()
                .filter(|ctl| ctl.state == WorkerState::Idle)
                .count()
                .max(1) as u64;
            // 至少给 1 字节；再夹到这一枪自己的区间长度 —— 预占超过它实际能交付的量会让
            // 其它 worker 在这段时间里白等一份用不掉的额度。
            let share = (available / free_slots).max(1).min(iv.len());
            self.budget_share[worker] = share;
            cap = iv.start.saturating_add(share.saturating_sub(1)).min(iv.end);
        }
        self.caps[worker].store(cap, Ordering::Release);
        self.progress[worker].store(fetch::NO_PROGRESS, Ordering::Release);
        self.synced_progress[worker] = fetch::NO_PROGRESS;
        if self.jobs[worker].send(Job::Fetch { iv, rate_bps: rate }).is_err() {
            return Some(Stop::WorkerGone { worker: Some(worker) });
        }
        self.current[worker] = Some(iv);
        self.workers[worker].state = WorkerState::Busy;
        self.record(TraceStep::Dispatched { worker, iv });
        None
    }

    fn any_in_flight(&self) -> bool {
        self.workers.iter().any(|ctl| matches!(ctl.state, WorkerState::Busy | WorkerState::Backoff { .. }))
    }

    /// A16：本次运行已经用掉的配额（预读基数 + 已交付 + 在途预占）。
    fn budget_spent(&self) -> u64 {
        self.budget_share
            .iter()
            .fold(self.run_delivered.saturating_add(self.budget_base), |acc, share| {
                acc.saturating_add(*share)
            })
    }

    fn budget_available(&self) -> u64 {
        self.cfg.max_bytes.saturating_sub(self.budget_spent())
    }

    /// 标着"在途"但心跳已消失的 worker：那一枪永远不会交回结论。
    fn dead_busy_worker(&self) -> Option<usize> {
        (0..self.workers.len()).find(|w| {
            self.workers[*w].state == WorkerState::Busy && !self.alive[*w].load(Ordering::Acquire)
        })
    }

    fn wait_duration(&self) -> Duration {
        let now = Instant::now();
        let mut wake = self.last_tick + LOOP_POLL;
        for ctl in &self.workers {
            if let WorkerState::Backoff { until } = ctl.state {
                wake = wake.min(until);
            }
        }
        // 下限 1 ms：`recv_timeout(ZERO)` 在个别实现上等于"立刻回 Timeout"，那会变成忙等。
        wake.saturating_duration_since(now).max(Duration::from_millis(1))
    }

    fn save_if_due(&mut self) -> Option<Stop> {
        let pending = self.sched.pending_bytes();
        // `pending` 变小 ⇔ 有字节被确认进覆盖集 ⇔ 关键事件发生过（见 `pending_bytes()` 的定义）。
        let moved = pending < self.saved_pending;
        // `&& let` 的短路顺序是判据的一部分：`save_state()` 只在确实该保存时才被调到。
        if moved
            && self.last_save.elapsed() >= Duration::from_secs(STATE_SAVE_MAX_INTERVAL_SECS)
            && let Err(e) = self.save_state()
        {
            return Some(Stop::StateFailed(format!("{e:#}")));
        }
        None
    }

    /// 交出一拍快照（勘误 **E21** 甲）。**本模块与 [`Engine::tick_if_due`] 是 `on_tick` 仅有的
    /// 两处调用点**，而它们分别就是判据⑦说的那两处：主循环那一拍、收尾那一拍。
    ///
    /// 三条次序都是判据，不是风格：
    /// 1. **先读账、后挪锚点、最后才调闭包**。锚点如果在取快照之前就挪了，下一拍的分子分母
    ///    会少算同一截（"两拍之差"变成"两拍之差减一次回调时间"）；而闭包看到的快照与
    ///    下一拍的锚点因此**不可能来自同一个时刻** —— 这正是判据③那句"同一拍取"的可执行版本。
    /// 2. **观察者不在场时一个字都不动**（早退）。不是为了省几纳秒：那种"没挂观察者也要把锚点
    ///    往前挪"的写法会让 `tick_if_due` 的闸每拍都重新成立，等于在默认路径上多跑一条循环。
    /// 3. 八格里没有任何东西来自 `store.len()`（E7 的两本账）：进度行的百分比只看覆盖集，
    ///    完成判定仍然只在 `completion_verdict` 那一处。
    fn emit_tick(&mut self) {
        if self.cfg.on_tick.is_none() {
            return;
        }
        let now = Instant::now();
        let gap = now.saturating_duration_since(self.tick_at);
        let snap = RunSnapshot {
            total_len: self.total_len,
            // 判据①：覆盖集这一格只做一次减法，不给调度器开新 API。
            covered_bytes: self.total_len.saturating_sub(self.sched.pending_bytes()),
            run_delivered: self.run_delivered,
            // 判据③：与上一格同一拍取，减掉断点带回来的起点。
            run_bytes_duplicated: self.stats.bytes_duplicated.saturating_sub(self.dup_base),
            // 判据④：两拍之差，不是全程平均。
            instant_bytes_per_sec: instant_rate_bytes_per_sec(
                self.run_delivered.saturating_sub(self.tick_delivered),
                gap,
            ),
            active_workers: busy_worker_count(&self.workers),
            workers_total: self.cfg.workers,
            slow_retry_rounds: slow_retry_rounds(&self.workers),
        };
        self.tick_at = now;
        self.tick_delivered = self.run_delivered;
        if let Some(on_tick) = &self.cfg.on_tick {
            on_tick(&snap);
        }
    }

    /// 主循环那一拍的出口：**距上一发快照不小于 `LOOP_POLL` 才交**（判据⑦的"每拍至多一次"）。
    ///
    /// 为什么这一句必须留在循环里、而不是把 `emit_tick` 直接写进 `on_report`：主循环醒来的次数
    /// 由"有没有结论回来"决定，一块 64 KiB 就是一次 ⇒ 挂在结论上就是每块一条进度行（与 **E8①**
    /// 那条"flush 绝不进每块循环"同族）。为什么也不为它加第二个计时器：`recv_timeout` 的期限
    /// 本来就是 `last_tick + LOOP_POLL`，循环最迟一秒醒一次，闸自己会到期。
    fn tick_if_due(&mut self) {
        if self.cfg.on_tick.is_some() && self.tick_at.elapsed() >= LOOP_POLL {
            self.emit_tick();
        }
    }

    /// 主循环。
    fn drive(&mut self) -> Stop {
        let stop = self.pump();
        // 判据⑦的第二处、也是最后一处：**收尾那一拍再交一次终值**。
        // 放在这里而不是 `finish` 里：`finish` 有两支直接 `bail`（落盘/断点坏了），那种时候
        // 观察者照样该看到"停在哪一格"；而此刻所有结论都已经记完账，这一发就是本轮的终值。
        self.emit_tick();
        stop
    }

    /// 循环本体（`drive` 是它的包装，只为在出口处补那一发终值快照）。
    fn pump(&mut self) -> Stop {
        loop {
            if let Some(worker) = self.dead_busy_worker() {
                return Stop::WorkerGone { worker: Some(worker) };
            }
            self.expire_backoffs();
            if let Some(stop) = self.ping_free_workers() {
                return stop;
            }
            // 没人在途、也没人在退避 ⇒ 调度器给不出活（`dispatch` 在有活时绝不回 `Idle`）⇒ 收工。
            // A16：预算到量而任务还没下完 ⇒ 这一次运行是"停住"不是"失败"；下完则 `Done` 优先。
            if !self.any_in_flight() {
                if self.budget_exhausted && !self.sched.is_done() {
                    return Stop::BudgetReached;
                }
                return Stop::Done;
            }
            if let Some(stop) = self.save_if_due() {
                return stop;
            }
            let waited = self.wait_duration();
            match self.reports.recv_timeout(waited) {
                Ok(rep) => {
                    if let Some(stop) = self.on_report(rep) {
                        return stop;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // 所有 sender 都没了 = 所有 worker 线程都退了（正常收工或被杀干净）。
                Err(RecvTimeoutError::Disconnected) => return Stop::WorkerGone { worker: None },
            }
            self.last_tick = Instant::now();
            self.tick_if_due();
        }
    }

    /// 一条 worker 结论的全部后续：记账、喂事件、安排退避。交回 `Some(stop)` 表示主循环该停了。
    fn on_report(&mut self, rep: WorkerReport) -> Option<Stop> {
        let WorkerReport { worker, iv, outcome } = rep;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            // `run_window` 只在 `store.write_at` 失败时上抛：那一块真落了几字节**无从知道**，
            // 而盘用不了的时候重试也换不来一块盘 ⇒ 停整个任务，交 `Err`（`finish` 那一支）。
            Err(e) => return Some(Stop::StoreFailed(format!("{e:#}"))),
        };
        // 调度器认给这个 worker 的区间；正常就等于它刚打的那一枪，只有被收紧过才不同。
        let owned = self.current[worker].unwrap_or(iv);
        let (events, landed, duplicated) = translate(&outcome, iv, owned, worker);
        self.stats.bytes_delivered = self.stats.bytes_delivered.saturating_add(landed);
        self.stats.bytes_duplicated = self.stats.bytes_duplicated.saturating_add(duplicated);
        self.run_delivered = self.run_delivered.saturating_add(landed);
        self.budget_share[worker] = 0;
        if self.cfg.max_bytes > 0 && self.budget_spent() >= self.cfg.max_bytes {
            self.budget_exhausted = true;
        }
        for event in events {
            self.record(TraceStep::Fed(event));
            self.sched.on(event);
        }

        match &outcome {
            Outcome::Advanced { .. } | Outcome::Complete => {
                // 成功一次就把这条链的退避计数归零（spec §6.1 的退避挂在**连续**失败上）。
                self.workers[worker].attempt = 0;
                self.workers[worker].state = WorkerState::Idle;
                self.current[worker] = None;
                None
            }
            Outcome::Stalled | Outcome::Failed(_) => {
                // 原因要活着走到用户眼前（E12⑦）：`Stalled` 没有载荷，那句只能由本模块写。
                self.last_note = Some(match &outcome {
                    Outcome::Failed(k) => failure_note(k),
                    _ => "窗口预算到点，这一枪一个字节都没落盘（Stalled）".to_string(),
                });
                let attempt = self.workers[worker].attempt;
                match next_retry_delay(attempt, self.cfg.max_retries) {
                    None => Some(Stop::RetriesExhausted { worker, attempted: attempt }),
                    Some(delay) => {
                        self.workers[worker].attempt = attempt.saturating_add(1);
                        self.stats.retries = self.stats.retries.saturating_add(1);
                        self.workers[worker].state = WorkerState::Backoff { until: Instant::now() + delay };
                        self.current[worker] = None;
                        None
                    }
                }
            }
        }
    }

    /// 收工：先 `Stop` 再 join（**E8②**：`flush` 期间持有全局写锁，绝不许和活着的 writer 并发）。
    /// 交回崩了的 worker 序号 —— 它们**永远**不许变成 `ok = true`。
    fn stop_and_join(&mut self) -> Vec<usize> {
        for tx in &self.jobs {
            let _ = tx.send(Job::Stop); // 线程已退就发不出去，那正是要在 join 里数出来的一件事
        }
        let mut dead = vec![];
        for (index, handle) in std::mem::take(&mut self.handles).into_iter().enumerate() {
            if handle.join().is_err() {
                dead.push(index);
            }
        }
        dead
    }

    /// 收尾：flush 一次 → 落最终断点 → 判两本账 → 校验 → 按 `--keep-state` 删不删。
    fn finish(&mut self, stop: Stop) -> Result<Report> {
        let dead_workers = self.stop_and_join();
        let target = self.store.final_path().to_path_buf();
        let budget_reached = matches!(&stop, Stop::BudgetReached);

        // `Stop` 里有两种是"盘用不了"，直接 `Err`；但**先**把 flush/save 试完，让残留的账尽量落盘
        // （下一轮才可能续），它们的失败只在还没有更原因素时才算主因。
        let mut fatal: Option<String> = match &stop {
            Stop::StoreFailed(msg) => Some(format!("落盘失败，整任务停止：{msg}")),
            Stop::StateFailed(msg) => Some(format!("断点文件写不进去，整任务停止：{msg}")),
            _ => None,
        };

        // **E8① 的判据是"收尾调一次、绝不进每块循环"**，不是"全二进制只有一个调用点"：
        // 另一处在 `run_whole_once`（`data_file` 那条路线，与本函数互斥）。这里此刻 writer 已全部 join（E8②）。
        if let Err(e) = self.store.flush() {
            fatal = fatal.or(Some(format!("收尾同步目标文件失败：{e:#}")));
        }

        let mut on_disk = 0u64;
        match self.store.len() {
            Ok(len) => on_disk = len,
            Err(e) => fatal = fatal.or(Some(format!("收尾读目标文件长度失败：{e:#}"))),
        }

        // 最终断点：成败都落（失败那一支正是"下次要接着下"的那份依据），成功后再决定删不删。
        if let Err(e) = self.save_state() {
            fatal = fatal.or(Some(format!("最终断点写不进去：{e:#}")));
        }

        if let Some(msg) = fatal {
            bail!(msg);
        }

        let sched_done = self.sched.is_done();
        let mut ok = matches!(&stop, Stop::Done) && completion_verdict(sched_done, on_disk, self.total_len);
        let mut message: Option<String> = None;

        // **崩过的 worker 不许交回成功**：`join()` 报错的两种后果都在这里合流
        // （心跳那条路已经先把它变成 `WorkerGone`，这一条兜住"跑完了才发现崩过"的形状）。
        if !dead_workers.is_empty() {
            ok = false;
            message = Some(format!("{} 个 worker 线程崩了（序号 {dead_workers:?}）：它手上那一枪没有结论", dead_workers.len()));
        }

        if !ok && message.is_none() {
            message = Some(match stop {
                Stop::RetriesExhausted { worker, attempted } => format!(
                    "worker {worker} 的重试到顶（已试 {attempted} 次，--max-retries={:?}）：{}",
                    self.cfg.max_retries,
                    self.last_note.clone().unwrap_or_else(|| "最后一次失败没有留下原因串".to_string())
                ),
                Stop::WorkerGone { worker: Some(worker) } => format!("worker {worker} 的线程不在了：它那一枪不会有结论"),
                Stop::WorkerGone { worker: None } => "所有 worker 线程都不在了：回报通道已断开".to_string(),
                Stop::BudgetReached => format!(
                    "已达到本次运行字节预算（--max-bytes={}）：断点已保留，再次运行会继续",
                    self.cfg.max_bytes
                ),
                Stop::Done if !sched_done => format!(
                    "没有活可派了，调度器却说还剩 {} 字节没归属（内部矛盾，不敢当成完成）",
                    self.sched.pending_bytes()
                ),
                Stop::Done => format!("归属已定但盘上只有 {on_disk}/{} 字节（E7：覆盖集不算已写字节数）", self.total_len),
                Stop::StoreFailed(_) | Stop::StateFailed(_) => unreachable!("上面已经用 fatal 提前 bail"),
            });
        }

        if ok && let Some(want) = &self.cfg.verify_sha256 {
            match sha256_of_path(&target) {
                Err(e) => {
                    ok = false;
                    message = Some(format!("{e:#}"));
                }
                Ok(got) if !hash_matches(&got, want) => {
                    ok = false;
                    message = Some(sha256_mismatch_message(&got, want));
                }
                Ok(_) => {}
            }
        }

        // 删断点：只有"确实下完了"才删；`--keep-state` 留着（spec §4）。校验不符那一支**必须保留**，
        // 让用户自己决定（spec §6.7）—— 上面的 `ok = false` 已经让这一句不会执行。
        // 删不掉不改写 `ok`（下载本身已经完成，"跑完了但没跑全"说的不是这件事），只留一句话。
        if ok
            && !self.keep_state()
            && let Err(e) = fs::remove_file(&self.state_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            message = Some(format!("下载已完成，但断点文件删不掉（{}）：{e}", self.state_path.display()));
        }

        let confirmed = coverage_bytes(&self.sched.snapshot_intervals());
        Ok(Report {
            bytes_total: self.total_len,
            delivered: on_disk,
            confirmed,
            duplicated: self.stats.bytes_duplicated,
            retries: self.stats.retries,
            elapsed: self.started.elapsed(),
            ok,
            budget_reached,
            trace: std::mem::take(&mut self.trace),
            fresh_reason: self.fresh_reason.clone(),
            message,
        })
    }
}

/// 覆盖集的字节数。**只用于上报 `confirmed`，绝不允许参与完成判定**（E7；判定在
/// `completion_verdict`，它的签名里没有这个量）。
fn coverage_bytes(intervals: &[Interval]) -> u64 {
    intervals.iter().map(Interval::len).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::WINDOW_MAX_TIMEOUT_SECS;
    use crate::state::Snapshot;
    // 9A 的夹具要的三样：进程内计数器、带时间戳的快照流水、一个"刚释放的空闲端口"。
    use std::net::{SocketAddr, TcpListener};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// **E13④ 要求的表驱动用例**：退避表逐格钉死，零睡眠。
    ///
    /// 三列各自的来源：`1/2/4/8/16` 是 spec §6.1 的原文；第 6 次起 `30 s` 是 §6.2（"超过 5 次不判
    /// 整任务失败，转入 30 s 无限慢重试"）；`--max-retries` 封顶整条链是 §6.2 末句。
    /// `attempt` 一律是"这条链上已经重试过几次"，所以第一行 `(0, None, 1s)` 说的是"第一次重试等 1 秒"。
    #[test]
    fn retry_clock_follows_the_spec_table() {
        let table: &[(u32, Option<u32>, Option<u64>)] = &[
            // —— 没给 --max-retries：快速五次，之后无限慢重试 ——
            (0, None, Some(1)),
            (1, None, Some(2)),
            (2, None, Some(4)),
            (3, None, Some(8)),
            (4, None, Some(16)),
            (5, None, Some(30)),   // §6.2：第 6 次起转 30 s，而不是判整任务失败
            (6, None, Some(30)),
            (9, None, Some(30)),
            (u32::MAX, None, Some(30)),   // 无限慢重试不许因为计数溢出而崩或停
            // —— 给了 --max-retries：封顶的是整条链（快速 + 慢速一起数）——
            (0, Some(0), None),    // 封顶 0 ⇒ 一次都不重试
            (0, Some(1), Some(1)),
            (1, Some(1), None),    // 已经重试满 1 次 ⇒ 第 2 次不给
            (4, Some(5), Some(16)),
            (5, Some(5), None),    // 快速用满 ⇒ 慢速一档也不给留
            (5, Some(6), Some(30)),
            (6, Some(7), Some(30)),
            (7, Some(7), None),    // 慢速重试同样计入上限
            (8, Some(7), None),
        ];
        for (attempt, cap, want) in table {
            let got = next_retry_delay(*attempt, *cap);
            let want_dur = want.map(Duration::from_secs);
            assert_eq!(got, want_dur, "attempt={attempt} max_retries={cap:?}");
        }
    }

    /// 上限的**边界方向**单独钉：`Some(n)` 放行的必须正好是 `n` 档，`None` 必须一档都不封。
    /// 上面那张表读得出趋势，但"差一档"这种缺陷要一格具体的格子才拦得住
    /// （`attempt >= cap` 写成 `attempt > cap` 时，本用例的第一条立刻红，而表格其余 17 格全绿）。
    #[test]
    fn the_cap_admits_exactly_that_many_retries_and_no_more() {
        for cap in 0u32..12 {
            let allowed: Vec<Option<Duration>> =
                (0..cap + 3).map(|a| next_retry_delay(a, Some(cap))).collect();
            let given = allowed.iter().filter(|d| d.is_some()).count();
            assert_eq!(given, cap as usize, "封顶 {cap} 次却放行了 {given} 档: {allowed:?}");
            assert!(allowed[cap as usize].is_none(), "第 {} 档该收手（封顶 {cap}）", cap + 1);
        }
        // 没封顶：再往后也永远有下一档（§6.2 的"无限"）。
        for attempt in [5u32, 500, 100_000, u32::MAX] {
            assert_eq!(next_retry_delay(attempt, None), Some(Duration::from_secs(30)), "attempt={attempt}");
        }
    }

    /// 退避的**量级**必须落在可用范围内，且每一档都不许短到"等于没退避"：
    /// 1 秒是最短的一档，而窗口预算的下界是 15 秒 —— 重试比一次请求还短的话，
    /// 主循环里"先等到 deadline 再收事件"的那一步就白等了（这条是给引擎的等待逻辑定标的）。
    #[test]
    fn every_permitted_delay_is_a_real_pause_below_the_longest_window_budget() {
        for cap in [None, Some(1), Some(3), Some(FAST_RETRY_MAX), Some(64)] {
            for attempt in 0u32..64 {
                if let Some(d) = next_retry_delay(attempt, cap) {
                    assert!(d.as_secs() >= 1, "退避不许是零秒（那等于立刻重打同一枪）: {d:?}");
                    assert!(
                        d.as_secs() <= WINDOW_MAX_TIMEOUT_SECS,
                        "退避不该长过最宽的一次窗口预算，否则主循环的等待没有上界: {d:?}"
                    );
                }
            }
        }
    }

    /// **E7**：完成判定要**两个**条件，而覆盖集的字节数没有入口进来。
    ///
    /// 最要紧的是最后两格：`is_done()` 只看"归属"，416 封存（`SegmentComplete` 不看盘）之后
    /// 覆盖集可以说"全长都归我了"而盘上还差一截 —— 拿那个数去过 spec §6.7 的闸就是 E7 点名
    /// 禁止的事。这里用一个 8 字节的形状把两本账分开摆：`confirmed == total` 而 `on_disk < total`。
    #[test]
    fn completion_verdict_needs_both_books_and_never_the_covering_set() {
        let table: &[(&str, bool, u64, u64, bool)] = &[
            ("两本账都对得上", true, 8, 8, true),
            ("调度器没说完，盘上够了也不成（还有区间在途）", false, 8, 8, false),
            ("调度器说完了，盘上少一个字节（416 封存过没落盘的段）", true, 7, 8, false),
            ("盘上比全长多（上一轮写歪的尾巴）也不能算完", true, 9, 8, false),
            ("空文件：两个 0 就是完成", true, 0, 0, true),
            ("什么都没干", false, 0, 8, false),
        ];
        for (case, sched_done, on_disk, total, want) in table {
            assert_eq!(
                completion_verdict(*sched_done, *on_disk, *total),
                *want,
                "用例「{case}」：is_done={sched_done} on_disk={on_disk} total={total}"
            );
        }

        // 覆盖集那个量**不参与**判定，所以它相等而落盘量不等时必须还是假。
        let sealed = [Interval { start: 0, end: 7 }];
        assert_eq!(coverage_bytes(&sealed), 8, "覆盖集自己说的是 8 字节");
        assert!(!completion_verdict(true, 7, 8), "覆盖集 == 全长而盘上只有 7 ⇒ 不许过闸（E7）");
    }

    /// `Advanced{bytes}` → `Progress{abs_end}` 的换算必须是**含端点**的，而且不许越过区间。
    ///
    /// 这条钉的是派发里那句 `iv.start + bytes`（`fetch::Outcome` 的文档同样这么写）。
    /// 差一字节的具体后果见模块文档那一节：一个从没写过的字节被报成已确认，两本账都对得上、
    /// 文件却是坏的。三格各有分工：
    /// - `bytes = 1`（窗口只落了一个字节）⇒ `abs_end == start`，写成 `start + 1` 就跳过了那个字节；
    /// - `bytes = iv.len()`（窗口正好填满）⇒ `abs_end == end`，写成 `end + 1` 就把下一个区间的第一
    ///   个字节报成已确认，调度器随后把它从待下载里摘掉（`note_progress` 的 `min(a.iv.end)` 会夹住
    ///   自己那一段，但**尾巴交回之后**那条字节就永远没人要了）；
    /// - `bytes = 0` ⇒ `None`（没什么可报的）。
    #[test]
    fn the_advanced_mapping_is_inclusive_and_bounded_to_the_interval() {
        let iv = Interval { start: 100, end: 199 }; // 100 个字节
        let table: &[(u64, Option<u64>)] = &[
            (0, None),                    // 一个字节都没落地
            (1, Some(100)),               // 只有第 100 个字节
            (50, Some(149)),              // 写到 149（含）
            (100, Some(199)),             // 整个区间：正好等于 iv.end，绝不是 end+1
        ];
        for (bytes, want) in table {
            assert_eq!(progress_abs_end(iv.start, *bytes), *want, "bytes={bytes}");
        }
        assert_eq!(progress_abs_end(iv.start, 100), Some(iv.end), "填满区间时报的必须是含端点的终点");
        assert_ne!(progress_abs_end(iv.start, 100), Some(iv.end + 1), "end+1 会把下一段的第一字节吞掉");

        // `Complete` 不带字节，但它承诺的就是全长 ⇒ 同一个含端点终点（`fetch::run_window` 的 `>` 判据）。
        let (events, landed, _) = translate(&Outcome::Complete, iv, iv, 3);
        assert_eq!(landed, 100, "Complete 落的字节就是区间全长");
        assert_eq!(
            events,
            vec![Event::Progress { worker: 3, abs_end: 199 }, Event::SegmentComplete { worker: 3 }],
            "Complete 折成 SegmentComplete（**不是** WindowDone，E7）"
        );
    }

    /// 事件翻译表整体钉一遍（`translate` 文档那张表的每一条）。
    #[test]
    fn one_fetch_outcome_maps_to_exactly_the_documented_events() {
        let iv = Interval { start: 0, end: 999 };
        let cases: Vec<(Outcome, Vec<Event>, u64)> = vec![
            (Outcome::Advanced { bytes: 400 }, vec![Event::Progress { worker: 1, abs_end: 399 }, Event::WindowDone { worker: 1 }], 400),
            (Outcome::Complete, vec![Event::Progress { worker: 1, abs_end: 999 }, Event::SegmentComplete { worker: 1 }], 1000),
            (Outcome::Stalled, vec![Event::Failed { worker: 1 }], 0),
            (Outcome::Failed(FailKind::Http(503)), vec![Event::Failed { worker: 1 }], 0),
            (Outcome::Failed(FailKind::Transport("对端拆了线".into())), vec![Event::Failed { worker: 1 }], 0),
            (Outcome::Failed(FailKind::ValidatorChanged), vec![Event::Failed { worker: 1 }], 0),
        ];
        for (outcome, want_events, want_landed) in cases {
            let (events, landed, duplicated) = translate(&outcome, iv, iv, 1);
            assert_eq!(events, want_events, "{outcome:?}");
            assert_eq!(landed, want_landed, "{outcome:?} 的落盘量");
            assert_eq!(duplicated, 0, "没被收紧过的区间不产生重复账：{outcome:?}");
        }

        // 空区间那一支（`run_window` 看见 `iv.is_empty()` 直接 `Complete`，一枪都不发）：
        // 没有可报的偏移，只交回封存事件本身。
        let empty = Interval { start: 5, end: 4 };
        assert_eq!(translate(&Outcome::Complete, empty, empty, 0).0, vec![Event::SegmentComplete { worker: 0 }]);
    }

    /// **重复字节**只有一种来源：字节落在了调度器认给这个 worker 的区间之外。
    /// （阶段5A 之前那节模块文档给的例子是"收紧后在途那一枪读完整个窗口"；**E14**① 的终点帽
    /// 把这条砍到"最多越过帽一个 64 KiB 分块"，剩下的两条是帽追不回的位置——收紧落在这一枪
    /// 结束之后、或新终点落在这枪早已读过的位置之下。`translate` 算的是**口径**，与帽怎么收紧无关，
    /// 所以这里的夹具仍然直接给 `(这一枪的区间, 落盘字节, 此刻认给的区间)` 三元组。）
    #[test]
    fn duplicate_bytes_count_only_what_fell_outside_the_owned_interval() {
        let shot = Interval { start: 0, end: 999 }; // 被派的是 1000 个字节
        let tightened = Interval { start: 0, end: 399 }; // 中途被收到 400 个字节
        // 期望列全是**写死的字面量**：原先这一格写的是 `s.start + n - 1` —— 拿被测公式现算期望值
        // (T8 评审 B 的"测试诚实性"之一)，`progress_abs_end` 差一字节的变异会把期望跟着一起挪，
        // 当场变成没断。现在两侧不共享任何算式。
        // 一行 = (名字, 这一枪的区间, 落盘字节, 调度器认给的区间, 期望落盘, 期望重复, 期望 Progress 的含端点绝对偏移)。
        // 抽成别名只为读得出来，不是为了省字符。
        type Row = (&'static str, Interval, u64, Interval, u64, u64, Option<u64>);
        let table: &[Row] = &[
            ("没收紧过：全是真进度", shot, 400, shot, 400, 0, Some(399)),
            ("收紧后读完窗口：越界那截算重复", shot, 1000, tightened, 1000, 600, Some(999)),
            ("收紧后正好收到新终点", shot, 400, tightened, 400, 0, Some(399)),
            ("收紧后还少于一新终点", shot, 250, tightened, 250, 0, Some(249)),
            ("一个字节都没落地", shot, 0, tightened, 0, 0, None),
            ("起点就在新区间之后（整枪都白读）", shot, 100, Interval { start: 500, end: 999 }, 100, 100, Some(99)),
        ];
        for (case, s, bytes, owned, want_landed, want_duplicated, want_abs_end) in table {
            let (events, landed, duplicated) = translate(&Outcome::Advanced { bytes: *bytes }, *s, *owned, 0);
            assert_eq!(landed, *want_landed, "{case}");
            assert_eq!(duplicated, *want_duplicated, "{case}");
            // 事件里报的是**真实**的含端点偏移，越界那一段由调度器自己夹住（`note_progress`）。
            // 零进度那一支没有偏移可报，只剩收尾那一条。
            match want_abs_end {
                None => assert_eq!(events, vec![Event::WindowDone { worker: 0 }], "{case}"),
                Some(abs_end) => assert_eq!(events[0], Event::Progress { worker: 0, abs_end: *abs_end }, "{case}"),
            }
        }
    }

    /// **E14③（乙-label）的口径钉死**：这本账 = "落在归属范围之外、**可能**被别人重下的字节"
    /// （E14③ 批准的就是这个强度，不是"必然重下"），
    /// 来源**只有这一处**（`translate` 算的那笔；**E17** 把 `run` 里那笔 kill+续传的账整笔撤掉了 ——
    /// "盘上有、覆盖集里没有"绝大多数是从没下载过的洞，拿它倒推就是 E14③ 禁止的第二本账），
    /// 而且**不是**流量账单。撤掉那笔之后"上一轮在途窗口已落盘、本轮会重下"的字节**不在任何账里**，
    /// 这条已知的少报写在 `state::Stats::bytes_duplicated` 的字段文档上。
    ///
    /// **五个断言块**（修复轮 8F-3a 数过：原先这里写"三条格子"，漏了 ④ 与 `Complete` 那一支）
    /// 各自挡一种写法：
    /// - ① 读满自己那个窗口 ⇒ **零**。把 `duplicated` 写成"这一枪读了多少"（阶段4 那本就是这么读的：
    ///   同一形状报 8,334,284 个字节，而线上多打 8,465,356 个字节，差 1.6%，看着几乎就是账单）在这一格当场红 ——
    ///   那一枪读的字节本来就归它，一个字节都没被重下。
    /// - ② 帽之后多落的那一块才是账：`fetch::cap_allows` 在块首问 ⇒ 越帽的一块整块落，
    ///   这本账看到的就只有那一块。
    /// - ③ 起点落在归属之外（陈旧帽/别的形状）⇒ 整枪都算，不许只算"越出终点的右边"。
    /// - ④ 这本账不许为负（`landed` 比归属短 ⇒ 0，不是 `usize` 下溢）。
    /// - 外加一块**没编号**的：同一格在 `Complete` 那一支也要成立（归属没缩 ⇒ 零，`Complete`
    ///   不许凭空造出重复账）。
    #[test]
    fn the_duplicate_book_counts_only_what_fell_outside_the_owned_range() {
        let window = crate::consts::WINDOW_BYTES;
        let shot = Interval { start: 0, end: window - 1 };
        let (_, landed, duplicated) = translate(&Outcome::Advanced { bytes: window }, shot, shot, 0);
        assert_eq!(landed, window, "整窗都是落盘量");
        assert_eq!(duplicated, 0, "①读满自己那个窗口不是重复：帽/归属没动，一个字节都不会被重下");

        let owned = Interval { start: 0, end: 100_000 };
        let crossed = 100_001 + 65_536;
        let (_, landed, duplicated) = translate(&Outcome::Advanced { bytes: crossed }, shot, owned, 0);
        assert_eq!(landed, crossed);
        assert_eq!(duplicated, 65_536, "②越帽的那一块整块落 ⇒ 这本账的全部就是那一块（64 KiB）");

        let all_mine = Interval { start: 500, end: 999 };
        let (_, _, duplicated) = translate(
            &Outcome::Advanced { bytes: 100 },
            Interval { start: 0, end: 99 },
            all_mine,
            0,
        );
        assert_eq!(duplicated, 100, "③整枪都落在归属之外 ⇒ 全是账（不许只算越过右终点的那截）");

        // 同一格在 `Complete` 那一支也要成立：`Complete` 交回的是区间全长，归属没缩就是零。
        let (_, landed, duplicated) = translate(&Outcome::Complete, shot, shot, 0);
        assert_eq!((landed, duplicated), (window, 0), "Complete 不该凭空造出重复账");

        // ④ 这本账不许为负：`landed` 比归属短的时候是 0，不是 `usize` 下溢（debug 直接 panic）。
        let short = translate(&Outcome::Advanced { bytes: 10 }, shot, owned, 0);
        assert_eq!(short.2, 0, "落得比归属还短 ⇒ 没有重复");
    }

    /// `Retighten` **不是**一枪：它只把在途那一枪的目标提前。
    ///
    /// 这条是模块文档"`Retighten` 不是一枪"那节的判据，也是 `TraceStep` 分两支的理由。
    /// 写成 `Fire` 的后果具体可测：worker 阻塞在 `read()` 里，这条 `Job` 排在当前那枪之后，
    /// 起点仍是 `iv.start` ⇒ 已确认落盘的前缀被重下一遍（`a_failed_segments_written_prefix_survives_and_is_not_downloaded_twice`
    /// 那条集成用例的镜像面）。
    #[test]
    fn a_retighten_moves_the_target_end_but_never_fires_a_shot() {
        let iv = Interval { start: 100, end: 299 };
        assert!(matches!(
            action_for_worker(Action::Assign { worker: 3, iv }),
            Shot::Fire { worker: 3, iv: got } if got == iv
        ));
        assert!(matches!(
            action_for_worker(Action::Retighten { worker: 3, iv }),
            Shot::Tighten { worker: 3, iv: got } if got == iv
        ));
        assert!(matches!(action_for_worker(Action::Idle), Shot::Nothing));
        // 顺序也要说清：调度器交回 `[Retighten(incumbent), Assign(new)]`，
        // 前者记 `Tightened`、后者记 `Dispatched` ⇒ `dispatched()` 与线上 Range 一比一。
        let mut sched = Scheduler::new(400, 1);
        let first = sched.on(Event::Connected(0));
        let second = sched.on(Event::Connected(1));
        assert_eq!(first.len(), 1, "第一枪只有 Assign");
        assert!(matches!(first[0], Action::Assign { .. }));
        assert_eq!(
            second.iter().map(|a| matches!(a, Action::Retighten { .. })).collect::<Vec<_>>(),
            vec![true, false],
            "第二枪该是先 Retighten 再 Assign，且只有后者是一枪：{second:?}"
        );
        // 两条动作指向的**不是同一个** worker：`Retighten` 给在途的那个（0），`Assign` 给来领活的这个（1）。
        // 引擎按动作里的序号记账，不按"当前正在 ping 的那个"，否则收紧会记到别人头上。
        assert!(matches!(second[0], Action::Retighten { worker: 0, .. }));
        assert!(matches!(second[1], Action::Assign { worker: 1, .. }));
    }

    /// `Fresh` / `Resume` / `Abort` 三条路各走到哪里（**E13②**：引擎自己走 load→probe→decide）。
    #[test]
    fn fresh_resume_and_abort_route_to_their_own_plan() {
        let validator = Validator {
            etag: Some("\"e\"".into()),
            last_modified: None,
            total_len: 100,
            final_url: "http://x/y".into(),
            ignore_mtime: false,
        };
        let expected_validator = validator.clone();
        // Abort：`Err`，而且**不许**带任何执行意图（调用方在这里连 `Store::open` 都还没走到）。
        let Err(e) = plan_from_decision(Decision::Abort("断点要口令".into())) else {
            panic!("Abort 必须变成 Err");
        };
        assert_eq!(e.to_string(), "断点要口令", "原因要原样走到用户眼前");

        // Fresh（没有断点）：truncate、没有种子、没有原因、账从零。
        let plan = plan_from_decision(Decision::Fresh { reason: None }).expect("Fresh 是合法结论");
        assert!(plan.truncate, "Fresh 必须覆盖残留的半截文件（spec §4 的 --no-resume 同一支）");
        assert!(plan.seed.is_empty(), "Fresh 没有可播种的段表");
        assert!(plan.fresh_reason.is_none(), "本来就没有断点 ⇒ 没有需要解释的事");
        assert_eq!(plan.stats, Stats::default());
        assert!(plan.resume_validator.is_none(), "Fresh 不该带续传验证器");

        // Fresh（冲突重来）：原因串必须活着走到 Report（`src/resume.rs` 对 Task 9 的承诺）。
        let plan = plan_from_decision(Decision::Fresh { reason: Some("ETag 变了".into()) }).expect("Fresh 是合法结论");
        assert_eq!(plan.fresh_reason.as_deref(), Some("ETag 变了"));

        // Resume：不动文件、段表进种子、账从上次的 stats 接着数。
        let snap = Snapshot::stub_for_test(
            validator,
            vec![SegView { start: 0, end: 39 }, SegView { start: 60, end: 99 }],
        );
        let mut snap = snap;
        snap.stats = Stats { bytes_delivered: 70, bytes_duplicated: 6, retries: 2 };
        let plan = plan_from_decision(Decision::Resume(snap)).expect("Resume 是合法结论");
        assert!(!plan.truncate, "续传绝不许截断文件：那会把已确认的字节抹掉");
        assert_eq!(plan.seed, vec![Interval { start: 0, end: 39 }, Interval { start: 60, end: 99 }]);
        assert_eq!(plan.stats.retries, 2, "重试计数是跨轮的账，不能从 0 重数");
        assert_eq!(plan.stats.bytes_delivered, 70);
        assert!(plan.fresh_reason.is_none());
        assert_eq!(
            plan.resume_validator.as_ref(),
            Some(&expected_validator),
            "Resume 必须把断点验证器带进 Plan（R17 的 If-Range 值从它算）"
        );
    }

    #[test]
    fn if_range_is_absent_for_fresh_runs_and_degraded_peers() {
        let validator = Validator {
            etag: Some("\"e\"".into()),
            last_modified: None,
            total_len: 100,
            final_url: "http://x/y".into(),
            ignore_mtime: false,
        };
        assert!(if_range_for(None, true, false).is_none(), "Fresh 没有断点验证器，不许发");
        assert!(
            if_range_for(Some(&validator), false, false).is_none(),
            "不认 Range 的对端回 200 是正常退化，不是验证器变了"
        );
        assert_eq!(if_range_for(Some(&validator), true, false).as_deref(), Some("\"e\""));
        assert_eq!(
            if_range_for(Some(&validator), true, true).as_deref(),
            Some("\"e\""),
            "--ignore-mtime 不压 ETag"
        );
    }

    /// 播种之后的调度器确实**不重下**已确认区间（这条本来由阶段 1 的等价用例钉，这里补的是
    /// "引擎用的那一格"：断点里那两条区间进不去的位置才是真危险）。
    #[test]
    fn the_seed_only_leaves_the_gaps_for_the_scheduler_to_dispatch() {
        let seed = [Interval { start: 0, end: 39 }, Interval { start: 60, end: 99 }];
        let mut sched = Scheduler::resume(100, 16, &seed);
        assert_eq!(sched.pending_bytes(), 20, "只有 40..=59 还没归属");
        let actions = sched.on(Event::Connected(0));
        assert!(
            matches!(actions[0], Action::Assign { iv, .. } if iv == Interval { start: 40, end: 59 }),
            "派出去的必须是那条缝，不是整份文件：{actions:?}"
        );
    }

    /// 断点说的字节**盘上可能根本没有**（用户删了目标文件、或上一轮 save 之后被杀）。
    /// 种子必须夹到真实的 EOF，否则那批字节被算成已确认 ⇒ 每一轮都只能报 `ok = false`
    /// 而永远续不上（失败那一支按裁定留着断点文件）。
    #[test]
    fn the_seed_is_clamped_to_what_is_actually_on_disk() {
        let seed = [Interval { start: 0, end: 39 }, Interval { start: 60, end: 99 }];
        let table: &[(&str, u64, &[Interval])] = &[
            ("文件被删空：整张表都不作数", 0, &[]),
            ("只写到第一个洞之前", 20, &[Interval { start: 0, end: 19 }]),
            ("正好到第一行末尾", 40, &[Interval { start: 0, end: 39 }]),
            ("落在洞里：第二行一个字节都没有", 50, &[Interval { start: 0, end: 39 }]),
            ("越过第二行的终点：原样", 140, &seed),
            ("盘上正好缺最后一行", 80, &[Interval { start: 0, end: 39 }, Interval { start: 60, end: 79 }]),
        ];
        for (case, on_disk, want) in table {
            assert_eq!(clamp_seed_to_disk(&seed, *on_disk), *want, "{case}：on_disk={on_disk}");
        }
        // 夹完仍然是 `check_segments` 收得下的形状：只会更远，不会新增相邻。
        let clamped = clamp_seed_to_disk(&seed, 80);
        assert!(clamped.windows(2).all(|w| w[1].start > w[0].end + 1), "夹完还升序且不相邻：{clamped:?}");
        // 夹掉的字节回到待下载，而不是被记成"已经落盘"：调度器必须能把它们重新派出去。
        let sched = Scheduler::resume(100, 16, &clamped);
        assert_eq!(sched.pending_bytes(), 40, "已确认 0..=39 与 60..=79 共 60 字节，剩下 40 个还得下");
    }

    /// 平均速率：除零要挡住（第一轮没有任何测量），单位是**字节/秒**（`window_budget` 按字节除）。
    #[test]
    fn running_rate_guards_the_division_and_is_bytes_per_second() {
        let table: &[(u64, Duration, u64)] = &[
            (0, Duration::ZERO, 0),                                        // 同一瞬间：不许 panic
            (0, Duration::from_secs(3), 0),                                // 还没字节
            (1_000_000, Duration::from_secs(1), 1_000_000),                 // 1 MB/s
            (1_000_000, Duration::from_millis(500), 2_000_000),             // 半秒交完
            (1, Duration::from_micros(1), 1_000_000),                       // 一个字节一微秒
            (8 * 1024 * 1024, Duration::from_millis(1), 8_388_608_000),   // 8 MiB / 1 ms = 8 GiB/s
        ];
        for (delivered, elapsed, want) in table {
            assert_eq!(running_rate_bytes_per_sec(*delivered, *elapsed), *want, "delivered={delivered} elapsed={elapsed:?}");
        }
        // 速率 0 是"未知"，`window_budget` 据此给最宽预算 —— 这里钉的是两端接得上。
        assert_eq!(fetch::window_budget(0), Duration::from_secs(WINDOW_MAX_TIMEOUT_SECS));
        // u64 上界那一格不许绕回成小数（saturating）。
        assert!(running_rate_bytes_per_sec(u64::MAX, Duration::from_micros(1)) >= running_rate_bytes_per_sec(1, Duration::from_micros(1)));
    }

    /// `--sha256` 那道闸的比对：大小写与首尾空白不算不符，空期望**永不放行**。
    #[test]
    fn hash_matches_is_case_insensitive_but_never_passes_an_empty_expectation() {
        let table: &[(&str, &str, &str, bool)] = &[
            ("一样", "ab12", "ab12", true),
            ("只是大小写不同", "AB12", "ab12", true),
            ("期望串两侧有空白", "ab12", "  ab12\n", true),
            ("少一个字符", "ab12", "ab1", false),
            ("真不一样", "ab12", "ab13", false),
            ("空期望（恒真的闸比没有闸更坏）", "ab12", "", false),
            ("两边都空", "", "", false),
        ];
        for (case, got, want, pass) in table {
            assert_eq!(hash_matches(got, want), *pass, "{case}：got={got:?} want={want:?}");
        }
    }

    /// `sha2` 的用法与十六进制写法各钉一格（已知向量，不经文件系统）。
    #[test]
    fn the_hasher_reads_a_stream_in_chunks_and_prints_lower_case_hex() {
        // 公开向量：SHA-256("abc")。
        assert_eq!(sha256_of_reader(&mut "abc".as_bytes()).expect("读内存不该失败"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // 超过一个 64 KiB 块：分块循环的两条支路都要走到（`take` 满块 + 尾巴）。
        let big = vec![0u8; 64 * 1024 * 3 + 7];
        assert_eq!(sha256_of_reader(&mut big.as_slice()).expect("读内存不该失败"), hex_lower(&Sha256::digest(&big)));
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex_lower(&[]), "");
    }

    /// `FailKind` 的三个变体各留下一句能给人看的话（**E12⑦**：不许在这里再截断）。
    #[test]
    fn failure_note_keeps_the_transporters_words_and_adds_no_truncation() {
        assert_eq!(failure_note(&FailKind::Http(416)), "服务器对这一枪回了 416");
        assert_eq!(failure_note(&FailKind::ValidatorChanged),
            "服务端说这份文件已经变了（带 If-Range 的请求回了 200，或 416/206 的范围对不上）");
        // Transport 的载荷**原样**透传：生产侧已经把它压成单行、截到 200 字符、摘掉 URL
        // （`fetch::transport_note` 是唯一的实现）。这里再截一次就会互相吃掉信息（E12⑦），
        // 所以连"看着顺眼一点"的修饰都不许做：载荷里有换行也照原样交回去。
        let long = "y".repeat(200);
        assert_eq!(failure_note(&FailKind::Transport(long.clone())), long, "200 字符的载荷一格都不许多截");
        assert_eq!(failure_note(&FailKind::Transport("带换行\n".into())), "带换行\n", "只透传，不改写");
        assert_eq!(failure_note(&FailKind::Transport(String::new())), "", "空载荷不是 panic 也不是占位文案");
    }

    /// `Report::replay` 与 `dispatched()`/`tightened()` 的对应关系（阶段 5 那两道链接的地基）。
    ///
    /// 手工搭一段真实形状的 trace（一个 400 字节的文件、两个 worker、一次收紧），
    /// 断言：回放给出的 `Assign` 区间集合与 `dispatched()` 逐条相等、`Retighten` 与 `tightened()`
    /// 逐条相等。阶段 5 用它对线上 Range，不必自己再写一个"预测器"（§7.2 禁止的那种读法）。
    #[test]
    fn the_trace_replays_into_the_same_actions_it_recorded() {
        let total = 400u64;
        let min_seg = 1u64;
        let mut sched = Scheduler::new(total, min_seg);
        let mut trace = vec![TraceStep::Seeded(vec![])];
        let mut dispatched = vec![];
        let mut tightened = vec![];
        // 真实跑一遍：Connected(0) → 派活；Connected(1) → 收紧 + 派活；Progress；WindowDone。
        for event in [Event::Connected(0), Event::Connected(1)] {
            let actions = sched.on(event);
            trace.push(TraceStep::Fed(event));
            for action in actions {
                match action_for_worker(action) {
                    Shot::Fire { worker, iv } => {
                        trace.push(TraceStep::Dispatched { worker, iv });
                        dispatched.push(iv);
                    }
                    Shot::Tighten { worker, iv } => {
                        trace.push(TraceStep::Tightened { worker, iv });
                        tightened.push(iv);
                    }
                    Shot::Nothing => {}
                }
            }
        }
        let first = Interval { start: 0, end: 199 };
        trace.push(TraceStep::Fed(Event::Progress { worker: 0, abs_end: first.end }));
        sched.on(Event::Progress { worker: 0, abs_end: first.end });
        trace.push(TraceStep::Fed(Event::WindowDone { worker: 0 }));
        sched.on(Event::WindowDone { worker: 0 });

        let report = Report {
            bytes_total: total,
            delivered: total,
            confirmed: 0,
            duplicated: 0,
            retries: 0,
            elapsed: Duration::ZERO,
            ok: true,
            budget_reached: false,
            trace,
            fresh_reason: None,
            message: None,
        };
        let replayed = report.replay(total, min_seg);
        let (assigns, retightens) = sched_snapshot_pairs(&replayed);
        assert_eq!(assigns, report.dispatched(), "回放给出的 Assign 必须与引擎真打出去的一枪逐条相等");
        assert_eq!(retightens, report.tightened(), "回放的 Retighten 必须与记下的收紧逐条相等");
        assert!(!report.tightened().is_empty(), "两个 worker 的形状里至少该有一次收紧");
        // 条数写成**字面量**（T8 评审 B 的测试诚实性条目）：这一段形状是手工搭的，两次 `Connected`
        // 各派出什么、劈在哪一格全都可知 ⇒ "至少有一条"那种读法会放过少派/多派一枪的变异。
        assert_eq!(report.dispatched().len(), 2, "两枪：第一次 Connected 整段派出，第二次 Connected 领到劈出来的右半");
        assert_eq!(report.tightened().len(), 1, "一次收紧：劈分时 incumbent 的在途区间被收到中点之前");
        assert_eq!(report.fed().len(), 4, "四条事件：两次 Connected、一次 Progress、一次 WindowDone");
        // 这一格原先断的是 `report.confirmed` —— 上面几行自己刚写进去的字面量 0，等于没断
        // （T8 评审 B）。改成断**被测代码 `Report::replay` 产出的完整动作序列**：带 worker 序号，
        // 比 `assigns`/`retightens` 两列区间投影更严（区间对而序号错也红）。
        assert_eq!(
            replayed,
            vec![
                Action::Assign { worker: 0, iv: Interval { start: 0, end: 399 } },
                Action::Retighten { worker: 0, iv: Interval { start: 0, end: 199 } },
                Action::Assign { worker: 1, iv: Interval { start: 200, end: 399 } },
            ],
            "回放必须给出与这段真跑完全相同的动作序列（含 worker 序号与中点劈分的位置）"
        );
    }

    /// 回放结果折成两组区间（`Assign` / `Retighten`），好与 trace 的两条分支一一对上。
    fn sched_snapshot_pairs(actions: &[Action]) -> (Vec<Interval>, Vec<Interval>) {
        // 返回类型写成元组而不是结构体：它只在这条用例里活着。
        (
            actions.iter().filter_map(|a| match a {
                Action::Assign { iv, .. } => Some(*iv),
                _ => None,
            }).collect(),
            actions.iter().filter_map(|a| match a {
                Action::Retighten { iv, .. } => Some(*iv),
                _ => None,
            }).collect(),
        )
    }

    // ==================== 9A（勘误 **E21** 甲）：运行态快照的数据面 ====================
    //
    // 这一节的夹具**不起线程、不碰网络**（只有 (d) 那一发 POST 真要去敲一个端口）。理由不是省事：
    // 快照那八格全部来自引擎脚下的量（`sched` / `stats` / `run_delivered` / `workers` / `cfg`），
    // 而它们的**变化**在真引擎里由"一条 worker 结论"驱动 —— 那结论正好是 `reports` 通道里的
    // 一个 `WorkerReport`，测试可以直接把它投进去，于是节拍、速率、计数三格都钉得住。
    //
    // 不经 `engine::run` 的代价要照实说：试探与断点那两段没走过。E21 甲的十条判据里**没有一条**
    // 依赖试探内容或断点内容（②说的是"不许拿 0 当未知哨兵"、⑧说的是"`--data-file` 那一支 0 次"），
    // 而判据⑧那一条**必须**经 `run` 走一遍（它问的正是 `run` 的分岔），(d) 就是这么写的。
    //
    // ⚠ 派发说"用本文件既有的 stub/夹具助手"：`mod tests` 里**没有**任何对端夹具（既有的用例全是
    // 纯函数与手工 trace），而 `tests/harness` 的 `Stub`/node 对端在 lib 单元测试面上够不着
    // —— 它住在集成测试的编译单元里。所以这里的夹具是新搭的，不冒充"复用了既有夹具"。

    /// **E21 验收下限点名的证据形状**：一个计数器 + 一份带时间戳的快照流水。
    ///
    /// 为什么不许拿打印当证据：`cargo test` 会把 stdout 吞进"捕获的输出"，"一行也没打"证明不了
    /// "一次也没调"；`AtomicUsize` 记的是**被测代码真的调过那个闭包**这件事。
    /// 时间戳与快照同拍取：(c) 那条"两拍之差"要用它反推间隔。
    #[derive(Clone)]
    struct TickSink {
        hits: Arc<AtomicUsize>,
        log: Arc<Mutex<Vec<(Instant, RunSnapshot)>>>,
    }

    impl TickSink {
        fn new() -> TickSink {
            TickSink { hits: Arc::new(AtomicUsize::new(0)), log: Arc::new(Mutex::new(vec![])) }
        }

        fn count(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        /// 清账：(d) 要用**同一个**观察者在两条路线之间对账，先清零才比得出"这一支一次都没有"。
        fn reset(&self) {
            self.hits.store(0, Ordering::SeqCst);
            self.log.lock().expect("快照流水没有竞争者").clear();
        }

        fn taken(&self) -> Vec<(Instant, RunSnapshot)> {
            self.log.lock().expect("快照流水没有竞争者").clone()
        }

        /// 交给 `Config` 的那只手。刻意**只**做两件事：加一计数、记一条时间戳 + 快照 ——
        /// E21⑨ 给实现者的纪律（不许在回调里做长阻塞 I/O）在这里同样成立，否则节拍被夹具自己拖歪。
        fn handle(&self) -> OnTick {
            let hits = Arc::clone(&self.hits);
            let log = Arc::clone(&self.log);
            Arc::new(move |snap: &RunSnapshot| {
                hits.fetch_add(1, Ordering::SeqCst);
                log.lock().expect("快照流水没有竞争者").push((Instant::now(), *snap));
            })
        }
    }

    /// 9A 夹具：一台**手搭、不起线程**的引擎 + 一个留在测试手里的结论发送端。
    struct TickFixture {
        engine: Engine,
        /// 测试往这里投一条结论，主循环就走一遍**真实的** `on_report` 记账路径。
        reports: Sender<WorkerReport>,
        /// 主循环发给 worker 的 `Job` 必须有人**接着**（发送端还活着才不会被判 `WorkerGone`）⇒
        /// 接收端由夹具自己留着，不交给任何线程（这里一个 worker 线程都没起）。
        _jobs: Vec<Receiver<Job>>,
        target: PathBuf,
        state_path: PathBuf,
    }

    impl TickFixture {
        fn cleanup(self) {
            let _ = fs::remove_file(&self.target);
            let _ = fs::remove_file(&self.state_path);
        }
    }

    /// 手搭一台与 `Engine::new` 同副样子的引擎，只差两件事：**没有 worker 线程**、结论通道留在测试手里。
    ///
    /// `total_len` 同时进调度器与快照；`tag` 只用来把临时文件分名（lib 用例是并行跑的）。
    fn hand_engine(tag: &str, cfg: Config, total_len: u64) -> TickFixture {
        let dir = std::env::temp_dir();
        let target = dir.join(format!("dlm-9a-{tag}.bin"));
        let state_path = dir.join(format!("dlm-9a-{tag}.dlm.json"));
        let url = format!("http://127.0.0.1:9/dlm-9a-{tag}.bin");
        let task = Arc::new(Task {
            url: url.clone(),
            referer: None,
            cookies: None,
            data_file: None,
            user: None,
            password: None,
            dir: dir.clone(),
            name: Some(format!("dlm-9a-{tag}.bin")),
            flags: 0,
        });
        let store = Arc::new(Store::open(&target, true).expect("夹具要能建目标文件"));
        let (reports_tx, reports_rx) = channel::<WorkerReport>();
        let (job_tx, job_rx) = channel::<Job>();
        let started = Instant::now();
        let sched = Scheduler::new(total_len, cfg.min_seg);
        let saved_pending = sched.pending_bytes();
        let engine = Engine {
            task,
            cfg,
            store,
            sched,
            total_len,
            accept_ranges: true,
            validator: Validator {
                etag: None,
                last_modified: None,
                total_len,
                final_url: url,
                ignore_mtime: false,
            },
            state_path: state_path.clone(),
            stats: Stats::default(),
            jobs: vec![job_tx],
            reports: reports_rx,
            handles: vec![],
            workers: vec![WorkerCtl { state: WorkerState::Idle, attempt: 0 }],
            alive: vec![Arc::new(AtomicBool::new(true))],
            caps: vec![Arc::new(fetch::EndCap::new(fetch::NO_END_CAP))],
            progress: vec![Arc::new(AtomicU64::new(fetch::NO_PROGRESS))],
            synced_progress: vec![fetch::NO_PROGRESS],
            current: vec![None],
            trace: vec![TraceStep::Seeded(vec![])],
            fresh_reason: None,
            run_delivered: 0,
            budget_share: vec![0],
            budget_base: 0,
            budget_exhausted: false,
            last_save: started,
            last_tick: started,
            saved_pending,
            started,
            last_note: None,
            tick_at: started,
            tick_delivered: 0,
            dup_base: 0,
        };
        TickFixture { engine, reports: reports_tx, _jobs: vec![job_rx], target, state_path }
    }

    /// 一条"这一枪落了这么多字节 / 这一枪坏了"的结论。
    ///
    /// `iv` 给什么都行：`run_delivered` 只看这条结论交回的字节，调度器认不认这一段只影响覆盖集
    /// （而覆盖集这一格由 (e) 直接钉，不必绕主循环）。
    fn report(worker: usize, iv: Interval, outcome: Outcome) -> WorkerReport {
        WorkerReport { worker, iv, outcome: Ok(outcome) }
    }

    /// 节拍上界：`运行秒数上取整 + 2`（**E21** 的验收下限 (b) 原文，`+2` 是留给"收尾那一拍"与
    /// 时钟粒度的余量）。写在这里而不是抄在断言里，是为了让 (b) 与 (d) 用同一个数。
    fn beat_bound(elapsed: Duration) -> usize {
        elapsed.as_secs() as usize + usize::from(elapsed.subsec_nanos() > 0) + 2
    }

    /// (a)：**默认没有观察者，而"没有观察者"不改变引擎脚下的任何一笔账**。
    ///
    /// 判据分两层，各挡一种写法：
    /// - `Config::default().on_tick.is_none()` 挡的是"默认塞一只 no-op 闭包"（变异 m4）。
    ///   这一句才是真针：字段是 `Option<Arc<dyn Fn..>>`，没交出去的闭包**根本没有调用点**，
    ///   所以"None 时一次都不调"这件事在类型上就成立，拿计数器去证它等于证一句空话 ——
    ///   下面那个计数器保留下来只为了"没有第二条暗渠道"这一句（引擎不许绕开 `Config` 自己找观察者）。
    /// - 三笔账的字面量（`run_delivered` / `bytes_duplicated` / 结论）挡的是"挂观察者与不挂走了两条路"。
    #[test]
    fn the_default_config_carries_no_observer_and_none_changes_no_book() {
        assert!(Config::default().on_tick.is_none(), "E21⑨ 的默认必须是 `None`：默认路径一个新字段都不挂");
        let by_update = Config { workers: 3, ..Config::default() };
        assert!(by_update.on_tick.is_none(), "结构体更新语法也不该凭空长出观察者");
        assert!(by_update.clone().on_tick.is_none(), "`Config` 派生 `Clone` ⇒ 字段是 `Arc`，克隆不许把它变成 `Some`");

        let counter = Arc::new(AtomicUsize::new(0));
        let cfg = Config { workers: 1, max_retries: Some(0), ..Config::default() };
        let mut fx = hand_engine("none", cfg, 1_000_000);
        // 三条结论照投：两条进度 + 一条收工的失败（`max_retries = Some(0)` ⇒ 失败即封顶，见 `Stop`）。
        fx.reports.send(report(0, Interval { start: 0, end: 999 }, Outcome::Advanced { bytes: 1_000 })).expect("投结论");
        fx.reports.send(report(0, Interval { start: 1_000, end: 1_999 }, Outcome::Advanced { bytes: 2_000 })).expect("投结论");
        fx.reports.send(report(0, Interval { start: 2_000, end: 2_999 }, Outcome::Failed(FailKind::Http(503)))).expect("投结论");
        let stop = fx.engine.drive();

        assert!(matches!(stop, Stop::RetriesExhausted { worker: 0, attempted: 0 }), "主循环要照常走到收工结论：{stop:?}");
        assert_eq!(fx.engine.run_delivered, 3_000, "没有观察者也要照常记账（3_000 = 上面两条结论的字面量）");
        assert_eq!(fx.engine.stats.bytes_duplicated, 0, "没有观察者也不该凭空多出重复账");
        assert_eq!(counter.load(Ordering::SeqCst), 0, "引擎不许绕开 `Config` 自己找观察者");
        assert!(!fx.state_path.exists(), "观察者缺席时不许多出任何落盘产物");
        fx.cleanup();
    }

    /// (b)：有观察者时**每拍至多一次**，而且**绝不是一块一条**（判据⑦，变异 m1 的针）。
    ///
    /// 夹具的形状：六条"块级"结论每 250 ms 投一条（全部挤在两拍之内），再让一条失败结论收工。
    /// 于是"每块都打"那种写法会在 1.7 秒的运行里交回 6 发以上，而上界算出来只有 4；
    /// 正常写法交回 2 发（越过 `LOOP_POLL` 的那一拍 + 收尾那一拍）。
    /// ⚠ 结论之间隔 250 ms 而不是零间隔：真时钟下 `recv_timeout` 可能差几毫秒早醒，
    /// 把两条结论挤进同一次迭代就没有"跨拍"可言（这条夹具的节拍实测在 1.5–1.9 s 之间浮动）。
    #[test]
    fn a_sink_gets_one_tick_per_beat_and_never_one_per_block() {
        let sink = TickSink::new();
        let cfg = Config { workers: 1, max_retries: Some(0), on_tick: Some(sink.handle()), ..Config::default() };
        let mut fx = hand_engine("beat", cfg, 100_000_000);

        let pacer = {
            let reports = fx.reports.clone();
            thread::spawn(move || {
                for k in 0..6u64 {
                    thread::sleep(Duration::from_millis(250));
                    let iv = Interval { start: k * 1_000, end: k * 1_000 + 999 };
                    if reports.send(report(0, iv, Outcome::Advanced { bytes: 1_000 })).is_err() {
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(200));
                let _ = reports.send(report(0, Interval { start: 6_000, end: 6_999 }, Outcome::Failed(FailKind::Http(503))));
            })
        };

        let started = Instant::now();
        let stop = fx.engine.drive();
        let elapsed = started.elapsed();
        pacer.join().expect("节拍线程要收得回来");
        assert!(matches!(stop, Stop::RetriesExhausted { .. }), "夹具该由那条失败结论收工：{stop:?}");
        assert_eq!(fx.engine.run_delivered, 6_000, "六条结论**都真落了盘**（不然次数上限就是在比一个空夹具）");

        let n = sink.count();
        let cap = beat_bound(elapsed);
        assert!(n >= 2, "运行 {elapsed:?} 至少该有两发（越过 `LOOP_POLL` 的一拍 + 收尾那一拍），实测 {n}");
        assert!(n <= cap, "每拍至多一次 + 收尾一发 ⇒ 上界 {cap}（运行 {elapsed:?}），实测 {n}：多出来就是打进每块那条循环里了");
        fx.cleanup();
    }

    /// (c)：速率那一格是**两拍之差**，不是全程平均（判据④，变异 m2 的针）。
    ///
    /// 夹具故意把一次运行拉成"先慢后快"：第一拍之前只交 1_000 个字节，之后一次交 900_000 个字节。
    /// 于是"Δ法"与"平均法"给出的数差近两倍 ⇒ 两种写法**分得开**（分不开就是恒真夹具，
    /// 本项目已经栽过四次，所以这里先把这句话写成一条断言再写判据）。
    ///
    /// 期望值全部由**可观测量**现算：Δ 由两格 `run_delivered` 相减（并断言它恰等于夹具写死的
    /// 900_000，证明落在两拍之间的就是那一发），间隔由观察者的时间戳相减。被测的那一格
    /// 必须站在 Δ 这一边（3% 以内：引擎量间隔的那一刻与闭包进门的那一刻只差微秒），
    /// 而且必须离平均法那一侧足够远。
    #[test]
    fn the_rate_cell_is_the_difference_between_two_beats_not_the_whole_run_average() {
        let sink = TickSink::new();
        let cfg = Config { workers: 1, max_retries: Some(0), on_tick: Some(sink.handle()), ..Config::default() };
        let mut fx = hand_engine("rate", cfg, 100_000_000);
        let anchor = fx.engine.started;

        let pacer = {
            let reports = fx.reports.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(50));
                if reports.send(report(0, Interval { start: 0, end: 999 }, Outcome::Advanced { bytes: 1_000 })).is_err() {
                    return;
                }
                // 1.45 s：稳稳落在第一拍**之后**（第一拍在 ~1.0 s），又快于第二拍（~2.0–2.4 s）。
                thread::sleep(Duration::from_millis(1_400));
                if reports.send(report(0, Interval { start: 1_000, end: 900_999 }, Outcome::Advanced { bytes: 900_000 })).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(1_550));
                let _ = reports.send(report(0, Interval { start: 901_000, end: 901_999 }, Outcome::Failed(FailKind::Http(503))));
            })
        };

        let stop = fx.engine.drive();
        pacer.join().expect("节拍线程要收得回来");
        assert!(matches!(stop, Stop::RetriesExhausted { .. }), "夹具该由那条失败结论收工：{stop:?}");

        let log = sink.taken();
        let pair = log
            .windows(2)
            .rev()
            .find(|w| w[1].1.run_delivered > w[0].1.run_delivered)
            .unwrap_or_else(|| panic!("要有一对相邻快照之间真有字节增量，夹具的节拍塌了：{log:?}"));
        let (ta, snap_a) = pair[0];
        let (tb, snap_b) = pair[1];

        let delta = snap_b.run_delivered - snap_a.run_delivered;
        assert_eq!(delta, 900_000, "落在两拍之间的那一发必须是上面写死的 900_000，实测 {delta}");
        let gap_us = u64::try_from(tb.duration_since(ta).as_micros()).expect("快照流水按时间升序");
        assert!(gap_us >= 50_000, "两拍之间至少隔开 50 ms，否则 1 ms 下限会让这一格变成除法噪声：{gap_us} µs");
        let by_difference = delta.saturating_mul(1_000_000) / gap_us;
        let run_us = u64::try_from(tb.duration_since(anchor).as_micros()).expect("收尾在开工之后");
        let by_whole_run_average = snap_b.run_delivered.saturating_mul(1_000_000) / run_us;

        assert!(
            by_difference >= by_whole_run_average * 6 / 5,
            "这一格得先**分得开**两种写法才有资格当判据：Δ法 {by_difference} vs 平均法 {by_whole_run_average}"
        );
        let drift = snap_b.instant_bytes_per_sec.abs_diff(by_difference);
        assert!(
            drift <= by_difference / 30 + 1,
            "E21④：速率那一格要等于两拍之差 {by_difference}，实测 {}（偏差 {drift}）",
            snap_b.instant_bytes_per_sec
        );
        assert!(
            snap_b.instant_bytes_per_sec.abs_diff(by_whole_run_average) * 5 >= by_whole_run_average,
            "E21④ 点名的那个坑：把全程平均 {by_whole_run_average} 交出去当显示速率（实测 {}）。慢启动段会把它算低",
            snap_b.instant_bytes_per_sec
        );
        assert_eq!(snap_a.run_delivered, 1_000, "第一拍只该看到那一发 1_000 字节的结论");
        fx.cleanup();
    }

    /// (d)：`--data-file` 那一支**一次都不交**（判据⑧，变异 m3 的针）。
    ///
    /// 与 (b) 配成一对写在**同一条用例**里：先让**同一个观察者**在可分段那条路上收到若干发
    /// （证明这套接线真的通），清零，再走 `run` 的 `--data-file` 分岔，要求它是 0。
    /// 单写一条"0 次"的负断言不叫证据 —— 它同样能被一个"根本没接上"的实现满足。
    ///
    /// 那一支这轮走的是**真** `engine::run`：URL 指向一个刚释放的空闲端口（bind 之后 drop ⇒
    /// 连接被拒），POST 那一枪自己得出"没成"的结论。网络失败不是夹具的替身，是那条路的正常出口
    /// （`run_whole_once` 不重发，spec R7 / E12⑧）；这一格断的只有"tick 次数"，不是那条结论的内容。
    #[test]
    fn the_data_file_route_emits_no_ticks_while_the_segment_route_emits_some() -> anyhow::Result<()> {
        let sink = TickSink::new();
        let cfg = Config { workers: 1, max_retries: Some(0), on_tick: Some(sink.handle()), ..Config::default() };

        // ① 可分段那一支：先把接线跑通。
        let mut fx = hand_engine("pair", cfg, 100_000_000);
        fx.reports.send(report(0, Interval { start: 0, end: 999 }, Outcome::Advanced { bytes: 1_000 })).expect("投结论");
        fx.reports.send(report(0, Interval { start: 1_000, end: 1_999 }, Outcome::Failed(FailKind::Http(503)))).expect("投结论");
        let stop = fx.engine.drive();
        assert!(matches!(stop, Stop::RetriesExhausted { .. }), "夹具该收工：{stop:?}");
        let segment_ticks = sink.count();
        assert!(segment_ticks >= 1, "对照那一发要真收到过 tick，否则下面的 0 只是在比一个没接上的观察者：{stop:?}");
        fx.cleanup();

        // ② `--data-file` 那一支：同一个观察者，清零之后必须还是 0。
        sink.reset();
        assert_eq!(sink.count(), 0, "清零本身");
        let dir = std::env::temp_dir();
        let body = dir.join("dlm-9a-datafile-body.bin");
        let target = dir.join("dlm-9a-datafile.bin");
        fs::write(&body, b"q=hello").expect("写 POST 体文件");
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let port = listener.local_addr()?.port();
        drop(listener); // 让这一发 POST 撞在"没人听"上：那条路自己会得出"没成"的结论
        let task = Task {
            url: format!("http://127.0.0.1:{port}/post"),
            referer: None,
            cookies: None,
            data_file: Some(body.clone()),
            user: None,
            password: None,
            dir: dir.clone(),
            name: Some("dlm-9a-datafile.bin".to_string()),
            flags: 0,
        };
        let report_of_whole_route = run(&task, Config { workers: 1, on_tick: Some(sink.handle()), ..Config::default() })?;
        assert!(!report_of_whole_route.ok, "指向已关闭端口的 POST 不该被报成成功");
        assert_eq!(report_of_whole_route.trace.len(), 0, "那一支没有调度器，一条事件都不该被喂出去");
        assert_eq!(sink.count(), 0, "E21⑧：`--data-file` 那一支 0 次 tick（对照上面可分段那一支收到过 {segment_ticks} 发）");

        let _ = fs::remove_file(&body);
        let _ = fs::remove_file(&target);
        let _ = fs::remove_file(state::path_for(&target));
        Ok(())
    }

    /// (e)：八格各自的**出处**逐格钉死（判据①②③⑤⑥）。
    ///
    /// 这一格不走主循环、不等节拍：直接把引擎脚下的量摆成已知的样子，再交**一发**快照。
    /// 摆状态而不是猜时序，是因为这里要比的是"这一格读的是哪一本账"，不是"什么时候读"
    /// （节拍与次数归 (b)，速率出处归 (c)）。
    #[test]
    fn every_snapshot_cell_comes_from_the_documented_source() {
        let sink = TickSink::new();
        let cfg = Config { workers: 1, on_tick: Some(sink.handle()), ..Config::default() };
        let mut fx = hand_engine("cells", cfg, 1_000);

        // 覆盖集：`0..=399` 已确认 ⇒ 按判据① `covered = total_len − pending_bytes()` = 1000 − 600。
        fx.engine.sched = Scheduler::resume(1_000, 16, &[Interval { start: 0, end: 399 }]);
        assert_eq!(fx.engine.sched.pending_bytes(), 600, "先自证那本账：调度器说还差 600 字节");
        // 跨轮账（判据③）：断点带回来的 dup 是 40，本轮加到 150 ⇒ 快照只能交出差值 110。
        fx.engine.dup_base = 40;
        fx.engine.stats.bytes_duplicated = 150;
        fx.engine.run_delivered = 1_000_000;
        fx.engine.tick_delivered = 0;
        // 锚点放在**未来** ⇒ `saturating_duration_since` 给 0 ⇒ 间隔被判据④的下限夹成 1 ms
        // ⇒ 速率那一格可以写成字面量（公式本体由 (f) 那张表钉，不靠这里）。
        fx.engine.tick_at = Instant::now() + Duration::from_secs(5);
        fx.engine.cfg.workers = 3;
        fx.engine.workers = vec![
            WorkerCtl { state: WorkerState::Busy, attempt: 7 },
            WorkerCtl { state: WorkerState::Backoff { until: Instant::now() + Duration::from_secs(30) }, attempt: 3 },
            WorkerCtl { state: WorkerState::Idle, attempt: 0 },
        ];

        fx.engine.emit_tick();
        let log = sink.taken();
        assert_eq!(log.len(), 1, "直接交快照：一发");
        let snap = log[0].1;
        assert_eq!(snap.total_len, 1_000, "判据②：`total_len` 不是 Option，也不许拿 0 当未知哨兵");
        assert_eq!(snap.covered_bytes, 400, "判据①：covered = total_len − pending_bytes()，**不是** store.len()");
        assert_eq!(snap.run_delivered, 1_000_000, "判据③：用的就是引擎已有的那一格 `run_delivered`");
        assert_eq!(snap.run_bytes_duplicated, 110, "判据③：本次 dup 增量 = 150 − 40，跨轮那份起点要减掉");
        assert_eq!(snap.instant_bytes_per_sec, 1_000_000_000, "判据④：Δ=1e6 字节 ÷ 1 ms 下限 = 1e9 字节/秒");
        assert_eq!(snap.active_workers, 1, "判据⑤：只数 Busy");
        assert_eq!(snap.workers_total, 3, "`workers_total` = `cfg.workers`");
        assert_eq!(snap.slow_retry_rounds, 2, "判据⑥：跨 worker **最大值** = 7 − FAST_RETRY_MAX(5)，不是各链相加");
        assert!(fx.engine.any_in_flight(), "同一副状态下 `any_in_flight()` 是真的（Busy + Backoff 都算）");
        assert_ne!(snap.active_workers, 2, "两处判据不许互换：`active_workers` 不能把退避中那一条也算成在途");
        fx.cleanup();
    }

    /// (f)：判据④那条式子本体（表驱动，零时钟依赖）。
    ///
    /// 最后一行不是装饰：它把"Δ 法"与"全程平均法"在**同一副输入**上摆成两个不同的数，
    /// 于是"谁将来把两者并成一条式子"会当场红 —— (c) 用的是真节拍，这一格用的是字面量。
    #[test]
    fn the_instant_rate_formula_is_the_microsecond_quotient_with_a_one_ms_floor() {
        let table: &[(u64, Duration, u64)] = &[
            (0, Duration::ZERO, 0),                                        // 没字节 + 同一瞬间：不许 panic
            (1_000, Duration::from_secs(1), 1_000),                         // 1 秒 1000 字节
            (65_536, Duration::from_millis(500), 131_072),                  // 半秒一满块
            (1_000_000, Duration::from_millis(1), 1_000_000_000),           // 下限那一格：1 ms
            (500, Duration::from_micros(1), 500_000),                       // 1 µs 也要夹到 1 ms（不夹就是 5e8 的假高速）
            (900_000, Duration::from_millis(1_200), 750_000),               // (c) 那一格的同形状对照
        ];
        for (delta, gap, want) in table {
            assert_eq!(instant_rate_bytes_per_sec(*delta, *gap), *want, "Δ={delta} gap={gap:?}");
        }
        // 两条式子必须**是**两条式子：同一时刻、同一笔账，给出的数不一样。
        assert_eq!(running_rate_bytes_per_sec(901_000, Duration::from_millis(2_500)), 360_400);
        assert_ne!(
            instant_rate_bytes_per_sec(900_000, Duration::from_millis(1_200)),
            running_rate_bytes_per_sec(901_000, Duration::from_millis(2_500)),
            "E21④：显示速率与 `window_budget` 的分子是两个数，不许共用"
        );
        // 上界那一格不许在加法上回绕。
        assert!(instant_rate_bytes_per_sec(u64::MAX, Duration::from_micros(1)) >= instant_rate_bytes_per_sec(1, Duration::from_micros(1)));
    }

    /// (g)：`active_workers` 与 `slow_retry_rounds` 的规则各自钉死（判据⑤⑥）。
    #[test]
    fn the_busy_count_and_the_slow_retry_rounds_follow_their_own_rules() {
        let later = Instant::now() + Duration::from_secs(30);
        let busy = |attempt| WorkerCtl { state: WorkerState::Busy, attempt };
        let idle = |attempt| WorkerCtl { state: WorkerState::Idle, attempt };
        let back = |attempt| WorkerCtl { state: WorkerState::Backoff { until: later }, attempt };
        // 一行 = (名字, 这条那几条 worker 状态, 期望 Busy 条数, 期望慢重试轮数)
        let rows: &[(&str, &[WorkerCtl], usize, u32)] = &[
            ("全场空闲", &[idle(0), idle(0)], 0, 0),
            ("Busy 才算在途", &[busy(0), idle(0)], 1, 0),
            ("退避中不算在途，但它的 attempt 照样进慢重试那一格", &[back(9)], 0, 4),
            ("两条 Busy 加一条退避", &[busy(0), busy(0), back(9)], 2, 4),
            ("取最大值而不是相加", &[idle(7), idle(2)], 0, 2),
            ("刚好用满快速那一档 ⇒ 0（第 6 次才排下头一发 30 s）", &[busy(5)], 1, 0),
            ("一个 worker 都没起来 ⇒ 两格都是 0", &[], 0, 0),
            ("u32 上界不许绕回", &[back(u32::MAX)], 0, u32::MAX - FAST_RETRY_MAX),
        ];
        for (case, workers, want_active, want_rounds) in rows {
            assert_eq!(busy_worker_count(workers), *want_active, "{case}");
            assert_eq!(slow_retry_rounds(workers), *want_rounds, "{case}");
        }
    }

    /// A16：字节预算按"预读基数 + 已交付 + 在途预占"算，派枪时把额度折成终点帽；
    /// 在途预占在结论回来时归零，花到顶后置 `budget_exhausted`。
    #[test]
    fn the_byte_budget_slices_shots_and_stops_after_the_cap() {
        let cfg = Config { workers: 1, max_bytes: 1_000, ..Config::default() };
        let mut fx = hand_engine("budget", cfg, 10_000);
        fx.engine.budget_base = 250;

        let first = Interval { start: 0, end: 9_999 };
        assert!(fx.engine.dispatch(0, first).is_none());
        assert_eq!(fx.engine.budget_share[0], 750, "可用额度 1000-250，单 worker 全给它");
        assert_eq!(fx.engine.caps[0].load(Ordering::Acquire), 749, "帽是 start+share-1");

        assert!(
            fx.engine
                .on_report(report(0, first, Outcome::Advanced { bytes: 700 }))
                .is_none()
        );
        assert_eq!(fx.engine.budget_share[0], 0, "结论回来后在途预占归零");
        assert!(!fx.engine.budget_exhausted, "还差 50 字节，不许提前收工");
        assert_eq!(fx.engine.budget_available(), 50);

        let second = Interval { start: 750, end: 9_999 };
        assert!(fx.engine.dispatch(0, second).is_none());
        assert_eq!(fx.engine.budget_share[0], 50);
        assert_eq!(fx.engine.caps[0].load(Ordering::Acquire), 799);

        assert!(
            fx.engine
                .on_report(report(0, second, Outcome::Advanced { bytes: 50 }))
                .is_none()
        );
        assert!(fx.engine.budget_exhausted, "花到顶必须置位");
        assert_eq!(fx.engine.budget_available(), 0);
    }
}
