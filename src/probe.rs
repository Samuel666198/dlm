//! probe：多连接分段下载的**第一枪** —— 在本项目里第一次真发 HTTP 请求的模块。
//!
//! 一发请求要问出四件事（spec §2 的模块表把 `probe` 的职责写成"无 Range 试探；读满预读量后掐断，
//! 保留已收字节"，本模块按派发更正为"限量预读"，见下面第 2 条）：
//! 1. 这份文件**现在**多长、有没有 `ETag`/`Last-Modified`、认不认 `Range`、跟随重定向后**落在哪**；
//! 2. 顺带取回一小段**有界的**body 前缀（预读），engine 之后把它写在偏移 0 ——
//!    退化成单连接时这一段不用重取；
//! 3. 把事实交回 `resume::decide`（Task 5 的续传闸）与 `state`（断点文件）。
//!
//! ## 本模块的形状决定（都有对应用例钉着）
//!
//! 1. **一次请求，不多问第二枪。** 长度优先从 `Content-Range` 的分母取（我们那条限量 Range 换回来的
//!    206 自己就把全长说了），取不到就退到 `Content-Length`。**不**为了"问出一个更大的长度"再发
//!    第二条请求：那是简报没有、spec 也没要求的动作，而且会给对端多打一发。
//! 2. **限量靠 `Range`，不靠 drop**（简报 Step 4 的机制，派发第 4 条保留）：`task.segmentable()` 时发
//!    `bytes=0-<prelude_max-1>`，响应于是被**完整读完**、连接干净。这同时也是**不用** `Connection: close`
//!    的前提（spec §10 勘误 **R5**、计划 **E2**：Task 6/7 不得加 close、不得设
//!    `pool_max_idle_per_host(0)`）—— Task 0 实测 416 卡死的根因在本仓库测试服务器的收尾
//!    （推流循环漏 `res.end()`），不在 reqwest 的连接池；加 close 会把"复用连接上打 416"这条真实
//!    路径从测试覆盖里关掉，正是勘误 **E3** 守卫用例要跑的那条。
//! 3. **`!task.segmentable()`（带 POST 体）时不发 `Range`**：POST 语义下 Range 不成立（spec §2.2）。
//!    这一支没有头可以限量 ⇒ 只能"读到上限就停手"，连接由 reqwest 自己收尾（Task 0 在本仪器上实测：
//!    只读预读量就丢弃响应时，harness 看到的是连接被立即拆掉、下一枪走新连接）。内存边界由
//!    `read_bounded` 守住，绝不允许把整份 body 读进 `Vec`。
//! 4. **不认 `Range` 的对端：`total_len` 就是这一次响应交出来的字节数。** 本仓库的 `/noranges` 刻意把
//!    body 截在 1 MiB，所以那一支的 `total_len == 1_048_576`，不是 48 MiB。这不是要绕过的缺陷：
//!    一个无视 `Range` 的服务器说不出比它正在交出的更多的东西。
//!    **对退化下载的含义**：引擎随后按这份 `total_len` 单连接取回**恰好这一次响应装得下的量**
//!    （spec §2.2 第二条：没有 `Accept-Ranges` 或首请求回 `200` 而不是预期的 `206` ⇒ 段表退化为单段、
//!    且不重试分段）。要拿到比这更多的字节只能靠对端自己愿意发 —— 我们不会替它编一个更大的长度，
//!    也不会补一枪去打听（见上面第 1 条）。
//! 5. **落地 URL 里带着凭据 ⇒ 拒**（Task 5 评审 **C3** / 派发第 9 条）：`ProbeFacts.final_url` 来自响应
//!    的**最终** URL，是**服务器控制**的值。`state` 会把它落盘、`resume` 的 ② 会把它连同断点里那份
//!    一起写进原因串打到终端。`Task::validate()`（勘误 **E9**）只检查**用户给的** `task.url`，
//!    够不着这里 ⇒ 一条 `Location: http://alice:s3cret@host/x` 就能把凭据塞进我们写的文件和打出的
//!    消息里，绕过 §4 硬规约的原意。裁定是**拒绝**（既不悄悄收下，也不悄悄改写 ② 要比的那个串），
//!    并且错误消息**不回显**那条 URL。判据复用 `task::authority_has_userinfo`（同一个函数，
//!    与 E9 那一侧不会读成两样）。
//! 6. **验证子按响应头的原样值保存，不归一化**（与 `resume::ProbeFacts` 的文档同一立场）；
//!    但 `ETag`/`Last-Modified` 里出现**非 UTF-8 字节**时只能记成 `None`：`ProbeFacts` 与断点文件
//!    都装不下它，`If-Range` 更没有强验证子可用。这不是"忽略噪音"—— 少了一把验证子，
//!    `resume` 的 ③ 会自动落到 ④ 的长度兜底（Task 5 裁定 2），所以是**降级但可解释**的形状。
//! 7. **试探带满 9 项契约头**（`Referer`/`Cookie`/`Authorization: Basic`/`Accept-Encoding: identity`/
//!    `Accept: */*`）：`resume` 的 ①b 判的是"确认这些字节的请求与这次的请求是不是同一个"，
//!    如果**第一枪**（定全长与 ETag 的这一枪）与后面几枪带的头不一样，`total_len`/`ETag` 就可能
//!    出自另一份文档（同一台服务器换个 Cookie 给不同长度很常见）。`Accept-Encoding: identity` 另有一层：
//!    压过体之后 `Content-Length` 说的就不是文件字节数了。
//!
//! ## 本模块**没**做的事（照实登记，不是遗漏）
//!
//! - **POST 形状下第一枪仍是 `GET`，不带 `--data-file` 的 body**（简报 Step 4 的字面形状：
//!   "GET 无 Range，若 `task.segmentable()` 则加 Range"）。于是对 POST 任务，`total_len`/`ETag`
//!   是从**那条 GET** 读出来的，而真正下载发的是 POST。派发点名的更正里没有动这一条，我按简报实现，
//!   并把风险登记在报告里：这是"9 项契约里唯一一处试探与下载不是同一个请求"的形状。
//! - 不重试、不退避（spec §6.1/§6.2 的账归 fetch/engine），不落盘、不碰 `store`，不读断点文件。
//!   本模块唯一的 I/O 就是那一发请求。
//! - 不校验 `total_len` 与已下段表是否自洽（那是 `resume` 的 ④ 与 `state::check_segments`）。

