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
//!    **T99-G1f 收成一条三支合取的 task-aware 判据**（半支 1 userinfo ｜半支 2 形状｜
//!    **第三支：落地串里出现本任务 `--password` 的六种形状 ⇒ 拒**；短于 4 字节的口令不比内容，
//!    下限与理由住在 `carries_our_password` 的文档里）。
//!    动机与实测口径（半支 2 在落地口恒不触发、`@` 可被对面按相对引用折进 **path**）全写在
//!    `landed_url_is_refused` 的文档里；七个落地口（本模块 `run()` 的两处 ＋ `facts_from_response`
//!    一处 ＋ `fetch` 四处：`run_window` 两支 ＋ `run_whole` 两支）共用这一条判据，
//!    `authority_has_userinfo` / `canonical_http_authority` 本体一个字没动，动的是调用点。
//!    **T99-H2 §3：同一份内容判据再接到另外两个服务器可控字段上**（`etag` / `last_modified`）。
//!    `ProbeFacts` 的装填口只有两处 —— `facts_from_response` 与本模块 `empty_file_facts`
//!    （416 + `bytes */0` 那一支**不调** `facts_from_response`，只把闸接进去会漏掉它）——
//!    两处各判一次，命中即拒，措辞是同一句 `LANDED_VERIFIER_CREDENTIALS`，同样不回显值。
//! 6. **验证子按响应头的原样值保存，不归一化**（与 `resume::ProbeFacts` 的文档同一立场）；
//!    但 `ETag`/`Last-Modified` 里出现**非 UTF-8 字节**时只能记成 `None`：`ProbeFacts` 与断点文件
//!    都装不下它，`If-Range` 更没有强验证子可用。这不是"忽略噪音"—— 少了一把验证子，
//!    `resume` 的 ③ 会自动落到 ④ 的长度兜底（Task 5 裁定 2），所以是**降级但可解释**的形状。
//!    原样装填**之前**还有一道内容闸（T99-H2 §3，见上面第 5 条末段）：这两个字段和落地 URL 一样是
//!    服务器可控的自由文本，装着本任务口令的六种形状时这份事实不收。
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

/// 落地 URL 带 userinfo、形状不规范、或**带着本任务口令的那六种形状**时那句**不回显 URL** 的拒绝
/// （模块文档第 5 条）。
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
/// T99-G1f 之后这一句同样覆盖第三判据（落地串里带着本任务口令）—— 危害与措辞同源，不另起一句。
/// 它**不**覆盖 `etag` / `last_modified` 那一道：那里被带的不是地址，措辞另有 `LANDED_VERIFIER_CREDENTIALS`。
pub(crate) const LANDED_CREDENTIALS: &str = "服务器重定向后的地址在主机名前面带着凭据（userinfo 那一段）：\
                                  它会被写进断点文件、并出现在续传冲突的原因里，因此拒绝收下这份事实；\
                                  本消息不回显那条地址";

/// 服务器交回的 `ETag` / `Last-Modified` 里带着本任务口令的那六种形状时那句**不回显值**的拒绝
/// （T99-H2 §3；模块文档第 5 条末段与第 6 条）。
///
/// 为什么不复用 `LANDED_CREDENTIALS`：那句的主语是"重定向后的地址"，而这里被带出口令的是**验证子**
/// ——`resume` 的 ③ 那句 `ETag 与断点不符（断点 {saved:?} → 现在 {now:?}）` 会把这两个值原样打进
/// 终端（② 那句打的是落地 URL，归上一句管）。主语错了，用户就会去改 URL 而不是去问服务器。
/// 与那句同一条纪律：**一个字段值都不抄**，只点名是哪两个字段。
const LANDED_VERIFIER_CREDENTIALS: &str = "服务器交回的验证子（ETag 或 Last-Modified）里带着本任务的口令：\
                                  它会被写进断点文件、并出现在续传冲突的原因里，因此拒绝收下这份事实；\
                                  本消息不回显那两个值";

/// std 字母表（RFC 4648 §4，末两位是 `+` `/`）。
const STD_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// url-safe 字母表（RFC 4648 §5，末两位是 `-` `_`）：Basic 口令被塞进 URL 时对面更常给这一份。
const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// **内容判据的长度下限（字节数，不是 char 数）**，T99-H2 §2。
///
/// 判据从 1 种形状扩到 6 种之后，`--password 1` / `--password abc` 这类短口令会让**几乎每条**
/// 落地串、每个 `ETag` 都被拒（4 字节的 base64 只有 4～8 个字符，撞进 CDN 路径里那一大片
/// 十六进制/base64 样子的串是常态），而措辞还是那句"带着凭据"＝误导。
/// 取 4 的理由（一句话）：**4 字节口令的 std base64 已经是 6 个字符（`YWJjZA==`），
/// 再短一位（3 字节 ⇒ `YWJj`）就是 CDN 路径里十六进制串最常见的邻居** —— 于是"4 字节"是
/// "短口令误杀"与"检测面积"的分界，比它短的不比内容。
/// 短于下限时**只剩 userinfo 与形状那两支**（它们与口令无关，恒在），这一格是**文档化的边界**，
/// 不是藏起来的宽松：见 `README.md`「凭证与边界」与 `src/state.rs` 模块头第 1 条。
const PASSWORD_CONTENT_FLOOR: usize = 4;

/// 字节 → base64。`alphabet` 选 std 或 url-safe，`pad` 决定是否补 `=`（四种形状由此两两组合）。
///
/// 为什么本仓库自己写一只 base64 编码器：依赖上限钉死在 6 个 crate（用户 2026-10-15 的裁定），
/// 而这里是**编码**我们手上那份口令、不是解码对面 —— 与 `hex_lower` / 手写 sha256
/// （`src/engine.rs`）同一先例。旧那句"本仓库无 base64 工具 ⇒ 检测不到"随 T99-H2 §1 作废。
///
/// ⚠ 接进判据之前，这只函数先按**已知真值**校准过（RFC 4648 §10 那七例 ＋ 一只独立仪器交的两份
/// 字母表分家格），校准账与两处校准期间量出的错见 `.superpowers/sdd/…/t99-H2-report.md`；
/// 盘上的钉子是 `the_base64_helper_is_calibrated_against_rfc_4648_and_a_second_instrument`。
fn base64_of(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        // 不足 3 字节的尾巴按 0 补齐算出符号位，**但多出来的符号不发**（这就是 padding 那一档）。
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let group = (b0 << 16) | (b1 << 8) | b2;
        out.push(alphabet[((group >> 18) & 0b11_1111) as usize] as char);
        out.push(alphabet[((group >> 12) & 0b11_1111) as usize] as char);
        match chunk.len() {
            // 1 字节 ⇒ 只剩 2 个比特，第二个符号已经是补零后的那一档，后面没有第三个符号。
            1 => {
                if pad {
                    out.push_str("==");
                }
            }
            2 => {
                out.push(alphabet[((group >> 6) & 0b11_1111) as usize] as char);
                if pad {
                    out.push('=');
                }
            }
            _ => {
                out.push(alphabet[((group >> 6) & 0b11_1111) as usize] as char);
                out.push(alphabet[(group & 0b11_1111) as usize] as char);
            }
        }
    }
    out
}

/// 我们手上那份口令的**四种 base64 形状**（std/url-safe × 带/不带 padding）。
fn password_base64_forms(password: &str) -> [String; 4] {
    let bytes = password.as_bytes();
    [
        base64_of(bytes, STD_ALPHABET, true),
        base64_of(bytes, STD_ALPHABET, false),
        base64_of(bytes, URL_ALPHABET, true),
        base64_of(bytes, URL_ALPHABET, false),
    ]
}

