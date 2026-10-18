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
/// **入参先 trim**（T99-G1，与 `validate()` 读同一份串）：`probe::with_contract_headers` 与 `fetch`
/// 复用**这一个**判据决定 `Authorization: Basic` 上不上线。不同步的后果是"闸按 trim 后的串放行了
/// `  https://host`，发头时按原串算出 `false` ⇒ 该带的凭据静默不带" —— 与闸那一侧是同一个"两把串"的病。
/// `scheme_separator_len` 本体不动（它是 `validate()` 与这里共用的底层形状判断，牵到 `probe.rs`
/// 那条"服务器控制的落地 URL"检查，那一支的入参不由我们控制）。
pub(crate) fn url_is_https(url: &str) -> bool {
    let url = url.trim();
    match scheme_separator_len(url) {
        // `scheme_separator_len` 给的是 `"://" 之后`的下标，减 3 正好回到 scheme 末尾 ⇒ 那一段就是
        // scheme 名本身（不含冒号）。
        Some(after_scheme) => url[..after_scheme - 3].eq_ignore_ascii_case("https"),
        None => false,
    }
}

impl Task {
    /// 唯一的校验闸（规则只住这一处，见勘误 E23 附记的纪律）。
    ///
    /// **三条判据读的都是 `self.url` trim 之后的那一份**，不是原串：reqwest 走 WHATWG 基本 URL
    /// 解析器，那个解析器会剥掉首尾空白与控制字符 —— 于是 `" \thttp://host/a.bin"` 照样解析成功、
    /// 照样把请求发出去，而读原串的判据（第 1 字节起的 `http://`、`://` 的 scheme 形状、authority
    /// 里的 `@`）会同时恒假。后果由控制员实测记录在勘误 **E32**：口令可在明文 http 上线、也可随
    /// `task.url` 落进断点文件。这里只把「过闸的串」与「判 scheme 的串」统一成同一份。
    ///
    /// **存储形状一个字都不改**：`task.url` 照原样落盘（续传要用），凭据之所以不可能再落盘，
    /// 是因为闸会把这类输入拒掉，**不是**因为我们把串改干净了 —— 别把这一发"顺手修"成 trim 后回写。
    pub fn validate(&self) -> anyhow::Result<()> {
        // `str::trim` 按 `char::is_whitespace` 判空白（空格 / `\t` / `\n` / `\r` / `\u{0b}` / `\u{0c}`），
        // 比手写 `trim_matches(|c| …)` 的字符清单更全 —— 用 `trim()`，别退回字符清单那一版。
        let url = self.url.trim();
        anyhow::ensure!(!url.is_empty(), "缺少 URL");
        let plain_http = url.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://"));
        anyhow::ensure!(!(plain_http && self.password.is_some()),
            "明文 HTTP 不发送口令：请改用 https:// 或去掉 --password");
        // 勘误 E9：凭据写在 URL 的 userinfo 里会**原样落盘**两处（`task.url` 与 `validator.final_url`），
        // 而 `state` 删不掉 URL（续传要用）。所以这条只在**这一个**闸口里判，`state` 只指路、不重复实现。
        // 入参交进来的是**已 trim 的** `url`（`authority_has_userinfo` 本体不 trim，见它的文档）。
        anyhow::ensure!(!authority_has_userinfo(url),
            "URL 的 host 前不能带凭据（userinfo，形如 https://用户:口令@主机/…）：\
             请把它去掉并改用 --user / --password 传入");
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
        // 没有 scheme 的输入按"authority 从串首算起"处理（保守那一支）：含 @ 就拒，不含就放行到
        // 后面的真正失败（reqwest 会报"relative URL without a base"）。
        assert!(t("alice:s3cret@x/a.bin").validate().is_err(), "无 scheme 但 authority 含 @ ⇒ 拒");
        assert!(t("x/a.bin").validate().is_ok(), "无 scheme 且不含 @ ⇒ 本闸不管（不是 URL 解析器）");
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

    // ---- T99-G1：URL 首尾空白不得绕过凭据闸 ----
    //
    // 这四把负向判据（1/2/3 加既有那格）钉的是**同一个契约**：`validate()` 与 `url_is_https()` 读的是
    // **同一份串**（trim 之后的），不是"过闸看原串、判 scheme 看另一份"。实测证据（V1 未带凭据跑完、
    // V2 明文 http 上线 Basic、V4 口令进断点文件）由控制员记在计划勘误 **E32**，这里不复述过程。
    // 三把正向对照臂（4/5/6）的作用相同：证明 trim 只把判据读正，没有把合法写法与保守那一支一起改掉。

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
        // 正向对照臂：证明上面那把 trim 没把合法写法一起杀掉 —— 带首尾空白的 `https://`（不带口令）
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
