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
//!    206 自己就把全长说了），取不到就退到 `Content-Length` —— **这一退只在不回 `206` 时成立**
//!    （第 11 条：206 的 `Content-Length` 是本次窗口的大小，不是文件长度）。**不**为了"问出一个更大的
//!    长度"再发第二条请求：那是简报没有、spec 也没要求的动作，而且会给对端多打一发。
//! 2. **限量靠 `Range`，不靠 drop**（简报 Step 4 的机制，派发第 4 条保留）：`task.segmentable()` 时发
//!    `bytes=0-<prelude_max-1>`。**对端认 Range 时**响应于是被**完整读完**、连接干净；这同时也是**不用**
//!    `Connection: close`
//!    的前提（spec §10 勘误 **R5**、计划 **E2**：Task 6/7 不得加 close、不得设
//!    `pool_max_idle_per_host(0)`）—— Task 0 实测 416 卡死的根因在本仓库测试服务器的收尾
//!    （推流循环漏 `res.end()`），不在 reqwest 的连接池；加 close 会把"复用连接上打 416"这条真实
//!    路径从测试覆盖里关掉，正是勘误 **E3** 守卫用例要跑的那条。
//!    **对端无视那条 `Range` 时（评审 I1 补的限定），上面那句"连接干净"不成立**：body 比预读量大，
//!    `read_bounded` 停在上限、剩下的字节留在连接里 ⇒ 那条连接是被**拆掉**的，不回池复用。拆连接本身
//!    安全（我们拿到的前缀是完整的、body 从偏移 0 起没错位），但 **Task 8/10 不许把"试探之后连接可
//!    复用"当成退化路径的性质**：E3 的**三条**守卫用例跑的都是非退化那一支（`/blob` 答 206、读满预读）
//!    —— 两条 probe 版用例 `same_connection_drained_206_then_416_both_complete` 与
//!    `probe_own_drained_prelude_then_a_416_on_the_same_client`（**位置：这两条住在
//!    `tests/integration.rs`，不在本模块** —— 修复轮 8F-4a 把原先那句"本模块两条"改掉；本模块
//!    `src/probe.rs` 的单元用例按上面那句注释自己就写明"不碰网络"，造不出连接复用这件事），外加 `fetch` 版
//!    `a_drained_window_leaves_the_connection_reusable`（修复轮 8F-3a 数过：原先写"那两条"漏了第三条），
//!    退化那一支不在它们的覆盖里。**"不在覆盖里"这件事到今天仍然只对着 E3 那三条用例成立** ——
//!    8F-2 之后退化形状多了一台仪器（`tests/harness/mod.rs` 的 `spawn_scripted_peer` + `Shot::IgnoresRange`
//!    能交出**比一个窗口宽**的整条 body，node 的 `/noranges` 那份 1 MiB 封顶只是**那条路由**的性质，
//!    不是退化形状本身的性质），它钉的是"退化下载一枪下完"（`a_degraded_peer_is_fetched_in_one_body_shot_not_one_per_window`），
//!    **不是**"试探之后连接可复用" —— 后面这句仍然是**推理、不是实测**，照旧当约束用。
//! 3. **`!task.segmentable()`（带 POST 体）时不发 `Range`**：POST 语义下 Range 不成立（spec §2.2）。
//!    **评审 F1 之后这一支在 `run()` 里已经到不了** —— 带体的任务在发请求之前就被拒（见"本模块没做的
//!    事"第 1 条）。判据**留着**不删：`prelude_range` 只回答"这条任务能不能发 Range"，而"能不能试探"
//!    是另一道闸，两道闸各管各的，将来若 F1 的裁定被改回来，这一支仍然挡得住"给 POST 发 Range"。
//!    没 Range 可限量的形状于是只剩 `prelude_max == 0`（用例
//!    `a_zero_prelude_limit_reads_no_body_and_sends_no_range`）与"对端无视我们那条 Range"（用例
//!    `probe_detects_server_without_ranges`：服务端发了 1 MiB，`read_bounded` 仍停在 236032）。
//!    内存边界由 `read_bounded` 守住，绝不允许把整份 body 读进 `Vec`。
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
//!    **T99-G1c 把这一处的判据加严到两条并联**（`landed_url_is_refused`）：除了 userinfo，
//!    **非规范形状**（`http:/host`、`http:\\host`、多斜杠、http/https 之外的 scheme）也拒 ——
//!    理由与 E9 同一条：这份串会原样落进 `validator.final_url`。
//!    `authority_has_userinfo` 本体一个字没动，动的是调用点。
//! 6. **验证子按响应头的原样值保存，不归一化**（与 `resume::ProbeFacts` 的文档同一立场）；
//!    但 `ETag`/`Last-Modified` 里出现**非 UTF-8 字节**时只能记成 `None`：`ProbeFacts` 与断点文件
//!    都装不下它，`If-Range` 更没有强验证子可用。这不是"忽略噪音"—— 少了一把验证子，
//!    `resume` 的 ③ 会自动落到 ④ 的长度兜底（Task 5 裁定 2），所以是**降级但可解释**的形状。
//! 7. **试探带满 9 项契约头**（`Referer`/`Cookie`/`Authorization: Basic`/`Accept-Encoding: identity`/
//!    `Accept: */*`）：`resume` 的 ①b 判的是"确认这些字节的请求与这次的请求是不是同一个"，
//!    如果**第一枪**（定全长与 ETag 的这一枪）与后面几枪带的头不一样，`total_len`/`ETag` 就可能
//!    出自另一份文档（同一台服务器换个 Cookie 给不同长度很常见）。`Accept-Encoding: identity` 另有一层：
//!    压过体之后 `Content-Length` 说的就不是文件字节数了。
//! 8. **`Authorization` 由 `with_contract_headers` 自己按 scheme 挡**（评审 **F2**）：只有
//!    `task::url_is_https(&task.url)` 为真才带上那条头。判据**不放在** `Task::validate()` 那一侧，
//!    因为那边只管"明文 HTTP + **口令**"，而 `--user` 单独给出时它一句话都不说 —— 用户名本身也是
//!    半副凭据。**这道闸刻意只管 `Authorization`**：`Referer`/`Cookie` 不看 scheme 照发（spec §4
//!    点名的是口令，勘误 E9/R6 把这两个头登记为按设计带上/落盘），把不对称补成"全都挡"会改掉
//!    退化路径的语义。Task 7 复用这个函数时**不要**把这道闸扩大，也不要因 `fetch` 只跑 https
//!    就把它删回"无条件带 Basic"—— 四条方向都由单元用例钉着
//!    （`no_authorization_header_is_built_for_a_url_that_is_not_https` /
//!    `the_scheme_predicate_behind_that_gate_reads_every_shape_of_url` /
//!    `the_authorization_header_is_built_over_https_and_carries_both_halves` /
//!    `referer_and_cookie_cross_the_scheme_gate_by_design`），明文那一侧另有集成用例
//!    `probe_puts_the_task_headers_on_the_wire_but_not_credentials_in_the_clear` 拿**服务端日志**作证。
//! 9. **`416` + `Content-Range: bytes */0` + 我们确实发了 Range ⇒ 交回一份"全长 0"的事实**，而不是
//!    报错（评审 **F3** 裁定：**保留**；简报没写这一支）。空文件是合法的对端对象，"拒不下载一个
//!    0 字节的文件"本身就是缺陷。三条判据**并列**，缺一律拒（另两支由
//!    `a_416_that_is_not_the_empty_file_shape_is_still_an_error` 钉着）：只看到 416 就当空文件会把
//!    `total_len = 0` 交给一个 4 MiB 的文件，在引擎里读起来就是"这活儿干完了"。
//!    `accept_ranges = true` 这一味是**解读**而不是抄头：本仓库的服务器与窄对端的 416 分支都**不**发
//!    `Accept-Ranges`（线上取证见该用例文档 ③），依据是"416 本身就是按 Range 语义给的回答"。
//!    对端自己没 Range 可谈时（`prelude_max == 0`）我们不发 Range，那一支的 416 就退回报错。
//! 10. **`accept_ranges` 是合取，不是"两条证据取其一"**（评审 **I1**，判据住在 `peer_accepts_ranges`；
//!     派发点名的 spec §2.2 第二条）：头说 `bytes` 与"这次真按 Range 答了 206"两条**取其一**算证据，
//!     但**我们发过 `Range` 而响应不是 `206`** 时无条件为假。spec 那句写的是"无 `Accept-Ranges` **或**
//!     首请求返回 `200` 而非预期的 `206`"—— 那个"或"挂在**退化的触发条件**上，不是"两条证据可以互相
//!     顶替"。旧写法把合取写成了析取，于是"发 `Accept-Ranges: bytes` 却把 `bytes=0-236031` 当没有"的
//!     对端（反代吞 Range、只把源站的头透出来是常见形状）被报成认 Range ⇒ `scheduler` 的
//!     `Event::NoRanges`（spec §2.1 里叫 `ServerSaysNoRanges`）退化永不触发（段表不合并，靠后的
//!     区间会把内容写错位置），`resume` 的 ⑤
//!     也会替一份多段快照说话。**没有**给 `ProbeFacts` 加字段：这个合取已经把全部信号收进了一味，
//!     而那个结构是 Task 5 的契约（五字段，多处按字面构造）。四格真值表由单元用例
//!     `peer_accepts_ranges_is_conjunctive_once_we_sent_a_range` 钉，线上那一侧由
//!     `a_peer_that_answers_200_to_our_range_is_not_range_capable_however_its_header_reads` 与
//!     `an_ordinary_redirect_still_lands_and_is_reported` 钉（后一条的期望**是本轮更正掉的**：它原来
//!     钉的正是这个缺陷）。
//! 11. **`206` 说不出可用的总长 ⇒ 拒绝，不拿 `Content-Length` 兜底**（评审 **I2**，`total_len` 的
//!     第二参）：206 的 `Content-Length` 按定义只是**本次窗口**的大小，而窗口又正好等于预读量，于是
//!     那份谎**永远自洽** —— `accept_ranges` 靠真 206 读成 true、预读长度对上假全长、spec §6 第 7 条
//!     "没给 `--sha256` 就只校验已写字节数 == total_len"照样通过 ⇒ 236 KiB 的截断文件被报成 48 MiB
//!     下完了，之后每一次续传都同意这个谎。这正是本函数自己那条注释（"把它当文件全长会让引擎以为
//!     文件只有预读那么大"）警告的形状。触发它**不需要**对端撒谎：`Content-Range: bytes 0-236031/*`
//!     是 RFC 9110 §14.3 允许的合法回答（总数未知），分母是垃圾、或 206 干脆不带 `Content-Range`
//!     也算。修法与"两个长度头都没有"那一支同一立场（不猜一个数、错误里**不回显那个数字**）；
//!     `Content-Length` 兜底只在**非 206**（退化的 `200`，模块文档第 4 条）时合法。
//!
//! ## 本模块**没**做的事（照实登记，不是遗漏）
//!
//! - **带 `--data-file` 的任务一律不试探**（评审 **F1** 的裁定，`run()` 里那道 `ensure!`）。
//!   简报 Step 4 的字面形状是"GET 无 Range，`segmentable()` 时加 Range"，于是对带体的任务第一枪会
//!   退化成**一条无体的 GET**：取回的 `total_len`/`ETag`/`final_url`/`accept_ranges` 说的是那条 GET，
//!   而真正下载发的是 POST ⇒ `resume::decide` 的 ②③④ 与 ⑤ 就在校验"一条我们从未发过的请求确认过的
//!   字节"，那正是 Task 5 补 ①b 要挡的形状。另一条显而易见的"修法"（试探也发 POST，之后再 POST 一次
//!   下载）**被明确否决**：它把用户的表单交**两次**，对端可能留下真副作用（下单/投票/发信），
//!   而用户以为工具只下一次。
//!   **交接（Task 7 修复轮已完成，不再是待裁事项）**：`engine`（Task 8）在这种形状上**跳过 probe**，
//!   事实取自 `fetch` **真正发出的那一枪**的响应头。落地的形状是把"响应头 + 状态 + 有没有发 Range +
//!   落地 URL → `ProbeFacts`"抽成 `pub(crate)` 的 `facts_from_response`（本模块**唯一**一份读法），
//!   `run()` 与 `fetch::run_whole` 各调它一次 —— 两枪的头仍与 `with_contract_headers` 同源，
//!   否则 ①b 又落空。抽取是**行为不变**的：`total_len` / `peer_accepts_ranges` / `header_text` /
//!   落地 URL 那道闸四个判据与两段拒绝措辞都逐字搬走，钉它们的老用例一条都没改。
//!   这一支的用例是 `a_data_file_task_is_refused_before_the_probe_touches_the_wire`（断"拒在上网之前"）。
//! - 不重试、不退避（spec §6.1/§6.2 的账归 fetch/engine），不落盘、不碰 `store`，不读断点文件。
//!   本模块唯一的 I/O 就是那一发请求。
//! - 不校验 `total_len` 与已下段表是否自洽（那是 `resume` 的 ④ 与 `state::check_segments`）。