/// 一个十六进制字符 → 0～15，不是十六进制 ⇒ `None`。
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 百分号解码，返回**字节**：`%XX`（大小写十六进制都认）→ 一个字节；裸 `%`、`%` 后不足两字符、
/// `%zz` 这类看不懂的形状**原样保留**，**永不失败**、**永不 panic**。
///
/// 为什么是"解码待判串"而不是"按表编码我们的口令"（T99-H2 §1 的裁定）：`url` 那份串里哪些字符会被
/// 写成 `%XX` 是**对面（和 reqwest）**的决定，手写一张"猜哪些字符会编码"的表迟早与对面分家 ——
/// 这正是 E32 第 4 条立的规矩（凡与外部解析器共享一个形状判断，就按对面对输入做的事判，别自己猜）。
/// 解码反过来做只需要 20 行、无需表，而且天然覆盖"对面编码了我们没编码的那一个字符"这一族方向差。
///
/// 返回 `Vec<u8>` 而不是 `String`：`%ff` 解出来不是合法 UTF-8，而**判据要的就是字节**
/// （口令是 UTF-8 字节序列，待判串解码后是一串字节，两侧都在字节平面上比，见 `carries_our_password`）。
/// `+` **不**解成空格：那是 `application/x-www-form-urlencoded` 的约定，不是百分号编码（RFC 3986）的
/// 约定，`urllib.parse.unquote` 同一读法；这一族的欠账登记在报告"这发看不见什么"一节。
fn percent_decoded_bytes(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 字节 haystack 里有没有字节 needle（`needle` 为空 ⇒ 假：`windows(0)` 会 panic，而"空口令"本来
/// 就不该让任何东西被拒 —— 见 `PASSWORD_CONTENT_FLOOR` 那一段）。
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.len() >= needle.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// **内容判据本体**（T99-H2 §1 把 G1f 那句 `url.contains(p)` 换掉的东西）：串里出现了本任务口令的
/// **六种形状**之一就算带着凭据。位置无关（path、query、fragment、验证子值里都算）。
///
/// 六种形状，与"怎么比"一一对应：
/// 1. **原样**（G1f 现役那一支；`p` 与 haystack 都在字节平面上）；
/// 2. **对面把口令百分号编码** ⇒ 把待判串解码成字节再比**原样**口令（不猜哪些字符会编码）；
///    3～6. **对面把我们这份口令按 base64 回显** ⇒ `std`/`url-safe` × 带 padding / 不带 padding 四形，
///    拿这四个串去比待判串（编码的是**我们手上那份**，不是解对面的码）。
///
/// ⚠ **按字节比，不按 char 比**：口令是非 ASCII 时（`--password 中文`）它的 UTF-8 字节序列才是
/// 会出现在线上的东西，而百分号解码交回的是**字节**（可能不是合法 UTF-8）；两侧统一到字节平面，
/// 非 ASCII 口令与它 percent-encoded 的形式（`%E4%B8%AD%E6%96%87`）才落在同一格判据上。
///
/// **下限**：口令短于 `PASSWORD_CONTENT_FLOOR`（4 字节）时**不比内容**（含空串），只剩调用点那两支
/// （userinfo／形状）在判 —— 这一格由 `an_empty_or_short_password_compares_no_content_at_all` 钉着
/// （G1f 的 m1 那发旧针按新语义改写：旧的是"空口令拒一切"，今天的是"空串与 1～3 字节都不比内容"）。
///
/// `landed_url_is_refused` 的第三支与 `verifiers_carry_our_password` 用的是**这同一个函数**：
/// 一条规则不许有两本账（三个服务器可控字段 × 同一份形状集合）。
fn carries_our_password(haystack: &str, password: Option<&str>) -> bool {
    let Some(p) = password else { return false };
    if p.len() < PASSWORD_CONTENT_FLOOR {
        return false;
    }
    let needle = p.as_bytes();
    if contains_bytes(haystack.as_bytes(), needle) {
        return true; // 形 1：原样
    }
    if contains_bytes(&percent_decoded_bytes(haystack), needle) {
        return true; // 形 2：对面按百分号编码回显
    }
    password_base64_forms(p)
        .iter()
        .any(|form| contains_bytes(haystack.as_bytes(), form.as_bytes())) // 形 3～6
}

/// 两个验证子字段里有没有装着本任务口令的六种形状之一（T99-H2 §3）。
///
/// 为什么 `etag` / `last_modified` 要单列一道：它们和 `final_url` 一样是**服务器可控的自由文本**、
/// 一样原样落进断点文件、一样被 `resume` 的原因串（③ 那句 `{saved:?} → {now:?}`）打回终端。
/// 形状那两支在它们身上**不成立**（`ETag` 从来不是一条 URL），所以这里只接内容判据。
/// 装填口两处都判：`facts_from_response`（正常那一支）与 `empty_file_facts`
/// （416 + `bytes */0` 那一支**不调** `facts_from_response`）。
fn verifiers_carry_our_password(
    etag: Option<&str>,
    last_modified: Option<&str>,
    password: Option<&str>,
) -> bool {
    [etag, last_modified]
        .into_iter()
        .flatten()
        .any(|value| carries_our_password(value, password))
}

/// 落地 URL 的**调用点**判据（T99-G1c §1.2 立的并联闸，T99-G1f 收成三条合取）：任一不过就拒。
/// 七个落地口（`probe::run` 两处 ＋ `facts_from_response` 一处 ＋ `fetch` 四处）共用**这一条**，
/// 一条规则不许有两本账。
///
/// 1. `task::authority_has_userinfo` —— E9 / Task 5 评审 C3 那一支，**本体一个字没动**；
/// 2. `task::canonical_http_authority` —— T99-G1c 新立的"只认规范写法"：非规范形状
///    （`http:/host`、`http:\\host`、多斜杠、其它 scheme、空 host）也拒；这里的"空 host"含**只有端口**
///    那一族（`https://:8443/a.bin`，判据是 authority 首字节为 `:`，T99-G1d **裁定一**）。
/// 3. **第三支（T99-G1f 立，T99-H2 §1/§2 加宽）**：本任务给过的口令**不少于 4 字节**且落地串里出现
///    它的**六种形状**之一 ⇒ 拒（判据本体在 `carries_our_password`）。判据不是"形状"而是"内容"，
///    位置无关（path/query 都算）。
///
/// 第三支为什么必须存在（T99-G1e §1/§2 实测）：对面交回 `Location: http:/alice:s3cret@host/…`
/// 这类串时，url 2.5.8 按**相对引用**解析 —— `@` 连同 `alice:s3cret` 落进 **path**，落地串形状
/// 完全规范、authority 干净 ⇒ 半支 1 与半支 2 都看不见，`probe::run` 实测 `Ok`，那条串照原样
/// 进 `ProbeFacts.final_url`。
/// 危害模型要写准（用户 2026-10-19 裁定二，**不是**"凭据泄露给第三方"——对面本来就拿得到这个口令）：
/// 修的是"**它被我们自己的手**经 `final_url` 写回用户磁盘上那份号称永不含口令的断点文件、
/// 并被 `resume` 的 ② 原因串原样打回终端"（`src/state.rs` 模块头第 1 条那句"这一支由校验闸拒绝"
/// 管的是用户自己写的 URL，服务器交回的那一半此前没人管）。
///
/// ⚠⚠ **下限那一刀不许省，也不许改回"只判空串"**：`str::contains("")` 恒真 ⇒ 漏了一刀，"给了空口令"
/// 会让**每一条**落地 URL 都被拒，等于把下载整条路砸掉，而且是一根恒红的假闸；反过来，`--password 1`
/// 这类 1～3 字节口令若也比内容，判据扩到六形之后会**误杀几乎每条落地串**（措辞还是"带着凭据"＝误导）。
/// 这一格由单元用例 `an_empty_or_short_password_compares_no_content_at_all` 钉着
/// （m-floor 变异：把下限改回 `!p.is_empty()` ⇒ 短口令放行那一侧的对照臂必须红）。
///
/// ## 三支在落地口的实测口径（覆盖 T99-G1c 那两句旧话）
///
/// T99-G1c 原注释写"两条是并集关系，各有对方够不着的一族"—— **在落地口这一句已被 T99-G1e
/// 实测证伪**（十条服务器给的 `Location`，交回的落地串**一支**都不是"无 `@` 但非规范"：凡对面
/// 真跟随的，交回的都是 `http://<非空 authority>/…`，scheme 已小写化、`://` 恰好一次、authority
/// 里无 `@`）。现行口径：
/// - **半支 1 与第三支在落地口是活的**（半支 1：`Location` 里斜杠 ≥ 2 时 `@` 真落在 authority，
///   实测 L2；第三支：`@` 折进 path 那一族，实测 L1/L3/L4，形状两闸放行、只有内容判据看得见）；
/// - **半支 2 在落地口是 belt-and-braces（实测恒不触发）**：留着它不是新增死分支（它是现役代码，
///   `Task::validate()` 侧仍在两处用例下活着），而是给"对面升级后交回非规范串"留的形状兜底；
/// - ⚠ **这整个结论只在这台仪器 ＋ reqwest 0.13.5 / url 2.5.8 / tower-http 0.6.11 / http 1.5.0
///   上成立**（L1/L3/L4 的 path 折入分支纯粹是 url crate 的实现分支）——升级对面任一版本
///   **必须复量** `tests/integration.rs` 的 `measure_the_landing_strings_…` 那张表。
/// - **T99-H2 关掉了这条具名欠账**（旧话："对面把口令以 Basic 的 base64 形式或百分号编码形式回显
///   进 path 时，字面值比对看不见" ⇒ 现在看得见）。口径要说准：看得见的是**"对面按标准编码我们这份
///   口令"**（percent / base64 的四种标准形状），**不是**"对面任意变形"（大小写打乱、加盐、二次编码、
///   `+` 代表空格的表单编码那一族仍然看不见 —— 逐条登记在报告"这发看不见什么"一节）。
///   ⚠ 归因也要改准：这一族的成因从来不只是"对面怎么编码"，**还有我们自己的手**：
///   `url` 是 **reqwest 序列化之后**的串，`Location` 里的一个空格落到 `validator.final_url`
///   已经变成 `%20` ⇒ 口令里只要含空格／`%`／`#`／任何非 ASCII，旧那句 `url.contains(p)` 本来就打不中。
///
/// ⚠ **入参不由我们控制**（是服务器给的），所以这里不做归一之外的任何改写：只判、只拒，
/// 不改写、不回显。
pub(crate) fn landed_url_is_refused(url: &str, password: Option<&str>) -> bool {
    authority_has_userinfo(url) || canonical_http_authority(url).is_none() || carries_our_password(url, password)
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
            // 先把同一个判据（三支合取，含本任务口令的字面值那一支）套上去，换成一句不回显的拒绝。
            if e.url().is_some_and(|u| landed_url_is_refused(u.as_str(), task.password.as_deref())) {
                bail!("{LANDED_CREDENTIALS}");
            }
            return Err(e).context("试探请求发不出去");
        }
    };

    let status = resp.status();
    // `headers` 与 `final_url` 先取出来：`resp` 随后要被 body 的读取消耗掉（`Read` 要 `&mut`）。
    let headers = resp.headers().clone();
    let final_url = resp.url().clone();
    // 模块文档第 5 条那道闸（三支合取：userinfo ／形状／本任务口令的六种形状），
    // 排在**任何**可能复述这条 URL 的消息之前。
    if landed_url_is_refused(final_url.as_str(), task.password.as_deref()) {
        bail!("{LANDED_CREDENTIALS}");
    }

    // 416 单独一支：我们那条 `bytes=0-…` 撞上 416，只有"文件是空的"这一种读法站得住
    // （`Content-Range: bytes */0`）。见用例 `a_zero_length_file_yields_zero_length_facts_instead_of_an_error`。
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let reported = header_text(&headers, CONTENT_RANGE).and_then(|v| content_range_total(&v));
        if range.is_some() && reported == Some(0) {
            // 这一支**不调** `facts_from_response`，`etag` / `last_modified` 在这里直接装进事实
            // ⇒ 验证子那道内容闸必须在这里也走一遍（T99-H2 §3；装填口＝ `empty_file_facts`）。
            return empty_file_facts(&headers, final_url.into(), task.password.as_deref())
                .map(|facts| ProbeResult { facts, prelude: vec![] });
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

    let facts = facts_from_response(status, range.is_some(), &headers, final_url.as_str(), task.password.as_deref())?;
    let prelude = read_bounded(resp, prelude_max)?;
    Ok(ProbeResult { facts, prelude })
}

