use std::fmt;
use std::path::PathBuf;
use crate::consts::FLAG_IGNORE_MTIME;

/// 任务模型。`Clone`/`PartialEq`/`Eq` 走派生（Task 9 的用例直接 `assert_eq!` 两个 `Task`）；
/// **只有 `Debug` 是手写的** —— 见下面那个 `impl` 的说明。
#[derive(Clone, PartialEq, Eq)]
pub struct Task {
    pub url: String,
    pub referer: Option<String>,
    pub cookies: Option<String>,
    pub data_file: Option<PathBuf>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub dir: PathBuf,
    pub name: Option<String>,
    pub flags: u32,
}

/// 手写 `Debug`：除 `password` 外每个字段照派生的形状打印，口令只以 `Some("[redacted]")` / `None` 出现。
///
/// 为什么不 derive（评审 F4 / 勘误 E9）：Task 8 的错误路径会把 `Task` 打进日志（`{:?}`、`{e:?}` 的
/// anyhow 因果链），派生的 `Debug` 会把口令原文交出去 —— "口令不落盘、不外传"这条硬规定管的是**值**，
/// 不是"有没有这个字段"。`state::TaskView` 那条"结构里干脆少一个字段"的防线在这里用不了：
/// `Task` 本身必须带着口令走到请求发送那一刻。
///
/// 刻意**只**动 `Debug`：比较与克隆的语义一个字都没改（掩码不影响 `PartialEq`），
/// 用例 `redacting_debug_does_not_make_two_passwords_equal` 钉的就是这条边界 ——
/// 没有它，"把 password 从 Debug 里删掉"可以顺手退化成"把 password 从相等性里删掉"。
impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("url", &self.url)
            .field("referer", &self.referer)
            .field("cookies", &self.cookies)
            .field("data_file", &self.data_file)
            .field("user", &self.user)
            // 掩码而不是省略：读日志的人要能看出"这个任务带了口令"。
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field("dir", &self.dir)
            .field("name", &self.name)
            .field("flags", &self.flags)
            .finish()
    }
}

/// 闸判的那份串＝reqwest（WHATWG 基本 URL 解析器）**真拿去解析**的那份。**第一步**（共两步，
/// 第二步是 [`canonical_http_authority`]）：
/// 1. 删掉**全部** ASCII tab / LF / CR —— 这一刀在规范里是"任意位置"，不是只删首尾；
/// 2. 再删掉**首尾**的「C0 控制符或空格」（`U+0000`–`U+001F` 与 `U+0020`）。
///
/// **只做形状归一，不改 `Task::url` 本体**：这里返回的是一份新串，落盘用的还是原串（见 `validate()` 文档）。
///
/// 为什么 T99-G1c 之后这一步**还留着**：那一步把 `" \thttp://…"` 与 `"http\t://…"` 这类写法还原成
/// 规范形状，于是它们走的是"userinfo / 明文 HTTP"那两支（措辞对用户可行动），而不是被第二步当成
/// 非规范写法拒掉 —— 这是**用户体验**那一侧，不是安全边界；安全边界在第二步（少认形状＝绕过，
/// 而"把首尾空白当规范"并不放行任何凭据：还原之后 authority 里的 `@` 照样看得见）。
///
/// 为什么 `str::trim()` 不够（T99-G1 那一发用的正是它）：`trim` 按 `char::is_whitespace` 判首尾，
/// 于是第 2 步**少认**了 C0 里非空白的那批（`U+0000`–`U+0008`、`U+000E`–`U+001F`），
/// 第 1 步则**整个缺失**。少认的后果是 `get(..7)` 前缀判据与 `scheme_separator_len` 同时认不出 scheme
/// ⇒ 明文 HTTP 那一支与 userinfo 那一支**两道闸一起恒假**。实测形状与后果记在勘误 **E32**，不复制表格。
///
/// **多认/少认哪个方向危险要记清**：少认＝绕过（要防的就是这一支）；多认＝保守地多拒一条串，代价只到
/// 一句报错（可接受）——这类非规范写法现在由 `validate()` 在 CLI 这一路当场以"参数写错了"拒、到不了
/// reqwest（T99-G1c 反转默认），不再"放过去交给对面报错"。本函数选的是"照解析器剥"，两边都不多：
/// `U+00A0` / `U+2028` 这类 Unicode 空白解析器不剥，我们也不再剥（`trim()` 以前会剥，那是多认支，不算缺陷，但对齐之后没必要留着）。
fn url_for_gate(raw: &str) -> String {
    let without_ct = raw
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect::<String>();
    without_ct
        .trim_matches(|c: char| (c as u32) <= 0x20)
        .to_string()
}

/// `scheme://` 之后的字节下标；没有合法 scheme 时 `None`。
///
/// scheme 按 RFC 3986 = `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`，**大小写都算**：
/// 与 `validate()` 里那行 `eq_ignore_ascii_case` 同一立场（Task 3 的修复轮教过：`starts_with` 不够，
/// 大写 `HTTPS://` 不能成为绕过闸口的写法）。这里连"是不是 scheme"都要判，所以比一下字母集合，
/// 而不是随手 `find("://")` —— `x://y` 那种前面不是 scheme 的串不当它分隔符。
fn scheme_separator_len(url: &str) -> Option<usize> {
    let sep = url.find("://")?;
    let mut bytes = url.get(..sep)?.bytes();
    let starts_alpha = bytes.next().is_some_and(|b| b.is_ascii_alphabetic());
    let rest_ok = bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    (starts_alpha && rest_ok).then_some(sep + 3)
}

/// URL 的 **authority** 段（`scheme://` 之后、第一个 `/`、`?` 或 `#` 之前）里是否出现 `@`，
/// 即 RFC 3986 的 userinfo 组件（勘误 E9：这种形态的凭据一律拒绝）。
///
/// 只查 authority 是刻意的：`@` 在 path/query/fragment 里完全合法（`https://x/a@b.bin`、
/// `?to=me@host`），整串 `url.contains("@")` 会把正常 URL 一起拒掉。
///
/// 没有合法 scheme 时把"串首到第一个 `/`、`?`、`#`"当 authority（保守那一支）。
///
/// **这一支为什么仍然要保守**：不写 scheme 的串与分隔符变体现在由 `validate()` 在 CLI 这一路当场
/// 以"参数写错了"拒掉（T99-G1c 反转默认），到不了发送点 ⇒ 这一支服务的不是用户那一行，而是
/// **第二个调用点**：`probe::landed_url_is_refused` 用**同一个**判据检查**服务器重定向交回的落地串**，
/// 那份串不经 `validate()`、形状不由我们控制，判不出 scheme 时只能取保守那一支 ——
/// 这一侧少认＝凭据被放过，没有"交给对面报错"这种兜底可指望。
///
/// **`pub(crate)` 是 Task 6 的可见性改动（派发第 9 条），逻辑一个字没动**：`probe` 要用**同一个**判据
/// 检查**服务器控制的**落地 URL（Task 5 评审 C3；E9 那道闸只管用户自己给的 `task.url`，够不着这里）。
/// 一条规则有两个版本的判据，迟早读成两样 —— 复用比复制便宜，也比在 `probe` 里再写一个解析器可靠。
pub(crate) fn authority_has_userinfo(url: &str) -> bool {
    let after = scheme_separator_len(url).map_or(url, |n| &url[n..]);
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    after[..end].contains('@')
}

/// **只认规范写法**的形状闸（T99-G1c 的设计：**反转默认** —— 不再试图认出所有坏写法，只放行规范写法）。
///
/// 产品承诺（README 与 `Task::validate()` 的文档同一句话）：
/// > 本 CLI 只接受 `http://` 或 `https://` 的规范写法：scheme 之后必须紧跟恰好 `://`，
/// > 主机段（authority，终止符是 `/` `\` `?` `#` 之一）**非空、且第一个字节不是 `:`**
/// > （这两刀合起来＝主机名非空；后一半是 T99-G1d **裁定一** 把"authority 非空"收到 host 层那一刀），
/// > 里面不许出现 `@`。
/// > 任何其它形状一律当场以"参数写错了"拒绝。
///
/// 返回 `Some(true)` = 规范的 `https://`、`Some(false)` = 规范的 `http://`、
/// `None` = **这条 URL 本 CLI 不接受**（调用点报错，不是降级、不是转交 reqwest 去报错）。
///
/// 为什么只有这一支站得住（前两轮各留一族的根因）：reqwest 走 WHATWG 解析器，那个解析器对
/// **special scheme** 在冒号后吃掉任意一串 `/` 与 `\` —— `http:/host`、`http:\host`、`http:////host`
/// 都照样解析成带 userinfo 的 http URL（控制员 2026-10-18 实测三族，见 T99-G1b 报告 §6.1）。
/// **逐形状清单追不上对面实现的接受集**，所以这里把判据收成一个有限集合：
/// `None` 那一支覆盖"所有非规范形状"，而放行集是 reqwest 接受集的**真子集** ⇒ 放行的每一条
/// 我们判的 scheme/authority 就是它解析出来的那一套，不存在"判的是一回事、解析成另一回事"。
/// 代价是**多拒**（`ftp://` 之类原本交给 reqwest 报错的串现在由我们当场拒），
/// 那是用户可见的行为变更，见 README 与勘误；少认才有安全后果（凭据上明文线、进断点文件），
/// 多认只有一句报错。
///
/// **不引入 `Url::parse`**：真解析一次要 `url` crate ⇒ 破"直接依赖只许 6 个 crate"的用户上限
/// （2026-10-15 定的），这一支已被明确否掉，所以判据只能做成形状。
///
/// 四步判据，缺一即 `None`（入参是**原始** URL：第一步归一住在本函数里，两个调用点不可能吃到两份串）：
/// 1. [`url_for_gate`] 的那两刀（tab/LF/CR 与首尾 C0）；
/// 2. 归一之后的串首必须是 `http://` 或 `https://`（大小写不敏感），且 `://` **恰好一次** ——
///    分隔符之后紧跟的第一个字节不许是 `/` 或 `\`（那正是"少斜杠/多斜杠/反斜杠"那一族）；
/// 3. authority（分隔符之后到第一个 `/`、`\`、`?`、`#`）里不含 `@`。
///    终止符集合带 `\` 是刻意的：WHATWG 对 special scheme 把 `\` 当路径分隔符 ⇒ `http://h\\a@x`
///    的 authority 是 `h`，那个 `@` 在 path 里、不是凭据。少带一个终止符就会把 path 里的 `@`
///    读成凭据 ⇒ 多拒一条本来抓得下来的合法写法（本项目的老账：`split(['?','#'])` 少一个终止符
///    就少一半形状 —— 同一族反方向犯的错，所以 `https://h:8443/a.bin?u=alice@x#frag` 与
///    `http://[::1]:8080/u:p@x` 两格都在表里钉着）。
///    ⚠ 残余一面要说窄：`validate()` 那一侧还**并联**着 `authority_has_userinfo`（简报明令本体不许动，
///    它的终止符集合只有 `/` `?` `#`），所以 `http://h\\a@x` 在闸上仍按 userinfo 拒 ——
///    方向是"多拒"（不是绕过），登记在 T99-G1c 报告 §5，不在本发的射程内。
/// 4. **主机名非空**（T99-G1c-R1 **裁定三** 立的是"authority 非空"，T99-G1d **裁定一** 把同一刀收到 host 层）：
///    空 host 的两种长相都在这里拒 —— 整串为空（`https://`、`http://`）与**空 host 带端口**
///    （`https://:8443/a.bin`、`https://:/a.bin`，判据是 authority 首字节为 `:`）。
///    以前这两族都过得了形状闸（`url_is_https` 还给真），失败点落在 reqwest 的 `builder error: empty host`
///    （真下载退 1、`--no-start` 退 0）⇒ 现在由我们当场拒，不再靠对面兜底。
///    为什么用首字节而不是切端口：合法 host 的首字节不可能是 `:`（IPv6 字面量按 RFC 3986 必带方括号），
///    所以这一刀数学上碰不到任何合法写法 —— 表里 `https://[::1]:8080/a.bin` 那一格就是它的正向对照臂。
pub(crate) fn canonical_http_authority(raw: &str) -> Option<bool> {
    canonical_shape_of(&url_for_gate(raw))
}

