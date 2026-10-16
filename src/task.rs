use std::fmt;
use std::path::PathBuf;
use crate::consts::{FLAG_IGNORE_MTIME, PROBE_READ_BYTES, MIN_SEGMENT_BYTES, DEFAULT_CONNECTIONS};

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
pub(crate) fn url_is_https(url: &str) -> bool {
    match scheme_separator_len(url) {
        // `scheme_separator_len` 给的是 `"://" 之后`的下标，减 3 正好回到 scheme 末尾 ⇒ 那一段就是
        // scheme 名本身（不含冒号）。
        Some(after_scheme) => url[..after_scheme - 3].eq_ignore_ascii_case("https"),
        None => false,
    }
}

impl Task {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.url.is_empty(), "缺少 URL");
        let plain_http = self.url.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://"));
        anyhow::ensure!(!(plain_http && self.password.is_some()),
            "明文 HTTP 不发送口令：请改用 https:// 或去掉 --password");
        // 勘误 E9：凭据写在 URL 的 userinfo 里会**原样落盘**两处（`task.url` 与 `validator.final_url`），
        // 而 `state` 删不掉 URL（续传要用）。所以这条只在**这一个**闸口里判，`state` 只指路、不重复实现。
        anyhow::ensure!(!authority_has_userinfo(&self.url),
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
    /// 供 scheduler 初始化用
    pub fn limits(&self) -> (u64, u64, usize) { (PROBE_READ_BYTES, MIN_SEGMENT_BYTES, DEFAULT_CONNECTIONS) }
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
