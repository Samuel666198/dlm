//! 窗口化子请求：领一个字节区间，把 body 流进 `store` 的正确偏移，**最多读满一个窗口就收手**。
//!
//! 依据：spec §2 模块表（`fetch` 行「领一个 `Action`，发 `Range: start-`，流式交给 `store`，
//! 回报事件」）、spec §3 的 `STALL_TIMEOUT` 与 spec §10 勘误 **R1**（reqwest blocking 没有
//! `read_timeout` ⇒ "15 秒无数据即判卡"只能靠**窗口化子请求**实现）、spec §6.4（416）、
//! plan 第 760–803 行与其勘误 **E5**/**E7**，以及派发对简报的六条裁定（下面逐条落在代码里）。
//!
//! ## 为什么"窗口"能顶掉 `read_timeout`
//!
//! 一次请求只领 `WINDOW_BYTES` 个字节：读满就主动收手（把没读的尾巴留在连接上拆掉），交回
//! `Advanced{bytes}`，让 `scheduler` 重新决定。于是一条**卡住**的连接最多占住一个窗口预算，
//! 而不需要一个库级读超时 —— 也**不需要**抢占一个正阻塞在 `read()` 上的线程（R1 的副作用：
//! 不泄漏线程，且这与 IDM 实测形状一致：那次 8 条连接里 6 条是被主动掐断的）。
//!
//! ## 六条裁定分别落在哪儿（用例名见每条文档）
//!
//! 1. **`run_window` 多带一个 `total_len`**（裁定 1）：简报的签名里没有文件长度，而"末段发
//!    开放终点"那条规则（spec §2 表 / §3 对 IDM 的观测）要拿 `iv.end` 与 `total_len - 1` 比 ——
//!    没有这个参数那条规则**不可计算**。Task 8 从 `ProbeFacts` 拿它。
//! 2. **416 不是一律 `Complete`**（裁定 2，作废 plan 第 800 行；勘误 **E7** 的 2026-10-16 追加段）：
//!    我们每个区间都是由 `total_len` 推出来的，所以收到 416 本身**就是**"服务端长度在我们脚下变了"
//!    的证据。判据 = 读响应里的 `Content-Range: bytes */N`：**`N == total_len` 才 `Complete`**
//!    （那个区间确实一个字节都不需要），`N != total_len`、分母是垃圾、或压根没发这个头 ⇒
//!    `Failed(ValidatorChanged)`，让 spec §6.1/§6.2 的重试上限真介入，而不是悄悄封一份截断文件。
//!    `Complete` 在 Task 8 那里折成 `Event::SegmentComplete`（**不看已写字节就把整段并进 done**），
//!    所以这个值必须只说"真的没活儿"，别无分号。
//!    可达形状按 **E12**②是 `iv.start >= total_len`（续传时区间起点已落在 EOF 上），也就是
//!    `iv.start` 与 `total_len` 在野外**是两个不同的数** —— 判据必须能把它们分辨开（修复轮 C）：
//!    正支把起点放到 `total_len + 4096`，反支让分母正好等于 `iv.start`。
//!    用例：`a_416_whose_denominator_is_our_length_is_complete`（正支，`iv.start > total_len`）、
//!    `a_416_that_answers_with_our_interval_start_is_validator_changed`（分母=起点那一格，外加
//!    "没有 `Content-Range`"与"分母读不出来"两格）、
//!    `a_416_with_a_disagreeing_or_missing_denominator_is_validator_changed`（反支三种形状）、
//!    `a_beyond_length_interval_on_a_shorter_server_is_validator_changed`（真服务器那一支）。
//! 3. **简报第 3 条用例的前提是假的**（裁定 3，E5：`/flaky` 从不在中途打断，它只是把服务长度
//!    改成 4 MiB）：于是拆成三条诚实的用例 —— (a) 越界 ⇒ 416 ⇒ `ValidatorChanged`（`/flaky`）；
//!    (b) body 中途被掐 ⇒ `/drop/<n>` ⇒ 见第 4 条；(c) 真·卡住 ⇒ 进程内静默对端 ⇒ `Stalled`，
//!    外加一条"读侧超时"（对端只交头、body 一个字节不发）钉住 `is_timeout` 的分类。
//! 4. **短 body / 掐断 / 超时都不许丢字节账**（裁定 4）：这一枪**写下去**的字节是真进度 ——
//!    `scheduler::release()` 保住一段已写的起点前缀，Task 8 拿到 `Advanced{bytes}` 就接着派。
//!    只有**一个字节都没落盘**那一支才交回失败/卡住（零进度才是重试上限该计的数）。
//!    `store.len()` 始终是落盘量的真相（E7/E8），所以这条只是"别把已确认的写盘量扔掉"。
//! 5. **窗口上限正好是 `WINDOW_BYTES` 个字节**（裁定 5）：`bytes` 与 `store.len()` 由同一个
//!    `stop` 算出来，任何一处差一个字节都当场红。简报那句
//!    `assert!(... == vec![0,0,0,0] || true)` 是恒真式、什么都没断，删掉；真 oracle 是块号取证
//!    （`require_block_alignment`：blob 每 4 KiB 块首写着自己的块号）。
//!    顺带一处**简报的第二个不可成立点**：它取证用的 `1_000_000` **不是 4096 的整数倍**，而块号只
//!    写在块首 ⇒ 拿那个起点断"内容对不对"，断的其实是未初始化填充字节。用例改用 4096 对齐的起点。
//!    **`Complete` 的判据是 `cursor > iv.end` 而不是 `>=`**（修复轮 B）：`iv.end` 是**含端点**的，
//!    所以"区间最后一个字节到了"才是 `Complete` 的承诺，而 `cursor` 必须走到 `iv.end + 1` 才兑现它。
//!    区间长度为 `WINDOW_BYTES + 1` 时 `stop = iv.start + WINDOW_BYTES = iv.end` ⇒ `cursor` 最多
//!    **正好等于** `iv.end`，那一个字节还没到，只能 `Advanced`；写成 `>=` 就会把从没落盘的尾巴封成
//!    `SegmentComplete`（E7/E9：封存不看盘）。两条边界各一条用例，字节级取证：
//!    `an_interval_of_exactly_one_window_is_complete`（`iv.end` 那个下标**读得出来**）与
//!    `an_interval_one_byte_wider_than_the_window_is_never_complete`（同一个下标**读不出来**）。
//! 6. **发了 `Range` 而对端回 `200` ⇒ 先流掉前 `iv.start` 个字节再落盘**（裁定 6，计划 **E7**②
//!    交给 Task 7 的下游义务）：那种 body 的第 0 字节对应**文件偏移 0**，直接按 `iv.start` 写就是把
//!    内容写进中段。丢字节是**流式**的（每块 64 KiB 的读循环里切掉一段），不把整个 body 吸进内存。
//!    用例：`a_degraded_200_body_is_skipped_not_written_at_the_wrong_offset`（node 的 `/noranges`，
//!    真对端 + 块号取证）、`a_degraded_200_is_skipped_as_stream_not_as_a_buffered_body`
//!    （进程内节拍对端，另加一条"对端在客户端挂断前写出了多少字节"钉住不许缓冲整个 body）。
//!
//! ## 阶段5A：E14 的两条（终点帽 + 窗口终点）改了这个模块的对外形状
//!
//! 1. **`run_window` 又多一个参数：`cap: &EndCap`**（勘误 **E14**① 乙）。**E12① 那条
//!    "签名多了 `total_len`"以 E14 为准再改一次** —— 现在七个参数。
//!    `EndCap` 就是 `AtomicU64`，装的是"这一枪最多还允许写到哪个**含端点**的偏移"：
//!    `run_window` 进门先把自己的窗口终点 `stop - 1` **降**进去（`lower_end_cap`），之后读循环
//!    每块在块首问一次 `cap_allows`；越过帽就停手，交回的还是已有的 `Advanced { bytes }`
//!    （不是新的结论形状）。调用方给 `&AtomicU64::new(NO_END_CAP)` 就等于"外面没人喊停"，
//!    行为与这一条之前**逐字节相同** —— 集成用例里那二十多处 `&harness::uncapped()` 就是这个意思。
//!    帽为什么只能**往下**（`fetch_min`；E14 括号里那个 `fetch_max` 做不到它同时要求的性质，
//!    见 [`lower_end_cap`] 的文档），以及帽低于起点时为什么夹回起点（零字节的 `Advanced` 是
//!    Task 8 数不到的那种进度），两条用例各钉一格。
//! 2. **`Range` 的开放终点判据里的"终点"是窗口**（勘误 **E14**② 戊）：条件从 `iv.end + 1 >= total_len`
//!    改成 `min(iv.end, iv.start + WINDOW_BYTES - 1) + 1 >= total_len`，而显式写法发的也是**那个**
//!    终点（窗口那一格），不是 `iv.end`。文件真正那一尾仍然发 `bytes={start}-`（IDM 的观察，
//!    spec §3，用例 `range_start_open_ended_but_respects_explicit_end` 的③那一格钉着），
//!    变的只有一件事：**窗口够不着 EOF 的那一枪不许再问 EOF**（旧写法对 `[236032, 8 MiB-1]`
//!    发 `bytes=236032-`，等于授权对端流 7.75 MiB，而我们最多读 4 MiB 就收手 ⇒ 阶段4 那笔
//!    重复流量的起点）。判据的取法改动**只**影响"区间比一个窗口宽"的形状：区间比窗口窄时
//!    `min` 取到的就是 `iv.end`，与 E14 之前逐字相同（用例 `only_the_interval_that_reaches_the_end...`
//!    的其余格子因此一条没动）。
//! 3. **`probe` 不受这两条影响**：试探那条 `bytes=0-<prelude_max-1>` 住在 `probe::prelude_range`，
//!    与这里的 `range_header` 是两个函数（E11① 把试探与窗口分成两条入口之后本来就不共用终点判据），
//!    所以 `/noranges` 的 1,048,576、"206 没有 `Content-Range` 就拒"、E11③ 那张真值表都原样活着。
//!
//! ## 另外四条钉住的性质
//!
//! - **`206` 却说不出它交的是哪一段**（没有 `Content-Range`，或它的起点不等于 `iv.start`）⇒
//!   `Failed(ValidatorChanged)`。偏移核对不了的字节**一个都不许写盘**：那是裁定 6 的镜像面 —— 那边
//!   是"200 的 body 起点在 0"，这边是"206 声称的起点与我们问的不是同一个"。
//!   **两个分支都得钉**（修复轮 A）：`asked_at.is_none()` 那种写法在"缺头 + `iv.start == 0`"那一格
//!   读起来与正确判据一模一样，而它放过去的是"对端答了另一个偏移、我们照自己问的偏移写"⇒ 文件错位
//!   且看起来完整。用例 `a_206_that_cannot_show_its_offset_is_validator_changed`（缺头那一格）+
//!   `a_206_that_claims_another_offset_than_the_one_we_asked_for_writes_nothing`（起点不符那一格，
//!   两条：对端声称 0 / 声称一个第三方数，两条都另加"文件长度仍然是 0"的独立取证）。
//! - **不发 `Connection: close`、也不设 `pool_max_idle_per_host(0)`**（spec §10 **R5** / 计划 **E2**，
//!   派发点名 Task 6/7 都不得加）：`fetch` 根本不建 `Client`（`agent` 由调用方持有并复用），
//!   而 `a_drained_window_leaves_the_connection_reusable` 用服务端的连接号证明**整段读完**那一枪
//!   留下的连接确实被下一枪复用了（加了 close 这条路径就从覆盖里消失了）。
//!   窗口没用满而收手的那一枪（`Advanced`）必然把 body 剩在连接里 —— 那条连接是被拆掉的，
//!   这与 `probe.rs` 模块文档第 2 条对退化支的说明同一个性质，不许当成 bug 去"修"。
//! - **头与 `probe` 同源**：这一枪的头由 `probe::with_contract_headers` 拼（`Referer`、`Cookie`、
//!   `Accept-Encoding: identity`、`Accept: */*`，以及只在 https 上发的 `Authorization: Basic`），
//!   `fetch` 里不重写第二个版本 —— ①b 判的正是"请求是不是同一个"，两处各写一遍迟早读成两样。
//!   本模块只往上加 `Range` 与窗口预算那一条 `timeout`。
//!   用例 `fetch_carries_the_nine_item_contract_headers_on_the_wire`（服务端日志取证）。
//! - **`Transport` 带着原因**（修复轮 D 的契约改动，勘误 **E12**⑦）：`FailKind::Transport(String)`
//!   里那句是"这一次为什么没成"，由**唯一一个**构造器 [`transport_note`] 产出（`run_window` 与
//!   `run_whole` 共用，两支的措辞不会分岔）：错误自己的 message + `source()` 链上第一条带来新信息
//!   的，折成一行、截到 200 个**字符**（按字节切会在中文环境里 panic）、**不抄 body**、reqwest 抄在
//!   尾巴上那截 URL **摘掉**（URL 不是原因，而 Task 8 打印这句时旁边就站着这条 URL）。
//!   代价要说清：`String` 不 `Copy` ⇒ `FailKind` 与 `Outcome` 从 `Copy` 降级成 `Clone`
//!   （Task 8 交回结论那一步改成克隆，性质不变：交出去一份、手里还留一份能打印）。
//!   用例：四条形状的单元用例（`a_transport_note_…` / `the_note_…` / `a_cause_we_author_…`）+
//!   线上那三条（`a_transport_failure_from_the_execute_side_names_the_cause_without_repeating_the_url`
//!   钉两处 `execute` 支、`a_transport_note_holds_no_byte_of_the_body_it_handed_back` 钉"到过读缓冲的
//!   那 4096 个字节一个都不进账"、`a_body_that_never_starts_…` / `a_whole_shot_cut_off_…` 钉两处读支）。
//!   两处零进度支**没有**错误对象（`read` 给的是 `Ok(0)`），原因由本模块自己写死，仍走同一个构造器。
//!
//! ## 与 reqwest blocking 的形状关系（实测过的事实，含**本轮推翻的一条**）
//!
//! - **窗口预算是"整请求期限"，body 也算在里面**（本轮实测，推翻上一轮的判断）：用例
//!   `the_window_budget_is_a_whole_request_deadline_on_the_read_side_too`。形状：对端每 260 ms 交
//!   64 KiB（连续静默从没超过 260 ms），预算 15 s ⇒ **15.02 s 到点**，此时只交了 3,801,088 字节，
//!   而对端的 body 总长 6 MiB、`written()` 显示它还在写 ⇒ 掐掉这一枪的是我们自己的期限。
//!   上一轮读 `src/blocking/wait.rs` 的 `timeout()` 在**调用时**算 deadline 那一行，写成"每次 `read`
//!   重新起算的静默期限，慢但活着的节拍打不中它"—— 那条判断**没有**被分辨得出来的实验证过：
//!   当时那条用例（`a_live_but_paced_peer_...`，`/sz/524288s`）的对端 1.6 s 就交完 body，
//!   两种语义给出同一个结论，是一条瞎断言（它现在仍然留着，但只钉"活着 ≠ 卡死"那一件事）。
//!   真正的机制在异步侧：请求级 `timeout` 也管 body。
//! - **对 Task 8 的后果**（这就是派发要"钉住是哪一种语义"的理由）：一个窗口的墙钟时间**上界就是
//!   预算**，与对端活不活着无关 ⇒ 填满一个窗口需要的速率是 `WINDOW_BYTES / window_budget(rate)`。
//!   比这慢的对端会在窗口**没填满**时被我们自己的期限掐掉 ⇒ 交回 `Advanced{bytes}`（进度是真的），
//!   **不是** `Stalled`（"预算到点且零字节落盘"才叫 `Stalled`，见上面那一节）。
//!   于是"慢"在 Task 8 那里读起来是"每轮最多 15–120 s 的一轮一轮真进度"，既不算失败也不算卡死；
//!   `rate_bps` 估高了 ⇒ 预算偏小 ⇒ 每轮被提前掐、总时长变长，但不丢字节、也不误报卡死。
//! - **别假设"不配 timeout 就等于永不等"**（派发点名）：Task 0 实测一条没配 timeout 的 reqwest 请求
//!   在 ~30.005 s 自己死掉，时长来源未查清。本模块**永远**显式配预算（`run_window` 与 `run_whole`
//!   都是），正是为了不让那种未查清的内部期限替我们决定语义。
//! - `Response::read` 的超时**不是** `reqwest::Error` 而是被包了一层的 `io::Error`（本轮实测形状：
//!   `io::Error{kinds: Other, payload: reqwest::Error{Kind::Body ← Kind::Body ← TimedOut}}`；
//!   上一轮记的是 `Kind::Decode ← TimedOut`，同一次误判的一部分 —— 两条都只是描述，分类不依赖它）。
//!   所以判"这是超时"要把 payload 再 `downcast_ref::<reqwest::Error>()` 一次（`is_read_timeout`），
//!   而 `reqwest::Error::is_timeout()` 是**顺着 source 链**找 `TimedOut` 的，包裹深度变了也认。
//!   只按 `kind()` 分的话，读侧超时会被计成一次传输失败 —— 两条用例
//!   （`a_peer_that_never_answers_is_stalled` 走 `execute` 支、
//!   `a_peer_that_answers_but_sends_no_body_is_stalled_not_a_transport_failure` 走 `read` 支）
//!   各钉一支。`run_whole` 复用同一个 `is_read_timeout`，那一支的分类不必再测一遍。
//!
//! ## `Stalled` 的定义（控制方已批准为定义）
//!
//! **窗口预算到点，且这一枪一个字节都没落到盘上。** 正面陈述这件事是因为 `Outcome::Stalled` 装不下
//! 字节数：它只能诚实地说"什么都没到"。活着但对端很慢 ⇒ 已经落盘的字节是真进度 ⇒ `Advanced{bytes}`，
//! Task 8 重新起一个窗口、重新起一份预算。于是"卡住"永远是重试上限该数的那种次数，而"慢"永远带着进度。
//!
//! ## 本模块**没**做的事（照实登记，不是遗漏）
//!
//! - 不重试、不退避、不落断点文件、不碰 `scheduler`：那些账都在 Task 8 的主循环里。
//! - 不校验 `sha256`、不 `flush`：`Store::flush`（= `sync_all`）只许在任务收尾调一次（**E8**①）。
//! - 不发 `Content-Type`（`run_whole` 那一条）：任务形状里没有供给它的字段，猜一个会悄悄改掉服务器
//!   怎么处理这份请求。**这是留给 Task 9（CLI）的开放问题**，不是本模块能替他做的决定。
//! - `write_at` 失败这条路径**没有测试**：仓库里没有能造出"句柄已打开、写失败"的注入口（`Store::file`
//!   是 `pub(crate)`，Task 3 的 deferred 条目明确不许为此放宽可见性，Windows 上删文件也不影响已打开的
//!   句柄）；钉它需要把 `Store` 换成 trait，那是 Task 3 的形状变更。两条入口都按同一个立场处理：
//!   `?` 往上抛，不把半截写当进度。

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::{Body, Client, Request};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;