use std::io::Read;

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, Request, RequestBuilder, Response};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_LENGTH,
    CONTENT_RANGE, COOKIE, ETAG, LAST_MODIFIED, RANGE, REFERER,
};
use reqwest::StatusCode;

use crate::resume::ProbeFacts;
use crate::task::{authority_has_userinfo, Task};

/// 试探之后关于远端文件**此刻**的事实 + 已取回的预读字节。
///
/// 字段名与类型按 plan 第 717 行的 `Produces` 写死（集成测试
/// `probe_result_shape_is_the_one_task_5_and_task_8_consume` 用解构钉住它，改名即编译失败）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    /// 交回给 `resume::decide` 的那份事实（类型住在 `src/resume.rs`，见那里的 `ProbeFacts` 文档）。
    pub facts: ProbeFacts,
    /// body 的前 `prelude_max` 字节（对端给得更少就只有那么多）。engine 把它写在偏移 0。
    pub prelude: Vec<u8>,
}

impl ProbeResult {
    /// Task 5 的闸口要的那份事实（派发第 5 条：`ProbeFacts` 住在 `resume.rs`，本模块只提供取回口）。
    ///
    /// 交回**克隆**而不是引用：调用方（Task 8）拿到后要把它拼进 `state::Validator` 落盘，
    /// 而 `ProbeResult` 里的 `prelude` 还要继续用 —— 按值取回比让两份东西互相借着好用。
    pub fn facts(&self) -> ProbeFacts {
        self.facts.clone()
    }
}

/// 落地 URL 带 userinfo 时那句**不回显 URL** 的拒绝（模块文档第 5 条）。
/// 两处失败路径共用同一个常量：成功响应里的落地 URL、以及 reqwest 在跟随重定向时失败的
/// 那个 URL —— 一条规则不该有两个版本的措辞。
///
/// 这段文本里**一个 URL 成分都没有**（没有 at 号、没有 scheme），用例
/// `the_landing_gate_rejects_a_userinfo_authority_and_never_echoes_it` 钉的就是这件事：
/// 这句会被 Task 8/9 打进终端与日志，而它旁边的信息（断点文件里的 `final_url`、② 的原因串）
/// 正是我们不让它带凭据的那两处。指路用词（userinfo / 主机名前面）而不是抄地址。
const LANDED_CREDENTIALS: &str = "服务器重定向后的地址在主机名前面带着凭据（userinfo 那一段）：\
                                  它会被写进断点文件、并出现在续传冲突的原因里，因此拒绝收下这份事实；\
                                  本消息不回显那条地址";