/// [`canonical_http_authority`] 的第二到四步（第一步已由调用方做过；拆开是因为 `validate()`
/// 手里已经有那份归一后的串，两处共用同一个形状判据本体，不复读原文）。
fn canonical_shape_of(url: &str) -> Option<bool> {
    let after = scheme_separator_len(url)?;
    // `scheme_separator_len` 交回的是 `"://"` 之后的下标，减 3 回到 scheme 末尾 ⇒ 那一段就是 scheme 名。
    let scheme = url.get(..after - 3)?;
    let is_https = if scheme.eq_ignore_ascii_case("https") {
        true
    } else if scheme.eq_ignore_ascii_case("http") {
        false
    } else {
        return None; // http/https 之外的 scheme（含 `httpx://`、`ftp://`）本 CLI 不接受
    };
    let rest = url.get(after..)?;
    // `://` 之后又是 `/` 或 `\` ⇒ 分隔符不止一次（`http:///h`、`http:////u:p@h`、`http://\h`）。
    if rest.starts_with(['/', '\\']) {
        return None;
    }
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // 第四步（T99-G1c-R1 **裁定三** 立的是"authority 非空"；T99-G1d **裁定一** 把同一刀收到 **host** 层）：
    // host 为空 ⇒ 非规范，而"host 为空"只有两种长相 —— authority 整串为空（`https://`、`http://`），
    // 或**以 `:` 开头**（空 host + 端口：`https://:8443/a.bin`、`https://:/a.bin`）。
    // 以前后一种过得了形状闸（`url_is_https` 还给真、`--no-start` 退 0 打出摘要），失败点落在对面：
    // 真下载由 reqwest 报 `builder error: empty host`（退 1，凭据上不了线）。方向本就安全，
    // 但那句话变成"靠对面兜底"—— 本项目对"两把串"付过太多次学费，所以宁可多拒两条
    // （合法用户不会写空 host）。代价：`--no-start` 那一臂从退 0 变退 2。
    //
    // **为什么是 `starts_with(':')` 而不是 `split(':').next()`**：URL 的 host 段第一个字节不可能是
    // `:`（IPv6 字面量按 RFC 3986 必须带方括号，`[::1]:8080` 的首字节是 `[`），所以这一刀
    // **数学上碰不到任何合法写法**。`split(':').next()` 会把 `[::1]:8080` 算成 `[` —— 侥幸不错，
    // 但那是靠"没人写裸 IPv6"兜底，正是本项目不要的那种判据。
    // 下面那一刀已经把 authority 里**任意位置**的 `@` 拒掉了 ⇒ 这里不必再为 userinfo 做剥离。
    if authority.is_empty() || authority.starts_with(':') {
        return None;
    }
    if authority.contains('@') {
        return None; // authority 里的凭据形态（E9 那一支在 `validate()` 里另有更具体的措辞）
    }
    Some(is_https)
}

/// 这条 URL 的 scheme 是不是 `https`（**大小写都算**；认不出合法 scheme 时给假）。
///
/// 新增的 `pub(crate)` 辅助函数（评审 **F2**）：**不改动上面任何函数的行为**，只是把 `validate()`
/// 里那句 scheme 比较同一个立场做成可复用的判据，交给 `probe::with_contract_headers` 决定
/// `Authorization: Basic` 上不上线（Task 7 的 `fetch` 复用同一个构造器）。
///
/// 为什么这道闸要有个正式的判据而不是各写各的：
/// - `validate()` 那条规则只管**明文 HTTP + 口令**（spec §4），**用户名单独给出**时它一句话都不说，
///   可那半副凭据也一样不该在明文的 socket 上跑。
/// - scheme 的比较必须大小写不敏感（Task 3 的修复轮教过：`HTTP://` 一样是明文），且要认 RFC 3986
///   的 scheme 形状 —— 所以复用上面那个 `scheme_separator_len`，而不是随手 `starts_with("https://")`。
/// - **认不出 scheme ⇒ 假**（保守那一支）：不写 scheme 的串现在由 `validate()` 在 CLI 这一路当场拒
///   （T99-G1c 反转默认），到不了发送点 ⇒ 这一支不是在"替对面报错"，而是这条谓词自己的方向：
///   它是 `pub(crate)` 的纯判据，契约不许假设"调用点已经把坏形状过滤掉了"，而认不出是加密线时
///   唯一安全的取法是不送凭据（误判成真的后果是口令上明文线）。
///
/// **入参先归一，再过形状闸**（T99-G1 加的是 `trim()`，T99-G1b 换成 `url_for_gate`，T99-G1c 再叠上
/// [`canonical_http_authority`]，与 `validate()` 读同一份串）：
/// **只有"规范写法且 scheme 是 `https`"才给 `true`** —— 非规范形状（`https:/\host`、`https:////host`、
/// `ftp://`…）一律 `false`，那是保守那一支（认不出是加密线就不发凭据），与闸那一支同源：
/// 判据与 `validate()` 走的是同一个函数的同一次归一，两把串不许分家。
/// `probe::with_contract_headers` 与 `fetch` 复用**这一个**判据决定 `Authorization: Basic` 上不上线。
/// 不同步的后果是"闸按归一后的串放行了 `\u{1}https://host`，发头时按原串算出 `false` ⇒
/// 该带的凭据静默不带" —— 与闸那一侧是同一个"两把串"的病。
/// `scheme_separator_len` 本体不动（它是 `validate()` 与这里共用的底层形状判断，牵到 `probe.rs`
/// 那条"服务器控制的落地 URL"检查，那一支的入参不由我们控制）。
pub(crate) fn url_is_https(url: &str) -> bool {
    canonical_http_authority(url) == Some(true)
}

/// **只查"越目录"那一半**：按 `/` 与 `\` 两种分隔符切段后，任一段恰为 `.` 或 `..` ⇒ 这份名字会把
/// 落点带出 `--out-dir`（切段与"整段相等"的理由见 [`out_name_leaves_out_dir`] 第 3 刀）。
///
/// ⚠ **本发（T99-G5c-R1）的轴 = 这道闸对两种名字来源用的是*不同宽度*的判据**（控制员裁定表，
/// 不许改判：不许统一成"两边都查"，不许统一成"两边都不查"，**更不许 sanitize**）：
///
/// | 名字来源 | 拒 `.`／`..` 段 | 拒 `/`、`\` 打头 | 拒「字母＋`:`」盘符形状 |
/// |---|---|---|---|
/// | 用户显式 `--out-name` | 拒（[`out_name_leaves_out_dir`] 全判据，现役不动） | 拒（同上） | **拒（同上，现役不动）** |
/// | 由 URL 末段推导 | **拒**（本函数） | 不可能出现（末段不含 `/`） | **不拒** |
///
/// 为什么推导名那一支**不查**文件名合法性（盘符形状），四条理由：
/// 1. 用户 2026-10-18 那句裁定是"**落点不许越出 `--out-dir`**"。`u:p@x`、`a:b` 这种带冒号的末段
///    **不出目录** —— "这个文件名在 Windows 上到底能不能创建"是另一件事，**没人裁过**，不该由一道
///    越目录闸顺手收下。
/// 2. 更硬的一条：本项目那张形状表（`the_gate_admits_only_the_canonical_http_shape`）把
///    `http://[::1]:8080/u:p@x` 定为**规范写法、必须放行**。两道闸对同一条 URL 一个说"规范"、
///    一个说"参数写错了"＝**两本账**，正是勘误 E32 那一族病（G5c 半成品那一版红的就是这一格）。
/// 3. 显式那一支维持全判据**不是**因为冒号名字在 Unix 上合法（它也合法），而是因为**用户亲手打的串**
///    里 `C:` 打头就是绝对路径意图，拒它是把落点说清楚；推导那一支的输入不是用户亲手打的名字，
///    是 URL 的一个片段 ⇒ **只对"走出目录"负责**。
/// 4. 这条不对称由 `the_derived_name_and_the_explicit_one_are_not_held_to_the_same_width` 钉着
///    （同一个串走推导 ⇒ 放行，走 `--out-name` ⇒ 拒）。缺那一臂，下一个改这里的人会把它"顺手统一"：
///    统一成两边都查＝重新引入上面第 2 条那一红并与形状表变成两本账，统一成两边都不查＝把越目录放开。
fn escapes_out_dir(name: &str) -> bool {
    name.split(['/', '\\']).any(|seg| seg == ".." || seg == ".")
}

/// **用户显式给的 `--out-name`** 那一格会不会把落点带出 `--out-dir`（T99-G5 第 1 条立的闸，
/// 用户 2026-10-18 裁定＝**拒掉**；本函数是它对**显式名**那一支的**全判据**，现役宽度不动）。
///
/// 为什么这一闸也得管到推导名：用户 2026-10-18 的裁定原文是"**落点不许越出 `--out-dir`**"，
/// 那句话里**没有**"只在用户显式命名时才管用"的限定 ⇒ G5 那一版只管了显式名，是同一条承诺没做完。
/// 但补完的是**覆盖面**而不是**宽度**：推导名那一支走 [`escapes_out_dir`]，只查越目录那一半。
///
/// 危害的来源只有一句实现事实：`Task::target_path()` 是 `self.dir.join(name)`，而 `PathBuf::join`
/// 遇到**绝对形状**会整份丢掉左半边（`D:\out`.join(`/n`) ＝ `/n`），遇到 `.`／`..` 段则在真实文件系统
/// 解析时把落点挪到父目录／挪到目录本身。
///
/// ⚠ **危害要说准（注释与报告同一句话）**：`target_path()` 读的是 `task.url` —— **用户自己打进去的
/// 那一条**，不是服务器重定向交回的 `final_url` ⇒ 这一族是**用户自伤**那一格（自己写了个会走出目录的
/// URL／名字），**不是远端可利用的漏洞**。所以措辞停留在"参数写错了"那一族（天然退 2），
/// 不许写成"目录穿越攻击已修"，也不许升级成安全告警的口吻。
///
/// G5 当时把推导名那一支判成"带不出洞"，理由写在旧注释里：`rsplit('/')` 只取最后一段
/// （`…/a/../../evil.bin` 的末段是 `evil.bin` —— 这一支确实安全，新闸不许误杀它）。
/// 控制员 2026-10-19 用真二进制实测（`--no-start`）证明**漏了另一半**：末段**本身就可以是** `..` 或 `.`：
/// `--url https://127.0.0.1:1/..` 的落点是 `<out-dir>\..`（走出去了），`…/.` 的落点是 `<out-dir>\.`
/// （落在目录本身）。⇒ 判据现在查的是 [`Task::effective_name`]，即 `join` 真会拿到的那份串。
///
/// 只禁这两类，**相对子路径照旧允许**（`sub/x.bin` 必须仍放行）：裁的是"不许越出去"，
/// 不是"不许有子目录"。判据三刀（第 1、2 刀只落在**显式名**那一支上，见 [`escapes_out_dir`] 那张表）：
/// 1. 串首是 `/` 或 `\` ⇒ 绝对形状（POSIX 绝对路径／Windows UNC 与根）。
/// 2. 前两字节是「ASCII 字母 + `:`」（`C:` 形状）⇒ Windows 盘符前缀。
///    `C:x` 在 Windows 上是"A 盘当前目录里的 x"，同样是"离开 `--out-dir`"的写法，所以只看前两字节、
///    不要求后面跟分隔符。代价：Unix 上合法文件名 `a:b.txt` 会被多拒一条 —— 方向是多拒（一句可行动的
///    报错），不是少认，且与"这一族在 Windows 上真是路径"这件事一致。
/// 3. 按 `/` **与** `\` 两种分隔符切段，任一段恰为 `..`（T99-G5c 起**还包括** `.`）⇒ 走出目录／走到目录本身。
///    ⚠ **按"整段相等"判，绝不按"包含子串"判**：`v1.2.bin`、`a...b`、`my.file.tar.gz` 都是合法名字，
///    换成 `name.contains('.')` 或 `contains("..")` 就会把它们一起拒掉 —— 本发的 m2 变异守的正是这一条。
///    这一刀单独住 [`escapes_out_dir`]：它是"越目录"那一半，也是推导名那一支**唯一**用到的判据。
///    `.` 这一格是 G5c 补的：`--url …/.` 推出来的末段就是 `.`，`join` 交回的是目录本身而不是目录里的
///    一个文件（`<out-dir>\.`），同样不是"落在 `--out-dir` 里"的形状。
///    分隔符集合带 `\` 是刻意的：闸必须对着**用户脚下这台机器**的最宽解释判，而 Windows 上
///    `..\x` 就是父目录（少带一个分隔符＝`..\evil.bin` 整串被当成一个文件名放过去，父目录照进）。
///
/// **读的是原串、不做归一**：`url_for_gate` 那两刀（tab/LF/CR 与首尾 C0）是给 WHATWG 解析器对齐的，
/// 文件名侧没有对面那个"吃掉空白与多余斜杠"的解析器可对齐 —— `join` 拿到什么就写什么。
/// 因此这一闸**不比 `target_path()` 更宽容，也不比它更严**：判的就是 `join` 真会怎么做，
/// 而且（G5c 之后）判的是**同一份串** —— 两侧都读 [`Task::effective_name`]，不存在"闸看一本账、
/// 落盘看另一本账"。**宽度**按来源分两支（那张表），**取的那份串**不分两支。
fn out_name_leaves_out_dir(name: &str) -> bool {
    let b = name.as_bytes();
    if b.first().is_some_and(|c| *c == b'/' || *c == b'\\') {
        return true;
    }
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return true;
    }
    escapes_out_dir(name)
}