use crate::consts::{WINDOW_BYTES, WINDOW_MAX_TIMEOUT_SECS, WINDOW_MIN_TIMEOUT_SECS};
use crate::probe;
use crate::resume::ProbeFacts;
use crate::scheduler::Interval;
use crate::store::Store;
use crate::task::{Task, authority_has_userinfo};

/// 一次从 socket 读的块大小（与 harness 服务器的出块粒度一致，也是 `store.write_at` 的粒度）。
///
/// 为什么固定而不是"读满窗口为止"：窗口最多 `WINDOW_BYTES` = 4 MiB，一次 `read` 就要一个这么大的
/// 缓冲 ⇒ 每个 worker 常驻 4 MiB（8 个 worker 就是 32 MiB），而且 `write_at` 的临界区会被一次
/// 巨大的拷贝占住。64 KiB 的粒度让"读一块、写一块、放锁"交替进行。
const CHUNK_BYTES: usize = 64 * 1024;

/// `FailKind::Transport` 那句原因的**长度上界**（字符数，不是字节数 —— 勘误 **E12**⑦）。
///
/// 为什么是 200 个**字符**：这一句要跟着进度行打到用户眼前，长了会把行撑断；而这个 crate 跑在
/// Windows 的中文环境里，错误消息（以及它后面挂着的 hyper/rustls 那些）里随时可能出现多字节字符，
/// 按**字节**切 `&s[..200]` 会切在一个字符中间 ⇒ 当场 panic。截断这件事不该有 panic 的余地。
const TRANSPORT_NOTE_MAX_CHARS: usize = 200;

/// `run_window` 里"读到干净 EOF、可是一个字节都没落到盘上"那一支的原因（那里**没有**错误对象可抄）。
///
/// 为什么这一支没有 `io::Error`：`resp.read()` 交回的是 `Ok(0)`（对端正常收尾），失败**不在**任何
/// 一次调用上，而在"区间要字节而对端一份都不给"这个事实里。措辞刻意只用中文、一个 ASCII 字母都不带
/// —— 集成用例 `a_transport_note_holds_no_byte_of_the_body_it_handed_back` 就是拿这条性质当探针：
/// 那一枪的响应体是 4096 个 `'r'`，账里只要混进任何一个字节，断言立刻红。
const WINDOW_EOF_WITHOUT_BYTES: &str = "对端交出了响应头，响应体却在任何字节被读到之前就干净结束了";

/// `run_whole` 里"对端声明了长度、响应体却干净结束在零字节"那一支的原因（同样没有错误对象）。
///
/// 这一支在本仓库的夹具下**打不进去**（`Content-Length` 用尽才有 `Ok(0)`，body 比它短会被 reqwest
/// 报成读错误 ⇒ 归 [`transport_note`] 那一条），它是"200 带一个比 body 大的分母"那种骗子对端的形状；
/// 代码检查成立、没有用例，与它下面那条 `bytes != facts.total_len` 的防御分支同一地位。
const WHOLE_EMPTY_DOCUMENT: &str = "对端声明了长度，响应体却一个字节都不给就干净结束了";

/// 把一次传输失败压成**一句话的账**（`FailKind::Transport` 的载荷；契约见 [`TRANSPORT_NOTE_MAX_CHARS`]）。
///
/// `run_window` 与 `run_whole` 的四个"有错误对象"的失败支**都**走这里（两支不会各写一套措辞），
/// 另外两支（[`WINDOW_EOF_WITHOUT_BYTES`] / [`WHOLE_EMPTY_DOCUMENT`]）那里根本没有错误对象，
/// 于是把自造的那句原因套进一个 `io::Error` 再走同一个函数 —— 为的是"一行 / 截断 / 不重复"这三条
/// 规则只有**一个**实现，而不是自造句免检。
///
/// 四条规则（都是契约，不是风格）：
/// 1. **只读错误自己的 `Display` 与它的 `source()` 链**：取链上第一条**带来新信息**的（reqwest 的
///    包法是一层套一层、同一句话重复两三次 —— 实测形状
///    `io::Error{Other ← reqwest::Error{Kind::Body ← Kind::Body ← TimedOut}}`，见模块文档），
///    重复的跳过、空的跳过，找到一条就收手（不是整条链拼起来：那会把 200 字符的额度喂给废话）。
/// 2. **压成一行**：`split_whitespace` 折叠所有换行、回车与制表符（Windows 上 `io::Error` 的消息
///    常带 `\r\n`，多行会把 Task 8 的单行进度栏拆成两截）。
/// 3. **截到 200 个字符，且只在 `char` 边界切**：超了就在最后一格放一个 `…`，总长仍恰好 200 个字符。
/// 4. **绝不抄 body 字节，也不自己拼 URL**：签名只收 `&(impl Error + ?Sized)` ⇒ 那些字节在这里
///    没有通路进来；reqwest 把 URL 抄在 `Display` 的尾巴上（`error.rs:300-302`，形状
///    `"... for url (https://…)"`），那一段**摘掉** —— URL 不是**原因**，而 Task 8 打印这一句时
///    旁边本来就站着这条 URL。（凭据那一半不用在这里防：`Task::validate()` 已经拒绝 userinfo 形态
///    的 URL，`probe::LANDED_CREDENTIALS` 那道闸也守着落地地址。）
fn transport_note(err: &(impl std::error::Error + ?Sized)) -> String {
    /// reqwest 的 `Display` 把 URL 附在消息尾巴上的那一段（`reqwest-0.13.5/src/error.rs:300-302`）。
    const URL_TAIL: &str = " for url (";

    /// 一条消息去掉"尾巴上那截 URL"之后的样子。`find` 给的字节下标落在 ASCII 子串开头 ⇒ 切片
    /// 必在 `char` 边界上，不会把多字节字符劈开。
    fn without_url(msg: &str) -> &str {
        match msg.find(URL_TAIL) {
            Some(at) => &msg[..at],
            None => msg,
        }
    }

    let mut note = without_url(&err.to_string()).to_string();
    // 链上第一条"说了新东西"的：同句重复（reqwest 那种套娃）与空串都不算话。
    let mut source = err.source();
    while let Some(cause) = source {
        // 先绑住 `to_string()` 的那个 `String`：`without_url` 交回的是借出去切片的 `&str`，
        // 直接 `let line = without_url(&cause.to_string())` 会让临时值在语句结束就掉（E0716）。
        let msg = cause.to_string();
        let line = without_url(&msg);
        if !line.is_empty() && !note.contains(line) {
            note.push_str(": ");
            note.push_str(line);
            break;
        }
        source = cause.source();
    }
    // 规则 2：一行。规则 3：按字符截，不按字节。
    let flat = note.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= TRANSPORT_NOTE_MAX_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(TRANSPORT_NOTE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// 这一枪为什么没成。三个变体各自对应 Task 8 该记的**一种账**：
/// `Http` 是"服务器有说法"（状态码进错误消息），`Transport` 与 `ValidatorChanged` 是"对端不对劲"
/// （spec §6.1/§6.2 的快速 5 次 / 慢速 30 s 封顶挂在这两支上），而 `ValidatorChanged` 额外说明
/// **"服务端长度与我们脚下这份不一致"** —— 那一支最终要报明确的"长度变了"，不是笼统的传输失败（**E7**）。
///
/// `Transport` **带着原因**（勘误 **E12**⑦，本轮的契约改动）：裸枚举交回 Task 8 只剩"网络错了"
/// 这种没法行动的话，而 reqwest / `io::Error` 本来就说了"读到一半对端拆了线"还是"连不上"。
/// 那句载荷的形状由 [`transport_note`] 一处定死（一行、≤ [`TRANSPORT_NOTE_MAX_CHARS`] 字符、
/// 不抄 body、不重复 URL）。也因此 `FailKind` 与 `Outcome` **不再是 `Copy`**（`String` 不 `Copy`），
/// 改成 `Clone` —— Task 8 的 worker 线程照样能把结论发出去，只是走克隆。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailKind {
    Http(u16),
    Transport(String),
    ValidatorChanged,
}

/// 这一枪的结论。Task 8 把它折成 `scheduler::Event`：
/// `Advanced{bytes}` → `Progress{abs_end: iv.start + bytes}`（+ 之后由主循环决定 `WindowDone`/续派）、
/// `Complete` → `SegmentComplete`、`Stalled`/`Failed` → `Failed`。
///
/// `bytes` 的语义（裁定 4/5 同源）：**这一枪经 `store.write_at` 确认落盘的字节数**，不是"从 socket
/// 读到的字节数"，也不是"区间剩下的字节数"。`Complete` 不带字节：它是"这个区间不需要（再）取字节"，
/// 而不是"取到了 `iv.len()` 个字节"（后者由 `Advanced` 说）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 这一枪**有**字节落到盘上了（哪怕随后被掐断、超时或读到比期望短的 body）。
    Advanced { bytes: u64 },
    /// 这个区间不需要（再）取字节：区间是空的，或 body 按对端自己声明的长度**完完整整**交完了。
    /// 短 body / 被掐断**绝不**给这一支（那会把段的尾巴永远封存）。
    Complete,
    /// **定义（正面陈述，不是靠排除法）：窗口预算到点，且这一枪一个字节都没落到盘上。**
    ///
    /// 为什么必须带上"零字节"这半句：`Stalled` 这个变体装不下字节数，所以它只能说"什么都没到"。
    /// "活着但慢"的对端在预算里已经写下去的字节是真进度，只能由 `Advanced{bytes}` 交回 ——
    /// 那一支让 Task 8 重新起一个窗口、重新起一份预算（裁定 3(c) + 裁定 4 末句"失败路径不许丢字节账"
    /// 的**唯一**自洽读法）。控制方已把这条批准为定义（修复轮裁定），用例
    /// `a_live_but_paced_peer_is_advanced_rather_than_misread_as_stalled` 钉的就是"活着的节拍
    /// 对端永远不该被判成卡住"。
    Stalled,
    /// 零进制的失败（重试上限该计的数）。三个 `FailKind` 分别对应 Task 8 的三种账，见那里。
    Failed(FailKind),
}