/// 发**一发**试探请求，取回事实与预读。
///
/// `agent` 由调用方持有并复用（Task 8 的整轮都用同一个客户端，这样连接池才有意义）；
/// `prelude_max` 是预读上限（`consts::PROBE_READ_BYTES` 是它的默认值，Task 9 允许覆盖）。
pub fn run(task: &Task, agent: &Client, prelude_max: u64) -> Result<ProbeResult> {
    // 请求边界上的第二道闸：`Task::validate()` 管着"明文 HTTP 不送口令"与 E9 的 userinfo URL。
    // 与 `state::Snapshot::from_task` 同一个立场（落盘点是第二个强制点，这里是**发请求**这个第二强制点），
    // 规则本身只住在 `Task::validate()` 一处。**不加 `.context()`**：anyhow 的 `to_string()` 只显示
    // 最外层，包一层会把"口令"三个字从消息里挤掉（`state.rs` 里同一条理由）。
    task.validate()?;

    let range = prelude_range(task, prelude_max);
    let req = build_request(agent, task, range.as_deref())?;
    let resp = match agent.execute(req) {
        Ok(resp) => resp,
        Err(e) => {
            // 跟随重定向**中途**失败时，那条带凭据的落地 URL 只出现在 reqwest 的错误里
            // （`Error::url()` 就是它，见 reqwest 0.13.5 `async_impl/client.rs` 里
            // `Pending::poll` 用 tower-http 的 `RequestUri` 覆写 `self.url` 那一段）⇒
            // 先把同一个判据套上去，换成一句不回显的拒绝。
            if e.url().is_some_and(|u| authority_has_userinfo(u.as_str())) {
                bail!("{LANDED_CREDENTIALS}");
            }
            return Err(e).context("试探请求发不出去");
        }
    };

    let status = resp.status();
    // `headers` 与 `final_url` 先取出来：`resp` 随后要被 body 的读取消耗掉（`Read` 要 `&mut`）。
    let headers = resp.headers().clone();
    let final_url = resp.url().clone();
    // 模块文档第 5 条那道闸，排在**任何**可能复述这条 URL 的消息之前。
    if authority_has_userinfo(final_url.as_str()) {
        bail!("{LANDED_CREDENTIALS}");
    }

    // 416 单独一支：我们那条 `bytes=0-…` 撞上 416，只有"文件是空的"这一种读法站得住
    // （`Content-Range: bytes */0`）。见用例 `a_zero_length_file_yields_zero_length_facts_instead_of_an_error`。
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let reported = header_text(&headers, CONTENT_RANGE).and_then(|v| content_range_total(&v));
        if range.is_some() && reported == Some(0) {
            return Ok(ProbeResult {
                facts: ProbeFacts {
                    etag: header_text(&headers, ETAG),
                    last_modified: header_text(&headers, LAST_MODIFIED),
                    total_len: 0,
                    final_url: final_url.into(),
                    // 服务器**按 Range 语义**回答了这一枪（416 是 Range 语义的回答），
                    // 只是文件恰好为空 —— 这不等于它不认 Range。
                    accept_ranges: true,
                },
                prelude: vec![],
            });
        }
        bail!(
            "试探请求收到 416（Range Not Satisfiable）：对端报回的总长是 {reported:?}，\
             与我们发的 {} 自相矛盾，无法确定这份文件的长度",
            range.as_deref().unwrap_or("(本次没发 Range)")
        );
    }
    if !status.is_success() {
        // 只报状态码，不把 URL 抄进消息：URL 由调用方自己给（`Task::validate` 已去过凭据），
        // 但这里的失败文本可能被 `{e:?}` 整条链打进日志，少一处复述就少一处。
        bail!("试探请求被服务器拒绝：HTTP {status}");
    }

    let total_len = total_len(&headers).with_context(|| {
        // 对端既没给 `Content-Range` 也没给 `Content-Length`（chunked 且不支持 Range 是这种形状）。
        // 猜一个数（尤其 0）比报错坏得多：0 在引擎里读起来就是"这活儿干完了"。见模块文档第 1 条。
        "对端既没回 Content-Length 也没回 Content-Range，无法确定文件长度（不猜一个数出来）"
    })?;
    let accept_ranges = accepts_bytes(header_text(&headers, ACCEPT_RANGES).as_deref())
        || range_honored(status, range.is_some(), &headers);

    let facts = ProbeFacts {
        etag: header_text(&headers, ETAG),
        last_modified: header_text(&headers, LAST_MODIFIED),
        total_len,
        final_url: final_url.into(),
        accept_ranges,
    };
    let prelude = read_bounded(resp, prelude_max)?;
    Ok(ProbeResult { facts, prelude })
}