impl Task {
    /// 唯一的校验闸（规则只住这一处，见勘误 E23 附记的纪律）。
    ///
    /// **T99-G5 第 1 条加了第五条判据**（落点名字的形状，见 [`out_name_leaves_out_dir`]）：
    /// 下面那句"四条判据读的都是同一份归一后的串"管的仍是 **URL 那四条**；名字那一条读
    /// [`Task::effective_name`] 那份**原串**，因为它要对齐的是 `PathBuf::join` 而不是 WHATWG 解析器。
    /// 规则依旧只住这一处。
    ///
    /// ⚠ **T99-G5c 改的是第五判据"查哪一份名字"，T99-G5c-R1 改的是"对不同来源用多宽的判据"**：G5 那一版
    /// 只查 `self.name`，于是`--out-name` 不给、名字由 URL 末段推导的那一半从没过分（实测 `--url …/..`
    /// 的落点是 `<out-dir>\..`）。现在第五判据查的是 `effective_name()` —— 也就是 `target_path()` 真拿去
    /// `join` 的那份串 ⇒ 闸与落盘共用同一个出口，不存在两本账。宽度则按来源分两支：显式名用全判据
    /// [`out_name_leaves_out_dir`]，推导名只用 [`escapes_out_dir`]（那张不对称表连同四条理由住在它的
    /// 文档里，`the_derived_name_and_the_explicit_one_are_not_held_to_the_same_width` 是它的针）。
    /// 危害仍是**用户自伤**那一格（`task.url` 是用户自己打的，不是服务器交回的 `final_url`），
    /// 不是远端可利用的漏洞。
    ///
    /// **四条判据读的都是同一份归一后的串**（`url_for_gate`），不是原串：reqwest 走 WHATWG 基本 URL
    /// 解析器，那个解析器会删掉全部 tab/LF/CR 与首尾的 C0 控制符或空格 —— 于是
    /// `" \thttp://host/a.bin"`、`"\u{1}http://host/a.bin"`、`"http\t://host/a.bin"` 都照样解析成功、
    /// 照样把请求发出去，而读原串的判据（第 1 字节起的 `http://`、`://` 的 scheme 形状、authority
    /// 里的 `@`）会同时恒假。后果由控制员实测记录在勘误 **E32**：口令可在明文 http 上线、也可随
    /// `task.url` 落进断点文件。这里把「过闸的串」与「判 scheme 的串」统一成同一份。
    ///
    /// **第四条是 T99-G1c 的"反转默认"**：前三条只认得**规范形状**上的坏味道，而 WHATWG 对 special
    /// scheme 的分隔符容忍（`http:/h`、`http:\h`、`http:////h`、少斜杠、反斜杠当分隔符）意味着
    /// **"认出所有坏写法"这一支补不完**（G1 与 G1b 各留一族，控制员实测）。所以第四条不再去认坏写法，
    /// 而是只放行规范写法。产品承诺（README 与 `canonical_http_authority` 的文档同一句话）：
    ///
    /// > **本 CLI 只接受 `http://` 或 `https://` 的规范写法：scheme 之后必须紧跟恰好 `://`，
    /// > 主机段（authority，终止符是 `/` `\` `?` `#` 之一）非空、首字节不是 `:`（合起来＝主机名非空）
    /// > 且里面不许出现 `@`。**
    /// > 任何其它形状一律**当场以"参数写错了"拒绝**（⇒ 天然落退出码 **2**）。
    ///
    /// 其它形状（含 `ftp://`、`file://`、不写 scheme 的串，以及"空 host"那一族：T99-G1c-R1 裁定三补的
    /// `https://`、`http://`，与 T99-G1d 裁定一补的**空 host 带端口** `https://:8443/a.bin`、`https://:/a.bin`）
    /// 都在这"任何其它"里。这是一次**用户可见的
    /// 行为变更**：这类串以前是"放过去交给 reqwest 报错"，现在由我们当场拒。判据的取舍方向只有一个：
    /// 少认（放行）才有安全后果，多认（拒一条 reqwest 本来抓得住的串）只代价一句报错。
    ///
    /// **存储形状一个字都不改**：`task.url` 照原样落盘（续传要用），凭据之所以不可能再落盘，
    /// 是因为闸会把这类输入拒掉，**不是**因为我们把串改干净了 —— 别把这一发"顺手修"成归一后回写。
    pub fn validate(&self) -> anyhow::Result<()> {
        // 归一规则只住在 `url_for_gate` 那一处（T99-G1b）：以前这里是 `self.url.trim()`，
        // 只按 `char::is_whitespace` 剥首尾 ⇒ 少认了首尾的 C0 与串中间的 tab/LF/CR（少认＝绕过，见其文档）。
        let url = url_for_gate(&self.url);
        anyhow::ensure!(!url.is_empty(), "缺少 URL");
        let plain_http = url.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://"));
        anyhow::ensure!(!(plain_http && self.password.is_some()),
            "明文 HTTP 不发送口令：请改用 https:// 或去掉 --password");
        // 勘误 E9：凭据写在 URL 的 userinfo 里会**原样落盘**两处（`task.url` 与 `validator.final_url`），
        // 而 `state` 删不掉 URL（续传要用）。所以这条只在**这一个**闸口里判，`state` 只指路、不重复实现。
        // 入参交进来的是**已归一的** `url`（`authority_has_userinfo` 本体不归一，见它的文档）。
        anyhow::ensure!(!authority_has_userinfo(&url),
            "URL 的 host 前不能带凭据（userinfo，形如 https://用户:口令@主机/…）：\
             请把它去掉并改用 --user / --password 传入");
        // 第四条（T99-G1c）：前三条管的是"规范形状上的坏味道"，这一条管"形状本身"。
        // 非规范 ⇒ 拒，不降级、不转交 reqwest 报错。排在前面的 E9 那条**位置是刻意的**：
        // 规范串上的 userinfo（`https://alice:口令@host/x`）必须继续收到那句能行动的措辞
        // （"改用 --user / --password 传入"），不能被这句泛泛的"参数写错了"顶掉。
        anyhow::ensure!(canonical_shape_of(&url).is_some(),
            "参数写错了：URL 要的是 `http://` 或 `https://` 的规范写法（scheme 之后紧跟恰好 `://`，\
             主机名非空且 host 前不带凭据），不接受 `http:/host`、`http:/\\host`、`http:////host` 这类分隔符变体，\
             也不接受 http/https 之外的 scheme 与不写 scheme 的串；`https://`（没有主机名）与 \
             `https://:8443/a.bin`（只有端口、没有主机名）这一族，请在 `://` 之后补上主机名");
        // 第五条（T99-G5 第 1 条立、T99-G5c 把覆盖面补完、T99-G5c-R1 定下宽度那张表）：前四条管 URL，
        // 这一条管**最终生效的那个名字**。排在最后**是刻意的**——它读的是 `effective_name()` 而不是
        // 归一后的 URL 串，把它插在中间会让"四条判据读同一份串"那句话变成需要读者自己分辨的例外。
        // 措辞走"参数写错了"那一族 ⇒ 天然落退出码 2（E23⑤：与引擎那句"校验不符"不同源）。排在最末
        // 还有一个可观察的后果：URL 与名字同时写错时，用户先看到 URL 那句。
        //
        // ⚠ 这里分成两支钉的是**判据宽度**，不是**取名的来源**：`name` 那一侧（显式名）用全判据
        // [`out_name_leaves_out_dir`]，`None` 那一侧（推导名）只用 [`escapes_out_dir`]（只查 `.`／`..`
        // 段）。两边读的都是 [`Task::effective_name`] 交回的**同一份串** ⇒ 闸与 `target_path()` 共用
        // 同一个出口，不存在两本账；那张不对称表与四条理由住在 [`escapes_out_dir`] 的文档里。
        // ⚠ 并且**没有**做任何"把 `..` 剥掉/换成 `download`"的 sanitize：静默改写用户的落点与用户
        // 裁的"拒掉"相反。
        // 两种来源的**措辞也分开**：显式名点名 `--out-name`，推导名说出"这条 URL 的末段"——
        // 否则没写 `--out-name` 的用户会去改自己根本没填过的那一格。
        let name = self.effective_name();
        let refused = match &self.name {
            Some(_) => out_name_leaves_out_dir(&name),
            None => escapes_out_dir(&name),
        };
        if refused {
            anyhow::bail!(match &self.name {
                Some(_) => "参数写错了：--out-name 要的是落在 --out-dir 里的名字，\
                            不能是绝对路径、也不能含 \"..\" 段（要子目录请直接写相对路径，如 sub/x.bin）",
                None => "参数写错了：这条 URL 的末段推不出一个落在 --out-dir 里的名字\
                        （末段是 \".\" 或 \"..\" 时落点会走到目录之外或目录本身）；\
                        请用 --out-name 显式给一个相对文件名",
            });
        }
        Ok(())
    }
    pub fn segmentable(&self) -> bool { self.data_file.is_none() }
    pub fn ignore_mtime(&self) -> bool { self.flags & FLAG_IGNORE_MTIME != 0 }
    /// **真正会落盘的那个名字**：用户给的 `--out-name`，不给时从 URL 末段推导。
    ///
    /// 提成一个函数只为一个理由（T99-G5c 第 1 条）：`target_path()` 与 `validate()` 必须读**同一份**串，
    /// 否则闸守的是"用户填的那个字段"，磁盘上落的却是另一本账 —— G5 那一版漏掉的正是这一格。
    ///
    /// 推导规则（**与 G5 之前逐字相同**，本发只搬位置不改语义：先砍掉 `?`/`#` 之后的 query 与
    /// fragment，再取最后一个 `/` 之后的那段）；那段为空（URL 以 `/` 收尾）或整条没有 `/` 之外的
    /// 末段时兜到 `download`。百分号**原样保留**，不做解码 —— 这一格是 STATUS 里"保留现状＋钉用例"
    /// 那条裁定的对象（`derived_names_from_the_url_take_their_last_segment_only`）。
    ///
    /// ⚠ 这里**不 sanitize**：推出来是 `..` 就照实是 `..`，由 `validate()` 拒掉，不由这里改写。
    fn effective_name(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let tail = self.url.split(['?', '#']).next().unwrap_or(&self.url);
        tail.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("download").to_string()
    }
    pub fn target_path(&self) -> PathBuf {
        self.dir.join(self.effective_name())
    }
    // 这里**不留任何常数访问器**：`Task` 只带每条任务真不一样的东西（URL、目录、名字、flags、
    // 凭据、验证子、body 文件）。以前有个 `limits() -> (预读, 段下限, 连接数)`，文档写着
    // "供 scheduler 初始化用"，而引擎只读 `.0`、调度器初始化读的是 `Config` ——
    // **文档正好说反**，照它填 `Config` 会把 `--connections` / `--min-seg` 静默吞成默认值。
    // 那两个引擎不读的数已删（8F-5 的 C1），剩下的那一个（`PROBE_READ_BYTES`）是全局常数，
    // 由 `engine::run` 直接读 `consts::PROBE_READ_BYTES` —— 它不属于某条任务，所以不住在 `Task` 上。
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(url: &str) -> Task {
        Task { url: url.into(), referer: None, cookies: None, data_file: None,
               user: None, password: None, dir: ".".into(), name: None, flags: 0 }
    }
    #[test]
    fn rejects_password_over_plain_http() {
        let mut x = t("http://example.com/a.bin");
        x.password = Some("hunter2".into());
        assert!(x.validate().is_err(), "明文 HTTP 带口令必须拒绝，不许降级发送");
    }
    #[test]
    fn allows_password_over_https() {
        let mut x = t("https://example.com/a.bin");
        x.password = Some("hunter2".into());
        assert!(x.validate().is_ok());
    }
    #[test]
    fn scheme_case_does_not_weaken_the_password_rule() {
        // scheme 按 RFC 3986 大小写不敏感，`HTTP://` 一样是明文 HTTP，不能因为调用方写了大写就绕过。
        for url in ["HTTP://example.com/a.bin", "HtTp://example.com/a.bin"] {
            let mut x = t(url);
            x.password = Some("hunter2".into());
            assert!(x.validate().is_err(), "{url} 是明文 HTTP，带口令必须拒绝");
        }
        // 反过来：大写 https 仍然是加密的，不能被误判成明文。
        let mut ok = t("HTTPS://example.com/a.bin");
        ok.password = Some("hunter2".into());
        assert!(ok.validate().is_ok(), "HTTPS:// 带口令必须放行");
    }
    #[test]
    fn post_body_disables_segmentation() {
        let mut x = t("https://example.com/a.bin");
        assert!(x.segmentable());
        x.data_file = Some("body.bin".into());
        assert!(!x.segmentable(), "带 POST 体时 Range 语义不成立，必须单段");
    }