/// 416 + `Content-Range: bytes */0` 那一支的**事实装填口**（模块文档第 9 条，评审 **F3** 裁定保留）。
///
/// 为什么不就地内联在 `run()` 里（T99-H2 §3 的实测处置）：这一支**不走** `facts_from_response`，
/// 旧形状是"直接构造 `ProbeFacts`、把 `header_text` 交回的 `etag`/`last_modified` 原样装填"，
/// 于是只把验证子闸接进 `facts_from_response` 会**漏掉它** —— 那一支同样有这两个字段（不是"根本没有
/// 那两个值"）。抽成一只函数有两个好处：闸与装填住在一处（漏不得），且这一支的拒与放行能在**单元层**
/// 真跑（`run()` 那一路要上网，而本机对端只有明文、带口令的任务在 `Task::validate()` 就被拒 ⇒
/// 进程级造不出"416 + 带口令的 ETag"这一格，见报告"这发看不见什么"）。
///
/// `final_url` 不在这里判：`run()` 里更前面那道 `landed_url_is_refused` 已经管住它（这一支的
/// 落地串与那道闸读的是同一份 `resp.url()`）。
fn empty_file_facts(headers: &HeaderMap, final_url: String, password: Option<&str>) -> Result<ProbeFacts> {
    let etag = header_text(headers, ETAG);
    let last_modified = header_text(headers, LAST_MODIFIED);
    if verifiers_carry_our_password(etag.as_deref(), last_modified.as_deref(), password) {
        bail!("{LANDED_VERIFIER_CREDENTIALS}");
    }
    Ok(ProbeFacts {
        etag,
        last_modified,
        total_len: 0,
        final_url,
        // 服务器**按 Range 语义**回答了这一枪（416 是 Range 语义的回答），
        // 只是文件恰好为空 —— 这不等于它不认 Range。
        accept_ranges: true,
    })
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
/// 落地 URL 那道凭据闸（模块文档第 5 条 / Task 5 评审 **C3** / 勘误 **E9** / T99-G1f 第三判据）
/// **在本函数里**：它是"事实能不能收下来"的一部分，而 `fetch` 这一枪同样可能被重定向到
/// 带 userinfo、带着本任务口令六种形状的落地地址。对 `probe::run` 是幂等的（那边排在更前面），
/// 对 `fetch::run_whole` 的落地 URL 是与它自己那一处 `probe::landed_url_is_refused` 双保险里
/// 更贴事实装填口的那一道。
/// `password` 由调用方从手上的 `task` 交进来（`task.password.as_deref()`）：第三判据是
/// **task-aware** 的——只比对**本任务给过的那份口令**可能出现的形状，不判形状之外的任何"坏味道"。
///
/// **T99-H2 §3 起本函数还多一道闸**：`etag` 与 `last_modified` 各判一次同一份内容判据
/// （`verifiers_carry_our_password`）。它们也是服务器可控的自由文本、也原样落进断点文件、也会被
/// `resume` 的 ③ 打进终端 —— 命中即拒，措辞是 `LANDED_VERIFIER_CREDENTIALS`，**不回显那两个值**。
/// 这一道闸在这里、而不在调用方：`fetch::run_whole` 走的是同一个函数，接一次管两条路。
pub(crate) fn facts_from_response(
    status: StatusCode,
    asked_for_range: bool,
    headers: &HeaderMap,
    final_url: &str,
    password: Option<&str>,
) -> Result<ProbeFacts> {
    if landed_url_is_refused(final_url, password) {
        bail!("{LANDED_CREDENTIALS}");
    }
    let etag = header_text(headers, ETAG);
    let last_modified = header_text(headers, LAST_MODIFIED);
    if verifiers_carry_our_password(etag.as_deref(), last_modified.as_deref(), password) {
        bail!("{LANDED_VERIFIER_CREDENTIALS}");
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
        etag,
        last_modified,
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
        // 那句常量是**落地 URL** 的拒绝文本（本模块另有 `LANDED_VERIFIER_CREDENTIALS` 管验证子，
        // T99-H2 §3 —— 两句各自主语不同，都不许带值）：它必须说清"凭据"与"重定向"，且不含任何 URL 片段。
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

    // ---- T99-G1f 第三判据的三臂（反臂 / 正臂 / 空口令臂，同发）----
    //
    // 为什么钉在单元层直调判据、而不是简报原本设想的"进程级三臂"（照实登记的偏差，理由实测）：
    // `Task::validate()` 先拒"明文 HTTP ＋ 口令"（spec §4 / E9，用例
    // `a_password_over_plain_http_never_leaves_the_socket` 钉着"一个字节都没上网"），
    // 本机对端又不做 TLS ⇒ 在这台仪器的明文桩上，**带口令的任务永远走不到落地闸**（走得到的
    // 任务永远 `password: None`，第三支恒假）。第三支真正可达的是 https 侧（口令只在那边上线），
    // 那维持"真 TLS 不碰本机"的具名欠账。所以：反臂/正臂/空口令臂在这里钉判据本身，
    // 进程级正臂（"统一判据没误杀无口令的落地路"）钉在
    // `tests/integration.rs::the_unified_landing_gate_still_lands_a_password_shaped_path_without_a_task_password`。

    /// 反臂：G1e L1 实测原文形状的落地串（形状规范、authority 干净、`alice:hunter2` 折在 **path**），
    /// 本任务口令正是 `hunter2` ⇒ 必须拒。两支旧闸都看不见它（这正是 T99-G1e §2 量出的那一格）。
    #[test]
    fn the_landing_gate_refuses_a_landing_url_carrying_this_tasks_password() {
        let l1 = "http://127.0.0.1:56296/alice:hunter2@127.0.0.1:56296/landed";
        // 夹具前提：这两支旧闸对这条串都是"放行"——拒它的只能是第三支。
        assert!(!authority_has_userinfo(l1), "夹具前提：L1 形状的 @ 不在 authority 里");
        assert!(canonical_http_authority(l1).is_some(), "夹具前提：L1 形状完全规范");
        // 第三支只看"字面值在不在串里"，位置无关。
        for url in [
            l1,
            "http://127.0.0.1:56296/landed?token=hunter2",
            "http://127.0.0.1:56296/hunter2",
            "https://host/a.bin#hunter2",
        ] {
            assert!(landed_url_is_refused(url, Some("hunter2")), "{url} 里带着本任务的口令字面值，必须拒");
        }
        // 拒绝走的是同一句不回显的话：措辞的"不含 URL 成分"由
        // `the_landing_gate_rejects_a_userinfo_authority_and_never_echoes_it` 钉；这里钉的是
        // **装填事实的口**（第三支的真落盘点）拒的就是那句常量、且消息里没有口令。
        let h = headers(&[("content-length", "10")]);
        let Err(err) = facts_from_response(StatusCode::OK, false, &h, l1, Some("hunter2")) else {
            panic!("final_url 就是从这里装填的：带本任务口令的落地串不许交回事实");
        };
        let msg = err.to_string();
        assert_eq!(msg, LANDED_CREDENTIALS, "同一个常量，不许有第二个版本：{msg}");
        assert!(msg.contains("凭据") && msg.contains("重定向"), "措辞要可行动：{msg}");
        assert!(!msg.contains("hunter2"), "拒绝消息不回显口令：{msg}");
    }

    /// 正臂（简报字面形状）：同一族落地串，但 path 里那串**不等于**本任务口令 ⇒ 照旧放行。
    /// 缺这一臂，第三支就是一根不知道自己会不会误杀的闸。
    #[test]
    fn the_landing_gate_accepts_password_shaped_paths_that_are_not_this_tasks_password() {
        let land = "http://127.0.0.1:56296/someone:else@127.0.0.1:56296/landed";
        assert!(!landed_url_is_refused(land, Some("hunter2")), "串里没有 hunter2，第三支不许开火");
        // 口令给过、但落地串里没有它：同一句话的正方向。
        assert!(!landed_url_is_refused("http://127.0.0.1:56296/landed", Some("hunter2")));
        // 第三支是 task-aware 的：任务没口令 ⇒ 对面爱在 path 里放什么口令形状的文本都归半支 1/半支 2
        // 管（G1e 实测：这种串两闸放行），内容判据**不**凭空替用户做"这看着像凭据"的决定。
        assert!(!landed_url_is_refused(land, None), "任务没口令，落地串按形状与 userinfo 判");
    }

    /// 空口令与短口令臂（T99-H2 §2 按新语义改写 G1f 那发 m1 针）：`--password ""` 的库层形状
    /// （`Some(String::new())`）与 1～3 字节的口令**一律不比内容**，配 4 字节的**正向对照**。
    ///
    /// 旧那一钉的是 `!p.is_empty()`（`str::contains("")` 恒真 ⇒ 摘了它每条落地 URL 都被拒）；
    /// 判据从 1 形扩到 6 形之后，光"非空"挡不住误杀：`--password 1` 的 base64 形状是 `MQ`/`MQ==`，
    /// 这两个串在 CDN 路径里满地都是 ⇒ 下限那一刀（`PASSWORD_CONTENT_FLOOR`）接管旧的那一刀，
    /// **空串仍然不比内容**（旧的那一格一个字符没少，m-floor 复旧时短口令臂必红）。
    #[test]
    fn an_empty_or_short_password_compares_no_content_at_all() {
        // 反臂（下限之外都不比内容）：串里就写着那份口令、或写着它的 base64 形状，也照旧放行。
        for password in ["", "1", "ab", "abc"] {
            for url in [
                "http://127.0.0.1:56296/x/abc",
                "http://127.0.0.1:56296/x/YWJj", // `base64("abc")`：形状在串里也不比
                "http://127.0.0.1:56296/x/MQ==", // `base64("1")`（带 padding 那一形）
                "http://127.0.0.1:56296/x/1",
                "http://127.0.0.1:56296/landed",
                "https://host:8443/a.bin?u=1",
                // 旧那发 m1 针的这一格照搬不动（G1e 的 L1 家族：`@` 折在 path 里、形状规范）：
                // 下限接管旧那一刀之后，这种串在空口令与短口令下仍必须放行。
                "http://127.0.0.1:56296/alice@127.0.0.1:56296/landed",
            ] {
                assert!(
                    !landed_url_is_refused(url, Some(password)),
                    "{password:?} 是 {} 字节的短口令 ⇒ 内容判据不该开火（误杀几乎每条落地串，\
                     而措辞还是\"带着凭据\"）：{url}",
                    password.len()
                );
            }
        }
        // 正臂（下限就是 4，不是 5）：4 字节的口令比中即拒。
        assert!(
            landed_url_is_refused("http://127.0.0.1:56296/x/abcd", Some("abcd")),
            "4 字节口令在落地串里被原样带着 ⇒ 必须拒（下限取 4 的钉子在这一格）"
        );
        // 下限只管**内容**这一支：userinfo 与形状那两支与口令无关，短口令任务照样拒（不许被"放宽"误伤）。
        assert!(landed_url_is_refused("https://alice:s3cret@host/x", Some("ab")), "userinfo 那支被下限带走了");
        assert!(landed_url_is_refused("http:/host/x", Some("")), "形状那支被下限带走了");
        // char 数不算，**字节数**才算：`中` 是一个 char、三个字节 ⇒ 在下限之外。
        assert!(
            !landed_url_is_refused("http://127.0.0.1:56296/中", Some("中")),
            "`中` 的 UTF-8 长度是 3 字节 ⇒ 与 1～3 字节的 ASCII 口令同一档（下限按字节，见 `PASSWORD_CONTENT_FLOOR`）"
        );
        // 对照方向（不许被"修 m-floor"的人顺手放宽）：非空且够长的口令照旧逐字判。
        assert!(landed_url_is_refused("http://127.0.0.1:56296/x/hunter2", Some("hunter2")));
    }

    // ---- T99-H2 §1：内容判据的六种形状（编码器与解码器先各自钉死，再进判据）----

    /// **编码器的校准针**：`base64_of` 的输入输出逐字写死，期望值**不问被测函数要**。
    ///
    /// 两处来源，都是另一台仪器交回的真值（本仓库另有一发仓库外一次性 `rustc` 探针跑同一张表，
    /// 账在 `.superpowers/sdd/2026-10-15-dlm-kernel/t99-H2-report.md`）：
    /// ① RFC 4648 §10 那七例（`""`/`f`/`fo`/`foo`/`foob`/`fooba`/`foobar`）——控制员已用另一台仪器
    ///    复核过这组期望值属实；② Python `base64.b64encode` / `urlsafe_b64encode` 交的、
    ///    **两个字母表必须分家**的格子（`ÿÿ`、`testÿ`、`pÿss`：std 给 `/`、url-safe 给 `_`），
    ///    七例本身全是 ASCII 字母，两个字母表在那一组上同值 ⇒ 只抄七例的话字母表写反了也测不出来
    ///    （这一格是校准期间新买的，不是装饰）。
    #[test]
    fn the_base64_helper_is_calibrated_against_rfc_4648_and_a_second_instrument() {
        // (输入, std 带 padding, std 不带, url-safe 带, url-safe 不带) —— 22 行 × 4 形逐字写死。
        const TABLE: &[(&str, &str, &str, &str, &str)] = &[
            // ① RFC 4648 §10
            ("", "", "", "", ""),
            ("f", "Zg==", "Zg", "Zg==", "Zg"),
            ("fo", "Zm8=", "Zm8", "Zm8=", "Zm8"),
            ("foo", "Zm9v", "Zm9v", "Zm9v", "Zm9v"),
            ("foob", "Zm9vYg==", "Zm9vYg", "Zm9vYg==", "Zm9vYg"),
            ("fooba", "Zm9vYmE=", "Zm9vYmE", "Zm9vYmE=", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy", "Zm9vYmFy", "Zm9vYmFy", "Zm9vYmFy"),
            // ② 另一台仪器（Python）交的表：本判据真正会拿到的形状，含长度非 3 的倍数、
            //    非 ASCII（UTF-8 字节）、以及两个字母表分家的 `/` ↔ `_`。
            ("1", "MQ==", "MQ", "MQ==", "MQ"),
            ("abc", "YWJj", "YWJj", "YWJj", "YWJj"),
            ("abcd", "YWJjZA==", "YWJjZA", "YWJjZA==", "YWJjZA"),
            ("abcde", "YWJjZGU=", "YWJjZGU", "YWJjZGU=", "YWJjZGU"),
            ("hunter2", "aHVudGVyMg==", "aHVudGVyMg", "aHVudGVyMg==", "aHVudGVyMg"),
            ("a b%c#", "YSBiJWMj", "YSBiJWMj", "YSBiJWMj", "YSBiJWMj"),
            ("中文", "5Lit5paH", "5Lit5paH", "5Lit5paH", "5Lit5paH"),
            ("ÿ", "w78=", "w78", "w78=", "w78"),
            ("ÿÿ", "w7/Dvw==", "w7/Dvw", "w7_Dvw==", "w7_Dvw"),
            ("testÿ", "dGVzdMO/", "dGVzdMO/", "dGVzdMO_", "dGVzdMO_"),
            ("pÿss", "cMO/c3M=", "cMO/c3M", "cMO_c3M=", "cMO_c3M"),
            ("secret+val/ue", "c2VjcmV0K3ZhbC91ZQ==", "c2VjcmV0K3ZhbC91ZQ", "c2VjcmV0K3ZhbC91ZQ==", "c2VjcmV0K3ZhbC91ZQ"),
        ];
        for (input, std_padded, std_unpadded, url_padded, url_unpadded) in TABLE {
            let bytes = input.as_bytes();
            assert_eq!(base64_of(bytes, STD_ALPHABET, true), *std_padded, "std 带 padding：{input:?}");
            assert_eq!(base64_of(bytes, STD_ALPHABET, false), *std_unpadded, "std 去 padding：{input:?}");
            assert_eq!(base64_of(bytes, URL_ALPHABET, true), *url_padded, "url-safe 带 padding：{input:?}");
            assert_eq!(base64_of(bytes, URL_ALPHABET, false), *url_unpadded, "url-safe 去 padding：{input:?}");
        }
        // 判据实际消费的是 `password_base64_forms` 那四元组：顺序与内容也要逐字钉一次
        // （顺序写反＝四个形状里少判两个，表驱动看不出来）。
        assert_eq!(
            password_base64_forms("ÿÿ"),
            ["w7/Dvw==".to_string(), "w7/Dvw".to_string(), "w7_Dvw==".to_string(), "w7_Dvw".to_string()],
            "四形的顺序：std 带 pad、std 去 pad、url 带 pad、url 去 pad"
        );
        assert_eq!(
            password_base64_forms("hunter2"),
            [
                "aHVudGVyMg==".to_string(),
                "aHVudGVyMg".to_string(),
                "aHVudGVyMg==".to_string(),
                "aHVudGVyMg".to_string()
            ],
            "ASCII 口令两个字母表同值（这正是七例测不出字母表分家的原因）"
        );
    }

    /// **百分号解码器的校准针**（同样逐字写死，期望值来自另一台仪器 `urllib.parse.unquote` 的同一语义）。
    ///
    /// 判据要的性质只有两条：**看得懂的 `%XX` 变成一个字节**、**看不懂的一律原样保留且永不失败**
    /// （待判串是服务器给的自由文本，解码器一旦 panic 就是把 DoS 交给了对面）。
    /// 返回**字节**：`%ff` 解出来不是合法 UTF-8，而判据比的正是字节。
    #[test]
    fn the_percent_decoder_keeps_every_shape_it_cannot_read() {
        const TABLE: &[(&str, &[u8])] = &[
            ("a%20b", b"a b"),                    // §1 点名的那一格：空格在线上就是 `%20`
            ("%68unter2", b"hunter2"),            // 对面把口令**整串**编码
            ("hunter%32", b"hunter2"),            // 对面只编码了其中一字节
            ("a%2fb", b"a/b"),
            ("a%2Fb", b"a/b"),                    // 十六进制大小写都认
            ("%E4%B8%AD%E6%96%87", "中文".as_bytes()), // 非 ASCII 口令的 UTF-8 字节序列
            ("%ff", &[0xFF]),                     // 解出来不是合法 UTF-8 ⇒ 只能按字节比
            ("a%00b", b"a\0b"),
            ("100%", b"100%"),                    // 裸 `%`：原样
            ("%", b"%"),
            ("%2", b"%2"),                        // `%` 后面不足两字符：原样
            ("%zz", b"%zz"),                      // 不是十六进制：原样
            ("%%20", b"% "),                      // 第一个 `%` 看不懂就照抄，第二个照常解（与 unquote 同读法）
            ("%2520", b"%20"),                    // 二次编码只解一层（本发**不**追第二次，见报告）
            ("+%2B", b"++"),                      // `+` **不**解成空格：那是 form-urlencoded 的约定
            ("no-percent-here", b"no-percent-here"),
            ("", b""),
        ];
        for (input, want) in TABLE {
            assert_eq!(percent_decoded_bytes(input), *want, "解码 {input:?}");
        }
    }

    /// 六种形状**逐形**都要让落地串被拒。每格的"待判串"里除字面形那一格外都**不含**口令的字面，
    /// 于是"这一格红"只能归因于被测的那一支 —— 摘掉百分号那支（m-pct）或摘掉 base64 那四支（m-b64）
    /// 都有**具体格子**红，不是只有"整体变严"。
    /// 表里每格另带两条**夹具前提**（形状规范、authority 无 `@`）：那两支旧闸不许替被测的那一支开火。
    #[test]
    fn the_content_criterion_sees_all_six_shapes_of_our_password_in_a_landing_url() {
        // (待判串, 本任务口令, 这一格钉的是哪一支)
        const CELLS: &[(&str, &str, &str)] = &[
            // 形 1：原样（G1f 现役那一支，保留）。
            ("http://127.0.0.1:56296/x/hunter2", "hunter2", "形 1 原样"),
            // 形 2：对面按百分号编码回显 ⇒ 解码待判串再比原样口令。旧那句 `url.contains(p)` 打不中，
            // 而且 §1 点名的正是这一族的**另一半成因**：口令里只要含空格／`%`／非 ASCII，
            // reqwest 序列化之后落地串里已经是 `%20`，**对面一个字都没改**也比不中。
            ("http://127.0.0.1:56296/landed?tok=hunter%32", "hunter2", "形 2 百分号（只编码了一字节）"),
            ("http://127.0.0.1:56296/%68unter2", "hunter2", "形 2 百分号（整串编码）"),
            ("http://127.0.0.1:56296/a%20b2", "a b2", "形 2 百分号（空格 → %20，§1 那一句的形状）"),
            ("http://127.0.0.1:56296/%E4%B8%AD%E6%96%87", "中文", "形 2 百分号 × 非 ASCII（字节平面）"),
            // 形 3～6：把**我们这份**口令做 std / url-safe × 带 / 不带 padding 四个形状。
            // `ÿÿ`（UTF-8 四字节）的四个形状两两不同 ⇒ 四支各自有格子，`_` 与 `=` 的差别都量得到。
            ("http://127.0.0.1:56296/w7/Dvw==", "ÿÿ", "形 3 std 带 padding"),
            ("http://127.0.0.1:56296/w7/Dvw", "ÿÿ", "形 4 std 去 padding"),
            ("http://127.0.0.1:56296/w7_Dvw==", "ÿÿ", "形 5 url-safe 带 padding"),
            ("http://127.0.0.1:56296/w7_Dvw", "ÿÿ", "形 6 url-safe 去 padding"),
            ("http://127.0.0.1:56296/5Lit5paH", "中文", "形 3～6 × 非 ASCII 口令"),
        ];
        for &(url, password, arm) in CELLS {
            assert!(
                canonical_http_authority(url).is_some(),
                "夹具前提：{url} 必须是规范写法，否则半支 2 会替{arm}开火"
            );
            assert!(!authority_has_userinfo(url), "夹具前提：{url} 的 authority 里不许有 @");
            if !arm.starts_with("形 1") {
                assert!(!url.contains(password), "夹具前提：{arm} 这一格里不该有口令的字面形：{url}");
            }
            assert!(
                landed_url_is_refused(url, Some(password)),
                "{arm} 没被拒：本任务口令 {password:?} 就以这一种形状写在落地串里（摘掉那一支＝这一格红）"
            );
        }

        // 对照臂（判据不许变成"看见什么都像凭据"）：这些串里没有本任务口令的任何一种形状。
        for url in [
            "http://127.0.0.1:56296/landed",
            "http://127.0.0.1:56296/w7/Dvw==",     // 是**别的**口令的形状
            "http://127.0.0.1:56296/YWJjZA==",     // `base64("abcd")`，本任务口令是 hunter2
            "http://127.0.0.1:56296/a%20b2",       // 百分号形属于别的口令
            "http://127.0.0.1:56296/normal.bin",
        ] {
            assert!(!landed_url_is_refused(url, Some("hunter2")), "第三支不许对 {url} 开火（口令是 hunter2）");
        }
    }

    // ---- T99-H2 §3：`etag` / `last_modified` 这两个服务器可控字段也过同一份内容判据 ----

    /// 装填口两处（`facts_from_response` 与 416 那一支 `empty_file_facts`）都必须拒，
    /// 且消息**不回显那两个值**；正常验证子与短口令必须放行（误杀对照臂）。
    #[test]
    fn the_verifier_gate_refuses_facts_whose_etag_or_last_modified_carries_our_password() {
        let secret = "hunter2";
        // 落地 URL 干净（两支旧闸都过），只有验证子里带着口令 ⇒ 拒的只能是新那道闸。
        // (口令, 字段, 值)：值全是 ASCII（`headers()` 那个夹具只装得下合法头值），
        // 而 `ÿÿ` 这一份口令本身是非 ASCII —— 它的 base64 形状是 ASCII，这正是那四形的用处。
        for (secret, name, value) in [
            ("hunter2", "etag", "\"hunter2\""),         // 形 1：字面
            ("hunter2", "etag", "\"%68unter2\""),       // 形 2：百分号（整串编码）
            ("hunter2", "etag", "\"a%20hunter2\""),     // 形 2：只有一个空格落到线上成了 %20
            ("hunter2", "etag", "\"aHVudGVyMg==\""),    // 形 3/4：std（带 pad 的串里同时有去 pad 那一形）
            ("hunter2", "last-modified", "hunter2-fixed"), // 另一个字段同一条判据
            ("hunter2", "last-modified", "\"y-aHVudGVyMg\""), // 另一个字段 × base64 去 padding
            ("ÿÿ", "etag", "\"x-w7_Dvw\""),             // 形 6：url-safe 去 padding（std 那两形看不见 `_`）
            ("ÿÿ", "last-modified", "w7/Dvw=="),         // 形 3：std 带 padding
        ] {
            let h = headers(&[("content-length", "10"), (name, value)]);
            let Err(err) = facts_from_response(StatusCode::OK, false, &h, "https://host/a.bin", Some(secret)) else {
                panic!("验证子 {name}={value:?} 里带着本任务口令 ⇒ 这份事实不许收下");
            };
            let msg = err.to_string();
            assert_eq!(msg, LANDED_VERIFIER_CREDENTIALS, "同一句常量，不许有第二个版本：{msg}");
            assert!(!msg.contains(secret) && !msg.contains(value), "拒绝消息回显了字段值：{msg}");
            assert!(msg.contains("验证子") && msg.contains("口令"), "措辞要点名是哪两个字段：{msg}");
            assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");
        }

        // 反方向（误杀对照臂）：**别的**口令的 base64 形状、百分号形状都不该让诚实验证子被拒
        // （判据是 task-aware 的，不是"这看着像凭据"）。
        let honest = headers(&[
            ("content-length", "10"),
            ("etag", "W/\"YWJjZA==\""), // `base64("abcd")`，本任务口令是 hunter2
            ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT"),
        ]);
        let facts = facts_from_response(StatusCode::OK, false, &honest, "https://host/a.bin", Some(secret))
            .expect("诚实验证子与口令无关，不许误杀");
        assert_eq!(facts.etag.as_deref(), Some("W/\"YWJjZA==\""), "放行时验证子照旧原样装填（不归一化）");
        assert_eq!(facts.last_modified.as_deref(), Some("Wed, 21 Oct 2015 07:28:00 GMT"));

        // 下限同样管这一支：3 字节口令即使字面撞在 ETag 里也**不比内容**（§2 的口径覆盖三个字段）。
        let short = headers(&[("content-length", "10"), ("etag", "\"abc\"")]);
        assert!(facts_from_response(StatusCode::OK, false, &short, "https://host/a.bin", Some("abc")).is_ok());
        // 没给口令 ⇒ 这一道闸恒不关火（对面爱在 ETag 里放什么放什么）。
        assert!(facts_from_response(StatusCode::OK, false, &short, "https://host/a.bin", None).is_ok());

        // 416 + `bytes */0` 那一支：它**不调** `facts_from_response`，所以单独钉一次同一道闸。
        let tainted = headers(&[("content-range", "bytes */0"), ("etag", "\"aHVudGVyMg==\"")]);
        let Err(err) = empty_file_facts(&tainted, "http://127.0.0.1:56296/sz/0".to_string(), Some(secret)) else {
            panic!("416 那一支的 ETag 带着本任务口令 ⇒ 不许交回事实（这一支不走 facts_from_response）");
        };
        let msg = err.to_string();
        assert_eq!(msg, LANDED_VERIFIER_CREDENTIALS, "同一句常量，不许有第二个版本：{msg}");
        assert!(!msg.contains(secret) && !msg.contains("aHVudGVyMg"), "拒绝消息回显了字段值：{msg}");

        // 那一支的**放行形状**一个字节都不许变（模块文档第 9 条的三条并列，与抽出前逐字段同值）：
        // `total_len` 恒 0、`accept_ranges` 恒 true（解读）、验证子原样。
        let empty = headers(&[("content-range", "bytes */0"), ("etag", "\"probe-abc\"")]);
        let facts = empty_file_facts(&empty, "http://127.0.0.1:56296/sz/0".to_string(), Some(secret))
            .expect("诚实的空文件事实照旧收得下");
        assert_eq!(
            facts,
            ProbeFacts {
                etag: Some("\"probe-abc\"".into()),
                last_modified: None,
                total_len: 0,
                final_url: "http://127.0.0.1:56296/sz/0".into(),
                accept_ranges: true,
            },
            "抽出这一支是**位置**的改变，不是读法的改变"
        );
    }

    // ---- T99-H2 §4：把"七个落地口统一 + 两处装填口都判验证子"从注释变成针 ----

    /// `src/probe.rs` **产品区**的形状针（写法照 `src/task.rs` 那根：切到第一个 `#[cfg(test)]` 之前、
    /// 跳过以 `//` 开头的行）。
    ///
    /// 为什么行为用例不够（本发存在的理由，与 `tests/integration.rs` 那根数 `fetch` 四次的针同源）：
    /// 现役那根只数 `probe::landed_url_is_refused(` 在 `src/fetch.rs` 恰好 4 次 ＋ 禁旧判据 ——
    /// **把任一处第二实参换成 `None`，次数不变、禁字不碰、四闸全绿**（m-arg 实测只有本发会红）。
    /// 本模块这两件事没人钉过：① 三支合取的调用点有几处、每处**有没有把口令交进来**；
    /// ② 416 那一支（它不调 `facts_from_response`）有没有走验证子闸。
    ///
    /// 四条断言：
    /// 1. `landed_url_is_refused(` 的定义在产品区恰好 1 次、**调用**恰好 3 次（`run()` 两处 ＋
    ///    `facts_from_response` 一处；`fetch` 那四处由集成层那根管，两条账合起来＝七个落地口）。
    /// 2. 三处调用的**第二个实参**逐个截出来（按括号深度配到右括号）：必须含 `password` 这个名字、
    ///    必须不是裸 `None` —— 这就是 m-arg 的靶子，也是"统一判据不许有人少交一份口令"的牙。
    /// 3. `verifiers_carry_our_password(` 的**调用**恰好 2 次（`facts_from_response` 一处 ＋
    ///    `empty_file_facts` 一处）：少了＝某一处装填口退回不判验证子。
    /// 4. 416 那一支（`if status == StatusCode::RANGE_NOT_SATISFIABLE {` 的体内）必须出现
    ///    `empty_file_facts(`，而 `empty_file_facts` 的体内必须出现验证子闸与那句常量：
    ///    钉"那一支确实走判据"这件事，因为**这台仪器造不出它的进程级夹具**（带口令的任务在明文对端
    ///    上先被 `Task::validate()` 拒，见上面 T99-G1f 那段与报告"这发看不见什么"）。
    #[test]
    fn the_landing_and_verifier_gates_have_their_documented_call_sites_and_none_of_them_drops_the_password() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("probe.rs");
        let all = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("读 {} 的源码文本失败：{err}（本发是源码扫描针，读不到就是没有判据）", path.display()));
        let src = match all.find("#[cfg(test)]") {
            Some(at) => &all[..at],
            None => panic!("src/probe.rs 里找不到 `#[cfg(test)]`——扫描区靠它切掉测试文本，不许退回整文件（针会扫到自己）"),
        };
        let code_lines = || src.lines().map(str::trim_start).filter(|l| !l.starts_with("//")).collect::<Vec<&str>>();
        let lines = code_lines();

        // 断言 1：定义一次、调用三次。
        let call_sites = |token: &str, def_token: &str| -> Vec<String> {
            lines
                .iter()
                .filter(|l| l.contains(token) && !l.contains(def_token))
                .map(|l| (*l).to_string())
                .collect()
        };
        assert_eq!(
            lines.iter().filter(|l| l.contains("fn landed_url_is_refused(")).count(),
            1,
            "`fn landed_url_is_refused(` 在产品区应当恰好定义一次（两本账）"
        );
        let landing_calls = call_sites("landed_url_is_refused(", "fn landed_url_is_refused(");
        assert_eq!(
            landing_calls.len(),
            3,
            "src/probe.rs 产品区里 `landed_url_is_refused(` 的**调用**应当恰好 3 次（`run()` 两处 ＋ \
             `facts_from_response` 一处），实得 {} 次：少了＝某个落地口退回旧判据或整处删掉，\
             多了＝判据又开了第二个入口。逐行：{landing_calls:?}",
            landing_calls.len()
        );

        // 断言 2：每处调用的第二个实参必须点名 password，不许是裸 None。
        for site in &landing_calls {
            let args = args_after_first_open_paren(site, "landed_url_is_refused(");
            let second = top_level_args(&args)
                .get(1)
                .unwrap_or_else(|| panic!("调用点少了第二个实参（判据签名要两份输入）：{site}"))
                .trim()
                .to_string();
            assert!(
                second.contains("password"),
                "落地闸的调用点把第二实参换掉了（{second:?}）⇒ 那一处不再知道本任务的口令，\
                 内容判据在那一个口上恒假（m-arg 禁的形状）：{site}"
            );
            assert_ne!(second, "None", "落地闸的调用点交回裸 `None` ⇒ 同一个口退回只看形状：{site}");
        }

        // 断言 3：验证子那道闸在两处装填口各判一次。
        let verifier_calls = call_sites("verifiers_carry_our_password(", "fn verifiers_carry_our_password(");
        assert_eq!(
            verifier_calls.len(),
            2,
            "`verifiers_carry_our_password(` 的**调用**应当恰好 2 次（`facts_from_response` ＋ \
             `empty_file_facts`），实得 {} 次：416 那一支不走 `facts_from_response`，\
             少一次就有一个装填口漏判。逐行：{verifier_calls:?}",
            verifier_calls.len()
        );

        // 断言 4：416 那一支确实经由 `empty_file_facts`，而它体内带着验证子闸。
        let branch = body_of(src, "if status == StatusCode::RANGE_NOT_SATISFIABLE");
        assert!(
            branch.contains("empty_file_facts("),
            "416 + `bytes */0` 那一支不再经由 `empty_file_facts(` ⇒ 那一支的 `etag`/`last_modified` \
             退回未经任何内容闸直接装填（T99-H2 §3 堵的就是这一格）"
        );
        assert_eq!(
            call_sites("empty_file_facts(", "fn empty_file_facts(").len(),
            1,
            "`empty_file_facts(` 的调用应当恰好 1 次（就是 416 那一支）"
        );
        let facts_body = body_of(src, "fn empty_file_facts(");
        assert!(
            facts_body.contains("verifiers_carry_our_password(") && facts_body.contains("LANDED_VERIFIER_CREDENTIALS"),
            "`empty_file_facts` 的体内没有验证子闸或没有那句常量 ⇒ 416 那一支的闸被掏空了"
        );
    }

    /// 从 `line` 里 `token` 后面的第一个 `(` 起，按括号深度截到配对的右括号，返回**括号内**文本
    /// （不含那对括号本身）。本针只看产品区那几行形状已知的调用点（不是解析器，与 `task.rs` 那根的
    /// 按大括号深度同源）。
    fn args_after_first_open_paren(line: &str, token: &str) -> String {
        let at = line.find(token).expect("调用点里应当有这只 token");
        let mut depth = 1i32; // 已经站在 `token` 自己那只左括号里
        let mut out = String::new();
        for c in line[at + token.len()..].chars() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return out;
                    }
                }
                _ => {}
            }
            if depth > 0 {
                out.push(c);
            }
        }
        panic!("调用点的括号没闭合（针的夹具跟不上实现形状）：{line}");
    }

    /// 按**顶层**逗号切实参（括号、方括号、尖括号、字符串字面量里的逗号不算分隔）。
    fn top_level_args(args: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        let mut current = String::new();
        for c in args.chars() {
            match c {
                '(' | '[' | '{' => {
                    depth += 1;
                    current.push(c);
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => {
                    out.push(std::mem::take(&mut current));
                }
                _ => current.push(c),
            }
        }
        out.push(current);
        out
    }

    /// 函数/语句块切片：从含 `head` 的第一行起按**大括号深度**截到闭合，只收非 `//` 开头的行
    /// （与 `src/task.rs` 那根的 `body_of` 同一写法）。
    fn body_of(src: &str, head: &str) -> String {
        let mut body = String::new();
        let mut depth = 0i32;
        let mut inside = false;
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if !inside {
                if !t.contains(head) {
                    continue;
                }
                inside = true;
            }
            depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
            body.push_str(line);
            body.push('\n');
            if depth == 0 && body.len() > 1 {
                break;
            }
        }
        assert!(inside, "产品区里找不到含 `{head}` 的非注释行——本该存在的判据/分支没了");
        assert_eq!(depth, 0, "`{head}` 的块在大括号深度上没闭合——针的夹具跟不上实现形状");
        body
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
        // 没有合法 scheme 的输入**在这里**断（不在上面那个循环里）。这一支的理由要说准：CLI 那一行
        // 不写 scheme 的串已由 `Task::validate()` 当场以"参数写错了"拒掉（T99-G1c 反转默认，退 2），
        // 到不了发送点，所以这里断的**不是**"对面建不出请求所以无所谓"那一族。判据仍然要给假，
        // 是因为它是一条 `pub(crate)` 的纯判据，契约不许假设"调用点已经过滤过"，
        // 而认不出是 TLS 时唯一安全的取法就是不送凭据。
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
        let facts = facts_from_response(StatusCode::PARTIAL_CONTENT, true, &h, "https://x/a.bin", None)
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
        let facts = facts_from_response(StatusCode::OK, false, &claims, "https://x/a.bin", None).expect("读得出事实");
        assert!(facts.accept_ranges, "没发 Range 时行为上没有证据可看，只能由头说");
        assert_eq!(facts.total_len, 10, "200 的 Content-Length 就是它交出来的整个 body（模块文档第 4 条）");
        let denies = headers(&[("content-length", "10"), ("accept-ranges", "none")]);
        assert!(!facts_from_response(StatusCode::OK, false, &denies, "https://x/a.bin", None).expect("读得出事实").accept_ranges);
        // 反方向（不许被顺手改掉）：发过 Range 而对方回 200 ⇒ 仍然不认（评审 I1 那一格）。
        assert!(!facts_from_response(StatusCode::OK, true, &claims, "https://x/a.bin", None).expect("读得出事实").accept_ranges);
    }

    #[test]
    fn the_shared_reader_applies_the_landing_credentials_gate_itself() {
        // 这道闸必须住在读法**里面**：`fetch::run_whole` 不经过 `probe::run`，外面那道闸够不着它；
        // 而它拒绝用的必须是同一个常量，不是第二个版本的措辞（`probe` 那一侧因此多一次同值判断，无副作用）。
        let h = headers(&[("content-length", "10")]);
        let Err(err) = facts_from_response(StatusCode::OK, false, &h, "https://alice:s3cret@host/x", None) else {
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