/// 要不要发限量 `Range`，以及发哪一条（模块文档第 2、3 条）。
///
/// - `task.segmentable()` 为假（带 POST 体）⇒ **不发**：POST 语义下 Range 不成立。
/// - `prelude_max == 0` ⇒ 不发（这形状是"只要头、不要体"，发了 `bytes=0--1` 是会砸在解析上的谎话）。
/// - 其余：`bytes=0-<prelude_max-1>`，服务端把响应截到正好预读量 ⇒ 读得满、连接干净。
fn prelude_range(task: &Task, prelude_max: u64) -> Option<String> {
    if task.segmentable() && prelude_max > 0 {
        Some(format!("bytes=0-{}", prelude_max - 1))
    } else {
        None
    }
}

/// 把一发试探请求拼出来（与 Task 7 共用 `with_contract_headers`，两枪的头必须同源）。
///
/// 之所以在 `run()` 之前就把 `Request` 建出来（而不是 `client.get(..).send()` 一步走）：
/// 这样"请求里到底带了什么"在单元层就看得见（见 `build_request_...` 那几条用例），
/// 不必为了一条头是否存在去打网络。
fn build_request(agent: &Client, task: &Task, range: Option<&str>) -> Result<Request> {
    let req = agent.get(&task.url);
    let req = match range {
        Some(r) => req.header(RANGE, r),
        None => req,
    };
    with_contract_headers(req, task)?
        .build()
        .context("试探请求拼不出来（URL 不合法？）")
}

/// 9 项契约里会进 HTTP 头部的那几项：`Referer`、`Cookie`、`Authorization: Basic`、
/// `Accept-Encoding: identity`、`Accept: */*`。
///
/// `pub(crate)` 是给 Task 7 复用的（`fetch` 的每一枪都要带同一批头）：两处各写一遍，
/// 迟早读成两样 —— 而 ①b 判的正是"请求是不是同一个"。
///
/// 口令的"明文 HTTP 不发送"这条**不在这里实现**：那是 `Task::validate()`（E9/spec §4）的判据，
/// `run()` 在发请求之前已经过了那道闸；在这里再判一次就是第二个版本的同一条规则。
pub(crate) fn with_contract_headers(req: RequestBuilder, task: &Task) -> Result<RequestBuilder> {
    let mut req = req.header(ACCEPT_ENCODING, "identity").header(ACCEPT, "*/*");
    if let Some(referer) = &task.referer {
        req = req.header(REFERER, header_value(referer, "Referer")?);
    }
    if let Some(cookies) = &task.cookies {
        req = req.header(COOKIE, header_value(cookies, "Cookie")?);
    }
    // 用户名或口令任一给出就带 Basic：只给用户名时是 `Basic base64("user:")`（与 curl 的 `-u user:`
    // 同一形状）。反过来"给了口令却悄悄不带"是绝对不行的形状。
    if task.user.is_some() || task.password.is_some() {
        req = req.basic_auth(task.user.as_deref().unwrap_or(""), task.password.as_deref());
    }
    Ok(req)
}

/// `String` → `HeaderValue`，错误里点名是哪个字段，但**不抄值**（`Cookie`/`Referer` 都可能带着凭据，
/// 把它们抄进消息等于替它们再多开一条外传通道 —— 与 Task 5 的 ①b 同一条纪律）。
fn header_value(value: &str, field: &'static str) -> Result<HeaderValue> {
    HeaderValue::from_str(value)
        .with_context(|| format!("{field} 里有 HTTP 头装不下的字符，请求发不出去（值不回显）"))
}