    // ---- 勘误 E9 / 评审 F3：URL 里带 userinfo 凭据 ⇒ 在唯一的校验闸口拒绝 ----

    #[test]
    fn rejects_credentials_carried_in_the_url_authority() {
        // `--url https://alice:s3cret@x/a.bin` 会把凭据原样落进 `task.url` 与 `validator.final_url`
        // 两处，而 `state` 删不掉 URL（续传要用）⇒ 唯一站得住的读法是**在 validate() 里拒**。
        for url in [
            "https://alice:s3cret@x/a.bin", // 用户名+口令
            "http://alice:s3cret@x/a.bin",  // 明文支路同样拒（与是否带 --password 无关）
            "https://alice@x/a.bin",        // 只有用户名的 userinfo 也是凭据形态
            "HTTPS://ALICE:S3CRET@x/a.bin", // scheme 大写不许绕过（与上面那行 eq_ignore_ascii_case 同一立场）
            "https://alice:s3cret@x",       // 没有 path 也一样是 userinfo
            "https://alice:s3cret@x/a.bin?q=1#f",
        ] {
            let Err(err) = t(url).validate() else {
                panic!("{url} 的 authority 里有 @，必须被 validate() 拒绝");
            };
            let msg = err.to_string();
            assert!(msg.contains("--user") && msg.contains("--password"), "错误要指使用这两个参数：{msg}");
            assert!(!msg.contains("s3cret"), "错误消息本身不该把口令抄一遍：{msg}");
        }
    }

    #[test]
    fn an_at_that_is_not_in_the_authority_is_not_credentials() {
        // 判据只看 authority：`@` 出现在 path/query/片段里是合法的，一律放行。
        for url in [
            "https://x/a@b.bin",           // path 里
            "https://x/dir/alice@example/a.bin",
            "https://x/a.bin?to=me@host",  // query 里
            "https://x/a.bin#frag@1",      // fragment 里
            "https://x:8080/a.bin",        // 端口不是 userinfo
            "https://[fe80::1]/a.bin",     // IPv6 字面量
        ] {
            assert!(t(url).validate().is_ok(), "{url} 的 authority 里没有 @，必须放行");
        }
        // 无 scheme 的串（`x/a.bin`、`/n`、`//h/x`）⇒ **本 CLI 不接受**，由形状闸（`validate()` 第四条）
        // 当场拒（T99-G1c 反转默认；以前那一支是"含 @ 就拒、不含就放行到 reqwest 的
        // relative URL without a base"，已被"只放行规范写法"废除）。
        assert!(t("alice:s3cret@x/a.bin").validate().is_err(), "无 scheme 但 authority 含 @ ⇒ 拒");
        assert!(t("http://h/a@b").validate().is_ok(), "规范写法、`@` 落在 path 里 ⇒ 不是凭据（正向对照臂）");
    }

    #[test]
    fn the_disk_gate_inherits_the_userinfo_rule() {
        // 规则只住在 `validate()` 这一处；`state::Snapshot::from_task` 已经调它（Task 4 裁定 ③），
        // 所以磁盘边界自动受保护 —— 这条用例证明"不需要在 state 里再写一遍"。
        let mut x = t("https://alice:s3cret@x/a.bin");
        x.password = Some("s3cret".into());
        assert!(x.validate().is_err(), "带 userinfo 的任务在 validate() 就被拒 ⇒ 到不了落盘点");
        // 改写成两个参数（我们期望用户走的形状）后，同一条 URL 的干净版本照常通过。
        x.url = "https://x/a.bin".into();
        x.user = Some("alice".into());
        assert!(x.validate().is_ok(), "口令走 --user/--password 是本工具支持的形状");
    }

    // ---- T99-G1 / G1b：URL 的"首尾空白 + 首尾 C0 + 串中间 tab/LF/CR"都不得绕过凭据闸 ----
    //
    // 这些负向判据（G1 的 1/2/3 加既有那格）钉的是**同一个契约**：`validate()` 与 `url_is_https()` 读的是
    // **同一份串**（`url_for_gate` 归一之后的），不是"过闸看原串、判 scheme 看另一份"。实测证据
    // （V1 未带凭据跑完、V2 明文 http 上线 Basic、V4 口令进断点文件，以及 G1b 那四种形状）由控制员记在
    // 计划勘误 **E32**，这里不复述过程。正向对照臂（G1 的 4/5/6）的作用相同：证明归一只把判据读正，
    // 没有把合法写法与保守那一支一起改掉。

    #[test]
    fn rejects_password_over_plain_http_with_leading_whitespace() {
        // 首尾空白不是新 scheme：reqwest 会把它剥掉，所以 ` \thttp://…` 就是明文 HTTP。
        // 修复前这里读的是原串，前 7 个字节是 `" \thttp:"` ⇒ `plain_http` 恒假 ⇒ 闸放行、下载在
        // **不带凭据**的情况下跑完（口令被静默吞掉，比拒发更糟，因为用户以为它生效了）。
        for url in [
            " \thttp://example.com/a.bin",
            "\t http://example.com/a.bin",
            "\nhttp://example.com/a.bin",
            "\u{0c}http://example.com/a.bin", // `char::is_whitespace` 也算它
            "http://example.com/a.bin ",      // 尾空白同一支（trim 两头）
        ] {
            let mut x = t(url);
            x.password = Some("hunter2".into());
            let Err(err) = x.validate() else {
                panic!("{url:?} 是带空白的明文 HTTP，配口令必须被闸拒掉");
            };
            let msg = err.to_string();
            assert!(msg.contains("明文 HTTP 不发送口令"), "错误文案要指改 https 或去掉 --password：{msg}");
            assert!(!msg.contains("hunter2"), "错误消息本身不该把口令抄一遍：{msg}");
        }
    }

    #[test]
    fn rejects_userinfo_behind_leading_whitespace() {
        // E9 那条 userinfo 闸同样只看 trim 后的串：` \thttp://u:p@host/` 在 reqwest 那里解析成功、
        // 折成 `Authorization: Basic` 发上线，同时 `task.url` 原样落盘 ⇒ 凭据两处都出去。
        // 修复前 `scheme_separator_len` 因 scheme 段以空白开头而返回 `None`，`authority_has_userinfo`
        // 于是拿整条串去找第一个 `/`，截到的那段里没有 `@` ⇒ 恒假。
        for url in [
            " \thttp://u:p@example.com/a.bin",
            "    https://alice:s3cret@x/a.bin",
            "\r\nhttps://alice@x/a.bin ",
        ] {
            let Err(err) = t(url).validate() else {
                panic!("{url:?} 的 authority 里有 @，首尾空白不许让它变成合法写法");
            };
            let msg = err.to_string();
            assert!(msg.contains("userinfo"), "错误要说出 userinfo 这个形状：{msg}");
            assert!(!msg.contains("s3cret"), "错误消息本身不该把口令抄一遍：{msg}");
        }
    }

    #[test]
    fn rejects_whitespace_only_url() {
        // `self.url.is_empty()` 对 `"   "` 为假 ⇒ 全空白的 URL 曾能过闸（评审 F3 的另一条 F8，
        // 与上面同一处毛病：判据读的是未 trim 的原串）。
        for url in ["   ", "\t", "\u{0b}\u{0c}", " \t\r\n "] {
            let Err(err) = t(url).validate() else {
                panic!("{url:?} 里除了空白什么都没有，必须报缺少 URL");
            };
            assert!(err.to_string().contains("缺少 URL"), "{url:?} 要走缺少 URL 那一支：{err}");
        }
    }

    #[test]
    fn accepts_https_url_with_leading_whitespace() {
        // 正向对照臂：证明上面那把归一没把合法写法一起杀掉 —— 带首尾空白的 `https://`（不带口令）
        // 仍然过闸。闸管的是"读哪份串"，不是"替用户把 URL 改干净"（`task.url` 照原样落盘）。
        for url in ["   https://example.com/a.bin", "\thttps://example.com/a.bin\t"] {
            assert!(t(url).validate().is_ok(), "{url:?} 是加密的、authority 无 @，必须放行");
        }
        // 同一份串上的口令支也照放行（与 `allows_password_over_https` 同一立场，只是多了首尾空白）。
        let mut x = t("  https://example.com/a.bin");
        x.password = Some("hunter2".into());
        assert!(x.validate().is_ok(), "https 带口令是本工具支持的形状，首尾空白不改变这一点");
    }

    #[test]
    fn url_is_https_ignores_surrounding_whitespace() {
        // 发头那一侧的判据必须与闸读同一份串：否则 `  https://h/` 过了闸却算出 `false`，
        // 该带的凭据静默不带（换来 401 而不是错误）。
        assert!(url_is_https("  https://h/"), "带首空白的 https 仍是 https");
        assert!(url_is_https("HTTPS://h/x "), "大写 scheme 加尾空白也仍是 https");
        assert!(!url_is_https("\thttp://h/"), "带首空白的明文 http 不是 https");
        assert!(!url_is_https("  not-a-url  "), "保守那一支仍成立：认不出 scheme ⇒ 假");
        assert!(!url_is_https("   "), "全空白 ⇒ 假（不是 panic，也不是真）");
        // T99-G1b 追加的两条：这一族的判据必须与闸读同一份**归一后**的串，而不只是"去掉空白"。
        assert!(url_is_https("\u{1}https://h/"), "前导 C0 不是新 scheme：解析器剥掉它，那还是 https");
        assert!(!url_is_https("ht\ntp://h/"), "串中间的 LF 被删掉后是 `http` ⇒ 不是 https（保守那一支照旧）");
    }

    #[test]
    fn clean_shapes_still_behave_as_before() {
        // 第二把正向对照臂：干净串的行为与修复前逐字一致 —— `rejects_password_over_plain_http`
        // 那格的串仍然退（那格本身没动），干净 `https://` 串仍然过。
        let mut plain = t("http://example.com/a.bin");
        plain.password = Some("hunter2".into());
        assert!(plain.validate().is_err(), "干净明文 HTTP 带口令必须拒绝，不许降级发送");
        let mut secure = t("https://example.com/a.bin");
        secure.password = Some("hunter2".into());
        assert!(secure.validate().is_ok(), "干净 https 带口令必须放行");
    }

