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
//!    用例：`a_416_whose_denominator_is_our_length_is_complete`（正支）、
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
//! 6. **发了 `Range` 而对端回 `200` ⇒ 先流掉前 `iv.start` 个字节再落盘**（裁定 6，计划 **E7**②
//!    交给 Task 7 的下游义务）：那种 body 的第 0 字节对应**文件偏移 0**，直接按 `iv.start` 写就是把
//!    内容写进中段。丢字节是**流式**的（每块 64 KiB 的读循环里切掉一段），不把整个 body 吸进内存。
//!    用例：`a_degraded_200_body_is_skipped_not_written_at_the_wrong_offset`（node 的 `/noranges`，
//!    真对端 + 块号取证）、`a_degraded_200_is_skipped_as_stream_not_as_a_buffered_body`
//!    （进程内节拍对端，另加一条"对端在客户端挂断前写出了多少字节"钉住不许缓冲整个 body）。
//!
//! ## 另外三条钉住的性质
//!
//! - **`206` 却说不出它交的是哪一段**（没有 `Content-Range`，或它的起点不等于 `iv.start`）⇒
//!   `Failed(ValidatorChanged)`。偏移核对不了的字节**一个都不许写盘**：那是裁定 6 的镜像面 —— 那边
//!   是"200 的 body 起点在 0"，这边是"206 声称的起点与我们问的不是同一个"。
//!   用例 `a_206_that_cannot_show_its_offset_is_validator_changed`。
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
//!
//! ## 与 reqwest blocking 的形状关系（两处实测过的事实）
//!
//! - `.timeout()` 是**整请求**的超时，但 blocking 层把它**每次 `read()` 重新起算**
//!   （`src/blocking/response.rs` 的 `impl Read for Response` → `wait::timeout(fut, self.timeout)`，
//!   而 `wait::timeout` 在**调用时**算 deadline）。所以它在我们这里的效果是"连续静默超过预算 ⇒ 到点"，
//!   正合 spec §3 那条 `STALL_TIMEOUT` 的本意；一个慢但一直在送的节拍打不中它
//!   （用例 `a_live_but_paced_peer_is_advanced_rather_than_misread_as_stalled`）。
//! - **别假设"不配 timeout 就等于永不等"**（派发点名）：Task 0 实测一条没配 timeout 的 reqwest 请求
//!   在 ~30.005 s 自己死掉，时长来源未查清。本模块**永远**显式配预算，正是为了不让那种未查清的
//!   内部期限替我们决定语义。
//! - `Response::read` 的超时**不是** `reqwest::Error`：blocking 层把它包成
//!   `io::Error::new(ErrorKind::Other, reqwest::Error{Kind::Decode ← TimedOut})`。所以判"这是超时"
//!   要把 payload 再 `downcast_ref::<reqwest::Error>()` 一次（`is_read_timeout`）。只按
//!   `kind()` 分的话，读侧超时会被计成一次传输失败 —— 两条用例
//!   （`a_peer_that_never_answers_is_stalled` 走 `execute` 支、
//!   `a_peer_that_answers_but_sends_no_body_is_stalled_not_a_transport_failure` 走 `read` 支）
//!   各钉一支。
//!
//! ## 本模块**没**做的事（照实登记，不是遗漏）
//!
//! - **不发 POST 体**。`run_window` 只发 `GET`（与 `probe::build_request` 同一形状）。带
//!   `--data-file` 的任务在 spec §2.2 第一条下是"单段顺序下载"，`iv.start` 恒为 0，所以裁定 6 那条
//!   跳字节在这种形状上是 0 字节的跳；**但**"下载那一枪应当发 POST"这件事简报与六条裁定都没写，
//!   `Task` 里也没有 Content-Type 字段可用（表单还是裸体、怎么编码都未定），已作为**待裁事项**交回
//!   控制方。这里不替它猜一个形状：猜错的 POST 会往别人的服务器上留下真副作用。
//! - **不收割 `ProbeFacts`**：`probe::run` 对带 `data_file` 的任务硬拒（Task 6 评审 F1），于是那种
//!   形状的长度/验证子必须取自**真发出去的那一枪**的响应头（`probe.rs` 模块文档"本模块没做的事"
//!   第 1 条把这一项写给 Task 7 + Task 8）。派发的 `Produces` 与六条裁定都没有这条路（`Outcome`
//!   装不下那份事实，而 `run_window` 的签名被裁定 1 钉死），所以同样是**待裁事项**：要么给
//!   `Outcome` 加一个带事实的变体，要么在 `probe` 里抽一个 `pub(crate)` 的"响应头 → 事实"共用口。
//! - 不重试、不退避、不落断点文件、不碰 `scheduler`：那些账都在 Task 8 的主循环里。
//! - 不校验 `sha256`、不 `flush`：`Store::flush`（= `sync_all`）只许在任务收尾调一次（**E8**①）。

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, Request};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;