/// 响应头里的一个值，按**原样文本**取（不做大小写/引号归一化，见模块文档第 6 条）。
/// 非 UTF-8 ⇒ `None`（那种值本模块的结构里装不下）。
fn header_text(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

/// 全长：先 `Content-Range` 的分母，退 `Content-Length`（模块文档第 1 条）。
///
/// 顺序不能反：206 的 `Content-Length` 是**本次响应**的字节数（限量预读时就是 `prelude_max`），
/// 把它当文件全长会让引擎以为文件只有预读那么大 —— 于是"下完 236032 字节就报告完成"。
/// 这条顺序由用例 `total_len_prefers_the_content_range_denominator` 钉着。
fn total_len(headers: &HeaderMap) -> Option<u64> {
    header_text(headers, CONTENT_RANGE)
        .and_then(|v| content_range_total(&v))
        .or_else(|| header_text(headers, CONTENT_LENGTH).and_then(|v| v.trim().parse().ok()))
}

/// `Content-Range` 里的**总长**那一段：`bytes 0-236031/50331648` ⇒ `Some(50331648)`；
/// `bytes */4194304` ⇒ `Some(4194304)`；`bytes 0-9/*`（总数未知）或任何看不懂的形状 ⇒ `None`。
///
/// 取**最后一个** `/` 之后：分段名本身不含 `/`，而这样写对 `bytes`/其他单位都成立。
fn content_range_total(value: &str) -> Option<u64> {
    let tail = value.rsplit('/').next()?.trim();
    if tail == "*" || tail.is_empty() {
        return None;
    }
    tail.parse().ok()
}

/// `Accept-Ranges` 是否说了 `bytes`（按逗号分 token、大小写不敏感）。
///
/// `none`、缺失、只列 `uncompressed` 之类 ⇒ 假。这一支只看**头**，"服务器实际按 Range 答了没有"
/// 是下面 `range_honored` 那一条，两条并列取其一（`/noranges` 两个都不成立，真实 206 两个都成立）。
fn accepts_bytes(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("bytes")))
}

/// 服务器**确实** honoring 了我们那条 Range：回 `206` 且带 `Content-Range`。
/// 这是"认 Range"的直接证据，比头更硬（有些服务器不发 `Accept-Ranges` 却照样答 206）。
fn range_honored(status: StatusCode, asked_for_range: bool, headers: &HeaderMap) -> bool {
    asked_for_range && status == StatusCode::PARTIAL_CONTENT && headers.contains_key(CONTENT_RANGE)
}