    // ---- T99-G1b：归一对齐 WHATWG 的两步（删**全部** tab/LF/CR ＋ 删**首尾** C0 或空格）----
    //
    // `trim()` 只按 `char::is_whitespace` 剥首尾 ⇒ 少认了两类：首尾的 C0 里非空白那批、串中间的 tab/LF/CR。
    // 少认＝绕过（要防的方向），多认＝保守地多拒一条写法。多拒的代价要说准：T99-G1c 反转默认之后，
    // 形状不合的串由 `validate()` 当场以"参数写错了"拒（退 2），根本到不了对面 ⇒ 代价是
    // **用户可见的行为变更**（README 已按这个口径写），不是"对面本来也会报错所以无所谓"。
    // 下面四把负向格逐条对应控制员在 G1 之后实测的四种形状（退 0 且对端真收到 `authorization: Basic`），
    // 过程与后果记在勘误 **E32**，这里不复述表格。两把正向格防"修过头"，并钉住"归一不回写"。

    #[test]
    fn rejects_password_over_plain_http_with_leading_c0_control() {
        // `U+0001` 不是空白（`char::is_whitespace` 为假），但落在 WHATWG 的"首尾 C0 控制符或空格"里，
        // 解析器照剥 ⇒ 这条在 reqwest 那里就是**明文** http。G1 之后实测：退 0、口令上了明文线（E32）。
        for url in [
            "\u{1}http://example.com/a.bin",
            "\u{1f}http://example.com/a.bin", // C0 的最后一个码位
            "\u{0}http://example.com/a.bin",  // NUL 也在那一刀里
            " \u{1}http://example.com/a.bin", // 空格与 C0 混着排在前面
            "\u{1}http://example.com/a.bin\u{1}", // 尾同一支（首尾都削）
        ] {
            let mut x = t(url);
            x.password = Some("hunter2".into());
            let Err(err) = x.validate() else {
                panic!("{url:?} 的 scheme 前只有 C0/空格，配口令必须被闸拒掉");
            };
            let msg = err.to_string();
            assert!(msg.contains("明文 HTTP 不发送口令"), "错误文案要指改 https 或去掉 --password：{msg}");
            assert!(!msg.contains("hunter2"), "错误消息本身不该把口令抄一遍：{msg}");
        }
    }

    #[test]
    fn rejects_userinfo_behind_leading_c0_control() {
        // 同一份串上的第二道闸（E9 的 userinfo）：前导 C0 曾经让 `scheme_separator_len` 返回 `None`，
        // `authority_has_userinfo` 于是拿整条串找第一个 `/`，截到的那段里没有 `@` ⇒ 恒假（实测退 0，E32）。
        for url in [
            "\u{1}http://u:p@example.com/a.bin",
            "\u{1f}https://alice:s3cret@x/a.bin",
            "\u{1} https://alice@x/a.bin", // C0 + 空格混排：两步都走到才认得出 scheme
            "\u{1}http://u:p@example.com/a.bin ",
        ] {
            let Err(err) = t(url).validate() else {
                panic!("{url:?} 的 authority 里有 @，前导 C0 不许让它变成合法写法");
            };
            let msg = err.to_string();
            assert!(msg.contains("userinfo"), "错误要说出 userinfo 这个形状：{msg}");
            assert!(!msg.contains("s3cret"), "错误消息本身不该把口令抄一遍：{msg}");
        }
    }

    #[test]
    fn rejects_tab_inside_the_scheme_separator() {
        // "删全部 tab/LF/CR"那一步（不只是首尾）：`http\t://…` 在解析器那里就是 `http://…`。
        // 修复前 tab 卡在 `h` 与 `://` 中间 ⇒ `get(..7)` 与 `find("://")` 两头都对不上 ⇒ 两道闸同时恒假。
        for url in [
            "http\t://u:p@example.com/a.bin",
            "h\tttp://u:p@example.com/a.bin", // tab 落在 scheme 名里也一样
            "https\t://alice:s3cret@x/a.bin", // scheme 大写/https 两支同一条路
            "http\t://alice@x",               // 只有用户名也是凭据形态
        ] {
            let Err(err) = t(url).validate() else {
                panic!("{url:?} 删掉中间的 Tab 就是带 userinfo 的 URL，必须被闸拒掉");
            };
            let msg = err.to_string();
            assert!(msg.contains("userinfo"), "错误要说出 userinfo 这个形状：{msg}");
            assert!(!msg.contains("s3cret"), "错误消息本身不该把口令抄一遍：{msg}");
        }
    }

    #[test]
    fn rejects_newline_inside_the_url_shape() {
        // 与上一格同一步的另一半：LF/CR 也照样被解析器**在任意位置**删掉 —— `ht\ntp://…` 删掉 `\n`
        // 之后才是 `http`，所以这一格证明的是"删全部 tab/LF/CR"那一刀，不是首尾那一刀。
        for url in [
            "ht\ntp://u:p@example.com/a.bin",
            "h\rttp://u:p@example.com/a.bin",
            "https\r\n://alice:s3cret@x/a.bin", // 两个都算，位置在 scheme 之后也一样
            "http://alice:s3cret@x/a\r.bin",    // 串尾方向的中间 CR 也不许把 authority 挪出 @
        ] {
            let Err(err) = t(url).validate() else {
                panic!("{url:?} 删掉中间的换行就是带 userinfo 的 URL，必须被闸拒掉");
            };
            let msg = err.to_string();
            assert!(msg.contains("userinfo"), "错误要说出 userinfo 这个形状：{msg}");
            assert!(!msg.contains("s3cret"), "错误消息本身不该把口令抄一遍：{msg}");
        }
        // 同一刀也喂 `plain_http` 那一支：中间 LF 的明文 http 配口令必须走"明文 HTTP 不发送口令"。
        let mut x = t("ht\ntp://example.com/a.bin");
        x.password = Some("hunter2".into());
        let Err(err) = x.validate() else {
            panic!("`ht\\ntp://…` 归一后是明文 HTTP，配口令必须被闸拒掉");
        };
        assert!(err.to_string().contains("明文 HTTP 不发送口令"), "要走那一支：{err}");
    }

    #[test]
    fn accepts_https_url_with_leading_c0_control_or_space() {
        // 正向对照臂 ①：归一不许把合法写法一起杀掉。带前导 C0（混着空格）的 `https://`、不带口令 ⇒ 过闸；
        // 同一份串上 `url_is_https` 必须给 `true` —— 凭据该发就发，走的是加密线，不能因为前导控制符就静默不发。
        for url in [
            "  \u{1}https://example.com/a.bin",
            "\u{1}https://example.com/a.bin",
            "\u{1f} https://example.com/a.bin\t", // 尾部混一个 Tab（同一步会删掉它）
            "\u{1}https://example.com/a@b.bin",   // `@` 在 path 里仍然只是 path
        ] {
            assert!(t(url).validate().is_ok(), "{url:?} 归一后是加密的、authority 无 @，必须放行");
            assert!(url_is_https(url), "{url:?} 的 scheme 就是 https，判据要给真");
        }
        // 口令支照放行（与 `accepts_https_url_with_leading_whitespace` 同一立场，只是多了 C0）。
        let mut x = t("\u{1}https://example.com/a.bin");
        x.password = Some("hunter2".into());
        assert!(x.validate().is_ok(), "https 带口令是本工具支持的形状，前导 C0 不改变这一点");
    }

    #[test]
    fn gate_normalization_does_not_rewrite_the_stored_url() -> anyhow::Result<()> {
        // 正向对照臂 ②（本发的"存储形状"契约）：`url_for_gate` 只产**判据读的那份串**，
        // 绝不回写 `task.url`。续传要拿原串重新发请求，改写过的话下一条任务的字节就对不上来源。
        const RAW: &str = "\u{1}http\t://host/a.bin";
        let x = t(RAW);
        assert!(x.validate().is_ok(), "{RAW:?} 归一后是明文 http、authority 无 @ 且不带口令 ⇒ 必须放行");
        assert_eq!(x.url, RAW, "闸只读归一后的那份，不许把归一结果写回 `task.url`");
        assert_eq!(url_for_gate(RAW), "http://host/a.bin", "归一本身的两步（对照读：不回写、只给判据）");

        // `Snapshot::from_task` 那条落盘路径同样不受影响：它只是调 `validate()`（规则仍只住一处，
        // 见 E23 附记），存下来的还是原串。夹具在本格里手搭 —— `state.rs` 一行没动。
        let validator = crate::state::Validator {
            etag: Some("E".into()),
            last_modified: Some("L".into()),
            total_len: 10,
            final_url: RAW.into(),
            ignore_mtime: false,
        };
        let snap = crate::state::Snapshot::from_task(
            &x,
            validator.clone(),
            vec![crate::state::SegView { start: 0, end: 9 }],
        )
        .expect("过闸的任务必须能构造快照");
        assert_eq!(snap.task.url, RAW, "断点文件里存的必须是原串，不是归一后的那份");
        assert_eq!(snap.validator.final_url, RAW, "`validator.final_url` 同样不回写");
        assert!(!snap.needs_secret, "这条任务本来就没带口令");

        // 反面：同一形状配上口令 ⇒ 落盘口也拒（`from_task` 调的就是这同一个闸，G1b 修的那份串自动生效）。
        let mut with_secret = x.clone();
        with_secret.password = Some("hunter2".into());
        let Err(err) = crate::state::Snapshot::from_task(&with_secret, validator, Vec::new()) else {
            panic!("前导 C0 + 中间 Tab 的明文 http 配口令，在落盘口也必须被拒");
        };
        assert!(err.to_string().contains("明文 HTTP 不发送口令"), "落盘口要走闸文案：{err}");
        Ok(())
    }

