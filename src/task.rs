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
/// **多认/少认哪个方向危险要记清**：少认＝绕过（要防的就是这一支）；多认＝保守地拒一条本来也走不出
/// reqwest 的串（可接受）。本函数选的是"照解析器剥"，两边都不多：`U+00A0` / `U+2028` 这类 Unicode 空白
/// 解析器不剥，我们也不再剥（`trim()` 以前会剥，那是多认支，不算缺陷，但对齐之后没必要留着）。
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
/// 没有合法 scheme 时把"串首到第一个 `/`、`?`、`#`"当 authority（保守那一支）：这种输入本来也走不出
/// reqwest（"relative URL without a base"），但凭据没有理由因为少写了 `https://` 就被放过。
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
/// > 主机段（authority，终止符是 `/` `\` `?` `#` 之一）**非空**且里面不许出现 `@`。
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
/// 4. **authority 非空**（T99-G1c-R1 **裁定三**，控制员裁的扩面）：`https://`、`http://` 这种空 host
///    以前过得了形状闸（`url_is_https` 还给真），失败点落在 reqwest 的 `builder error: empty host`
///    （真下载退 1、`--no-start` 退 0）⇒ 现在由我们当场拒，不再靠对面兜底。
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
    // 第四步（T99-G1c-R1 **裁定三**，控制员裁的扩面）：authority 为空 ⇒ 非规范。
    // 以前 `https://` / `http://` 过得了形状闸（`url_is_https` 还给真），失败点落在对面：
    // 真下载由 reqwest 报 `builder error: empty host`（退 1，凭据上不了线）。方向本就安全，
    // 但那句话变成"靠对面兜底"—— 本项目对"两把串"付过太多次学费，所以宁可多拒两条
    // （合法用户不会写空 host）。代价：`--no-start` 从退 0 变退 2。
    if rest[..end].is_empty() {
        return None;
    }
    if rest[..end].contains('@') {
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
/// - **认不出 scheme ⇒ 假**（保守那一支）：这种 URL 本来也走不出 reqwest（"relative URL without a
///   base"），但凭据没有理由因为少写了 `https://` 就被送出去。
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

impl Task {
    /// 唯一的校验闸（规则只住这一处，见勘误 E23 附记的纪律）。
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
    /// > 主机段（authority，终止符是 `/` `\` `?` `#` 之一）非空且里面不许出现 `@`。**
    /// > 任何其它形状一律**当场以"参数写错了"拒绝**（⇒ 天然落退出码 **2**）。
    ///
    /// 其它形状（含 `ftp://`、`file://`、不写 scheme 的串，以及 T99-G1c-R1 裁定三补的**空 host**：
    /// `https://`、`http://`）都在这"任何其它"里。这是一次**用户可见的
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
             host 前不带凭据），不接受 `http:/host`、`http:/\\host`、`http:////host` 这类分隔符变体，\
             也不接受 http/https 之外的 scheme 与不写 scheme 的串");
        Ok(())
    }
    pub fn segmentable(&self) -> bool { self.data_file.is_none() }
    pub fn ignore_mtime(&self) -> bool { self.flags & FLAG_IGNORE_MTIME != 0 }
    pub fn target_path(&self) -> PathBuf {
        let name = self.name.clone().unwrap_or_else(|| {
            let tail = self.url.split(['?', '#']).next().unwrap_or(&self.url);
            tail.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("download").to_string()
        });
        self.dir.join(name)
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
    // 少认＝绕过（要防的方向），多认＝保守地拒一条本来也走不出 reqwest 的串（可接受）。
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
    // 同一条根因（我们判的串 ≠ reqwest 真解析的串）已经打了三发：G1 `9cf00e6`（读 `trim()` 后的串）、
    // G1b `4fe6a7c`（自己做 WHATWG 两步预处理），每一发都只关掉"列举到的那一族"：控制员在 `4fe6a7c`
    // 之后实测 `http:/h`、`http:/\h`、`http:////h` 三型**照样**退 0 并把 `authorization: Basic` 打到
    // 明文线上（G1b 报告 §6.1）⇒ **逐形状清单追不上对面实现的接受集**。
    // 于是这一发改成"只放行规范写法"：`^https?://`（大小写不敏感、分隔符恰好一次）＋ authority
    // （终止符 `/` `\` `?` `#`）非空（R1 **裁定三**补的那一刀）且无 `@`。放行集是 reqwest 接受集的
    // **真子集**，所以"判的是一回事、它解析成另一回事"这一族在这里没有落点。
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
            // ① 控制员在 `4fe6a7c` 之后实测出来的第三族（分隔符变体，凭据写在 URL 里）
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
            // ⑥ 必须放行的规范写法（含简报点名的合法变体与两格 IPv6 —— 抓"终止符把 `]` 当分隔"那种过严）
            ("http://h/x", false, Want::Plain),
            ("HTTP://h/x", false, Want::Plain),
            ("HtTp://h/x", false, Want::Plain),
            ("http://h:8080/a.bin?x=1#f", false, Want::Plain),
            ("https://h:8443/a.bin?x=1#f", false, Want::Secure),
            ("http://[::1]:8080/a.bin", false, Want::Plain),
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
}