use std::io::Read;

use anyhow::{bail, ensure, Context, Result};
use reqwest::blocking::{Client, Request, RequestBuilder, Response};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_LENGTH,
    CONTENT_RANGE, COOKIE, ETAG, LAST_MODIFIED, RANGE, REFERER,
};
use reqwest::StatusCode;

use crate::resume::ProbeFacts;
use crate::task::{Task, authority_has_userinfo, canonical_http_authority, url_is_https};

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
///
/// `pub(crate)` 是 Task 7 的可见性改动（逻辑一个字没动）：`fetch` 的每一枪也可能被重定向到一条
/// 带凭据的落地 URL（spec §6.5 + Task 5 评审 C3），同一个闸必须用**同一句**话拒绝。
pub(crate) const LANDED_CREDENTIALS: &str = "服务器重定向后的地址在主机名前面带着凭据（userinfo 那一段）：\
                                  它会被写进断点文件、并出现在续传冲突的原因里，因此拒绝收下这份事实；\
                                  本消息不回显那条地址";

/// 落地 URL 的**调用点**判据（T99-G1c §1.2）：两条闸**并联**，任一不过就拒。
///
/// 1. `task::authority_has_userinfo` —— E9 / Task 5 评审 C3 那一支，**本体一个字没动**；
/// 2. `task::canonical_http_authority` —— T99-G1c 新立的"只认规范写法"：非规范形状
///    （`http:/host`、`http:\\host`、多斜杠、其它 scheme、空 host）也拒。
///
/// 为什么要第 2 条：`final_url` 会原样落进 `validator.final_url`（与 E9 同一条理由），而
/// `resume` 的 ② 会把它连同断点里那份一起打进原因串 —— 一条我们**判不出形状**的落地 URL
/// 不该被收下，即使它此刻看起来没有 `@`。
///
/// 两条是**并集**关系，不是包含关系，各有对方够不着的一族：
/// - 第 1 条更严的那一族：`@` 前面出现 `\`（它的终止符集合是 `/` `?` `#`，截到的 authority 更长，
///   于是看得见那个 `@`；第 2 条按四个终止符切，`\` 就是分隔 ⇒ authority 里没有 `@`）。
/// - 第 2 条更严的那一族：分隔符**根本不是** `://`（`http:/host`、`http:\\host`、多斜杠那一族）。
///   那种串下第 1 条走"没有合法 scheme ⇒ 从串首找第一个 `/`"的保守支，只要 `@` 前面有 `/` 就恒假 ——
///   控制员实测的 `http:/u:PASS@127.0.0.1:8899/x1.bin` 正是这一族（T99-G1b 报告 §6.1）。
///
/// 取并集 ⇒ 任一侧认出来的坏味道都拒，不会因为"共用一个判据"而互相放水。
///
/// ⚠ **入参不由我们控制**（是服务器给的），所以这里不做归一之外的任何改写：只判、只拒，
/// 不改写、不回显。
fn landed_url_is_refused(url: &str) -> bool {
    authority_has_userinfo(url) || canonical_http_authority(url).is_none()
}