    // ---- T99-G1c：**反转默认** —— 闸不再认坏写法，只放行规范写法 ----
    //
    // 同一条根因（我们判的串 ≠ reqwest 真解析的串）的实测账见计划文末勘误 **E32**：每一轮都只关掉
    // "列举到的那一族"，前几轮的做法（先只按 `trim()` 读、再做 WHATWG 两步预处理）实测之后，
    // `http:/h`、`http:/\h`、`http:////h` 这些分隔符变体**照样**退 0 并把 `authorization: Basic` 打到
    // 明文线上 ⇒ **逐形状清单追不上对面实现的接受集**。
    // 于是这一发改成"只放行规范写法"：`^https?://`（大小写不敏感、分隔符恰好一次）＋ authority
    // （终止符 `/` `\` `?` `#`）非空（R1 **裁定三**）、首字节不是 `:`（G1d **裁定一**：那一刀从
    // "authority 非空"收到"主机名非空"，夹住 `https://:8443/a.bin` 那族空 host 带端口的写法）且无 `@`。
    // 放行集是 reqwest 接受集的**真子集**，所以"判的是一回事、它解析成另一回事"这一族在这里没有落点。
    //
    // 每一格同时断**三件事**，为的是把 G1 立的"过闸的串＝判 scheme 的串"钉成结构而不是注释：
    // ① `validate()` 的结论，以及**走的是哪一支**（按错误里的字样判，不按消息位置判）；
    // ② `canonical_http_authority` 的形状裁决（`None` = 本 CLI 不接受）；
    // ③ `url_is_https` 与 ② 同向：被拒的串绝不能算 https（保守那一支 ⇒ 凭据不上线）。
    // 口令一律用同一个哨兵 `hunter2`，每格另断"错误消息里不许出现它，也不许出现 URL 里的凭据"。
    #[test]
    fn the_gate_admits_only_the_canonical_http_shape() {
        // 一格 = (原始 URL, 有没有给 --password, 期望)。
        #[derive(Clone, Copy)]
        enum Want {
            /// 规范的 `http://` ⇒ 放行，`url_is_https` 给假。
            Plain,
            /// 规范的 `https://` ⇒ 放行，`url_is_https` 给真。
            Secure,
            /// 拒 = (错误里必须出现的字样, `canonical_http_authority` 的裁决)。
            /// 裁决给 `Some(false)` 的那几格是"形状规范、但撞在明文+口令那一支上"；
            /// 其余一律 `None` —— **非规范形状绝不能落进放行那一支**。
            Refused(&'static str, Option<bool>),
        }
        use Want::Refused;
        const SECRET: &str = "hunter2";
        const URLINFO: &str = "userinfo";
        const PLAIN_TEXT: &str = "明文 HTTP 不发送口令";
        const SHAPE: &str = "参数写错了";
        const MISSING: &str = "缺少 URL";
        let rows: &[(&str, bool, Want)] = &[
            // ① 分隔符变体、凭据写在 URL 里那一族：实测账见计划文末勘误 **E32**
            ("http:/u:PASS@127.0.0.1:8899/x1.bin", false, Refused(SHAPE, None)),
            ("http:/\\u:PASS@127.0.0.1:8899/x2.bin", false, Refused(SHAPE, None)),
            ("http:////u:PASS@127.0.0.1:8899/x3.bin", false, Refused(SHAPE, None)),
            ("http:\\u:PASS@127.0.0.1:8899/x4.bin", false, Refused(URLINFO, None)),
            ("http:\\\\u:PASS@127.0.0.1:8899/x5.bin", false, Refused(URLINFO, None)),
            ("http:u:PASS@127.0.0.1:8899/x6.bin", false, Refused(URLINFO, None)),
            ("http:///u:PASS@127.0.0.1:8899/x7.bin", false, Refused(SHAPE, None)),
            ("https:/h/a.bin", false, Refused(SHAPE, None)),
            ("https:/\\h/a.bin", false, Refused(SHAPE, None)),
            ("https:////h/a.bin", false, Refused(SHAPE, None)),
            // 第三族不带凭据的那一半：旧判据连"这是明文 http"都认不出来（`plain_http` 要字面 7 字节）
            ("http:/h/a.bin", true, Refused(SHAPE, None)),
            // ② G1 那一族（首尾空白）与 G1b 那一族（首尾 C0、串中间 tab/LF/CR）
            ("\u{1}http://u:PASS@h/x", false, Refused(URLINFO, None)),
            ("http\t://u:PASS@h/x", false, Refused(URLINFO, None)),
            (" \thttp://u:PASS@h/x", false, Refused(URLINFO, None)),
            ("ht\ntp://u:PASS@h/x", false, Refused(URLINFO, None)),
            ("h\tttp://u:PASS@h/x", false, Refused(URLINFO, None)),
            ("https\r\n://alice:s3cret@h/x", false, Refused(URLINFO, None)),
            ("http://alice:s3cret@h/a\r.bin", false, Refused(URLINFO, None)),
            ("\u{1}http://h/x", true, Refused(PLAIN_TEXT, Some(false))),
            (" \thttp://h/x", true, Refused(PLAIN_TEXT, Some(false))),
            // ③ 规范形状上的 E9 两支（措辞与方向**一个字都没改**，只是现在只在规范串上跑）
            ("http://h/x", true, Refused(PLAIN_TEXT, Some(false))),
            ("HTTP://h/x", true, Refused(PLAIN_TEXT, Some(false))),
            ("HtTp://h/x", true, Refused(PLAIN_TEXT, Some(false))),
            ("https://alice:s3cret@h/x", false, Refused(URLINFO, None)),
            ("HTTPS://ALICE:S3CRET@h/x", false, Refused(URLINFO, None)),
            ("https://alice@h/x", false, Refused(URLINFO, None)),
            ("https://alice:s3cret@h/x/a?q=1#f", false, Refused(URLINFO, None)),
            // ④ 空与全空白（走"缺少 URL"那一支，不是形状那一支）
            ("", false, Refused(MISSING, None)),
            ("   ", false, Refused(MISSING, None)),
            ("\u{1}\u{0}\t ", false, Refused(MISSING, None)),
            // ⑤ http/https 之外的 scheme 与不写 scheme 的串：**改完由我们当场拒**（用户可见变更）
            ("ftp://h/x", false, Refused(SHAPE, None)),
            ("file:///c/tmp/a.bin", false, Refused(SHAPE, None)),
            ("httpx://h/x", false, Refused(SHAPE, None)),
            ("x/a.bin", false, Refused(SHAPE, None)),
            ("/n", false, Refused(SHAPE, None)),
            ("//h/x", false, Refused(SHAPE, None)),
            // ⑤b 空 host（T99-G1c-R1 **裁定三**，控制员裁的扩面）：以前是"闸放行 ⇒ 由 reqwest 报
            //      `builder error: empty host`"（`--no-start` 退 0、真下载退 1），现在由形状闸当场拒。
            //      顺带关掉上一发报告 §5.2 那一格：空 host 绝不能再被 `url_is_https` 算成真。
            ("https://", false, Refused(SHAPE, None)),
            ("http://", false, Refused(SHAPE, None)),
            //      同一族的另一半（T99-G1d **裁定一**：那一刀从"authority 非空"收到"主机名非空"）——
            //      "空 host 带端口"这一族以前过得了只查 `is_empty()` 的那一刀，失败点落在 reqwest 的
            //      `builder error: empty host`（`--no-start` 退 0、真下载退 1）⇒ 现在同样由我们当场拒。
            ("https://:8443/a.bin", false, Refused(SHAPE, None)), // 空 host + 端口：本发新夹的那一族
            ("https://:/a.bin", false, Refused(SHAPE, None)),     // 空 host + 空端口
            // ⑥ 必须放行的规范写法（含简报点名的合法变体与 IPv6 那几格 —— 抓"终止符把 `]` 当分隔"和
            //      "把端口那枚冒号当成 host 起点"那种**过严**；G1d **裁定一** 那一刀的正向对照臂就在这里）
            ("http://h/x", false, Want::Plain),
            ("HTTP://h/x", false, Want::Plain),
            ("HtTp://h/x", false, Want::Plain),
            ("http://h:8080/a.bin?x=1#f", false, Want::Plain),
            ("https://h:8443/a.bin?x=1#f", false, Want::Secure),
            ("http://[::1]:8080/a.bin", false, Want::Plain),
            // G1d **裁定一** 新增的正向对照臂：带方括号的 IPv6 字面量首字节是 `[` 而不是 `:` ⇒ 那一刀
            // 数学上碰不到它。这一格实测放行（`Admits(true)` 落在本表就是 `Want::Secure`），
            // 变异 m2（把刀换成 `authority.contains(':')`）实测到它红 ⇒ 它是真针，不是装饰。
            ("https://[::1]:8080/a.bin", false, Want::Secure),
            ("http://[::1]:8080/u:p@x", false, Want::Plain),
            ("https://[fe80::1]:8443/a@b.bin", false, Want::Secure),
            ("http://h/a@b.bin", false, Want::Plain),
            ("http://h/a.bin?to=me@host", false, Want::Plain),
            ("http://h/a.bin#frag@1", false, Want::Plain),
            ("https://h/x", true, Want::Secure),
            ("HTTPS://H/X", true, Want::Secure),
            // ⑦ 第一步（那两刀）还挣得到的那几格：还原成规范写法 ⇒ 放行，不是拒
            (" \thttp://h/x", false, Want::Plain),
            ("http://h/x\u{1}", false, Want::Plain),
            ("\u{1f} https://h/x\t", false, Want::Secure),
            ("ht\ntp://h/x", false, Want::Plain),
            ("http://h/a\tb.bin", false, Want::Plain),
        ];
        for (raw, with_password, want) in rows {
            let mut x = t(raw);
            if *with_password {
                x.password = Some(SECRET.into());
            }
            // ②与③：形状裁决与发头判据必须读**同一份串**（`canonical_http_authority` 内部就是那一步归一）。
            let shape = canonical_http_authority(raw);
            let https = url_is_https(raw);
            match *want {
                Want::Plain | Want::Secure => {
                    let secure = matches!(*want, Want::Secure);
                    if let Err(err) = x.validate() {
                        panic!("规范写法 {raw:?}（口令={with_password}）必须放行，实际被拒：{err}");
                    }
                    assert_eq!(shape, Some(secure), "{raw:?} 是规范写法 ⇒ 裁决必须是 Some({secure})");
                    assert_eq!(https, secure, "{raw:?}：`url_is_https` 必须与闸同向（分家的后果是该带的凭据静默不带）");
                    assert_eq!(x.url, *raw, "闸只读归一后的那份，绝不把归一结果写回 `task.url`");
                }
                Refused(needle, want_shape) => {
                    let Err(err) = x.validate() else {
                        panic!("{raw:?}（口令={with_password}）必须被闸拒掉 —— 放行就意味着凭据可以上明文线、\
                                也可以随 `task.url` 落进断点文件");
                    };
                    let msg = err.to_string();
                    assert!(msg.contains(needle), "{raw:?} 要走「{needle}」那一支，实际消息：{msg}");
                    for leak in [SECRET, "PASS", "s3cret", "alice"] {
                        assert!(!msg.contains(leak), "错误消息不该把凭据抄一遍（{leak:?}）：{msg}");
                    }
                    assert_eq!(shape, want_shape, "{raw:?} 的形状裁决与期望不符（闸与裁决必须同源）");
                    assert!(!https, "{raw:?} 已被拒 ⇒ 绝不能再被算成 https（保守那一支＝不发凭据）");
                }
            }
        }
    }

    // ---- 评审 F4：派生的 Debug 会把口令打出来 ----

    #[test]
    fn debug_prints_the_password_field_masked() {
        let mut x = t("https://x/a.bin");
        x.user = Some("alice".into());
        x.password = Some("hunter2".into());
        x.dir = "/tmp/d".into();
        x.name = Some("a.bin".into());
        x.flags = 1;
        let d = format!("{x:?}");
        assert!(!d.contains("hunter2"), "Debug 里不能有口令本尊（Task 8 的错误路径会打 Debug）：{d}");
        assert!(d.contains("password: Some(\"[redacted]\")"), "口令要显式渲染成掩码，而不是消失：{d}");
        // 其余字段照旧可见 —— 掩码只针对 password，别把调试价值一起抹掉。
        for want in [
            "url: \"https://x/a.bin\"",
            "user: Some(\"alice\")",
            "dir:",
            "name: Some(\"a.bin\")",
            "flags: 1",
        ] {
            assert!(d.contains(want), "Debug 里 {want} 该还在：{d}");
        }
        // 没口令时渲染成 None（与今天派生的形状一致），且不得留下空掩码。
        x.password = None;
        let clean = format!("{x:?}");
        assert!(clean.contains("password: None"), "没口令时该是 password: None：{clean}");
        assert!(!clean.contains("[redacted]"), "没口令就不该有掩码：{clean}");
    }

    #[test]
    fn redacting_debug_does_not_make_two_passwords_equal() {
        // 这条挡住"把 password 从结构里删掉/写个假 Debug 顺带把比较也改了"这种伪装修法：
        // PartialEq/Eq 的语义必须一个字都没动（Task 9 的用例要靠 assert_eq! 比较两个 Task）。
        let a = t("https://x/a.bin");
        let mut b = a.clone();
        b.password = Some("hunter2".into());
        let mut c = a.clone();
        c.password = Some("hunter3".into());
        assert_ne!(a, b, "没口令 != 有口令");
        assert_ne!(b, c, "两个不同的口令必须仍然不等（掩码只影响 Debug）");
        assert_eq!(b, b.clone(), "字段全同的两个 Task 必须相等（派生语义未变）");
        let mut d = b.clone();
        d.url = "https://x/b.bin".into();
        assert_ne!(b, d, "非口令字段的不同照样要能被比较发现");
    }

    #[test]
    fn target_path_uses_name_or_url_basename() {
        let mut x = t("https://example.com/dir/big.zip");
        assert_eq!(x.target_path().file_name().unwrap(), "big.zip");
        x.name = Some("renamed.zip".into());
        assert_eq!(x.target_path().file_name().unwrap(), "renamed.zip");
    }

    // ---- T99-G5 第 1 条：`--out-name` 不许把落点带出 `--out-dir`（用户 2026-10-18 裁＝拒掉）----

    /// 造一个"URL 干净、只让名字那一格可变"的任务。
    /// URL 必须干净：不然 `is_err()` 会由前面四条里的某一条交出来，这一发的牙就白装。
    /// 消息 needle 里额外点名 `--out-name` 也正是为这一句服务的（负向格必须死在**这一条**上）。
    fn named(url: &str, name: &str) -> Task {
        Task { name: Some(name.into()), ..t(url) }
    }

    /// 负向四格（简报点名的四格）＋ 简报在判据里另写明的三种 `..` 形状（`x/..`、单独的 `..`、
    /// 反斜杠版 `..\x` ⇒ 后者钉"分隔符集合带 `\`"那一刀，摘掉它这一格就绿）。
    /// 全部要求：`is_err()` 且消息同时含"参数写错了"与"--out-name"（E23⑤ 那一族 ⇒ 退出码 2）。
    #[test]
    fn rejects_out_name_that_leaves_the_out_dir() {
        const SHAPE: &str = "参数写错了";
        let table: &[(&str, &str)] = &[
            ("..", "结尾单独一个 `..`：简报判据里点名的形状"),
            ("../x", "父目录里的一个名字：落在 --out-dir 之外"),
            ("x/..", "`..` 不在串首也照样是往上走一层"),
            ("a/../../b", "中间夹着的 `..` 段，切段之后仍然看得见"),
            (r"..\x", "反斜杠版：Windows 上这是真的父目录，不是名字里的两个点"),
            ("/abs/x", "串首 `/`：PathBuf::join 会整份丢掉 --out-dir"),
            (r"C:\x", "盘符 ＋ 反斜杠：Windows 上的绝对路径"),
            (r"C:x", "只有盘符前缀（没有分隔符）：Windows 上＝该盘当前目录，仍在 --out-dir 之外"),
        ];
        for (name, why) in table {
            let Err(err) = named("https://example.com/a.bin", name).validate() else {
                panic!("--out-name {name:?}（{why}）必须被闸拒掉，实际放行了");
            };
            let msg = err.to_string();
            assert!(msg.contains(SHAPE), "{name:?} 要走\"参数写错了\"那一族（E23⑤ ⇒ 退 2），实际：{msg}");
            assert!(msg.contains("--out-name"), "{name:?} 的消息得点名是哪一格写错了，否则用户没法改：{msg}");
            assert!(!msg.contains("漏洞") && !msg.contains("exploit"),
                "这是\"参数写错了\"那一族的措辞，不许升级成安全告警的口吻：{msg}");
        }
    }

    /// 正向三格（防"修过头"，简报明令必须有）：这一闸裁的是**越出去**，不是**子目录能力**。
    /// `sub/x.bin` 必须仍然放行 ⇒ m2 变异（把判据改宽成"任何分隔符都拒"）只能红在这里。
    #[test]
    fn allows_relative_out_names_inside_the_out_dir() {
        for name in ["x.bin", "sub/x.bin", "a%20b.zip"] {
            let x = named("https://example.com/a.bin", name);
            assert!(x.validate().is_ok(), "{name:?} 是落在 --out-dir 里的相对名字，不许拒： {:?}",
                x.validate().err());
        }
        // 相对子路径不只是"过闸"，它还得真的落成子目录里（这一句钉的是闸与 target_path 没有分家）。
        let x = named("https://example.com/a.bin", "sub/x.bin");
        assert!(x.target_path().to_string_lossy().replace('\\', "/").ends_with("sub/x.bin"),
            "落点应当是 --out-dir 底下的 sub/x.bin：{}", x.target_path().display());
    }

    /// 派生名（`--out-name` 不给，从 URL 末段推）的现状四格 ＋ 一格"带洞的 URL 也推不出洞"。
    /// 本发**不裁"该不该解码 `%20`"**，只把现状钉死（STATUS 那行裁定＝保留现状＋钉用例）。
    #[test]
    fn derived_names_from_the_url_take_their_last_segment_only() {
        let table: &[(&str, &str)] = &[
            ("https://example.com", "example.com"),
            ("https://example.com/", "download"),
            ("https://example.com/q?x=1#f", "q"),
            // 百分号**原样保留**：这一发不许"顺手解码"（裁的是形状，不是编码）。
            ("https://example.com/a%20b.zip", "a%20b.zip"),
            // 对面真给一个长得像穿越的 URL，`rsplit('/')` 只取最后一段 ⇒ 推出来的名字带不出洞。
            ("https://example.com/a/../../evil.bin", "evil.bin"),
        ];
        for (url, want_tail) in table {
            let x = t(url);
            assert!(x.validate().is_ok(), "{url} 干净且没给 --out-name，必须放行：{:?}", x.validate().err());
            let got = x.target_path().file_name().map(|s| s.to_string_lossy().into_owned());
            assert_eq!(got.as_deref(), Some(*want_tail), "{url} 推导出来的落点末段与现状不符");
        }
    }

    // ---- T99-G5c 第 3 条（推导名那一半的负臂）＋ T99-G5c-R1（不对称表）----

    /// 推导名负臂（控制员 2026-10-19 用真二进制实测到的那两格 ＋ 同族的另一半形状）：
    /// 不给 `--out-name` 时，URL 末段**本身就可以是** `..` 或 `.` ⇒ 落点走到目录之外／目录本身。
    /// 要求三件：`is_err()`、走"参数写错了"那一族（E23⑤ ⇒ 退 2）、措辞点名**这条 URL 的末段**而不是
    /// `--out-name` —— 用户根本没填那一格，指错地方等于把用户支使去改一个空字段。
    /// m1 变异（把第五判据退回只查 `self.name`）只能红在这里。
    #[test]
    fn rejects_derived_names_that_walk_out_of_the_out_dir() {
        const SHAPE: &str = "参数写错了";
        const DERIVED: &str = "这条 URL 的末段";
        let table: &[(&str, &str)] = &[
            ("https://h/..", "末段单独一个 `..`：落点是 --out-dir 的上一级"),
            ("https://h/.", "末段是 `.`：落点是目录本身，不是目录里的一个文件"),
            ("https://h/a/..", "`..` 前面还有别的段：切段之后末段仍然是它"),
            ("https://h/../..", "两个 `..` 连着写：末段还是 `..`，不因为多走一层就换个判法"),
            ("https://h/..?q=1", "先砍 query 再取名 ⇒ 末段仍是 `..`，query 不是挡箭牌"),
            ("https://h/..#frag", "同上，fragment 也不是"),
            (r"https://h/..\x", r"反斜杠版：Windows 上 `..\x` 是真的父目录形状，切段带 `\` 才看得见"),
        ];
        for (url, why) in table {
            let x = t(url);
            assert!(x.name.is_none(), "夹具：这一族必须走推导名那一路，否则测的是显式判据");
            let Err(err) = x.validate() else {
                panic!("{url} 的末段（{why}）必须被闸拒掉，实际放行了");
            };
            let msg = err.to_string();
            assert!(msg.contains(SHAPE), "{url} 要走\"参数写错了\"那一族（E23⑤ ⇒ 退 2），实际：{msg}");
            assert!(msg.contains(DERIVED),
                "{url} 没给 --out-name，消息得说出矛头是这条 URL 的末段，否则用户会去改自己没填过的那格：{msg}");
            assert!(!msg.contains("漏洞") && !msg.contains("exploit"),
                "这是\"参数写错了\"那一族的措辞（用户自伤，不是远端可利用），不许升级成安全告警的口吻：{msg}");
        }
    }

    /// 推导名正臂（"修过头"的牙）：末段**长得像**越目录或像非法文件名，但都不走出目录 ⇒ 必须放行。
    /// - `a/../../evil.bin` 钉"我们没拿整条 URL 做子串匹配"（简报点名的那一格）；
    /// - `v1.2.bin`、`a...b`、`my.file.tar.gz`、`seg..ment` 钉"按整段相等判、不按包含子串判"
    ///   （m2 变异的红臂就是这些格）；
    /// - `u:p@x` 钉"推导名那一支不查盘符形状"（不对称表第三列，形状表也守着同一格）。
    #[test]
    fn allows_derived_names_that_only_look_like_an_escape() {
        // 一格 = (URL, 放行后 target_path() 应有的末段；`None` = 这一格只断放行，不断末段)。
        let table: &[(&str, Option<&str>)] = &[
            ("https://h/a/../../evil.bin", Some("evil.bin")),
            ("https://h/v1.2.bin", Some("v1.2.bin")),
            ("https://h/a...b", Some("a...b")),
            ("https://h/my.file.tar.gz", Some("my.file.tar.gz")),
            ("https://h/seg..ment", Some("seg..ment")),
            // ⚠ 这一格不断末段：Windows 上 `PathBuf::join("u:p@x")` 会把 `u:` 当**盘符前缀**整份丢掉
            // 左半边（实测 `target_path().file_name()` 交回的是 `p@x`）。闸**照旧不许**用这件事当拒的
            // 理由 —— 那是"这个名字在 Windows 上算哪条路径"，不是"它走出 --out-dir 的那一刀"，
            // 没人裁过；拿它当理由就是本发那张不对称表被推翻、与形状表重新变成两本账（m3 那一发）。
            ("http://[::1]:8080/u:p@x", None),
        ];
        for (url, want_tail) in table {
            let x = t(url);
            assert!(x.validate().is_ok(),
                "{url} 的末段不走出目录，闸不许拒（拒了就是修过头）：{:?}", x.validate().err());
            if let Some(want) = *want_tail {
                let got = x.target_path().file_name().map(|s| s.to_string_lossy().into_owned());
                assert_eq!(got.as_deref(), Some(want),
                    "{url} 既然放行，闸判的那份名字就必须是 target_path() 真拿去 join 的那份（两本账）");
            }
        }
    }

    /// **钉的是不对称本身，不是某一边的宽容**（控制员裁定表的代码形态）。
    /// 同一份串走两条路 ⇒ 结论可以不同：由 URL 末段**推导**时只查"越目录"那一半（[`escapes_out_dir`]），
    /// 用户**显式**给 `--out-name` 时用全判据（[`out_name_leaves_out_dir`]，含盘符形状，现役不动）。
    ///
    /// 为什么这一臂必须有：缺它，下一个改这里的人会把它"顺手统一"——
    /// 统一成两边都查＝把 `http://[::1]:8080/u:p@x` 那格重新判红、与形状表变成两本账（m3 就是这一发的针）；
    /// 统一成两边都不查＝把 `.`／`..` 那格放开（`..` 那一行守的就是这一支）。
    #[test]
    fn the_derived_name_and_the_explicit_one_are_not_held_to_the_same_width() {
        // 一格 = (那份串, 推导那一支放行吗, 显式那一支放行吗)。
        let table: &[(&str, bool, bool)] = &[
            ("u:p@x", true, false),  // 形状表点名的末段：字母＋冒号，但不走出目录
            ("a:b", true, false),    // 同一族的另一半（不带 `@`，排除"`@` 才是放行理由"）
            ("C:x", true, false),    // 真盘符形状：显式那一支按现役拒，推导那一支不查它
            ("..", false, false),    // 越目录那一半**两边都查** ⇒ 不许"统一成都不查"
            (".", false, false),     // 同上，落点是目录本身那一格
            ("x.bin", true, true),   // 两支都没有理由管的普通名字（正向对照）
        ];
        for (name, derived_ok, explicit_ok) in table {
            let url = format!("http://[::1]:8080/{name}");
            let derived = t(&url);
            assert!(derived.name.is_none(), "夹具：第一路必须是推导名");
            let derived_got = derived.validate().is_ok();
            assert_eq!(derived_got, *derived_ok,
                "{name:?} 走推导那一路应当{}（不对称表：只查越目录那一半）",
                if *derived_ok { "放行" } else { "被拒" });

            let explicit = named("https://example.com/a.bin", name);
            assert!(explicit.name.is_some(), "夹具：第二路必须是显式 --out-name");
            let explicit_got = explicit.validate().is_ok();
            assert_eq!(explicit_got, *explicit_ok,
                "{name:?} 走显式那一路应当{}（现役全判据，宽度本发不动）",
                if *explicit_ok { "放行" } else { "被拒" });

            if derived_got != explicit_got {
                // 两支结论不同的那几格才算钉住了不对称：消息必须各自指对字段。
                let msg = explicit.validate().err().unwrap().to_string();
                assert!(msg.contains("--out-name"),
                    "{name:?} 两支结论不同 ⇒ 显式那一支的措辞必须点名 --out-name，否则用户分不清：{msg}");
            }
        }
        // 不对称臂的另一半证据：形状表那条"规范写法必须放行"的 URL 在这里也必须是绿的。
        let x = t("http://[::1]:8080/u:p@x");
        assert!(x.validate().is_ok(), "形状表把这条定为规范写法 ⇒ 越目录闸不许对它说\"参数写错了\"");
    }

    /// **T99-G5d 第 1 件：一根结构针 —— "闸查的名字"与"`target_path()` 真拿去落盘的名字"必须是
    /// 同一份 `effective_name()` 的两次读取。**
    ///
    /// 为什么行为差分不够（本发存在的全部理由，T99-G5c-R2 实测）：让 `target_path()` 不复用
    /// `effective_name()`、自己再算一遍**等价**的末段（变异 m4 复现），实测 `--lib` 与
    /// `--test integration` 两层**零红**——两份算法今天确实给出同一个串，行为差分看不见"两处各写
    /// 一份等价算法"。能抓它的只有钉实现形状的结构针（与 G5 给 `src/fetch.rs` 四处立的那根是
    /// 同一格病灶的第二次出现，照同一形状办：按函数体切片判＋跳过注释行）。
    ///
    /// 三条断言，全部**按函数体切片**判、**跳过以 `//` 开头的行**（本项目已两次撞上"源码扫描的
    /// 两种假红：注释与字符串里的同名 token"——闸的文档注释里本来就写着 `effective_name`，
    /// 整文件 grep 会把注释当调用点）：
    /// 1. `fn validate(` 体内**出现** `self.effective_name()` —— 闸读的是最终生效的那个名字；
    /// 2. `fn target_path(` 体内**出现** `self.effective_name()` 且**不出现** `rsplit('/')` ——
    ///    落盘不许自己再算一遍（m4 复现实测红在这里，本发的存在证明）；
    /// 3. `fn effective_name(` 体内**出现** `rsplit('/')` —— 末段算法**只住这一处**。没有这一条，
    ///    "禁字＋出现"可以两句一起被掏空：把算法搬进 `target_path`、再删掉这里的，断言 1/2 各自
    ///    照逻辑仍是绿的（m5 实测红在这里）。
    ///
    /// 扫描区按文件里第一个 `#[cfg(test)]` 切断：**这根针自己的文本就带着这些 token**，不切就是
    /// 这种针的第三种假红（针扫到自己），顺手防掉。读文件走运行时 `fs::read_to_string` +
    /// `CARGO_MANIFEST_DIR`（与 G5 那根同形）：不是 `include_str!`，更不是把源码抄进测试当字符串
    /// ——抄一份的针既测不到实现也拦不住改动。
    ///
    /// ⚠ **这根针的失效条件**（不许读成"那条算法算得对"）：它钉的是"闸与落盘读同一处算法"这**一件**事，
    /// **不**保证这条末段算法本身算得对——`effective_name` 的取值对不对由
    /// `derived_names_from_the_url_take_their_last_segment_only` 那几发表钉。反过来，有人把
    /// `effective_name` **改名**且两侧同时改指新名，这一发也会红——那是"钉形状"的代价，刻意的：
    /// 一本账要有名字可查，换了名字就照新名字同步这根针，不许悄悄换账本。
    #[test]
    fn the_gate_and_the_landing_share_one_name_source_and_the_tail_algorithm_lives_only_in_effective_name() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("task.rs");
        let all = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("读 {} 的源码文本失败：{err}（本发是源码扫描针，读不到就是没有判据）", path.display()));
        let src = match all.find("#[cfg(test)]") {
            Some(at) => &all[..at],
            None => panic!("src/task.rs 里找不到 `#[cfg(test)]`——扫描区靠它切掉测试文本，不许退回整文件（针会扫到自己）"),
        };