/// 窗口预算：`max(WINDOW_MIN, min(WINDOW_MAX, WINDOW_BYTES / rate * 2))`（plan 第 796 行）。
///
/// 式子里的除法是简报没写的第三个坑：`rate_bps` 是**测出来**的速率，第一轮还没有任何测量结果时
/// 调用方给 0 是最自然的写法 —— 照字面 `WINDOW_BYTES / 0` 就是一次 panic（debug 直接炸）。
/// 取法：把 0 读成"慢到需要最长预算"，而不是 crash，也不是"速率未知就别超时"（后者会关掉整条
/// `Stalled` 通道）。上下界照 plan：下界 `WINDOW_MIN_TIMEOUT_SECS`，上界 `WINDOW_MAX_TIMEOUT_SECS`。
pub fn window_budget(rate_bps: u64) -> Duration {
    let seconds = if rate_bps == 0 {
        WINDOW_MAX_TIMEOUT_SECS
    } else {
        WINDOW_BYTES.saturating_div(rate_bps).saturating_mul(2)
    };
    Duration::from_secs(seconds.clamp(WINDOW_MIN_TIMEOUT_SECS, WINDOW_MAX_TIMEOUT_SECS))
}

/// 这一枪发哪一条 `Range`（spec §2 表里"发 `Range: start-`"、§3 对 IDM 开放终点的观测）。
///
/// **E14②（戊）改过这一条规则**，判据里的"终点"是**这一枪的窗口**，不是区间的终点：
/// `window_end = min(iv.end, iv.start + WINDOW_BYTES - 1)`，只有 `window_end + 1 >= total_len`
/// 才发 `bytes={start}-`。
///
/// 为什么原来那条（`iv.end + 1 >= total_len`）不成立：它把"这个区间**伸到**文件末尾"读成
/// "这一枪**会读到**文件末尾"，于是对一个比一个窗口宽的尾巴区间发出**无界**的请求 ——
/// 对端有权把整条尾巴推过来，而 `run_window` 最多读 `WINDOW_BYTES` 就收手，多出来的那一截
/// 全留在连接上（阶段4 实测 8 MiB 文件真多打 4.1 MiB 的机制起点，勘误 **E14**）。
/// 窗口本来就填不满区间时，请求必须只问到窗口那一格：那一段我们注定不会读。
///
/// 保住的那半：**文件真正的那一尾**（窗口终点 == `total_len - 1`）仍然发开放终点 ——
/// IDM 观测到的就是 `bytes={start}-`（spec §3），两种写法对端交的是同一批字节，多写一个终点
/// 是对同一个意思的两种表述。
/// 越过全长（`scheduler` 由 `total_len` 派区间，本不该出现）按"到文件末尾为止"读，同上一条。
fn range_header(iv: Interval, total_len: u64) -> String {
    // `WINDOW_BYTES - 1`：`iv.start + WINDOW_BYTES` 是**不含**的窗口上界（下面 `stop` 用的就是它），
    // 这里要的是含端点的最后一格，所以减一。`saturating_add`：起点贴近 `u64::MAX` 时不许回绕。
    let window_end = iv.end.min(iv.start.saturating_add(WINDOW_BYTES - 1));
    if window_end.saturating_add(1) >= total_len {
        format!("bytes={}-", iv.start)
    } else {
        format!("bytes={}-{}", iv.start, window_end)
    }
}

/// `Content-Range` 里**本次交的是哪一段**的起点：`bytes 262144-393215/8388608` ⇒ `Some(262144)`。
///
/// 只看起点：终点由我们的窗口与 `stop` 管，而"这段字节对应文件哪个偏移"才是能不能按偏移落盘的判据。
/// `bytes */4194304`（416 那一条的写法，没有本次区间）、缺失、任何看不懂的形状 ⇒ `None`
/// = "这个 206 说不出自己的位置"。总长那一段复用 `probe::content_range_total`，不在这里解析第二遍。
fn content_range_start(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    let unit = trimmed.get(..5)?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let rest = trimmed.get(5..)?.trim();
    let span = rest.split('/').next()?;
    if span == "*" {
        return None;
    }
    span.split('-').next()?.trim().parse().ok()
}

/// 这个 `io::Error` 是不是"窗口预算到点"。
///
/// `Response::read` 的超时**不是** `reqwest::Error` 而是被包了一层的
/// `io::Error::new(ErrorKind::Other, reqwest::Error{Kind::Body ← Kind::Body ← TimedOut})`（本轮实测，
/// 见模块文档），所以只问 `kind()` 一定读成"别的传输错误"。两支都问：`ErrorKind::TimedOut`
/// （std 自己的形状）与 payload 里的 `reqwest::Error::is_timeout()`（reqwest 的实际形状；它顺着
/// source 链找 `TimedOut`，所以包裹层数变了照样认）。集成用例
/// `a_peer_that_answers_but_sends_no_body_is_stalled_not_a_transport_failure` 钉的是 payload 那一支
/// （变异 M4 摘掉下钻 ⇒ 那条红），单元用例
/// `a_plain_io_error_is_not_a_timeout_and_stays_a_transport_failure` 钉负向与 std 那一支。
fn is_read_timeout(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::TimedOut
        || err
            .get_ref()
            .and_then(|payload| payload.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout)
}

/// 拼出这一枪（照 `probe::build_request` 的形状先建出 `Request`，单元层就看得见头与预算）。
///
/// **只服务可分段的形状**：`GET` + 一条 `Range`。带请求体的任务由 `build_body_request` 那一条负责，
/// 而 `run_window` 在走到这里**之前**就把那种任务拒了（F1 的裁定）。
fn build_request(agent: &Client, task: &Task, range: &str, budget: Duration) -> Result<Request> {
    let req = agent.get(&task.url).header(RANGE, range).timeout(budget);
    probe::with_contract_headers(req, task)?
        .build()
        .context("窗口请求拼不出来（URL 不合法？）")
}

/// 打开 `--data-file` 那份请求体，包成 reqwest 的**流式** body。
///
/// 取法（修复轮要我说清楚的那件事）：**流式**，不把体读进内存。`reqwest::blocking::Body` 内部是
/// `Kind::Reader(Box<dyn Read>, Option<u64>)`（`src/blocking/body.rs:155-168` 与 `impl From<File>`），
/// `Body::from(File)` 把 `len` 取成 `metadata().len()`；发送侧由 reqwest 的驱动线程一块一块从这条
/// `File` 里读（`into_async` 交回一个 `Sender`），而那个 `len` 在**发送时**写进请求的
/// `Content-Length`（`blocking/request.rs:139-152`：`if let Some(len) = len { insert(CONTENT_LENGTH, ..) }`），
/// 所以**不需要**为了写出长度而 `read_to_end`。内存代价：本模块这一侧只有那一个 64 KiB 的响应缓冲；
/// 一个 2 GiB 的体不会在这里变成 2 GiB 的 `Vec`。
///
/// 注意那条 `Content-Length` 不在 `build()` 出来的 `Request` 上（`build()` 只看头表，CL 是
/// `into_async` 那一步补的），所以单元层断不了它 —— 归集成用例
/// `a_data_file_task_is_posted_with_the_file_as_its_body_and_lands_the_response_body`，
/// 那里读的是**对端收到的**那一条头（并且那个取证对端只认 `Content-Length`、见到
/// `Transfer-Encoding` 就当场红）。
///
/// 错误消息：**点名路径，一个体的字节都不抄**（spec §4 那条硬规约管的是凭据，而 POST 体里
/// 常常就躺着一个表单口令）。
///
/// **先问"这是不是一个普通文件"**（本轮 drive-by，派发点名的第二条；**派发给的理由在本机不成立，
/// 于是按实测改这段说明，而不是迁就派发**）：那句"Windows 上 `File::open` 对目录是成功的、错误要到
/// reqwest 的驱动线程读体时才冒出来"实测**不是**这个形状 —— 对目录 `File::open` 在打开那一步就失败
/// （`PermissionDenied` / `拒绝访问。 (os error 5)`；Windows 要求目录句柄带
/// `FILE_FLAG_BACKUP_SEMANTICS`，std 不带）。所以这道闸买到的不是"少一次上线"，而是**一句说对了
/// 的话**：旧写法把"你给的是目录"报成"拒绝访问"，而那条路径明明存在、明明可读，用户照着去查 ACL
/// 就白跑一趟。闸的位置照旧必须在打开**之前**（`File` 还没建出来，自然没有任何东西能上线）；判据
/// 用 `is_file()` 而不是 `!is_dir()`：设备、FIFO、命名管那些形状同样当不了请求体，而"不存在的路径"
/// 落在这一道闸上给出的也是一句对的话。`File::open` 那一条 `with_context` 留着，管的是"确实是文件
/// 却打不开"那种真·权限/被占用失败 —— 那里 os error 才是有用的信息。
fn open_request_body(path: &Path) -> Result<Body> {
    ensure!(
        path.is_file(),
        "读 --data-file 的请求体：{} 不是一个能当请求体的普通文件（目录、设备或不存在的\
         路径都不行；请把 --data-file 指向一个真文件）",
        path.display()
    );
    let file = File::open(path)
        .with_context(|| format!("读 --data-file 的请求体文件失败：{}", path.display()))?;
    Ok(Body::from(file))
}

/// 拼出整份那一枪：`POST` + 文件当体 + **一条 `Range` 都不带**（F1 的裁定）。
///
/// - 方法：`POST`。带体的任务在 spec §2.2 第一条下**不可分段**（`Task::segmentable()` 为假就是这件事），
///   而"下载的字节来自哪条请求"只能由发请求的这一层回答 —— 发 GET 就是把**另一条**请求的字节当成果子。
/// - 没有 `Range`：`Range` 在 POST 上不成立（发出去只会得到一个自相矛盾的请求，或被对端忽略后
///   把整份 body 当成"区间的第 0 字节"，正是裁定 6 那一支的错读法）。
/// - **没有** `Content-Type`：见 `run_whole` 的文档（这是留给 Task 9 的开放问题，不许在这里猜）。
/// - 头与口令闸与 `probe` **同源**（`with_contract_headers`），一枪里只有 `Range`/体/预算不同。
///
/// `budget` 照窗口那一支同一个式子（`window_budget(rate_bps)`），语义也同一个：**整请求期限**
/// （本轮实测，见模块文档）—— 一份大或慢的整份下载会被它掐在 body 中间，后果见 `run_whole` 的文档。
fn build_body_request(agent: &Client, task: &Task, body: Body, budget: Duration) -> Result<Request> {
    let req = agent.post(&task.url).body(body).timeout(budget);
    probe::with_contract_headers(req, task)?
        .build()
        .context("整份那一枪拼不出来（URL 不合法？）")
}