/// 发**一发**试探请求，取回事实与预读。
///
/// `agent` 由调用方持有并复用（Task 8 的整轮都用同一个客户端，这样连接池才有意义）；
/// `prelude_max` 是预读上限。**今天它只有一个来源：`engine::run` 直接传 `consts::PROBE_READ_BYTES`**。
/// ⚠ 原先这句写的是"`consts::PROBE_READ_BYTES` 是它的默认值，Task 9 允许覆盖" —— 那句是**假功能**：
/// spec §5 那十二个长选项里**没有任何一个**能改这个数（`--connections`／`--min-seg` 之外没有旋钮），
/// 而 `Config` 里也没有对应字段 ⇒ "允许覆盖"落不了地，读者还会以为 CLI 该暴露它。
/// ⇒ **作废那句**。真要给覆盖口子，得先往 `Config` 加字段（契约变更）并补一条 spec §5 的选项，
/// 那是独立裁定，不在 v1，也不许由 Task 9 顺手发明。
pub fn run(task: &Task, agent: &Client, prelude_max: u64) -> Result<ProbeResult> {
    // 请求边界上的第二道闸：`Task::validate()` 管着"明文 HTTP 不送口令"与 E9 的 userinfo URL。
    // 与 `state::Snapshot::from_task` 同一个立场（落盘点是第二个强制点，这里是**发请求**这个第二强制点），
    // 规则本身只住在 `Task::validate()` 一处。**不加 `.context()`**：anyhow 的 `to_string()` 只显示
    // 最外层，包一层会把"口令"三个字从消息里挤掉（`state.rs` 里同一条理由）。
    task.validate()?;

    // 评审 **F1** 裁定：带 `--data-file` 的任务**不试探**（模块文档「本模块没做的事」第 1 条）。
    // 这一道闸必须排在**建请求之前**：本轮的缺陷正是"照简报 Step 4 的形状把 GET 打出去"，
    // 打出去的那一枪确认的是另一条请求的字节 ⇒ `resume` 的 ②③④ 与 ⑤ 就在校验从未确认过的东西。
    // 消息点名 `--data-file`（那是用户能改的那个旋钮），并且不抄体的任何一截、不抄凭据
    // （与 `header_value` 同一条纪律）。**不加 `.context()`**：理由同上面那道闸。
    ensure!(
        task.data_file.is_none(),
        "带 --data-file 的任务不做试探：GET 试探确认的是另一条（无体）请求的字节，\
         而 POST 试探会把同一份表单交两次（下单/投票/发信都是真副作用），两头都不接受"
    );

    let range = prelude_range(task, prelude_max);
    let req = build_request(agent, task, range.as_deref())?;
    let resp = match agent.execute(req) {
        Ok(resp) => resp,
        Err(e) => {
            // 跟随重定向**中途**失败时，那条带凭据的落地 URL 只出现在 reqwest 的错误里
            // （`Error::url()` 就是它，见 reqwest 0.13.5 `async_impl/client.rs` 里
            // `Pending::poll` 用 tower-http 的 `RequestUri` 覆写 `self.url` 那一段）⇒
            // 先把同一个判据套上去，换成一句不回显的拒绝。
            if e.url().is_some_and(|u| landed_url_is_refused(u.as_str())) {
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
    if landed_url_is_refused(final_url.as_str()) {
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

    let facts = facts_from_response(status, range.is_some(), &headers, final_url.as_str())?;
    let prelude = read_bounded(resp, prelude_max)?;
    Ok(ProbeResult { facts, prelude })
}

/// **响应头 → `ProbeFacts` 的唯一一份读法**（修复轮 F2 的裁定：`probe` 与 `fetch` 不许各写一个）。
///
/// 参数就是这条读法需要的四样东西：状态码、**我们这次有没有发 `Range`**、响应头、落地 URL。
/// 交回 Task 5 契约的那五个字段，其中两条是 Task 6 评审**强制**下来的读法，本函数把它们原样带走：
/// - `accept_ranges` 是**行为加权**的合取（评审 **I1**，判据在 `peer_accepts_ranges`）；
/// - `206` 说不出可用总长时**不许**退到 `Content-Length`（评审 **I2**，判据在 `total_len`）。
///
/// 抽出来是因为 `fetch::run_whole` 必须有同一份读法：带 `--data-file` 的形状**不能试探**
/// （`run()` 里那道 `ensure!`，评审 **F1**），它的事实只能取自真发出去的那一枪的响应头。
/// 两处各写一遍，迟早读成两样 —— 而 ①b/②③④ 校验的正是"确认字节的那个请求"。
///
/// ## 本函数**不**管的事（各调用方自己决定，不算重复实现）
///
/// "哪些状态码对我这一枪算可接受"是**每个模块自己的**决定，不是响应头的读法：
/// - `probe::run`：416 + `bytes */0` + 确实发了 Range ⇒ 空文件那份事实（评审 **F3**，三条并列），
///   其余非 2xx ⇒ 报错；
/// - `fetch::run_window`：416 ⇒ 拿分母与我们的 `total_len` 比（裁定 2 / 勘误 **E7**），是个 `Outcome`；
/// - `fetch::run_whole`：非 2xx ⇒ `Failed(Http(code))`，也是 `Outcome` 而不是 `Err`。
///
/// 所以那两支留在调用方，本函数只管"给定一个 2xx 的响应，这五个字段是什么"。
///
/// 落地 URL 那道凭据闸（模块文档第 5 条 / Task 5 评审 **C3** / 勘误 **E9**）**在本函数里**：
/// 它是"事实能不能收下来"的一部分，而 `fetch` 这一枪同样可能被重定向到带 userinfo 的落地地址。
/// 对 `probe::run` 是幂等的（那边排在更前面），对 `fetch` 是唯一的一道。
pub(crate) fn facts_from_response(
    status: StatusCode,
    asked_for_range: bool,
    headers: &HeaderMap,
    final_url: &str,
) -> Result<ProbeFacts> {
    if landed_url_is_refused(final_url) {
        bail!("{LANDED_CREDENTIALS}");
    }
    let total_len = total_len(headers, status).with_context(|| {
        // 两种"定不出长度"分开说，因为缺的东西不同（评审 **I2**）：
        // - 206：分母不可用（`/*`、垃圾、或压根没发 `Content-Range`）。这一支**不许**退到
        //   `Content-Length` —— 那只是本次窗口的大小，猜出来的数永远自洽（见 `total_len` 的文档）。
        // - 其余：两个头都没有（chunked 且不支持 Range 是这种形状）。
        // 两支都只点名**缺的是哪个头**，一个长度值都不抄进消息：猜一个数（尤其 0）比报错坏得多，
        // 把那个数抄出来也一样（用户会照着它判断，而它正是我们不确定起来的那个东西）。
        if status == StatusCode::PARTIAL_CONTENT {
            "服务器回了 206 却没在 Content-Range 里给出可用的文件总长度，无法确定文件长度\
             （206 的 Content-Length 只是本次窗口的大小，不拿它猜一个数出来）"
        } else {
            "对端既没回 Content-Length 也没回 Content-Range，无法确定文件长度（不猜一个数出来）"
        }
    })?;
    Ok(ProbeFacts {
        etag: header_text(headers, ETAG),
        last_modified: header_text(headers, LAST_MODIFIED),
        total_len,
        final_url: final_url.to_string(),
        accept_ranges: peer_accepts_ranges(status, asked_for_range, headers),
    })
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
/// **本函数只发 `GET`**（`agent.get(..)`，别无分号）：这是简报 Step 4 的字面形状，也是 F1 那道
/// `ensure!` 存在的原因 —— 带体的任务到不了这里。将来若要让试探覆盖 POST，改的应当是**去掉试探**
/// 而不是在这里加一个 POST 分支（理由见模块文档"本模块没做的事"第 1 条）。
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
/// ## `Authorization` 只在 https 上发（评审 **F2**），Task 7 **不要"顺手补上"**
///
/// 判据住在**这里**（不在 `Task::validate()` 里）：决定"什么东西上得了线"的地方就是构造请求头的
/// 这个地方。`Task::validate()` 那条规则管的是**明文 HTTP + 口令**（spec §4 / 勘误 E9），它**不说**
/// 只给了 `--user` 的任务 —— 可用户名本身也是半副凭据（`Basic base64("user:")`），在明文的 socket
/// 上一样是别人能读的。scheme 的比较复用 `task::url_is_https`（大小写不敏感，认不出 scheme 时算
/// "不是 https"），与 `validate()` 那一侧不会读成两样。
///
/// **这条闸是刻意不对称的**：`Referer` 与 `Cookie` 不看 scheme，照发。理由是裁定里点名的分工 ——
/// spec §4 那条硬规约说的是**口令**；勘误 **E9/R6** 把 `referer`/`cookies` 登记为"按设计落盘/带上"
/// 的字段。把它们一起挡掉看着更"安全"，实际会改掉退化路径的行为：Cookie 一丢，同一台服务器给的
/// 可能就是另一份文档，`total_len`/`ETag` 又出自另一个请求，①b 落空。**所以 Task 7 复用本函数时
/// 不要"热心地"把这道闸扩大到那两个头上**，也不要因为 `fetch` 只发 https 就把它删掉。
///
/// 被挡掉时本模块**不报错**（与口令那一半不同：那边是 `validate()` 直接拒整个任务）。用户如果真
/// 需要凭据，把 URL 写成 `https://` 就是唯一的修法 —— 而退化到 `http://` 的镜像站拿不到那个头。
pub(crate) fn with_contract_headers(req: RequestBuilder, task: &Task) -> Result<RequestBuilder> {
    let mut req = req.header(ACCEPT_ENCODING, "identity").header(ACCEPT, "*/*");
    if let Some(referer) = &task.referer {
        req = req.header(REFERER, header_value(referer, "Referer")?);
    }
    if let Some(cookies) = &task.cookies {
        req = req.header(COOKIE, header_value(cookies, "Cookie")?);
    }
    // 用户名或口令任一给出、**并且这条 URL 是 https** 才带 Basic：只给用户名时是
    // `Basic base64("user:")`（与 curl 的 `-u user:` 同一形状）。反过来"给了口令却悄悄不带"是
    // 绝对不行的形状 —— 所以这一支只在**明文**上被 scheme 挡，不在 https 上被别的东西挡。
    if (task.user.is_some() || task.password.is_some()) && url_is_https(&task.url) {
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

/// 全长：先 `Content-Range` 的分母，退 `Content-Length`（模块文档第 1 条、第 11 条）。
///
/// 顺序不能反：206 的 `Content-Length` 是**本次响应**的字节数（限量预读时就是 `prelude_max`），
/// 把它当文件全长会让引擎以为文件只有预读那么大 —— 于是"下完 236032 字节就报告完成"。
/// 这条顺序由用例 `total_len_prefers_the_content_range_denominator` 钉着。
///
/// **但 `206` 这一支连"退"都不许退**（评审 **I2**，集成用例
/// `a_206_without_a_usable_total_is_refused_instead_of_reading_the_window_as_the_length`）：
/// 分母取不到就交 `None` 让 `run()` 报错。理由是那份
/// 谎**永远自洽** —— `Content-Range: bytes 0-236031/*` 是 RFC 9110 §14.3 允许的合法回答（总长未知），
/// 退化读法于是把窗口 236032 报成全长，而 `accept_ranges` 靠真 206 读成 true、预读正好等于假全长、
/// spec §6 第 7 条"没给 `--sha256` 就只校验已写字节数 == total_len"照样通过 ⇒ 截断的 236 KiB 被报成
/// 48 MiB 下载完成，之后每次续传都同意这个谎。`200`（对端无视我们那条 Range，或本次根本没发 Range）
/// 才轮到 `Content-Length`：那一份就是它交出来的整个 body，模块文档第 4 条。
fn total_len(headers: &HeaderMap, status: StatusCode) -> Option<u64> {
    header_text(headers, CONTENT_RANGE)
        .and_then(|v| content_range_total(&v))
        .or_else(|| {
            if status == StatusCode::PARTIAL_CONTENT {
                None
            } else {
                header_text(headers, CONTENT_LENGTH).and_then(|v| v.trim().parse().ok())
            }
        })
}

/// `Content-Range` 里的**总长**那一段：`bytes 0-236031/50331648` ⇒ `Some(50331648)`；
/// `bytes */4194304` ⇒ `Some(4194304)`；`bytes 0-9/*`（总数未知）或任何看不懂的形状 ⇒ `None`。
///
/// 取**最后一个** `/` 之后：分段名本身不含 `/`，而这样写对 `bytes`/其他单位都成立。
///
/// `pub(crate)` 是 Task 7 的可见性改动（逻辑一个字没动）：`fetch` 判 416 时要读的就是
/// `bytes */N` 里那个 `N`（勘误 **E7**：`N` 与我们的 `total_len` 不一致 ⇒ 对端的长度在我们脚下
/// 变了，不许当成"这活儿干完了"）。两个模块各写一个 `Content-Range` 解析器，迟早读成两样。
pub(crate) fn content_range_total(value: &str) -> Option<u64> {
    let tail = value.rsplit('/').next()?.trim();
    if tail == "*" || tail.is_empty() {
        return None;
    }
    tail.parse().ok()
}

/// `Accept-Ranges` 是否说了 `bytes`（按逗号分 token、大小写不敏感）。
///
/// `none`、缺失、只列 `uncompressed` 之类 ⇒ 假。这一支只看**头**，"服务器实际按 Range 答了没有"
/// 是下面 `range_honored` 那一条；**两条怎么合住在那里**（`peer_accepts_ranges`），不在这里 ——
/// 头单独成立过不了评审 **I1** 那一道："它说了认 Range"和"它这次真按 Range 答了"不是一回事。
fn accepts_bytes(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("bytes")))
}

/// 服务器**确实** honoring 了我们那条 Range：回 `206` 且带 `Content-Range`。
/// 这是"认 Range"的直接证据，比头更硬（有些服务器不发 `Accept-Ranges` 却照样答 206）。
///
/// 反方向也成立，所以这一味只算**证据**、不算结论：回了 `206` 却没带 `Content-Range` 的
/// 是畸形对端（本模块随后还会因为定不出总长而拒它，见 `total_len`）。
fn range_honored(status: StatusCode, asked_for_range: bool, headers: &HeaderMap) -> bool {
    asked_for_range && status == StatusCode::PARTIAL_CONTENT && headers.contains_key(CONTENT_RANGE)
}

/// `ProbeFacts.accept_ranges` 的**合并规则**（评审 **I1**；模块文档第 10 条）。
///
/// 两条证据取其一（头说认 / 行为上答了 206），**但**我们发过 `Range` 而对端没回 `206` 时一律为假。
/// 依据是 spec §2.2 的第二条：它把"无 `Accept-Ranges`"与"首请求返回 `200` 而非预期的 `206`"列为
/// **两个各自独立的退化触发条件**（"或"字挂在触发条件上），不是两条可以互相顶替的证据。
/// HTTP 层面这件事没有第二种读法：我们问了区间、它给了 `200`，那它就不按 Range 答话，头里写什么都一样。
///
/// 旧写法 `accepts_bytes(..) || range_honored(..)` 取的是后一种读法，代价具体：
/// "发 `Accept-Ranges: bytes`、却把我们的 `bytes=0-236031` 当没有"的对端会被报成认 Range ⇒
/// `scheduler` 那条 `Event::NoRanges` 退化永不触发（段表不合并，**靠后的区间会把内容写错位置**），
/// `resume` 的 ⑤ 也会替一份多段快照说话、把接不上的进度当接得上。
///
/// 四格真值表（头有无 × 答没答 206）由 `peer_accepts_ranges_is_conjunctive_once_we_sent_a_range` 钉；
/// 线上那一侧由 `a_peer_that_answers_200_to_our_range_is_not_range_capable_however_its_header_reads`
/// 与 `an_ordinary_redirect_still_lands_and_is_reported` 钉（本仓库 `/noranges` 两条证据**一起**坏，
/// 单独它挡不住"合取还是析取"这一维）。
fn peer_accepts_ranges(status: StatusCode, asked_for_range: bool, headers: &HeaderMap) -> bool {
    let evidence = accepts_bytes(header_text(headers, ACCEPT_RANGES).as_deref())
        || range_honored(status, asked_for_range, headers);
    let ignored_our_range = asked_for_range && status != StatusCode::PARTIAL_CONTENT;
    evidence && !ignored_our_range
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
        // 带 POST 体：不发。F1 之后这一支在 `run()` 里到不了（带体的任务整个不被试探），留着的理由是
        // 模块文档第 3 条："能不能发 Range"与"能不能试探"是两道闸，不合并成一道。
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
        assert_eq!(
            total_len(&h, StatusCode::PARTIAL_CONTENT),
            Some(50_331_648),
            "取错边就变成\"文件只有预读那么大\""
        );
        // 200（对端无视我们那条 Range，或 `prelude_max == 0` 那一支根本没发 Range）：没有
        // Content-Range ⇒ 退 Content-Length。这一支**必须**继续成立：模块文档第 4 条说得很清楚，
        // 退化路径的长度就是这一次响应交出来的字节数。
        let degraded = headers(&[("content-length", "1048576")]);
        assert_eq!(total_len(&degraded, StatusCode::OK), Some(1_048_576));
        // 两个都没有 ⇒ None（`run` 在那里报错，绝不猜 0）。
        assert_eq!(total_len(&headers(&[("etag", "\"x\"")]), StatusCode::OK), None);
        // Content-Length 不是数字（chunked 或对端在说谎）⇒ 也走 None。
        assert_eq!(total_len(&headers(&[("content-length", "gzip-ish")]), StatusCode::OK), None);
    }

    /// 评审 **I2**：`206` 上"分母不可用"这一支**不许**退到 `Content-Length`（那是本次窗口的大小）。
    ///
    /// 三种形状都算：`*`（RFC 9110 §14.3 允许的"总数未知"，**不需要**对端撒谎）、分母不是数字、
    /// 206 干脆不带 `Content-Range`。线上取证（真打一次网络的那三条）归集成用例
    /// `a_206_without_a_usable_total_is_refused_instead_of_reading_the_window_as_the_length`；
    /// 这里钉的是判据本身，因为它同时钉住"别把窗口当全长"和"别顺手把 200 那支也一起改掉"。
    #[test]
    fn a_206_without_a_usable_content_range_total_has_no_fallback_to_the_window() {
        // 合法但对端说不出总数：窗口 236032 就在 Content-Length 上等着被误读。
        let unknown = headers(&[
            ("content-length", "236032"),
            ("content-range", "bytes 0-236031/*"),
            ("accept-ranges", "bytes"),
        ]);
        assert_eq!(total_len(&unknown, StatusCode::PARTIAL_CONTENT), None, "不是 236032");
        // 分母是垃圾（`content_range_total` 已经在 `bytes 0-9/*` 那一格给 None，这里是第二种坏法）。
        let garbage = headers(&[("content-length", "236032"), ("content-range", "bytes 0-236031/nine")]);
        assert_eq!(total_len(&garbage, StatusCode::PARTIAL_CONTENT), None);
        // 206 压根不带 `Content-Range`（畸形对端，`range_honored` 那一条也会给假）。
        let missing = headers(&[("content-length", "236032"), ("accept-ranges", "bytes")]);
        assert_eq!(total_len(&missing, StatusCode::PARTIAL_CONTENT), None);
        // 反方向（不能顺手改坏的那一支）：同一批头、状态是 `200` ⇒ 这一次它就是交出了整个 body，
        // `Content-Length` 兜底合法（模块文档第 4 条；线上取证是集成用例
        // `a_peer_that_answers_200_to_our_range_is_not_range_capable_however_its_header_reads`）。
        // 注意 `unknown` 里那条 `Content-Range: bytes 0-236031/*` 在 200 上同样是"分母不可用"，
        // 两种坏法（`/*` 与压根没发）在这一支上落到同一个读数 —— 变的只有状态码。
        assert_eq!(total_len(&unknown, StatusCode::OK), Some(236_032), "200 上分母不可用 ⇒ 照旧退 Content-Length");
        assert_eq!(total_len(&missing, StatusCode::OK), Some(236_032), "200 + 只有 Content-Length ⇒ 照旧退它");
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

    /// 评审 **I1** 的真值表：**发过 Range 之后，"头怎么说"就不作数了**。
    ///
    /// 为什么单测这一层（集成那两条钉的是线上形状，这一条钉的是判据的格子）：② 那一格是原裁定
    /// 留下的"行为比头硬"（本轮**没有**动它），③ 那一格才是缺陷 —— 旧写法把 `||` 用在了 ③ 上，
    /// 于是③ 读成 true。④（`/noranges`）在旧写法下也给 false，所以那条路由挡不住这个缺陷；
    /// 线上进取证因此用的是 ③ 那一格的两个真对端用例（见模块文档第 10 条）。
    #[test]
    fn peer_accepts_ranges_is_conjunctive_once_we_sent_a_range() {
        let header_and_206 = headers(&[("accept-ranges", "bytes"), ("content-range", "bytes 0-1/2")]);
        let only_206 = headers(&[("content-range", "bytes 0-1/2")]);
        let header_but_200 = headers(&[("accept-ranges", "bytes"), ("content-length", "2")]);
        let nothing = headers(&[("content-length", "2")]);

        // ① 发了 Range、回 206、带头：两条证据都在 ⇒ true（诚实的限量预读，`/blob` 的形状）。
        assert!(peer_accepts_ranges(StatusCode::PARTIAL_CONTENT, true, &header_and_206));
        // ② 回了 206 + Content-Range 但**没有** `Accept-Ranges` ⇒ true（原裁定：行为比头硬）。
        assert!(peer_accepts_ranges(StatusCode::PARTIAL_CONTENT, true, &only_206));
        // ③ **本轮的缺陷格**：头说 `bytes`，行为上把我们的 `bytes=0-…` 当没有（回 200）⇒ false。
        assert!(
            !peer_accepts_ranges(StatusCode::OK, true, &header_but_200),
            "问了区间却拿 200 回答 ⇒ 这个对端不按 Range 答话，头里写什么都一样"
        );
        // ④ 头没有、也没答 206（`/noranges`）⇒ false。
        assert!(!peer_accepts_ranges(StatusCode::OK, true, &nothing));

        // `Accept-Ranges: none` 与"头缺失"在 `accepts_bytes` 里同一读法（都不给 `bytes` 这个 token），
        // 所以 ③/④ 与 ② 的对应格子必须一致，别让"明说不认"反而比"没说"更容易被当成认。
        let says_none = headers(&[("accept-ranges", "none"), ("content-length", "2")]);
        assert!(!peer_accepts_ranges(StatusCode::OK, true, &says_none));
        let none_but_206 = headers(&[("accept-ranges", "none"), ("content-range", "bytes 0-1/2")]);
        assert_eq!(
            peer_accepts_ranges(StatusCode::PARTIAL_CONTENT, true, &none_but_206),
            peer_accepts_ranges(StatusCode::PARTIAL_CONTENT, true, &only_206),
            "头明说 none 与头缺失必须走同一格（都是\"没有行为以外的证据\"）"
        );

        // 没发 Range 的那两格（`prelude_max == 0`）只看头：那时我们没有行为证据可看。
        assert!(peer_accepts_ranges(StatusCode::OK, false, &header_but_200));
        assert!(!peer_accepts_ranges(StatusCode::OK, false, &nothing));
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
        // （`run()` 到不了这一支：F1 的 `ensure!` 先把带体的任务拒了。这里单测的是"万一有 Range 参数
        // 之外的入口"，`build_request` 自己也不许凭空长出一条 Range。）
        assert!(
            !req.headers().contains_key(RANGE),
            "带 POST 体时一条 Range 都不许出现（spec §2.2）：{:?}",
            req.headers()
        );
    }

    #[test]
    fn build_request_carries_the_nine_item_contract_headers_that_have_a_wire_name() {
        let mut t = task_with("https://x/a.bin", None);
        t.referer = Some("https://ref.example/page".into());
        t.cookies = Some("sid=42".into());
        t.user = Some("alice".into()); // https ⇒ F2 那道闸放行（它只看 scheme，别看别的）
        let req = build_request(&agent(), &t, Some("bytes=0-1")).expect("建请求");
        let h = req.headers();
        assert_eq!(h.get(REFERER).map(|v| v.as_bytes()), Some(&b"https://ref.example/page"[..]));
        assert_eq!(h.get(COOKIE).map(|v| v.as_bytes()), Some(&b"sid=42"[..]));
        assert_eq!(h.get(ACCEPT_ENCODING).map(|v| v.as_bytes()), Some(&b"identity"[..]));
        assert_eq!(h.get(ACCEPT).map(|v| v.as_bytes()), Some(&b"*/*"[..]));
        // base64("alice:") = YWxpY2U6；本用例**没有**口令参与，所以这里可以逐字断言。
        assert_eq!(h.get("authorization").map(|v| v.as_bytes()), Some(BASIC_ALICE_COLON));
        assert_eq!(req.url().as_str(), "https://x/a.bin", "请求打的就是任务那条 URL");

        // 反方向：什么契约头都没给 ⇒ 只有那两个通用头，绝不能凭空长出一个 Authorization。
        let bare = build_request(&agent(), &task_with("https://x/a.bin", None), None).expect("建请求");
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

    // ---- 评审 F1：带 `--data-file` 的任务**不许试探** ----

    /// 体文件的哨兵：写进真实的临时文件里，用来证明拒绝文案一个字都不抄那份体。
    const BODY_SENTINEL: &str = "form-body-sentinel-8f31";

    /// 造一个**真的**体文件（`<仓库>/target/t6-scratch/`，`target/` 已在 `.gitignore` 里）。
    ///
    /// 为什么要真文件而不是一个假路径：本条用例断的是"错误里没有体的字节"。拿一个不存在的
    /// 路径去断这句话是**恒真**的（谁都抄不出没读过的东西）；文件真的在那儿、内容真的带哨兵，
    /// 后来人若把"读体 → 把体放进请求/错误"当成修法，这条就会红。
    fn scratch_body(content: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("t6-scratch");
        std::fs::create_dir_all(&dir).expect("建 t6-scratch 目录");
        // 先扫掉上一轮崩在半路的残留（这条目录只有本用例用，串行跑，删别人的文件不可能发生）。
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let p = dir.join("f1-body.bin");
        std::fs::write(&p, content).expect("写体文件");
        p
    }

    #[test]
    fn a_task_carrying_a_data_file_is_refused_rather_than_probed_with_a_get() {
        let body = scratch_body(&format!("{BODY_SENTINEL}&cvv=999"));
        let mut t = task_with("https://not-a-real-host.invalid/a.bin", Some(&body.display().to_string()));
        t.user = Some("alice".into());
        t.password = Some("hunter2".into()); // https 上带口令是合法的 ⇒ 走到的是 data_file 那道闸，不是口令闸
        let Err(err) = run(&t, &agent(), 236_032) else {
            panic!("带 --data-file 的任务必须被 probe 拒掉：发 GET 会把另一条请求的事实当成本次的（①b 挡的那种）");
        };
        let msg = err.to_string();
        assert!(msg.contains("--data-file"), "错误要点名那个参数，用户才知道改什么：{msg}");
        assert!(!msg.contains(BODY_SENTINEL) && !msg.contains("cvv"), "错误里抄了体的字节：{msg}");
        assert!(!msg.contains("hunter2") && !msg.contains("alice"), "错误里抄了凭据：{msg}");
        assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");
        assert!(!msg.contains("发不出去"), "看起来是先发了请求才被网络拒（那已经把 GET 打出去了）：{msg}");
        let _ = std::fs::remove_file(&body);
    }

    // ---- 评审 F2：`Authorization` 的 https 闸住在**头部构造**里，不只在 `Task::validate()` ----

    /// `base64("alice:hunter2")`，逐字写出来只为了断言"两半都进了那条头"（只断"存在"看不出漏了口令）。
    const BASIC_ALICE_HUNTER2: &[u8] = b"Basic YWxpY2U6aHVudGVyMg==";
    /// `base64("alice:")`（curl `-u alice:` 的形状）。
    const BASIC_ALICE_COLON: &[u8] = b"Basic YWxpY2U6";

    #[test]
    fn no_authorization_header_is_built_for_a_url_that_is_not_https() {
        // 正主是前两条：`--user` 而**没有** `--password` 的任务，`Task::validate()` 一句话都不说
        // （它只拦"明文 HTTP + 口令"），可用户名本身也是半副凭据 ⇒ 这一支必须由构造头的地方自己挡住。
        // 口令那一半也照样列在这里：闸住在这里就意味着"与 validate() 无关也不上线"，不是补它的漏。
        for url in ["http://x/a.bin", "HTTP://x/a.bin", "HtTp://x/a.bin"] {
            let mut t = task_with(url, None);
            t.user = Some("alice".into());
            let req = build_request(&agent(), &t, None).expect("建请求");
            assert!(
                !req.headers().contains_key("authorization"),
                "{url} 不是 https，Authorization 不许出现在请求上：{:?}",
                req.headers()
            );

            let mut both = task_with(url, None);
            both.user = Some("alice".into());
            both.password = Some("hunter2".into());
            let req = build_request(&agent(), &both, None).expect("建请求");
            assert!(
                !req.headers().contains_key("authorization"),
                "{url} 不是 https，用户名+口令也不许上线：{:?}",
                req.headers()
            );
        }
    }

    #[test]
    fn the_scheme_predicate_behind_that_gate_reads_every_shape_of_url() {
        // 闸的判据本身（`task::url_is_https`）：大小写、缺 scheme、近似 scheme 都得对上，
        // 否则上面那条用例只会覆盖"写得最规矩的那一条 http"。
        // 没有合法 scheme 的输入**在这里**断（不在上面那个循环里）：那种 URL 根本建不出 reqwest 的
        // 请求（"relative URL without a base"），但判据仍然要给假 —— 认不出是 TLS 就不送凭据。
        for url in [
            "x/a.bin",                      // 无 scheme ⇒ 保守那一支
            "alice:s3cret@x/a.bin",         // 无 scheme 且带 userinfo（E9 那边另有其闸，这里照样不是 https）
            "httpx://x/a.bin",              // 差一个字母的 scheme 不是 https
            "https+proxy://x/a.bin",        // 复合 scheme 也不是
            "ftp://x/a.bin",
            "",                             // 空串（`validate()` 会拒，判据自己不许 panic）
            ":",
            "://x/a.bin",                   // `://` 前面不是 scheme
        ] {
            assert!(!url_is_https(url), "{url:?} 不该被当成 https");
        }
        for url in [
            "https://x/a.bin",
            "HTTPS://X/A.BIN",              // 大写 scheme 仍是 https：闸不许把加密连接误判成明文
            "HtTpS://x/a.bin",
            "https://x:8443/a.bin?u=alice@x#frag", // scheme 之外的 @ 不参与判断
        ] {
            assert!(url_is_https(url), "{url:?} 就是 https，凭据必须照发（误判会静默换来 401）");
        }
    }

    #[test]
    fn the_authorization_header_is_built_over_https_and_carries_both_halves() {
        let mut t = task_with("https://x/a.bin", None);
        t.user = Some("alice".into());
        t.password = Some("hunter2".into());
        let req = build_request(&agent(), &t, None).expect("建请求");
        assert_eq!(
            req.headers().get("authorization").map(|v| v.as_bytes()),
            Some(BASIC_ALICE_HUNTER2),
            "https 上必须照发，而且两半都在里面"
        );

        // 大写 scheme 仍然是 https：闸不能把加密连接误判成明文（那会静默丢掉凭据、换来 401）。
        let mut up = task_with("HTTPS://X/a.bin", None);
        up.user = Some("alice".into());
        up.password = Some("hunter2".into());
        let req = build_request(&agent(), &up, None).expect("建请求");
        assert_eq!(req.headers().get("authorization").map(|v| v.as_bytes()), Some(BASIC_ALICE_HUNTER2));

        // 只有用户名的 https 支：`Basic base64("user:")`，与 curl 同形。
        let mut bare = task_with("https://x/a.bin", None);
        bare.user = Some("alice".into());
        let req = build_request(&agent(), &bare, None).expect("建请求");
        assert_eq!(req.headers().get("authorization").map(|v| v.as_bytes()), Some(BASIC_ALICE_COLON));

        // 只有口令、没有用户名（`--password` 单独给）⇒ 用户名取空串，仍然是 `Basic`（"给了口令却悄悄不带"
        // 才是绝对不行的形状）。
        let mut pwonly = task_with("https://x/a.bin", None);
        pwonly.password = Some("hunter2".into());
        let req = build_request(&agent(), &pwonly, None).expect("建请求");
        assert!(req.headers().contains_key("authorization"), "{:?}", req.headers());
    }

    #[test]
    fn referer_and_cookie_cross_the_scheme_gate_by_design() {
        // **刻意的不对称**（评审 F2 的裁定，理由写在模块文档第 8 条）：`Referer`/`Cookie` 不看 scheme
        // 照发。spec §4 点名的只有口令；勘误 E9/R6 登记了 `referer`/`cookies` 是按设计落盘的字段。
        // 这一支存在的目的：后来人不要把这道闸顺手扩大到这两个头（那会悄悄改变退化路径的行为，
        // 而 cookie 一丢，同一台服务器给的可能就是另一份文档 —— ①b 又要落空）。
        let mut t = task_with("http://x/a.bin", None);
        t.referer = Some("https://ref.example/page".into());
        t.cookies = Some("sid=42".into());
        let req = build_request(&agent(), &t, None).expect("建请求");
        let h = req.headers();
        assert_eq!(h.get(REFERER).map(|v| v.as_bytes()), Some(&b"https://ref.example/page"[..]));
        assert_eq!(h.get(COOKIE).map(|v| v.as_bytes()), Some(&b"sid=42"[..]));
        assert!(!h.contains_key("authorization"), "这个夹具没给凭据，Authorization 不该凭空长出来：{h:?}");
    }

    // ---- 修复轮 F2：抽出来的那份"响应头 → 事实"的读法 ----

    /// 抽出来之后，`run()` 与 `fetch::run_whole` 用的必须是**这一个**函数。
    ///
    /// 这条用例钉的是"抽出来的是位置，不是判据"：五个字段的值与 `run()` 当年内联构造的那一份同值，
    /// 而 `asked_for_range` 那一格（`run_whole` 送进去的就是 `false`）走的还是 `peer_accepts_ranges`
    /// 里"没发 Range 时只看头"的既有分支 —— 与 `prelude_max == 0` 时 probe 的读法同一格。
    #[test]
    fn the_shared_reader_hands_back_the_same_five_fields_run_used_to_build() {
        let h = headers(&[
            ("content-range", "bytes 0-236031/50331648"),
            ("etag", "\"probe-abc\""),
            ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT"),
            ("accept-ranges", "bytes"),
        ]);
        let facts = facts_from_response(StatusCode::PARTIAL_CONTENT, true, &h, "https://x/a.bin")
            .expect("这一份响应读得出事实");
        assert_eq!(
            facts,
            ProbeFacts {
                etag: Some("\"probe-abc\"".into()),
                last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".into()),
                total_len: 50_331_648,
                final_url: "https://x/a.bin".into(),
                accept_ranges: true,
            },
            "落地 URL 按原样存、长度取分母、验证子不归一化 —— 与抽出前逐字段同值"
        );

        // `asked_for_range = false`（`run_whole` 那一格）：只看头。头说 bytes 就认，头说 none 就不认。
        let claims = headers(&[("content-length", "10"), ("accept-ranges", "bytes")]);
        let facts = facts_from_response(StatusCode::OK, false, &claims, "https://x/a.bin").expect("读得出事实");
        assert!(facts.accept_ranges, "没发 Range 时行为上没有证据可看，只能由头说");
        assert_eq!(facts.total_len, 10, "200 的 Content-Length 就是它交出来的整个 body（模块文档第 4 条）");
        let denies = headers(&[("content-length", "10"), ("accept-ranges", "none")]);
        assert!(!facts_from_response(StatusCode::OK, false, &denies, "https://x/a.bin").expect("读得出事实").accept_ranges);
        // 反方向（不许被顺手改掉）：发过 Range 而对方回 200 ⇒ 仍然不认（评审 I1 那一格）。
        assert!(!facts_from_response(StatusCode::OK, true, &claims, "https://x/a.bin").expect("读得出事实").accept_ranges);
    }

    #[test]
    fn the_shared_reader_applies_the_landing_credentials_gate_itself() {
        // 这道闸必须住在读法**里面**：`fetch::run_whole` 不经过 `probe::run`，外面那道闸够不着它；
        // 而它拒绝用的必须是同一个常量，不是第二个版本的措辞（`probe` 那一侧因此多一次同值判断，无副作用）。
        let h = headers(&[("content-length", "10")]);
        let Err(err) = facts_from_response(StatusCode::OK, false, &h, "https://alice:s3cret@host/x") else {
            panic!("落地 URL 带 userinfo（E9 / Task 5 评审 C3）时不许交回事实");
        };
        let msg = err.to_string();
        assert_eq!(msg, LANDED_CREDENTIALS, "同一个常量，不许有第二个版本：{msg}");
        for leak in ["alice", "s3cret", "@", "http://", "https://"] {
            assert!(!msg.contains(leak), "拒绝文本里出现了 URL 的成分（{leak}）：{msg}");
        }
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
    // - `probe_detects_server_without_ranges`：我们发了限量 Range，**对端无视它并交出 1 MiB** ⇒
    //   `prelude.len()` 必须正好是上限（这一支才是"上限说话"而不是"对端配合"的形状）；
    // - `probe_reads_prelude_and_reports_ranges_supported`：对端按 Range 截到上限，读满。
    // （原先排在这里的 `post_shaped_task_sends_no_range_and_still_bounds_the_prelude` 随评审 F1
    //   改成了"带体任务不试探"那条拒绝用例 —— 没有 Range 可发的那一支已经不存在了。）
}