use crate::consts::{WINDOW_BYTES, WINDOW_MAX_TIMEOUT_SECS, WINDOW_MIN_TIMEOUT_SECS};
use crate::probe;
use crate::scheduler::Interval;
use crate::store::Store;
use crate::task::{Task, authority_has_userinfo};

/// 一次从 socket 读的块大小（与 harness 服务器的出块粒度一致，也是 `store.write_at` 的粒度）。
///
/// 为什么固定而不是"读满窗口为止"：窗口最多 `WINDOW_BYTES` = 4 MiB，一次 `read` 就要一个这么大的
/// 缓冲 ⇒ 每个 worker 常驻 4 MiB（8 个 worker 就是 32 MiB），而且 `write_at` 的临界区会被一次
/// 巨大的拷贝占住。64 KiB 的粒度让"读一块、写一块、放锁"交替进行。
const CHUNK_BYTES: usize = 64 * 1024;

/// 这一枪为什么没成。三个变体各自对应 Task 8 该记的**一种账**：
/// `Http` 是"服务器有说法"（状态码进错误消息），`Transport` 与 `ValidatorChanged` 是"对端不对劲"
/// （spec §6.1/§6.2 的快速 5 次 / 慢速 30 s 封顶挂在这两支上），而 `ValidatorChanged` 额外说明
/// **"服务端长度与我们脚下这份不一致"** —— 那一支最终要报明确的"长度变了"，不是笼统的传输失败（**E7**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    Http(u16),
    Transport,
    ValidatorChanged,
}