/// 已写进盘里的字节数（`cursor` 是本模块唯一的进度真相：它只被 `write_at` 的返回值推进）。
fn written(iv: Interval, cursor: u64) -> u64 {
    cursor.saturating_sub(iv.start)
}

/// 这一枪的**终点帽**（勘误 **E14**① 乙）：把 `Action::Retighten` 递进正阻塞在 `read()` 里的那一枪。
///
/// 装的是"这一枪最多还允许写到哪个**含端点**的偏移"，初值 `u64::MAX` = "外面还没说话"，
/// `run_window` 一进读循环前会把它自己的窗口终点也**降**进去（见 `lower_end_cap` 那条纪律）。
/// 引擎每个 worker 一格 `Arc<AtomicU64>`：派这一枪之前重置成 `u64::MAX`，
/// 主线程处理 `Action::Retighten{worker, iv}` 时把 `iv.end` 降进去，
/// worker 手上那一枪在每块（64 KiB）之前查一次 ⇒ 收紧终于有了兑现的通道
/// （阶段4 的记录是"收紧记下来但不发新的一枪，多写的字节全算重复"）。
///
/// 为什么是"帽"而不是"新请求"：在途那一枪正阻塞在 `resp.read()` 里，中间没有任何通道能递话给它，
/// 除了它自己每读一块时看一眼的这一个格子。再发一枪要么排在当前这枪之后（起点还是 `iv.start`
/// ⇒ 把已写的前缀重下一遍），要么并发第二条连接（`workers` 的意义就变了）。
pub type EndCap = AtomicU64;

/// "外面没有帽"的那个值：`run_window` 只会把自己的窗口终点降进去，读满一个窗口照旧。
pub const NO_END_CAP: u64 = u64::MAX;

/// 把终点帽**往下**调（勘误 **E14**① 的原话是 `fetch_max`；能让"只降不升"成立的原语是
/// `fetch_min` —— `fetch_max` 是单调**升**，按字面写会把那条性质反过来，详见下面与模块文档）。
///
/// 要保住的那条性质：**一个来路不明的、更大的值，绝不允许把已经收紧的枪重新放宽**。
/// 可达的形状是"上一枪的帽还挂在格子上的同时，主线程给这一枪写了个更大的终点"——
/// 更直接的形状是并发：主线程刚把帽降到 `A`，另一个写入者（同一格子的另一方）写进 `B > A`。
/// 用 `fetch_min` 时 `B` 等于没写；用 `fetch_max` 时 `B` 生效 ⇒ 那一枪又读回宽区间。
/// 所以这里按**性质**落地，而不是按 E14 括号里那个原语名落地（等价写法是对
/// `u64::MAX - end` 用 `fetch_max`，读起来更绕，不取）。
/// 钉住的用例：`end_cap_lowering_is_monotonic_downward_only`（单元，两侧都钉）。
pub fn lower_end_cap(cap: &EndCap, new_end: u64) {
    cap.fetch_min(new_end, Ordering::AcqRel);
}

/// 读循环每一块只问一句：**这个终点帽还让不让继续写**（勘误 **E14**① 的收手判据）。
///
/// `cursor` 是"下一个要写的绝对偏移"（exclusive），`cap_end` 是"允许写的最后一个绝对偏移"（含端点）。
/// 判据是 `cursor > cap_end` 才停 ⇒ 越过帽的那一块是**整块**落的（帽之外最多一块 `CHUNK_BYTES`），
/// 这正是 `Report::duplicated` 那一笔的上界形状。刻意**不**把那一块切到帽为止：切法要新增一条
/// "半块写了、半块丢弃"的账，而 `write_at` 的字节账会从"读了多少写多少"变成两种写法各说一遍。
///
/// 那个 `max(shot_start)`：一个**比这一枪起点还低**的帽（只能来自陈旧格子，引擎重置后不可达）
/// 若照字面拦，会让这一枪一个字节都不写就交回 `Advanced { bytes: 0 }` —— 零字节的 `Advanced`
/// 在 Task 8 那里谁也数不到（`Progress` 给不出、重试封顶又不计它），正是 M9 那条纪律要拦的形状。
/// 所以起点以下不算话：**至少写一块，然后收手**。
fn cap_allows(cap_end: u64, cursor: u64, shot_start: u64) -> bool {
    cursor <= cap_end.max(shot_start)
}

/// 领一个字节区间，最多下**一个窗口**，把字节按绝对偏移交给 `store`。
///
/// `agent` 由调用方持有并复用（连接池才有意义）；`rate_bps` 只用来定窗口预算；
/// `total_len` 是 `ProbeFacts` 那份文件长度（裁定 1：末段要不要发开放终点必须由它决定，
/// 也用来判 416 的分母）。
///
/// `cap` 是这一枪的**终点帽**（[`EndCap`]，勘误 **E14**①）：读循环每块查一次，
/// `cursor` 一旦越过帽就**停手**，交回的还是已有的 `Advanced { bytes }`（不是新的结论形状）。
/// 传 `&AtomicU64::new(NO_END_CAP)` 就是"没有外面的帽"，行为与 E14 之前逐字节相同
/// —— `run_window` 自己那一格的窗口终点仍然会降进去，所以这一参数**不需要**调用方先填对。
/// 为什么放在最后一位而不是包进某个 `Options`：签名已经有六个参数，而这一个是唯一会变的。
/// **勘误 E12① 那条"签名多了 `total_len`"以 E14 为准再加一个。**
pub fn run_window(
    agent: &Client,
    task: &Task,
    iv: Interval,
    store: &Store,
    rate_bps: u64,
    total_len: u64,
    cap: &EndCap,
) -> Result<Outcome> {
    // 与 `probe::run` / `state::Snapshot::from_task` 同一个立场：规则只住在 `Task::validate()` 一处，
    // 但**发请求**这一刻是第二个强制点。明文 HTTP + 口令的任务到不了网络（用例
    // `the_password_gate_is_enforced_before_fetch_touches_the_wire` 用服务端日志钉"拒在上网之前"）。
    task.validate()?;

    // **F1**（修复轮的裁定）：带请求体的任务**不许**从窗口这条路走。
    // 这一支发的是 `GET` + `Range`，而那种任务的字节**只有**在 POST 的响应里才有意义 —— 照今天这样
    // 收下它，落盘的就是"一条我们从未发过的请求"的产物，与 Task 6 评审 F1 拒绝"对带体任务做 GET
    // 试探"是同一条理由（RED 阶段的取证：对端收到 `method=GET range=bytes=0-16383 body=""`，
    // 而盘上多出 25 个字节）。
    // 修法**不是**在这里补一条 POST：那种形状不分段（spec §2.2 第一条），窗口跳的那一枪等于把同一份
    // 表单**再交一次**（下单/投票/发信都是真副作用，Task 6 的裁定明确否决过）。所以当场拒，
    // 把调用方指到 `run_whole`。错误点名参数与入口，一个体的字节都不抄（spec §4）。
    ensure!(
        task.segmentable(),
        "带 --data-file 的任务不能走窗口那条路（`run_window` 只发 GET + Range）：\
         那种形状请调 `fetch::run_whole`（POST、不带 Range、事实取自这一枪）"
    );

    // 空区间 = 真的没活儿：不打任何一枪就交 `Complete`（Task 8 折成 `SegmentComplete` 是安全的）。
    if iv.is_empty() {
        return Ok(Outcome::Complete);
    }

    let range = range_header(iv, total_len);
    let req = build_request(agent, task, &range, window_budget(rate_bps))?;
    let mut resp = match agent.execute(req) {
        Ok(resp) => resp,
        Err(e) => {
            // 重定向**中途**失败时，带凭据的落地 URL 只出现在 reqwest 的错误里 ⇒ 同一个闸、同一句话
            // （`probe::LANDED_CREDENTIALS`，Task 5 评审 C3；与 `probe::run` 的错误支完全同形）。
            if e.url().is_some_and(|u| authority_has_userinfo(u.as_str())) {
                bail!("{}", probe::LANDED_CREDENTIALS);
            }
            // 这一支还没开始读 body ⇒ 必然零进度（`is_timeout` 在这里直接可用：`execute` 交回的是
            // `reqwest::Error`，不是被包了一层的 `io::Error`）。原因取自那一枪自己的错误（E12⑦）。
            return Ok(if e.is_timeout() {
                Outcome::Stalled
            } else {
                Outcome::Failed(FailKind::Transport(transport_note(&e)))
            });
        }
    };
    if authority_has_userinfo(resp.url().as_str()) {
        bail!("{}", probe::LANDED_CREDENTIALS);
    }

    let status = resp.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        // 裁定 2：**不是**"看到 416 就完事"。分母与我们这份 `total_len` 不一致（或取不到）就说明
        // 服务端长度在我们脚下变了 ⇒ `ValidatorChanged`，让重试上限介入（E7 的 2026-10-16 追加段）。
        let reported = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(probe::content_range_total);
        return Ok(if reported == Some(total_len) {
            Outcome::Complete
        } else {
            Outcome::Failed(FailKind::ValidatorChanged)
        });
    }
    // 裁定 6 的触发条件：我们**发了** `Range`，而对端答 `200` ⇒ body 的第 0 字节对应文件偏移 0，
    // 不是 `iv.start` ⇒ 先流掉 `iv.start` 个字节。
    // "本模块每一枪都发 Range"这个前提**现在由上面那道 F1 闸守着**（带体的任务进不了这条路，
    // 而 `run_whole` 是这一模块里唯一不发 Range 的入口，它也没有 `iv.start` 可跳）。
    // 前提一旦被拆掉（比如将来给这条路加上别的 body 形状），这一支必须改成 `sent_range && 200`，
    // 否则会把"我们本来就该拿整个 body"的合法 200 也跳掉 `iv.start` 个字节。
    let degraded = status == StatusCode::OK;
    if !degraded {
        if status != StatusCode::PARTIAL_CONTENT {
            return Ok(Outcome::Failed(FailKind::Http(status.as_u16())));
        }
        // 206 得说得出它交的是哪一段，否则偏移核对不了 ⇒ 一个字节都不写（模块文档"另外三条"第一条）。
        let asked_at = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_start);
        if asked_at != Some(iv.start) {
            return Ok(Outcome::Failed(FailKind::ValidatorChanged));
        }
    }

    // 窗口上界（**exclusive**）：区间终点与"至多 `WINDOW_BYTES` 个字节"里更近的那个（裁定 5）。
    // `bytes` 与 `store.len()` 都由这一个 `stop` 算出来 ⇒ 任何一处差一字节，两条断言一起红。
    let stop = iv.end.saturating_add(1).min(iv.start.saturating_add(WINDOW_BYTES));
    // **终点帽**（**E14**①）：先把这一枪自己的窗口终点降进去（`stop` 是 exclusive，帽是**含端点**的，
    // 所以是 `stop - 1`；`stop > iv.start >= 0` 恒成立 ⇒ 这里不会下溢）。
    // 之后读循环每块只看这一个格子：外面（引擎的 `Retighten`）降过就更早收手，没降过就照旧读满窗口。
    lower_end_cap(cap, stop - 1);
    let mut cursor = iv.start;
    let mut skip = if degraded { iv.start } else { 0 };
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut eof = false;
    let mut read_err: Option<std::io::Error> = None;
    while cursor < stop {
        // 每块**只查一次**帽：不 sleep、不另起线程读 socket、不引 `tokio`、不装轮询定时器（E14① 的形状约束）。
        // 判据与它的两条边界都住在 `cap_allows`（单元层用表钉死，不必真打一条 socket）。
        if !cap_allows(cap.load(Ordering::Acquire), cursor, iv.start) {
            break; // 收紧赶上了这一枪：剩下的尾巴由调度器重新决定给谁（`Advanced{bytes}` 已落盘的那些算数）
        }
        let n = match resp.read(&mut buf) {
            Ok(0) => {
                eof = true;
                break;
            }
            Ok(n) => n,
            Err(e) => {
                read_err = Some(e);
                break;
            }
        };
        let mut slice = &buf[..n];
        if skip > 0 {
            // **流式**丢弃：切掉这一块的前 `skip` 字节，剩下的照常落盘，剩下的 `skip` 由下一块承担。
            // 这里没有、也不许有一个 `Vec::with_capacity(iv.start)` —— 那等于把整个 body 吸进内存。
            let dropped = usize::try_from(skip.min(slice.len() as u64)).expect("不大于 slice 长度");
            skip -= dropped as u64;
            slice = &slice[dropped..];
            if slice.is_empty() {
                continue; // 这一块整块都是要丢的前导，一个字节都没写
            }
        }
        let take = slice.len().min((stop - cursor) as usize);
        // `write_at` 失败 ⇒ **往上抛**（T3 裁定 ①）：那一块里真有几个字节落了盘**无从知道**
        // （`write_all` 可能部分成功后才报错），把它记进 `Advanced` 就是让 `Progress` 说瞎话。
        // 已确认的字节仍在盘上，而盘上的真相归 `store.len()`（E7/E8），不归本函数的返回值。
        let wrote = store
            .write_at(cursor, &slice[..take])
            .with_context(|| format!("把窗口的字节写到偏移 {cursor} 失败"))?;
        // `saturating_add`（本轮 drive-by）：`cursor` 是本模块唯一的进度真相，它不许在加法上回绕。
        // 今天 `wrote <= take <= CHUNK_BYTES` 远不到 `u64` 的上界，可一旦哪里算错了，debug 构建会
        // panic（release 构建会**绕回 0**，然后从文件开头重写下整段）。这一条不改变任何可达行为。
        cursor = cursor.saturating_add(wrote as u64);
    }

    let bytes = written(iv, cursor);
    if let Some(err) = read_err {
        // 裁定 4：写下去过的字节都是真进度；只有零进度那两支才让重试上限去计。
        // 零进度那一支带着原因（E12⑦）：`err` 就是这一次读到的那个 `io::Error`（读侧超时的形状
        // 见模块文档；不是超时才走到 `Transport`，所以这里不需要 `is_read_timeout` 之外的判断）。
        return Ok(match (bytes, is_read_timeout(&err)) {
            (0, true) => Outcome::Stalled,
            (0, false) => Outcome::Failed(FailKind::Transport(transport_note(&err))),
            _ => Outcome::Advanced { bytes },
        });
    }
    if eof {
        // body 比区间短（`/drop`、`/sz/<小n>`、`/noranges` 交完 1 MiB 就结束）：绝不是 `Complete`
        // ——那会把这段的尾巴**永远封存**。一个字节都没落到盘上才是失败。
        return Ok(if bytes == 0 {
            // 这一支**没有**错误对象可抄：`read` 给的是 `Ok(0)`（对端正常收尾），失败在"区间要字节、
            // 对端一份不给"这个事实里 ⇒ 原因取自 [`WINDOW_EOF_WITHOUT_BYTES`]，但仍走
            // [`transport_note`] 那一个构造器（一行 / 截断 / 不重复的规则不许有第二套）。
            Outcome::Failed(FailKind::Transport(transport_note(&std::io::Error::other(
                WINDOW_EOF_WITHOUT_BYTES,
            ))))
        } else {
            Outcome::Advanced { bytes }
        });
    }
    // 走到这里是"自己收手"（`cursor >= stop`）：区间读完 ⇒ `Complete`；窗口用满 ⇒ `Advanced`。
    //
    // **`>` 而不是 `>=`**（评审 B）：`iv.end` 是**含端点**的，所以 `Complete` 承诺的是
    // "`iv.end` 那个字节也落了盘"，而 `cursor` 走到 `iv.end + 1` 才叫兑现了这个承诺。区间长度
    // `WINDOW_BYTES + 1` 时 `stop = iv.start + WINDOW_BYTES = iv.end` ⇒ `cursor` 最多**正好等于**
    // `iv.end`，那一个字节还没到 ⇒ 只能 `Advanced`。写成 `>=` 会把一个从没落盘的尾巴封成
    // `SegmentComplete`（E7/E9：封存不看盘）。钉它的用例：`an_interval_of_exactly_one_window_is_complete`
    // （左半边，含端点那个字节读得出来）与 `an_interval_one_byte_wider_than_the_window_is_never_complete`
    // （右半边，含端点那个字节**读不出来**）。
    Ok(if cursor > iv.end {
        Outcome::Complete
    } else {
        Outcome::Advanced { bytes }
    })
}