        // 签名行在**产品区**的非注释行里必须恰好出现一次：出现第二个同名函数＝第二本账。
        let sig_count = |sig: &str| -> usize {
            src.lines().filter(|l| !l.trim_start().starts_with("//") && l.contains(sig)).count()
        };
        for sig in ["fn validate(", "fn target_path(", "fn effective_name("] {
            assert_eq!(sig_count(sig), 1,
                "`{sig}` 在 src/task.rs 产品区应当恰好出现一次（两处同名＝两本账），实得 {} 次", sig_count(sig));
        }

        // 函数体切片：从签名行起按大括号深度截到函数结束，只收非 `//` 开头的行。
        // 这三个函数体里没有字符串/字符字面量带 `{`/`}`，按原文数大括号就够（字节级约定与 G5
        // 那根的计数同一脉：这一族针是形状针，不是解析器）。
        fn body_of(src: &str, sig: &str) -> String {
            let mut body = String::new();
            let mut depth: i32 = 0;
            let mut inside = false;
            for line in src.lines() {
                let t = line.trim_start();
                if t.starts_with("//") { continue; }
                if !inside {
                    if !t.contains(sig) { continue; }
                    inside = true;
                }
                depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
                body.push_str(line);
                body.push('\n');
                if depth == 0 { break; }
            }
            assert!(inside, "产品区里找不到含 `{sig}` 的非注释行——判据应当存在的函数没了");
            assert_eq!(depth, 0, "`{sig}` 的函数体在大括号深度上没闭合——针的夹具跟不上实现形状");
            body
        }