/// 读预读，**上限说话**：最多取 `prelude_max` 字节，多的留在连接里。
///
/// 这一条是 `Vec` 的内存边界（48 MiB 不许被读进来），也是模块文档第 3 条那支的唯一防线：
/// 没有 Range 可限时，"读到上限就停"是唯一合法的限量方式。
fn read_bounded(mut resp: Response, prelude_max: u64) -> Result<Vec<u8>> {
    let mut prelude = Vec::new();
    match resp.by_ref().take(prelude_max).read_to_end(&mut prelude) {
        Ok(_) => Ok(prelude),
        // 带上"已经读到多少"：这一支的排障问题是"对端在预读中途拆了连接"（`/drop/<n>` 的形状），
        // 断在哪一段直接决定 Task 7 的 `Stalled`/`Failed` 判法。
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("读预读体失败（对端在预读读完之前把连接断了？）已读到 {} 字节", prelude.len()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with(url: &str, data_file: Option<&str>) -> Task {
        Task {
            url: url.into(),
            referer: None,
            cookies: None,
            data_file: data_file.map(std::path::PathBuf::from),
            user: None,
            password: None,
            dir: ".".into(),
            name: None,
            flags: 0,
        }
    }

    /// 造一份响应头（只有解析用的那几个字段，形状与 harness 的一一对得上）。
    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                k.parse::<HeaderName>().expect("用例里的头名必须合法"),
                HeaderValue::from_str(v).expect("用例里的头值必须是合法 ASCII"),
            );
        }
        h
    }

    // ---- 限量 Range 的两个分支（简报 Step 4 / 派发第 4 条）----

    #[test]
    fn the_prelude_range_is_sent_only_for_segmentable_tasks() {
        let t = task_with("https://x/a.bin", None);
        assert_eq!(prelude_range(&t, 236_032).as_deref(), Some("bytes=0-236031"));
        // 带 POST 体：不发（POST 语义下 Range 不成立）。
        assert_eq!(prelude_range(&task_with("https://x/a.bin", Some("body.bin")), 236_032), None);
        // 上限 0：不发（不是 `bytes=0--1` 那种谎话）。
        assert_eq!(prelude_range(&t, 0), None);
        // 边界：`prelude_max` 是**字节数**，终点是它减一（含端点）⇒ 1 字节是 `bytes=0-0`、2 字节是 `bytes=0-1`。
        assert_eq!(prelude_range(&t, 1).as_deref(), Some("bytes=0-0"));
        assert_eq!(prelude_range(&t, 2).as_deref(), Some("bytes=0-1"));
    }

    // ---- 长度与验证子的解析 ----

    #[test]
    fn total_len_prefers_the_content_range_denominator() {
        // 206 的现场：Content-Length 是本次响应的 236032，全长在 Content-Range 的分母上。
        let h = headers(&[
            ("content-length", "236032"),
            ("content-range", "bytes 0-236031/50331648"),
            ("accept-ranges", "bytes"),
        ]);
        assert_eq!(total_len(&h), Some(50_331_648), "取错边就变成\"文件只有预读那么大\"");
        // 200（对端无视 Range，或 POST 那一支没发 Range）：没有 Content-Range ⇒ 退 Content-Length。
        let degraded = headers(&[("content-length", "1048576")]);
        assert_eq!(total_len(&degraded), Some(1_048_576));
        // 两个都没有 ⇒ None（`run` 在那里报错，绝不猜 0）。
        assert_eq!(total_len(&headers(&[("etag", "\"x\"")])), None);
        // Content-Length 不是数字（chunked 或对端在说谎）⇒ 也走 None。
        assert_eq!(total_len(&headers(&[("content-length", "gzip-ish")])), None);
    }

    #[test]
    fn content_range_total_reads_both_shapes_and_refuses_garbage() {
        assert_eq!(content_range_total("bytes 0-236031/50331648"), Some(50_331_648));
        assert_eq!(content_range_total("bytes */0"), Some(0), "416 那一支靠它把\"空文件\"读出来");
        assert_eq!(content_range_total("bytes */4194304"), Some(4_194_304));
        assert_eq!(content_range_total("bytes 0-9/*"), None, "总数未知不是 0");
        assert_eq!(content_range_total("bytes 0-9/"), None);
        assert_eq!(content_range_total(""), None);
        assert_eq!(content_range_total("items 0-9/10"), Some(10), "单位不参与解析（只看最后一个 / 之后）");
        assert_eq!(content_range_total("bytes 0-9/1_000"), None);
    }

    #[test]
    fn accepts_bytes_is_a_token_list_read_case_insensitively() {
        assert!(accepts_bytes(Some("bytes")));
        assert!(accepts_bytes(Some("BYTES")), "头值大小写不敏感");
        assert!(accepts_bytes(Some(" bytes ")), "空格不参与");
        assert!(accepts_bytes(Some("bytes, none")), "列了 bytes 就认");
        assert!(!accepts_bytes(Some("none")), "明说不认 Range");
        assert!(!accepts_bytes(Some("gzip, deflate")), "没有 bytes 这个 token");
        assert!(!accepts_bytes(None), "头缺失（/noranges 的形状）");
    }

    #[test]
    fn a_206_with_content_range_proves_range_support_without_the_header() {
        let h = headers(&[("content-range", "bytes 0-1/2"), ("content-length", "2")]);
        assert!(range_honored(StatusCode::PARTIAL_CONTENT, true, &h));
        // 三个方向都得是假：没问 Range、没回 206、回了 206 但没 Content-Range。
        assert!(!range_honored(StatusCode::PARTIAL_CONTENT, false, &h));
        assert!(!range_honored(StatusCode::OK, true, &h));
        assert!(!range_honored(StatusCode::PARTIAL_CONTENT, true, &headers(&[("content-length", "2")])));
    }

    #[test]
    fn validators_are_kept_verbatim_and_a_non_utf8_one_becomes_none() {
        // 原样保留：含引号的 ETag 不归一化（`resume` 的 ③ 按字符串比，改一个字符就换结论）。
        let h = headers(&[("etag", "\"probe-abc\""), ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT")]);
        assert_eq!(header_text(&h, ETAG).as_deref(), Some("\"probe-abc\""));
        assert_eq!(header_text(&h, LAST_MODIFIED).as_deref(), Some("Wed, 21 Oct 2015 07:28:00 GMT"));
        assert_eq!(header_text(&h, CONTENT_RANGE), None, "没发的头就是 None，不是空串");
        // 非 UTF-8：`ProbeFacts`/断点文件都装不下 ⇒ None（模块文档第 6 条，③ 自动落到 ④ 兜底）。
        let mut odd = HeaderMap::new();
        odd.insert(ETAG, HeaderValue::from_bytes(&[0xFF, 0xFE]).expect("构造一个非 UTF-8 的头值"));
        assert_eq!(header_text(&odd, ETAG), None);
    }

    // ---- 派发第 9 条：落地 URL 的凭据闸（判据复用 `task` 那一个函数）----

    #[test]
    fn the_landing_gate_rejects_a_userinfo_authority_and_never_echoes_it() {
        for url in [
            "https://alice:s3cret@host/x",
            "http://alice:s3cret@host/x",
            "https://alice@host/x", // 只有用户名也是凭据形态
            "HTTPS://ALICE:S3CRET@host/x",
            "https://alice:s3cret@host",
        ] {
            assert!(authority_has_userinfo(url), "夹具前提：{url} 的 authority 里有 @");
        }
        // 那条常量消息是本模块唯一的拒绝文本：它必须说清"凭据"与"重定向"，且不含任何 URL 片段。
        let msg = LANDED_CREDENTIALS;
        assert!(msg.contains("凭据") && msg.contains("重定向"), "措辞要可行动：{msg}");
        for leak in ["alice", "s3cret", "@", "http://", "https://"] {
            assert!(!msg.contains(leak), "拒绝文本里出现了 URL 的成分（{leak}）：{msg}");
        }
        assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");
    }

    #[test]
    fn the_landing_gate_leaves_an_at_that_is_not_in_the_authority_alone() {
        // 与 E9 同一判据 ⇒ 同一个边界：`@` 在 path/query/fragment 里合法，重定向落在这些位置不是凭据。
        for url in [
            "https://host/a@b.bin",
            "https://host/a.bin?to=me@host",
            "https://host/a.bin#frag@1",
            "https://host:8080/a.bin",
            "https://[fe80::1]/a.bin",
        ] {
            assert!(!authority_has_userinfo(url), "{url} 的 authority 里没有 @，不该被拒");
        }
    }

    // ---- 请求的形状（单元层就看得见，不必打网络）----

    /// 单元层的客户端：只用它**建**请求（`build_request` 不发），所以不碰网络。
    fn agent() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(3))
            .tcp_nodelay(true)
            .build()
            .expect("建 reqwest 客户端")
    }

    #[test]
    fn build_request_sends_the_prelude_range_only_when_the_task_is_segmentable() {
        let seg = task_with("http://x/a.bin", None);
        let req = build_request(&agent(), &seg, prelude_range(&seg, 236_032).as_deref())
            .expect("建请求");
        assert_eq!(req.headers().get(RANGE).map(|v| v.as_bytes()), Some(&b"bytes=0-236031"[..]));

        let post = task_with("http://x/a.bin", Some("body.bin"));
        let req = build_request(&agent(), &post, prelude_range(&post, 236_032).as_deref())
            .expect("建请求");
        assert!(
            !req.headers().contains_key(RANGE),
            "带 POST 体时一条 Range 都不许出现（spec §2.2）：{:?}",
            req.headers()
        );
    }

    #[test]
    fn build_request_carries_the_nine_item_contract_headers_that_have_a_wire_name() {
        let mut t = task_with("http://x/a.bin", None);
        t.referer = Some("https://ref.example/page".into());
        t.cookies = Some("sid=42".into());
        t.user = Some("alice".into()); // 只有用户名：明文 HTTP 也带得上（口令那一半有 validate() 管）
        let req = build_request(&agent(), &t, Some("bytes=0-1")).expect("建请求");
        let h = req.headers();
        assert_eq!(h.get(REFERER).map(|v| v.as_bytes()), Some(&b"https://ref.example/page"[..]));
        assert_eq!(h.get(COOKIE).map(|v| v.as_bytes()), Some(&b"sid=42"[..]));
        assert_eq!(h.get(ACCEPT_ENCODING).map(|v| v.as_bytes()), Some(&b"identity"[..]));
        assert_eq!(h.get(ACCEPT).map(|v| v.as_bytes()), Some(&b"*/*"[..]));
        // base64("alice:") = YWxpY2U6；本用例**没有**口令参与，所以这里可以逐字断言。
        assert_eq!(h.get("authorization").map(|v| v.as_bytes()), Some(&b"Basic YWxpY2U6"[..]));
        assert_eq!(req.url().as_str(), "http://x/a.bin", "请求打的就是任务那条 URL");

        // 反方向：什么契约头都没给 ⇒ 只有那两个通用头，绝不能凭空长出一个 Authorization。
        let bare = build_request(&agent(), &task_with("http://x/a.bin", None), None).expect("建请求");
        assert!(!bare.headers().contains_key("authorization"), "{:?}", bare.headers());
        assert!(!bare.headers().contains_key(COOKIE));
        assert!(bare.headers().contains_key(ACCEPT_ENCODING), "identity 是无条件的那一条");
    }

    #[test]
    fn a_header_value_the_wire_cannot_carry_is_named_but_not_echoed() {
        // 装不下的形状是 **CR/LF**（http  crate 允许 obs-text 那种非 ASCII 字节，所以拿"中文"试不出这一支
        // —— 我第一版就是这么写错的，实测照样 build 得出来）。CRLF 才是真风险：一个带换行的 Cookie
        // 值就是**请求头注入**的形状。
        //
        // 这一支必须**报错**，不许"跳过那个头继续发"：跳过的话这一枪就与用户要的请求不是同一个请求了，
        // 而 ①b 判的正是"确认字节的请求与这次的请求是不是同一个"。
        // 错误点名是哪个字段，但值一个字都不抄（`Cookie`/`Referer` 都可能带着凭据 ——
        // 与 Task 5 的 ①b 同一条纪律）。
        let mut t = task_with("http://x/a.bin", None);
        t.cookies = Some("sid=42\r\nInjected: yes".into());
        let Err(e) = build_request(&agent(), &t, None) else {
            panic!("带 CR/LF 的 Cookie 必须把请求建失败，不能悄悄不发这个头");
        };
        let msg = e.to_string();
        assert!(msg.contains("Cookie"), "错误要点名是哪个字段：{msg}");
        for leak in ["sid=", "Injected", "42"] {
            assert!(!msg.contains(leak), "错误里抄了字段的值（{leak}）：{msg}");
        }
        // Referer 同一条路（两个字段共用 `header_value`，只测一个等于另一个没测）。
        let mut r = task_with("http://x/a.bin", None);
        r.referer = Some("https://ref/\r\nInjected: yes".into());
        let Err(e) = build_request(&agent(), &r, None) else {
            panic!("带 CR/LF 的 Referer 同样要把请求建失败");
        };
        let msg = e.to_string();
        assert!(msg.contains("Referer"), "错误要点名是哪个字段：{msg}");
        assert!(!msg.contains("Injected"), "值不回显：{msg}");
    }

    #[test]
    fn run_refuses_an_illegal_task_before_touching_the_network() {
        // `run()` 的第一行就是 `task.validate()`：明文 HTTP 带口令（spec §4 / E9）在这一发打出去**之前**
        // 就被拒。这条走的是 `run()` 本身（不是直接调 `validate`），挡的是"忘了在请求边界加这道闸"；
        // 网络侧的独立证明归集成用例 `a_password_over_plain_http_never_leaves_the_socket`
        // （那里断"服务端一条请求都没看到"）。
        let mut t = task_with("http://not-a-real-host.invalid/a.bin", None);
        t.password = Some("hunter2".into());
        let Err(err) = run(&t, &agent(), 236_032) else {
            panic!("明文 HTTP 带口令的任务必须被 run() 拒掉，不许把它发出去");
        };
        let msg = err.to_string();
        assert!(msg.contains("口令"), "错误要说清是口令规则：{msg}");
        assert!(!msg.contains("hunter2"), "错误里不该抄口令：{msg}");
        // 这条错误必须是**校验**给的，不是连接/DNS 给的：后者不会带着"口令"两个字。
        assert!(!msg.contains("发不出去"), "看起来是先发了请求才被网络拒（那口令就已经出门了）：{msg}");
    }

    #[test]
    fn facts_hands_back_a_copy_of_the_same_record() {
        let facts = ProbeFacts {
            etag: Some("\"e\"".into()),
            last_modified: None,
            total_len: 10,
            final_url: "http://x/a".into(),
            accept_ranges: true,
        };
        let r = ProbeResult { facts: facts.clone(), prelude: vec![1, 2, 3] };
        assert_eq!(r.facts(), facts, "Task 5 的闸按值比，交回的必须是同一份");
        assert_eq!(r.prelude.len(), 3, "facts() 不许动预读");
    }

    // ---- 预读上限（内存边界）----
    //
    // 这一支**不写单元用例**：`read_bounded` 的机制就是 `Read::take(prelude_max)`，
    // 拿 `Cursor` 去测 `take` 只会证明 std 的行为，对着一个把上限写错的实现照样绿（恒绿夹具）。
    // 真正能证伪的是集成层那两条：
    // - `post_shaped_task_sends_no_range_and_still_bounds_the_prelude`：对端发 48 MiB，
    //   `prelude.len()` 必须正好是上限；
    // - `probe_reads_prelude_and_reports_ranges_supported`：对端按 Range 截到上限，读满。
}