/// **整份那一枪**：`--data-file` 的形状（POST + 文件当体 + 不带 `Range`）—— 一发请求、流到 EOF、
/// 顺带把 `ProbeFacts` 从**这一枪自己的响应头**里收割出来。
///
/// ## 为什么要有这一个入口（F1 + F2 两条裁定合成它）
///
/// 带请求体的任务**不能试探**：`probe::run` 在打出任何一枪之前硬拒（Task 6 评审 **F1**），理由是
/// "GET 试探确认的是另一条（无体）请求的字节"，而"试探也发 POST"要把同一份表单交两次。于是那种
/// 形状的长度/验证子/落地 URL 只能取自**真正下载的那一枪** —— 而那一枪是 `fetch` 发的。
/// 所以本函数把两件事一次做完：发 POST（F1），并用 `probe::facts_from_response` 这**同一个**读法
/// 收割事实（F2；不许有第二份"响应头 → 事实"的实现）。
///
/// ## Task 8 不许拿这一支去做的事：**续传这个形状**
///
/// 交回的那份 `ProbeFacts` 与这份字节是**同一条请求的两个产物**，不是一把能和旧事实比对的独立验证子。
/// 任何"续传前先比对一下"的写法在这种形状上都意味着**再 POST 一次**：
/// - 不许调 `probe::run`（它硬拒，Task 6 的 `ensure!`）；
/// - 不许把旧断点里的 `total_len`/`ETag` 与这一支收割的比完之后只续尾巴：`resume` 的 ①–⑤ 判的是
///   "另一条请求确认过的字节"与"这一条请求"是不是同一个，而这里两边是同一条；
/// - 中途失败之后**重新调 `run_whole` 就是把表单重交一次**（`Outcome::Advanced` 那一支只能当成
///   "这一份文档没拿全"来报，不能当成"从 `store.len()` 接着跳窗口"）；
/// - 断点文件对这种形状唯一站得住的读法是"重来"（`--data-file` 的任务 `resume` 走 `Fresh`，
///   并把原因说清楚）。
///
/// ## 与 `run_window` 的分工
///
/// | | `run_window` | `run_whole` |
/// |---|---|---|
/// | 方法 / `Range` | `GET` + 一条 `Range` | `POST` + 体，**没有** `Range` |
/// | 一次领多少 | 至多 `WINDOW_BYTES`（读满就收手） | 到 EOF 为止（不分窗口） |
/// | 落盘起点 | `iv.start` | `0` |
/// | 事实 | 由调用方从 probe 拿来（`total_len` 是入参） | 由本函数交回 |
/// | 能续吗 | 能（窗口是无副作用的 GET） | **不能**（见上面那一节） |
///
/// `Content-Type`：**不发**。任务形状里没有任何字段 supplying 它（`Task` 只有 `data_file` 这条路径，
/// spec §5 的 `--data-file F` 说的是"裸体文件"，表单还是裸体、怎么编码都没定），而猜一个
/// `application/octet-stream`（或 `application/x-www-form-urlencoded`）会**悄悄改掉服务器怎么处理这份请求**
/// —— 那是用户看得见的行为差别，不是我们这层能替他做的决定。已登记为**留给 Task 9（CLI）的开放问题**：
/// 要么加一个 `--content-type`，要么由 CLI 按文件后缀给一个并写进帮助文本。
///
/// 三处限制照实说：
/// - `agent.execute(..)` 走的是**阻塞**层：它会等对端把响应头交出来，但**不等**请求体送完。一份比
///   发送缓冲大得多的体（例如几 GiB）在这里仍然占着对端的读，直到我们收到响应头 —— 那是 reqwest 的
///   形状（`into_async` 把 `File` 交给驱动线程分块读），不是本函数新加的阻塞，也不改变内存代价。
/// - 对端**说不出长度**（chunked 且没有 `Content-Length`/`Content-Range`）时报错、不下：这一支与
///   `probe` 同一立场（"不猜一个数出来"）。曾考虑的另一种读法是把请求体的长度当成 `total_len` ——
///   那需要改 `facts_from_response`（本轮的授权是"只抽出、行为不变"），而且把一个**响应**事实的来源
///   换成了请求，本文件不干。
/// - **预算是整请求期限**（见模块文档那条本轮实测）：一份大或慢的整份下载**几乎必然**以
///   `Advanced{bytes}` 收场而不是 `Complete`（15–120 s 到点，body 还没完）。那不是可以"再来一枪"
///   的形状 —— 再来一枪就是把表单重交一次。Task 8 只能把它报成"这一份没拿全"并说明原因；
///   想要真正拿全，需要一个比 `window_budget` 更宽的期限（登记给 Task 8/9 的开放问题）。
pub fn run_whole(
    agent: &Client,
    task: &Task,
    store: &Store,
    rate_bps: u64,
) -> Result<(Outcome, Option<ProbeFacts>)> {
    // 与 `run_window` 同一个第二强制点：口令闸 + E9 的 userinfo 闸都住在 `Task::validate()`。
    task.validate()?;

    // 本入口**只**服务带请求体的形状。判据复用 `Task::segmentable()`（与 spec §2.2 第一条、
    // 与 `run_window` 那道闸同一个函数）：反过来放行就等于让一条可分段的任务绕过窗口化那条路。
    let body_path = task.data_file.as_deref().with_context(
        || "fetch::run_whole 只服务带 --data-file 的形状（POST、不分段、事实取自这一枪）；\
           其余形状请调 fetch::run_window".to_string(),
    )?;
    let body = open_request_body(body_path)?;

    let req = build_body_request(agent, task, body, window_budget(rate_bps))?;
    let mut resp = match agent.execute(req) {
        Ok(resp) => resp,
        Err(e) => {
            // 重定向**中途**失败：带凭据的落地 URL 只出现在 reqwest 的错误里 ⇒ 同一个闸、同一句话
            // （`probe::LANDED_CREDENTIALS`，Task 5 评审 C3）。这一支还没看到响应 ⇒ 没有事实可交。
            if e.url().is_some_and(|u| authority_has_userinfo(u.as_str())) {
                bail!("{}", probe::LANDED_CREDENTIALS);
            }
            return Ok((
                if e.is_timeout() {
                    Outcome::Stalled
                } else {
                    // 与 `run_window` 那**同一个**构造器（E12⑦）：两条入口的措辞不许分岔。
                    Outcome::Failed(FailKind::Transport(transport_note(&e)))
                },
                None,
            ));
        }
    };
    // **E9 那道闸在这一支同样成立**：`run_whole` 交回的 `ProbeFacts.final_url` 就是这条落地 URL，
    // 它会被 `state` 落盘、被 `resume` 的 ② 写进原因串。判据与措辞都不是第二个版本
    // （`task::authority_has_userinfo` + `probe::LANDED_CREDENTIALS`，与 `probe::run`、`run_window` 同源）。
    let final_url = resp.url().to_string();
    if authority_has_userinfo(&final_url) {
        bail!("{}", probe::LANDED_CREDENTIALS);
    }

    let status = resp.status();
    if !status.is_success() {
        // 非 2xx 是"服务器有说法"，折成 `Failed(Http)` 让 Task 8 记账；这一支**没有**长度/验证子
        // 可交，所以事实是 `None` 而不是"拿 4xx 的头硬凑一份"。
        return Ok((Outcome::Failed(FailKind::Http(status.as_u16())), None));
    }
    let headers = resp.headers().clone();
    // F2 的落点：**与 `probe` 同一个函数**把这五个字段读出来（`asked_for_range = false` —— 这一支
    // 一条 Range 都没发，所以 `accept_ranges` 只能看头说，这与 `prelude_max == 0` 时 probe 的读法一致）。
    let facts = probe::facts_from_response(status, false, &headers, &final_url)?;

    let mut cursor = 0u64;
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut read_err: Option<std::io::Error> = None;
    loop {
        match resp.read(&mut buf) {
            Ok(0) => break, // 干净 EOF：框架长度用尽（见下面那条 `bytes != total_len`）
            Ok(n) => {
                // 偏移 0 起、按读到多少写多少：本函数没有 `iv`，也没有窗口上界。
                // `write_at` 失败照 `run_window` 同一立场**往上抛**（半截写不许当进度）。
                let wrote = store
                    .write_at(cursor, &buf[..n])
                    .with_context(|| format!("把整份的字节写到偏移 {cursor} 失败"))?;
                // `saturating_add`：与 `run_window` 那一侧同一个处理（进度真相不许在加法上回绕）。
                cursor = cursor.saturating_add(wrote as u64);
            }
            Err(e) => {
                read_err = Some(e);
                break;
            }
        }
    }
    let bytes = cursor;
    if let Some(err) = read_err {
        // 与 `run_window` 同一支判法：写下去过的都是真进度，只有零进度才让重试上限去计
        // （零进度那一支的原因取自同一个 [`transport_note`]，措辞不会分岔）。
        return Ok((
            match (bytes, is_read_timeout(&err)) {
                (0, true) => Outcome::Stalled,
                (0, false) => Outcome::Failed(FailKind::Transport(transport_note(&err))),
                _ => Outcome::Advanced { bytes },
            },
            Some(facts),
        ));
    }
    // 走到这里 = 干净 EOF。
    if bytes == 0 && facts.total_len > 0 {
        // 对端声明了长度却一个字节都不给：零进度必须是**可数的失败**，不能是 `Advanced { bytes: 0 }`
        // （Task 8 拿到 0 字节进度 ⇒ 快速/慢速封顶一个也数不到 ⇒ 无限循环）。变异 M9 的同一条纪律。
        // 原因取自 [`WHOLE_EMPTY_DOCUMENT`]（这一支同样没有错误对象：`read` 给的是 `Ok(0)`）。
        // 可达性与下面那条 `bytes != facts.total_len` 同一地位 —— 代码检查成立、本仓库的夹具造不出
        // 那个形状，所以这一支**没有**用例（登记在报告里，不当成已验证性质）。
        return Ok((
            Outcome::Failed(FailKind::Transport(transport_note(&std::io::Error::other(
                WHOLE_EMPTY_DOCUMENT,
            )))),
            Some(facts),
        ));
    }
    if bytes != facts.total_len {
        // 干净 EOF 却没读完对端自己声明的长度 ⇒ 这个结论**不许**叫 `Complete`
        // （`Complete` 在 Task 8 那里折成"这段收尾了"，会把尾巴永远封存）。正常 HTTP/1.1 框架下走不到
        // 这一支：`Content-Length` 用尽才给 EOF，body 比它短会被 reqwest 报成读错误（上面那一支）。
        // 到得了这里的只有"200 带着一个比 body 大的 `Content-Range` 分母"那种骗子对端
        // （`total_len` 优先读分母，与 `probe` 同一读法）。**这条防御分支与 `write_at` 的 `Err`
        // 同一地位：代码检查成立、本仓库的夹具造不出那个形状**，没有用例，也不为它加注入口。
        return Ok((Outcome::Advanced { bytes }, Some(facts)));
    }
    Ok((Outcome::Complete, Some(facts)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{WINDOW_MAX_TIMEOUT_SECS, WINDOW_MIN_TIMEOUT_SECS};
    use reqwest::header::{ACCEPT, ACCEPT_ENCODING};

    fn task(url: &str) -> Task {
        Task {
            url: url.into(),
            referer: None,
            cookies: None,
            data_file: None,
            user: None,
            password: None,
            dir: ".".into(),
            name: None,
            flags: 0,
        }
    }

    // ---- 裁定 1 + spec §3 + **E14②（戊）**：开放终点的判据 ----

    #[test]
    fn only_the_interval_that_reaches_the_end_of_the_file_is_open_ended() {
        // 末段（`iv.end == total_len - 1`）**并且窗口够得着 EOF** ⇒ 开放终点。这是 spec §3 观测到
        // IDM 在发的那一条。**E14②（戊）动的就是这一格的夹具**：原来这一格喂的是
        // `start = 1_048_576`（区间伸到 EOF、但一个窗口填不满它），旧判据据此发 `bytes=1048576-`，
        // 等于授权对端把 47 MiB 一路流过来；戊之后那一格改由
        // `an_open_ended_request_is_for_the_window_that_reaches_eof_not_the_interval` 断"问窗口那一格"，
        // 这里把夹具换成真正的那一尾（起点 = 全长 - 一个窗口），**判据方向一字未动**：末段仍是开放终点。
        assert_eq!(
            range_header(Interval { start: 48 * 1024 * 1024 - WINDOW_BYTES, end: 48 * 1024 * 1024 - 1 }, 48 * 1024 * 1024),
            format!("bytes={}-", 48 * 1024 * 1024 - WINDOW_BYTES)
        );
        // 整份文件（起点 0、终点是最后一个字节）也算末段 ⇒ 同样是开放终点，不是 `bytes=0-<全长-1>`。
        assert_eq!(range_header(Interval { start: 0, end: 9 }, 10), "bytes=0-");
        // 终点在文件中间 ⇒ 必须带显式终点（少带就是对端多送一整个窗口）。
        assert_eq!(range_header(Interval { start: 0, end: 8 }, 10), "bytes=0-8");
        assert_eq!(range_header(Interval { start: 4, end: 8 }, 10), "bytes=4-8");
        // 单字节区间不是末段（终点不是最后一个字节）。
        assert_eq!(range_header(Interval { start: 4, end: 4 }, 10), "bytes=4-4");
        // 越过全长（`scheduler` 由 `total_len` 派区间，本不该出现；出现时按"到文件末尾为止"读）。
        assert_eq!(range_header(Interval { start: 4, end: 20 }, 10), "bytes=4-");
        // 全长 0：`total_len - 1` 会下溢（debug 直接 panic），`saturating_add` 那一支把它读成开放终点。
        // 这条形状到不了网络（区间必是空的，`run_window` 先交 `Complete`），但判据不许靠下溢活着。
        assert_eq!(range_header(Interval { start: 0, end: 0 }, 0), "bytes=0-");
    }

    /// **E14②（戊）**：判据里的"终点"是**窗口**，不是区间。这条用例是本轮唯一被改掉判据值的既有形状
    /// （原来断的是 `bytes=1048576-`）：区间 `[1 MiB, EOF]` 比一个窗口宽，`bytes=1048576-` 等于授权
    /// 对端把 47 MiB 一路推过来，而我们最多读 `WINDOW_BYTES` 就收手 ⇒ 那一截是真浪费
    /// （阶段4 实测 8 MiB 文件多打 4.1 MiB 的起点机制）。现在问的是**窗口那一格**。
    /// 另一半（真尾巴仍发开放终点）由上面那条的第一格与下面这组守住 —— 两条合起来才叫"没丢 IDM 的观察"。
    #[test]
    fn an_open_ended_request_is_for_the_window_that_reaches_eof_not_the_interval() {
        // 窗口正好落在 EOF：起点 = 全长 - WINDOW ⇒ `window_end == total_len - 1` ⇒ 开放终点。
        let total = 48 * 1024 * 1024;
        assert_eq!(
            range_header(Interval { start: total - WINDOW_BYTES, end: total - 1 }, total),
            format!("bytes={}-", total - WINDOW_BYTES)
        );
        // 窗口越过 EOF（剩下的比一个窗口少）⇒ 仍然开放终点，不许写成 `bytes=start-<全长-1>`。
        assert_eq!(
            range_header(Interval { start: total - 4096, end: total - 1 }, total),
            format!("bytes={}-", total - 4096)
        );
        // 区间伸到 EOF、但**窗口填不满它** ⇒ 问的是窗口那一格，不是 EOF（这就是 E14② 改的那一格）。
        assert_eq!(
            range_header(Interval { start: 1_048_576, end: total - 1 }, total),
            format!("bytes={}-{}", 1_048_576, 1_048_576 + WINDOW_BYTES - 1)
        );
        // 阶段4 那条惹祸的派单形状：8 MiB 文件的第一段 `[236032, 8388607]`。
        // 旧判据（`iv.end + 1 >= total_len`）在这里发 `bytes=236032-` ⇒ 对端有权流 7.75 MiB，
        // 而调度器同期把同一段收了四次 ⇒ 收手之后的每一字节都由别人重下。
        assert_eq!(
            range_header(Interval { start: 236_032, end: 8_388_607 }, 8_388_608),
            format!("bytes=236032-{}", 236_032 + WINDOW_BYTES - 1)
        );
        // 区间比窗口窄 ⇒ 窗口终点就是区间终点 ⇒ 与 E14 之前一模一样（戊只动"比窗口宽"那一支）。
        assert_eq!(
            range_header(Interval { start: 236_032, end: 236_032 + 65_536 - 1 }, 8_388_608),
            format!("bytes=236032-{}", 236_032 + 65_536 - 1)
        );
        // 起点与全长都贴着 `u64` 上界：加法不许回绕（回绕了就会把"问一个字节"发成开放终点）。
        assert_eq!(
            range_header(Interval { start: u64::MAX - 1, end: u64::MAX }, u64::MAX),
            "bytes=18446744073709551614-"
        );
    }

    // ---- 终点帽（E14① 乙）：只有往下才说话 ----

    #[test]
    fn end_cap_lowering_is_monotonic_downward_only() {
        // 要钉的性质：一个**更大**的值绝不允许把已经收紧的枪重新放宽。
        // 按 E14 括号里那个字面（`fetch_max`）写，第二行就会把帽抬回 4_000_000 ⇒ 这一格当场红
        // （抬回去之后那一枪会一路读到原来的窗口终点，重复下载又回来了）。
        let cap = AtomicU64::new(NO_END_CAP);
        lower_end_cap(&cap, 1_000_000);
        assert_eq!(cap.load(Ordering::Acquire), 1_000_000, "降下去的帽就是帽");
        lower_end_cap(&cap, 4_000_000);
        assert_eq!(cap.load(Ordering::Acquire), 1_000_000, "更大的值当没看见：帽只降不升");
        lower_end_cap(&cap, 900_000);
        assert_eq!(cap.load(Ordering::Acquire), 900_000, "再降还是算话：收紧是单调的，不是一次的");
        // 降到 `u64::MAX`（"没有帽"那个值）当然也不算话 —— 引擎重置格子走 `store`，不走这条路，
        // 这条断言钉的是"如果谁误把 NO_END_CAP 当成一次收紧，它不会把已经收紧的枪放宽"。
        lower_end_cap(&cap, NO_END_CAP);
        assert_eq!(cap.load(Ordering::Acquire), 900_000, "重置必须用 store：用这条路就是自相矛盾");
        // 起点那一格：0 是合法的含端点终点（文件第一个字节），必须降得下去。
        let first = AtomicU64::new(NO_END_CAP);
        lower_end_cap(&first, 0);
        assert_eq!(first.load(Ordering::Acquire), 0);
    }

    /// 收手判据本身（`cap_allows` 就是读循环里那句，不是它的复述）：
    /// 帽之外**最多一块**、块首查一次、起点以下的陈旧帽不算话。
    /// 真 socket 上"读到一半被帽掐住"归集成用例
    /// `a_shot_stops_at_the_end_cap_lowered_mid_body_and_keeps_what_it_wrote`。
    #[test]
    fn the_end_cap_stops_the_shot_after_at_most_one_more_chunk() {
        let chunk = CHUNK_BYTES as u64;
        // ① 恰好落在帽上：`cursor == cap_end` 还要写（那一格是允许写的最后一格）。
        assert!(cap_allows(9 * chunk, 9 * chunk, 0));
        // ② 越过去一格就收手 ⇒ 落点最多 `cap_end + 1`，而 `cap_end + 1` 是"整块写完"才到的，
        //    所以帽之外的字节数 ≤ 一块 —— 这一条就是 ③ 那笔账的上界形状。
        assert!(!cap_allows(9 * chunk, 10 * chunk, 0));
        // ③ 帽在两块之间：跨过它的那一块照样**整块**落（不切半块 ⇒ 少一个"半块算谁的"的账）。
        let cap_mid = 3 * chunk + chunk / 2;
        assert!(cap_allows(cap_mid, 3 * chunk + chunk / 2, 0), "块首查：正好到帽这一格还要读下一块");
        assert!(!cap_allows(cap_mid, 4 * chunk + chunk / 2, 0), "过了帽半块就已经该收手");
        // ④ 没有帽（初值那个 `u64::MAX`）：任何偏移都放行 ⇒ 读满窗口照旧。
        assert!(cap_allows(NO_END_CAP, u64::MAX - 1, 0), "NO_END_CAP 必须是真·不拦");
        // ⑤ 陈旧帽（低于这一枪的起点）：起点那一格仍然放行 ⇒ 至少写一块再收手，
        //    绝不交回 `Advanced { bytes: 0 }`（M9 那条纪律：零字节的 Advanced 谁也数不到）。
        assert!(cap_allows(100, 4_096, 4_096), "帽低于起点 ⇒ 拦不出零字节：起点这一格必须放行");
        assert!(!cap_allows(100, 4_097, 4_096), "写过起点一格之后就轮到帽说话");
    }

    // ---- 窗口预算（plan 第 796 行的式子 + 简报没写的除零） ----

    #[test]
    fn the_window_budget_clamps_at_both_ends_of_the_formula() {
        // 中间那支：`WINDOW_BYTES / rate * 2`。速率 4 MiB/s ⇒ 一个窗口正好 1 s ⇒ 预算 2 s，
        // 但下界是 15 s ⇒ 取 15。把速率换成 1 MiB/s ⇒ 8 s，仍被下界夹住。
        assert_eq!(window_budget(WINDOW_BYTES), Duration::from_secs(WINDOW_MIN_TIMEOUT_SECS));
        assert_eq!(window_budget(1_048_576), Duration::from_secs(WINDOW_MIN_TIMEOUT_SECS));
        // 慢速：128 KiB/s ⇒ 4 MiB / 128 KiB * 2 = 64 s ⇒ 在界内，照式子给。
        assert_eq!(window_budget(131_072), Duration::from_secs(64));
        // 更慢：32 KiB/s ⇒ 256 s ⇒ 被上界 120 s 夹住（不许一个窗口挂四分钟）。
        assert_eq!(window_budget(32_768), Duration::from_secs(WINDOW_MAX_TIMEOUT_SECS));
        assert_eq!(window_budget(1), Duration::from_secs(WINDOW_MAX_TIMEOUT_SECS));
    }

    #[test]
    fn an_unknown_rate_becomes_the_longest_budget_instead_of_a_division_panic() {
        // `rate_bps` 是测出来的：第一轮还没有任何测量结果，调用方给 0 是最自然的写法。
        // 照 plan 那个式子字面写就是一次除零 panic（不是报错，是直接砸进程）。
        assert_eq!(window_budget(0), Duration::from_secs(WINDOW_MAX_TIMEOUT_SECS));
    }

    // ---- 206 的位置声明 ----

    #[test]
    fn content_range_start_reads_only_the_offset_and_refuses_every_shape_that_has_none() {
        assert_eq!(content_range_start("bytes 262144-393215/8388608"), Some(262144));
        assert_eq!(content_range_start("bytes 0-1023/1024"), Some(0));
        assert_eq!(content_range_start("bytes 50331647-50331647/50331648"), Some(50331647));
        // 416 的写法（没有"本次区间"那一段）读不出起点。
        assert_eq!(content_range_start("bytes */4194304"), None);
        // **总长**未知不影响到得了起点：`bytes 0-1023/*` 说的是"这一段从 0 起"，位置照样可核对
        // （`probe` 那边拒它是另一件事 —— 那里缺的是长度，这里缺的要是起点才拒）。
        assert_eq!(content_range_start("bytes 0-1023/*"), Some(0));
        assert_eq!(content_range_start("none"), None);
        assert_eq!(content_range_start(""), None);
        assert_eq!(content_range_start("bytes abc-1023/1024"), None);
        assert_eq!(content_range_start("bytes"), None);
        // 单位名大小写不敏感（头字段值里的 token 是这个立场）；多余空格也一样认。
        assert_eq!(content_range_start("BYTES 7-1023/1024"), Some(7));
        assert_eq!(content_range_start("  bytes   7 - 1023 / 1024 "), Some(7));
    }

    // ---- 读侧超时的分类（两支里能在单元层构造的那一支） ----

    #[test]
    fn a_plain_io_error_is_not_a_timeout_and_stays_a_transport_failure() {
        // 负向那半边必须钉住：不然"任何 io::Error 都算 Stalled"也能让超时那条集成用例过。
        let reset = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "other side closed");
        assert!(!is_read_timeout(&reset));
        assert!(!is_read_timeout(&std::io::Error::other("boom")));
        // std 自己的超时形状认，reqwest 包了一层的形状由集成用例钉
        // （`reqwest::Error` 没有公开构造器，单元层造不出那个 payload ⇒ 那一支只有真对端能证）。
        assert!(is_read_timeout(&std::io::Error::new(std::io::ErrorKind::TimedOut, "waited too long")));
    }

    // ---- 请求的形状（不发 close、头与 probe 同源、预算真上了线） ----

    #[test]
    fn the_built_request_carries_the_range_the_budget_and_the_same_headers_as_the_probe() {
        let agent = Client::builder().build().unwrap();
        let mut t = task("https://example.com/a.bin");
        t.referer = Some("https://site.example/from".into());
        t.cookies = Some("sid=1".into());
        t.user = Some("alice".into());
        t.password = Some("hunter2".into());
        let budget = window_budget(131_072);
        let req = build_request(&agent, &t, "bytes=100-200", budget).unwrap();

        assert_eq!(
            req.method(),
            &reqwest::Method::GET,
            "窗口这条路只发 GET：带体的任务在 `run_window` 门口就被拒了（F1），那一支归 `build_body_request`"
        );
        assert_eq!(req.url().as_str(), "https://example.com/a.bin");
        assert_eq!(req.headers().get(RANGE).unwrap(), "bytes=100-200");
        assert_eq!(req.headers().get(ACCEPT_ENCODING).unwrap(), "identity");
        assert_eq!(req.headers().get(ACCEPT).unwrap(), "*/*");
        assert_eq!(req.headers().get("referer").unwrap(), "https://site.example/from");
        assert_eq!(req.headers().get("cookie").unwrap(), "sid=1");
        // https ⇒ Basic 由 `with_contract_headers` 自己发出去（同一次构造里判据是同一份）。
        let auth = req.headers().get("authorization").expect("https 上口令要上线").to_str().unwrap();
        assert!(auth.starts_with("Basic "), "口令只以 Basic 形态出去：{auth}");
        assert!(!auth.contains("hunter2"), "base64 之后的串里不该有明文口令：{auth}");
        // 预算真的挂在请求上（不是拼了个 Duration 扔掉）。
        assert_eq!(req.timeout().copied(), Some(budget));
        // 不发 `Connection: close`（spec §10 R5 / 计划 E2；行为侧的证明归集成用例
        // `a_drained_window_leaves_the_connection_reusable`）。
        assert!(req.headers().get(reqwest::header::CONNECTION).is_none());
    }

    #[test]
    fn the_plain_http_case_keeps_the_probe_s_authorization_gate() {
        // 与 `probe::with_contract_headers` 的 F2 闸同一个判据：fetch 不"顺手补上"那个头。
        let agent = Client::builder().build().unwrap();
        let mut t = task("http://example.com/a.bin");
        t.user = Some("alice".into());
        let req = build_request(&agent, &t, "bytes=0-", Duration::from_secs(15)).unwrap();
        assert!(req.headers().get("authorization").is_none(), "明文 socket 上半副凭据也不许出去");
        assert_eq!(req.headers().get(RANGE).unwrap(), "bytes=0-");
    }

    // ---- 对外形状（plan 第 765–767 行的 Produces） ----

    #[test]
    fn outcome_and_fail_kind_are_the_shape_task_8_consumes() {
        // 解构钉住变体名与字段：改名或加字段都会让 Task 8 的匹配当场编译失败，而不是运行时少一支。
        let advanced = Outcome::Advanced { bytes: 7 };
        let Outcome::Advanced { bytes } = advanced else { panic!("Advanced 还在吗：{advanced:?}") };
        assert_eq!(bytes, 7);
        for out in [Outcome::Complete, Outcome::Stalled, Outcome::Failed(FailKind::Http(403))] {
            match out {
                Outcome::Advanced { .. } | Outcome::Complete | Outcome::Stalled | Outcome::Failed(_) => {}
            }
        }
        // `bytes` 与 `total_len` 是 `u64`（Task 8 直接把它加进偏移）；`Http` 装的是 `u16` 状态码；
        // `Transport` 装的是那句**原因**（E12⑦：折成裸枚举到 Task 8 那里就只剩"网络错了"）。
        let kinds = [
            FailKind::Http(416),
            FailKind::Transport("响应体读到一半对端拆了线".to_string()),
            FailKind::ValidatorChanged,
        ];
        assert_eq!(kinds[0], FailKind::Http(416));
        assert_ne!(kinds[1], kinds[2], "Transport 与 ValidatorChanged 不许折成同一个账");
        // `Transport` 的**载荷**也是一笔账：同一支 Transport，原因不同就是两个不同的结论
        // （Task 8 要把那句话原样交给用户）。把它折成"只看涨不看话"的写法在这里当场红。
        assert_ne!(
            kinds[1],
            FailKind::Transport("连不上（Connection refused）".to_string()),
            "Transport 的原因进了相等性：不许两条不同的失败读成同一条"
        );
        // **`Clone`（本轮由 `Copy` 降级）**：`Transport` 带着 `String`，`Outcome` 就再也不是 `Copy`。
        // 要保住的那条性质没变：Task 8 的 worker 线程把结论发给主循环时不该在这里管所有权 ——
        // 现在是"克隆一份交出去、手里留一份能打印"，两份必须相等。
        let a = Outcome::Failed(FailKind::Transport("响应体读到一半对端拆了线".to_string()));
        let b = a.clone();
        assert_eq!(a, b, "克隆一份结论之后原件还在、且两份相等（Task 8 才能一边打日志一边把它发出去）");
    }

    // ---- 本轮 D：`FailKind::Transport` 那句原因的构造规则（勘误 E12⑦） ----

    /// 单元层用的**假错误**：`reqwest::Error` 没有公开构造器（模块文档末节记着同一条困难），
    /// 而这里要钉的四条规则（一行 / 截断 / 链首句 / 摘 URL）需要的恰恰是"把任意形状的消息与任意
    /// 形状的链喂进去"。真错误是不是那个形状，归集成用例（`a_transport_failure_from_the_execute_side_…`
    /// 与两条零进度支）。
    #[derive(Debug)]
    struct FakeErr {
        msg: String,
        cause: Option<Box<FakeErr>>,
    }

    impl FakeErr {
        fn new(msg: &str) -> Self {
            FakeErr { msg: msg.to_string(), cause: None }
        }
        fn caused_by(msg: &str, cause: FakeErr) -> Self {
            FakeErr { msg: msg.to_string(), cause: Some(Box::new(cause)) }
        }
    }

    impl std::fmt::Display for FakeErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.msg)
        }
    }

    impl std::error::Error for FakeErr {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.cause.as_ref().map(|c| &**c as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn a_transport_note_is_one_line_even_when_the_cause_is_three() {
        // Windows 上 `io::Error` 的消息常带 `\r\n`（OS 文本原样带着），而 Task 8 把它打在进度行里
        // ⇒ 多行会把那一行拆成三截。折叠规则只有一条：所有空白段 ⇒ 一个空格。
        let note = transport_note(&FakeErr::new("第一行\n第二行\r\n第三行"));
        assert_eq!(note, "第一行 第二行 第三行", "换行与回车都被折成空格，文字一个不丢：{note}");
        assert!(!note.contains('\n') && !note.contains('\r'), "原因必须是一行：{note:?}");
        // 前后多余空白也一并收掉（不然 `{note}` 打进进度行会留一个看起来像空格的空档）。
        assert_eq!(transport_note(&FakeErr::new("  头  \n  尾 ")), "头 尾");
    }

    #[test]
    fn a_long_transport_note_is_cut_on_a_char_boundary_not_through_a_character() {
        // 300 个中文（每个 3 字节）：按**字节**切到 200 会落在某个字符的中段 ⇒ `&s[..200]` 当场
        // panic（这就是契约写"char 边界"的原因：截断这件事不许有 panic 的余地）。
        let long = "断".repeat(300);
        let note = transport_note(&FakeErr::new(&long));
        assert_eq!(note.chars().count(), TRANSPORT_NOTE_MAX_CHARS, "上界是**字符**数，不是字节数");
        let mut head: String = long.chars().take(TRANSPORT_NOTE_MAX_CHARS - 1).collect();
        head.push('…');
        assert_eq!(note, head, "截断只动尾巴，前面照原样，并且看得出来是被截的：{note}");

        // 让"第 200 个字符"正好落在多字节字符上那种形状：199 个 ASCII + 一串中文。
        let mixed = format!("{}{}", "a".repeat(TRANSPORT_NOTE_MAX_CHARS - 1), "中文中文");
        let note = transport_note(&FakeErr::new(&mixed));
        assert_eq!(note.chars().count(), TRANSPORT_NOTE_MAX_CHARS);
        assert!(note.ends_with('…'), "按字符切才谈得上边界：{note}");
        // 没超长的一字不动（不许有"顺手裁剪合法文本"的读法）。
        assert_eq!(transport_note(&FakeErr::new("刚好没那么长")), "刚好没那么长");
    }

    #[test]
    fn the_note_appends_the_first_cause_line_that_says_something_new() {
        // reqwest 实测的套娃形状：外层与它包着的那层**同一句话**（`io::Error{Other ←
        // reqwest::Error{Kind::Body ← Kind::Body ← TimedOut}}`），只取"第一条带来新信息的"才是话。
        let err = FakeErr::caused_by(
            "request or response body error",
            FakeErr::caused_by(
                "request or response body error",
                FakeErr::caused_by("request or response body error", FakeErr::new("other side closed")),
            ),
        );
        assert_eq!(
            transport_note(&err),
            "request or response body error: other side closed",
            "重复的那几层不许各算一句话（那会把 200 个字符的额度喂给废话）"
        );
        // 空消息不算话，继续往下找。
        let err = FakeErr::caused_by("外层", FakeErr::caused_by("", FakeErr::new("真正的原因")));
        assert_eq!(transport_note(&err), "外层: 真正的原因");
        // 没有链的时候就只有自己这一句（不许补"未知错误"那种编出来的话）。
        assert_eq!(transport_note(&FakeErr::new("孤零零的一句")), "孤零零的一句");
        // 找到**一条**就收手：第三条不再进来（"first useful line" 是契约的措辞）。
        let err = FakeErr::caused_by("甲", FakeErr::caused_by("乙", FakeErr::new("丙")));
        assert_eq!(transport_note(&err), "甲: 乙", "链上只取首条新信息：{err:?}");
    }

    #[test]
    fn the_note_drops_reqwest_s_url_tail_because_the_caller_already_prints_the_url() {
        // reqwest 的 `Display` 把 URL 抄在尾巴上（`reqwest-0.13.5/src/error.rs:300-302`：
        // `write!(f, " for url ({url})")`）。留着它 = 同一件事说两遍（Task 8 打印这一句的时候
        // 旁边就站着这条 URL），而 URL 本来也**不是**原因。
        let note = transport_note(&FakeErr::new(
            "error sending request for url (https://example.com/a.bin?sid=1)",
        ));
        assert_eq!(note, "error sending request", "尾巴那截 URL 整个摘掉：{note}");
        assert!(!note.contains("http") && !note.contains("sid=1"), "URL 的成分还留着：{note}");
        // 链上那层的 URL 尾巴同样摘掉（hyper 的 cause 里也可能带）。
        let err = FakeErr::caused_by(
            "error decoding response body for url (http://127.0.0.1:9/x)",
            FakeErr::new("connection closed for url (http://127.0.0.1:9/x)"),
        );
        assert_eq!(transport_note(&err), "error decoding response body: connection closed");
    }

    #[test]
    fn a_cause_we_author_themself_still_goes_through_the_same_builder() {
        // 两支零进度支**没有**错误对象可抄（`read` 给的是 `Ok(0)`），于是把自造的原因套进
        // `io::Error` 走同一个构造器 —— "一行 / 截断 / 不重复"这三条规则不许有第二套，也不许免检。
        assert_eq!(
            transport_note(&std::io::Error::other(WINDOW_EOF_WITHOUT_BYTES)),
            WINDOW_EOF_WITHOUT_BYTES,
            "自造那句原因该原样进来、原样出去（它本来已经是一行且远短于上界）"
        );
        assert_eq!(transport_note(&std::io::Error::other(WHOLE_EMPTY_DOCUMENT)), WHOLE_EMPTY_DOCUMENT);
        // 这两句里一个 ASCII 字母/数字都不带 ⇒ 集成用例 `a_transport_note_holds_no_byte_of_the_body_…`
        // 能拿"那一枪的响应体全是 'r'"当探针：账里混进任何一个 body 字节都会当场红。
        for cause in [WINDOW_EOF_WITHOUT_BYTES, WHOLE_EMPTY_DOCUMENT] {
            assert!(
                cause.chars().all(|c| !c.is_ascii_alphanumeric()),
                "自造的原因里出现了 ASCII 字母/数字 ⇒ 那条探针就不成立：{cause}"
            );
        }
    }

    #[test]
    fn an_empty_interval_is_a_complete_without_any_request_shape() {
        // `run_window` 在 `validate()` 之后、拼请求之前就返回，所以这里连 `Client` 都不需要构造。
        // 网络侧的"确实一枪没发"归集成用例 `an_empty_interval_needs_no_request`（服务端日志取证）。
        let iv = Interval { start: 10, end: 9 };
        assert!(iv.is_empty());
        assert_eq!(written(iv, iv.start), 0);
    }

    // ---- 修复轮 F1：带请求体的那一枪的形状（POST + 体 + 不带 Range） ----

    /// 写一份**真的**体文件（`<仓库>/target/t7-scratch/`，`target/` 已在 `.gitignore` 里）。
    ///
    /// 为什么这一层也要真文件：本模块的 `open_request_body` 交出去的是 `Body::from(File)`，
    /// 而 reqwest 的 `Body` 只在**字节已经在手里**时才写得出 `Content-Length`（`Kind::Reader(_, len)`，
    /// `len` 取自 `metadata()`）。拿一个不存在的路径断"形状对不对"是**恒真**的（谁都建不出请求）。
    /// 目录只由本模块的用例用，跑完显式删；与 `probe.rs` 的 `t6-scratch` 分开，免得互相扫掉对方的文件。
    fn scratch_body(name: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("t7-scratch");
        std::fs::create_dir_all(&dir).expect("建 t7-scratch 目录");
        let p = dir.join(name);
        std::fs::write(&p, content).expect("写体文件");
        p
    }

    /// 体文件里那句哨兵：用来证明"错误消息一个字都不抄体的内容"（体的第 2 行就是一个表单口令）。
    const BODY_SENTINEL: &str = "session=abc&cvv=999&password=hunter2";

    #[test]
    fn the_body_shaped_request_is_a_post_carrying_the_file_and_not_a_single_range() {
        // F1 的裁定：**方法 = POST**、**体 = 文件字节**、**不带任何 Range**（spec §2.2 第一条：带请求体
        // 的任务不可分段，而 `Range` 在 POST 上不成立）。这一支是构造层的证据，线上那半（对端实际
        // 收到什么）归集成用例
        // `a_data_file_task_is_posted_with_the_file_as_its_body_and_lands_the_response_body`。
        let file = scratch_body("f1-shape.bin", BODY_SENTINEL.as_bytes());
        let agent = Client::builder().build().unwrap();
        let mut t = task("https://example.com/submit");
        t.data_file = Some(file.clone());
        t.referer = Some("https://site.example/form".into());
        t.cookies = Some("sid=1".into());
        t.user = Some("alice".into());
        t.password = Some("hunter2".into());
        let budget = window_budget(131_072);

        let body = open_request_body(&file).unwrap();
        let req = build_body_request(&agent, &t, body, budget).unwrap();

        assert_eq!(req.method(), &reqwest::Method::POST, "带体的那一枪必须是 POST：发 GET 就是把另一条请求的字节当成果子");
        assert!(req.headers().get(RANGE).is_none(), "POST 上一条 Range 都不许出现（spec §2.2）：{:?}", req.headers());
        // `Content-Length` **不在** `build()` 出来的头表上：它是 reqwest 在 `into_async` 那一步从
        // `metadata().len()` 补写的（见 `open_request_body` 的文档），所以这一层断不了它。
        // 线上那一条归集成用例
        // `a_data_file_task_is_posted_with_the_file_as_its_body_and_lands_the_response_body`
        // —— 那里读的是**对端收到的**头与 body，而对端只认 `Content-Length`（见 harness 的
        // `read_request_with_body`：见到 `Transfer-Encoding` 就当场红）。
        // **不发 Content-Type**：任务形状里没有供给它的字段，猜一个会悄悄改掉服务器怎么处理这份请求。
        // 这是登记给 Task 9（CLI）的开放问题，见 `run_whole` 的文档。
        assert!(req.headers().get(reqwest::header::CONTENT_TYPE).is_none(), "不许凭空发明 Content-Type：{:?}", req.headers());
        // 体的确实在请求上（`Body::from(File)` 之后 `as_bytes()` 读不出内容，只有"有没有体"看得出来；
        // 逐字节对不对归集成用例的对端取证）。
        assert!(req.body().is_some(), "POST 必须带着体");
        // 契约头与 `probe` 同源，口令那半在 https 上放行。
        assert_eq!(req.headers().get(ACCEPT_ENCODING).unwrap(), "identity");
        assert_eq!(req.headers().get(ACCEPT).unwrap(), "*/*");
        assert_eq!(req.headers().get("referer").unwrap(), "https://site.example/form");
        assert_eq!(req.headers().get("cookie").unwrap(), "sid=1");
        let auth = req.headers().get("authorization").expect("https 上口令要上线").to_str().unwrap();
        assert!(auth.starts_with("Basic ") && !auth.contains("hunter2"), "只以 Basic 形态出去：{auth}");
        assert_eq!(req.timeout().copied(), Some(budget), "窗口预算照同一个式子挂在整份那一枪上");
        assert!(req.headers().get(reqwest::header::CONNECTION).is_none(), "不发 Connection: close（R5/E2）");
        std::fs::remove_file(&file).expect("清体文件");
    }

    #[test]
    fn the_body_shaped_request_over_plain_http_still_drops_the_authorization_header() {
        // F2 那道 scheme 闸住在 `with_contract_headers`，`build_body_request` 不许"顺手补上"。
        // POST 的体里往往就带着口令，而那条头是**另一条**外传通道 —— 明文 socket 上一条都不许多。
        let file = scratch_body("f1-plain.bin", b"a=1");
        let agent = Client::builder().build().unwrap();
        let mut t = task("http://example.com/submit");
        t.data_file = Some(file.clone());
        t.user = Some("alice".into());
        let body = open_request_body(&file).unwrap();
        let req = build_body_request(&agent, &t, body, Duration::from_secs(15)).unwrap();
        assert!(req.headers().get("authorization").is_none(), "明文 socket 上半副凭据也不许出去：{:?}", req.headers());
        assert_eq!(req.method(), &reqwest::Method::POST, "闸只挡口令，不改方法");
        std::fs::remove_file(&file).expect("清体文件");
    }

    #[test]
    fn an_unreadable_body_file_is_named_but_nothing_of_its_bytes_reaches_the_message() {
        // 裁定：路径读不出来 ⇒ 报错，且消息**不抄体的字节**。
        // 这条断言的另一半（"字节确实在我们手里过"）由 `the_body_shaped_request_...` 那条负责：
        // 同一个哨兵在那里真上过请求，所以这里"消息里没有它"不是拿一个从没读过的东西自证。
        let missing = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("t7-scratch")
            .join("definitely-not-there.bin");
        let Err(err) = open_request_body(&missing) else {
            panic!("读不出来的体文件必须报错，不许悄悄发一条空体的 POST");
        };
        let msg = err.to_string();
        assert!(msg.contains("--data-file"), "错误要点名那个参数，用户才知道改什么：{msg}");
        assert!(msg.contains("definitely-not-there.bin"), "路径要点名（那是用户能给对的东西）：{msg}");
        assert!(!msg.contains("cvv") && !msg.contains("session=abc") && !msg.contains("hunter2"), "错误里抄了体：{msg}");
    }

    // ---- 修复轮 F1：两条入口互斥（各拒对方那种形状） ----

    #[test]
    fn run_window_refuses_a_body_carrying_task_before_any_request_shape_exists() {
        let agent = Client::builder().build().unwrap();
        let mut t = task("https://example.com/submit");
        t.data_file = Some("does-not-matter.bin".into());
        let store_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("t7-scratch");
        std::fs::create_dir_all(&store_dir).expect("建 t7-scratch 目录");
        let path = store_dir.join("f1-window-guard.bin");
        let store = Store::open(&path, true).expect("开测试目标文件");
        let Err(err) = run_window(
            &agent,
            &t,
            Interval { start: 0, end: 4095 },
            &store,
            10_000_000,
            48 * 1024 * 1024,
            &AtomicU64::new(NO_END_CAP),
        ) else {
            panic!("带 --data-file 的任务不该被窗口那条路接受（发 GET 就是拿另一条请求的字节）");
        };
        let msg = err.to_string();
        assert!(msg.contains("--data-file") && msg.contains("run_whole"), "错误要指路：{msg}");
        assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");
        assert_eq!(store.len().expect("读长度"), 0, "被拒的任务不许动过盘");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn run_whole_refuses_a_task_that_has_no_body_file() {
        // 两条入口的判据是**同一个函数**（`Task::segmentable()`）的两半：可分段 ⇒ `run_window`，
        // 带体 ⇒ `run_whole`。谁都不许同时接两种形状 —— 否则"用哪条路"就变成一个调用方的自由。
        let agent = Client::builder().build().unwrap();
        let t = task("https://example.com/a.bin");
        let store_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("t7-scratch");
        std::fs::create_dir_all(&store_dir).expect("建 t7-scratch 目录");
        let path = store_dir.join("f1-whole-guard.bin");
        let store = Store::open(&path, true).expect("开测试目标文件");
        let Err(err) = run_whole(&agent, &t, &store, 10_000_000) else {
            panic!("没有体的任务不该走 `run_whole`：那条路不分窗口，会把可分段的任务整份吸完");
        };
        let msg = err.to_string();
        assert!(msg.contains("--data-file") && msg.contains("run_window"), "错误要指路：{msg}");
        assert_eq!(store.len().expect("读长度"), 0, "被拒的任务不许动过盘");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn both_entries_validate_the_task_before_any_request_or_body_is_touched() {
        // `Task::validate()`（口令闸 + E9 的 userinfo 闸）在**两条**入口上都是第一行。
        // 顺序在这里有实质意义：那种任务连体文件都不该被打开（口令表单的体不该被读进我们的进程）。
        let agent = Client::builder().build().unwrap();
        let file = scratch_body("f1-order.bin", BODY_SENTINEL.as_bytes());
        let store_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("t7-scratch");
        std::fs::create_dir_all(&store_dir).expect("建 t7-scratch 目录");
        // 体文件与目标文件**必须不同名**（同名时 `Store::open(.., true)` 会把体一起截掉，
        // 于是"被拒的任务没读过体"这一半就成了恒真断言）。
        let path = store_dir.join("f1-order-store.bin");
        let store = Store::open(&path, true).expect("开测试目标文件");

        let mut plain = task("http://example.com/submit");
        plain.password = Some("hunter2".into());
        plain.data_file = Some(file.clone());
        let Err(err) = run_whole(&agent, &plain, &store, 10_000_000) else {
            panic!("明文 HTTP 带口令的任务必须被 `run_whole` 的 validate() 拒掉");
        };
        let msg = err.to_string();
        assert!(msg.contains("口令"), "错误要说清是口令规则：{msg}");
        assert!(!msg.contains("hunter2") && !msg.contains("cvv"), "两条都不许抄：{msg}");
        assert!(!msg.contains("--data-file"), "看起来是先开了体文件才被口令闸挡住：{msg}");

        // E9 那一支：URL 里带 userinfo 的任务在两条入口上都到不了网络（`probe::run` 同一个判据）。
        let mut credentialed = task("https://alice:s3cret@example.com/a.bin");
        credentialed.data_file = Some(file.clone());
        let Err(err) = run_whole(&agent, &credentialed, &store, 10_000_000) else {
            panic!("URL 里带 userinfo（E9）的任务必须被 `run_whole` 的 validate() 拒掉");
        };
        assert!(err.to_string().contains("--user"), "错误要指使用 --user/--password：{err}");
        assert!(!err.to_string().contains("s3cret"), "错误里不许抄凭据：{err}");

        assert_eq!(store.len().expect("读长度"), 0, "被拒的任务不许动过盘");
        drop(store);
        let _ = std::fs::remove_file(&path);
        std::fs::remove_file(&file).expect("清体文件");
    }
}