        let validate = body_of(src, "fn validate(");
        assert!(validate.contains("self.effective_name()"),
            "断言 1 红：`fn validate(` 的体内不再读 `self.effective_name()`——闸退回查另一本账\
             （例如只查 `self.name`），放下的名字与落盘的名字不再是同一份");

        let target = body_of(src, "fn target_path(");
        assert!(target.contains("self.effective_name()"),
            "断言 2a 红：`fn target_path(` 的体内不再读 `self.effective_name()`——这正是 m4 的形状：\
             落盘自己再算一遍末段，两份等价算法各住一处；行为差分对它实测零红（G5c-R2），只有这根针抓得到");
        assert!(!target.contains("rsplit('/')"),
            "断言 2b 红：`fn target_path(` 的体内出现了 `rsplit('/')`——落盘在自己算末段，m4 禁的那件事；\
             末段算法只许住在 `effective_name()` 一处");

        let effective = body_of(src, "fn effective_name(");
        assert!(effective.contains("rsplit('/')"),
            "断言 3 红：`rsplit('/')` 不在 `fn effective_name(` 的体内了——末段算法被搬走或删掉（m5）。这一条\
             证明断言 2 的禁字不是装饰：把算法搬进 target_path 再删掉这里的，断言 1/2 各自照逻辑仍是绿的");
    }

    /// **T99-G5d 第 2 件：一发行为差分——对表里每一行，两个**独立观察量**恒等：
    /// A＝"闸放不放"（`validate().is_ok()`），B＝"真拿去落盘的那个串的最后一段是不是个普通名字"。
    ///
    /// ⚠ **这一发不是为了抓 m4（抓不到，上一轮同样的改动实测零红，那是第 1 件结构针存在的证明），
    /// 是为了把"两条算法万一算得不一样"（m4′ 那一族）从"靠注释提醒"升级成"靠针报警"。**
    /// m4′ 具体红在哪一格（复现实测）：`https://h/..?x=1`——重算时不砍 query，末段成 `..?x=1`
    /// （普通名字 ⇒ B 真），而闸砍了 query 后末段是 `..`（A 假），两侧不等 ⇒ 报警。
    ///
    /// B 是**测试自己写的独立量具**，不复用产品里的 `escapes_out_dir`/`out_name_leaves_out_dir`
    /// ——差分要的就是两侧算法不同源；复用的话本发就退化成"断言 A==A"。切段方式与产品一致是巧合，
    /// 语义上是"我去看真拿去落盘那个串的最后一段"。
    ///
    /// ⚠ 为什么 B 用"字符串按两种分隔符切段看最后一段"，不用 `Path::components()`／`file_name()`
    /// （动手前的校准发在本机实测，简报里这句纠正成立）：`components()` 会把 `.` **规范化掉**——
    /// `Path::new("d/.").components()` 交回 `[Normal("d")]`，"最后一段"变成目录本身，
    /// `https://h/.` 那一格会**假红**；`file_name()` 对以 `..` 结尾的路径交回 `None`
    /// （`Path::new("d/..").file_name() = None`），读起来像产品 bug。⇒ 只剩字符串切段这一条路。
    ///
    /// ⚠ **本针的适用域（照实登记，报告同名一节）**：A==B 只在"名字本身不带分隔符段"的行上成立。
    /// 名字内部带分隔符而末段是普通名字的行（`--out-name ../x`、推导 `https://h/..\x`）实测
    /// **A 假而 B 真**——那是显式/推导两支的**判据宽度差**（全判据看所有段、B 量具天生只看最后一段），
    /// 不是两本账。把这些行塞进恒等表就是永久假红，所以拆出来单立一段"盲区段"，按实测登记成
    /// `(假, 真)`。（⚠ 简报把 `--out-name ../x` 写成"两侧都假"，本机实测为 A 假 B 真——
    /// 简报自身笔误，照实测办；两格真正的负臂由 `rejects_out_name_that_leaves_the_out_dir`
    /// 与 `rejects_derived_names_that_walk_out_of_the_out_dir` 守着。）
    #[test]
    fn what_the_gate_admits_and_what_the_landing_tail_shows_always_agree() {
        // 一格 = (URL, `--out-name`；`None` = 走推导名)。全是今天真存在的形状，A/B 见各行注。
        let table: &[(&str, Option<&str>)] = &[
            ("https://h/..", None),                                    // 推导＝`..` ⇒ 两侧都假
            ("https://h/.", None),                                     // 落点是目录本身 ⇒ 两侧都假
            ("https://h/..?x=1", None),                                // 砍 query 后末段仍是 `..` ⇒ 两侧都假；这一格是 m4′ 的牙
            ("https://h/a.bin#..", None),                              // 末段 `a.bin` ⇒ 两侧都真
            ("https://h/v1.2.bin", None),                              // 两侧都真（长得像 `..` 的合法名）
            ("https://h/u:p@x", None),                                 // 两侧都真——这一行同时是不对称表的差分侧
            ("https://example.com/a.bin", Some("sub/x.bin")),          // 显式子路径 ⇒ 两侧都真
            ("https://example.com/a.bin", Some("..")),                 // 显式名末段就是 `..` ⇒ 两侧都假
        ];
        for (url, name) in table {
            let x = match name { Some(n) => named(url, n), None => t(url) };
            let a = x.validate().is_ok();
            let seg = x.target_path().to_string_lossy().split(['/', '\\']).next_back().unwrap_or("").to_string();
            let b = seg != "." && seg != "..";
            assert_eq!(a, b,
                "url={url} --out-name={name:?}：闸放不放 A={a}，落盘串的最后一段是 {seg:?}（是普通名字吗 B={b}）——\
                 两侧不同意＝闸与落盘在读两本账（或两支宽度在这一格塌了，见文档注释的适用域声明）");
        }

        // 盲区段：名字内部带 `..` 段而末段普通 ⇒ B 量具天生看不见，实测登记为 (A 假, B 真)。
        // 哪天这两格翻面（例如有人把两支宽度"顺手统一"），这一段先响——它不是这件事的业主
        // （业主是不对称表那发），只是哨兵。
        for (url, name) in [("https://example.com/a.bin", Some("../x")), ("https://h/..\\x", None)] {
            let x = match name { Some(n) => named(url, n), None => t(url) };
            let a = x.validate().is_ok();
            let seg = x.target_path().to_string_lossy().split(['/', '\\']).next_back().unwrap_or("").to_string();
            let b = seg != "." && seg != "..";
            assert_eq!((a, b), (false, true),
                "url={url} --out-name={name:?} 是登记过的盲区格（末段普通、名字内部含 `..` 段）：实测组合翻了＝\
                 判据宽度被动过，去对照那支不对称表");
        }
    }
}