/// 这一枪的结论。Task 8 把它折成 `scheduler::Event`：
/// `Advanced{bytes}` → `Progress{abs_end: iv.start + bytes}`（+ 之后由主循环决定 `WindowDone`/续派）、
/// `Complete` → `SegmentComplete`、`Stalled`/`Failed` → `Failed`。
///
/// `bytes` 的语义（裁定 4/5 同源）：**这一枪经 `store.write_at` 确认落盘的字节数**，不是"从 socket
/// 读到的字节数"，也不是"区间剩下的字节数"。`Complete` 不带字节：它是"这个区间不需要（再）取字节"，
/// 而不是"取到了 `iv.len()` 个字节"（后者由 `Advanced` 说）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Advanced { bytes: u64 },
    Complete,
    Stalled,
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
/// 末段（`iv.end` 就是文件的最后一个字节，或**越过**它）发 `bytes={start}-`：那条写法本来就在说
/// "到文件末尾为止"，多写一个终点是对同一个意思的两种表述，而观测到 IDM 发的是前者。
/// 其余一律带显式终点 —— 少了那个终点，对端就会把一个窗口的字节之外的东西也送过来。
fn range_header(iv: Interval, total_len: u64) -> String {
    if iv.end.saturating_add(1) >= total_len {
        format!("bytes={}-", iv.start)
    } else {
        format!("bytes={}-{}", iv.start, iv.end)
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
/// `io::Error::new(ErrorKind::Other, reqwest::Error{Kind::Decode ← TimedOut})`（见模块文档），
/// 所以只问 `kind()` 一定读成"别的传输错误"。两支都问：`ErrorKind::TimedOut`（std 自己的形状）
/// 与 payload 里的 `reqwest::Error::is_timeout()`（reqwest 的实际形状，集成用例
/// `a_peer_that_answers_but_sends_no_body_is_stalled_not_a_transport_failure` 钉的就是它）。
fn is_read_timeout(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::TimedOut
        || err
            .get_ref()
            .and_then(|payload| payload.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout)
}

/// 拼出这一枪（照 `probe::build_request` 的形状先建出 `Request`，单元层就看得见头与预算）。
fn build_request(agent: &Client, task: &Task, range: &str, budget: Duration) -> Result<Request> {
    let req = agent.get(&task.url).header(RANGE, range).timeout(budget);
    probe::with_contract_headers(req, task)?
        .build()
        .context("窗口请求拼不出来（URL 不合法？）")
}

/// 已写进盘里的字节数（`cursor` 是本模块唯一的进度真相：它只被 `write_at` 的返回值推进）。
fn written(iv: Interval, cursor: u64) -> u64 {
    cursor.saturating_sub(iv.start)
}

/// 领一个字节区间，最多下**一个窗口**，把字节按绝对偏移交给 `store`。
///
/// `agent` 由调用方持有并复用（连接池才有意义）；`rate_bps` 只用来定窗口预算；
/// `total_len` 是 `ProbeFacts` 那份文件长度（裁定 1：末段要不要发开放终点必须由它决定，
/// 也用来判 416 的分母）。
pub fn run_window(
    agent: &Client,
    task: &Task,
    iv: Interval,
    store: &Store,
    rate_bps: u64,
    total_len: u64,
) -> Result<Outcome> {
    // 与 `probe::run` / `state::Snapshot::from_task` 同一个立场：规则只住在 `Task::validate()` 一处，
    // 但**发请求**这一刻是第二个强制点。明文 HTTP + 口令的任务到不了网络（用例
    // `the_password_gate_is_enforced_before_fetch_touches_the_wire` 用服务端日志钉"拒在上网之前"）。
    task.validate()?;

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
            // `reqwest::Error`，不是被包了一层的 `io::Error`）。
            return Ok(if e.is_timeout() {
                Outcome::Stalled
            } else {
                Outcome::Failed(FailKind::Transport)
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
    // 裁定 6 的触发条件：我们**发了** `Range`（本模块每一枪都发，见 `range_header`），而对端答 `200`
    // ⇒ body 的第 0 字节对应文件偏移 0，不是 `iv.start` ⇒ 先流掉 `iv.start` 个字节。
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
    let mut cursor = iv.start;
    let mut skip = if degraded { iv.start } else { 0 };
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut eof = false;
    let mut read_err: Option<std::io::Error> = None;
    while cursor < stop {
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
        cursor += wrote as u64;
    }

    let bytes = written(iv, cursor);
    if let Some(err) = read_err {
        // 裁定 4：写下去过的字节都是真进度；只有零进度那两支才让重试上限去计。
        return Ok(match (bytes, is_read_timeout(&err)) {
            (0, true) => Outcome::Stalled,
            (0, false) => Outcome::Failed(FailKind::Transport),
            _ => Outcome::Advanced { bytes },
        });
    }
    if eof {
        // body 比区间短（`/drop`、`/sz/<小n>`、`/noranges` 交完 1 MiB 就结束）：绝不是 `Complete`
        // ——那会把这段的尾巴**永远封存**。一个字节都没落到盘上才是失败。
        return Ok(if bytes == 0 {
            Outcome::Failed(FailKind::Transport)
        } else {
            Outcome::Advanced { bytes }
        });
    }
    // 走到这里是"自己收手"（`cursor >= stop`）：区间读完 ⇒ `Complete`；窗口用满 ⇒ `Advanced`。
    Ok(if cursor > iv.end {
        Outcome::Complete
    } else {
        Outcome::Advanced { bytes }
    })
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

    // ---- 裁定 1 + spec §3：开放终点的判据 ----

    #[test]
    fn only_the_interval_that_reaches_the_end_of_the_file_is_open_ended() {
        // 末段（`iv.end == total_len - 1`）⇒ 开放终点。这是简报与 spec 唯一要求发 `bytes={start}-` 的形状。
        assert_eq!(range_header(Interval { start: 1_048_576, end: 48 * 1024 * 1024 - 1 }, 48 * 1024 * 1024), "bytes=1048576-");
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

        assert_eq!(req.method(), &reqwest::Method::GET, "本轮 fetch 只发 GET（POST 那支是待裁事项）");
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
        // `bytes` 与 `total_len` 是 `u64`（Task 8 直接把它加进偏移）；`Http` 装的是 `u16` 状态码。
        let kinds = [FailKind::Http(416), FailKind::Transport, FailKind::ValidatorChanged];
        assert_eq!(kinds[0], FailKind::Http(416));
        assert_ne!(kinds[1], kinds[2], "Transport 与 ValidatorChanged 不许折成同一个账");
        // `Copy`：Task 8 的 worker 线程要把结论发给主循环，不该在这里管所有权。
        let a = Outcome::Stalled;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn an_empty_interval_is_a_complete_without_any_request_shape() {
        // `run_window` 在 `validate()` 之后、拼请求之前就返回，所以这里连 `Client` 都不需要构造。
        // 网络侧的"确实一枪没发"归集成用例 `an_empty_interval_needs_no_request`（服务端日志取证）。
        let iv = Interval { start: 10, end: 9 };
        assert!(iv.is_empty());
        assert_eq!(written(iv, iv.start), 0);
    }
}
