//! 代理（0.5.0 · 2）：只认我们自己用得上的三种写法。
//!
//! 设计约束：
//! - **空串 = 直连**（不是错误）。用户清空输入框就该回到直连，不该报"配置非法"。
//! - **缺端口就是错**：默认端口各协议不一样（http 8080 / socks 1080 是常见值但不是标准），
//!   猜一个等于把用户悄悄导去错误的地方。
//! - **密码只留在内存里**：`Redacted` 是唯一能拿去打日志/错误串的形态。

/// 一条可用的代理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    /// `http` / `socks5`
    pub scheme: &'static str,
    pub host: String,
    pub port: u16,
    /// 用户名（可选）
    pub user: Option<String>,
    /// 密码（可选）。**任何 Debug/日志路径都不得带出它。**
    pub password: Option<String>,
}

impl ProxySpec {
    /// 给 reqwest 用的完整 URL（含凭据）。只在建 client 时用。
    pub fn to_url(&self) -> String {
        match (&self.user, &self.password) {
            (Some(user), Some(pass)) => {
                format!(
                    "{}://{}:{}@{}:{}",
                    self.scheme, user, pass, self.host, self.port
                )
            }
            (Some(user), None) => format!("{}://{}@{}:{}", self.scheme, user, self.host, self.port),
            _ => format!("{}://{}:{}", self.scheme, self.host, self.port),
        }
    }

    /// 可以安全写进日志/界面的形态：**不含凭据**。
    pub fn redacted(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

/// 解析设置里的代理串。空串/全空白 → `Ok(None)`。
pub fn parse_proxy(text: &str) -> Result<Option<ProxySpec>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.contains(char::is_whitespace) {
        return Err("代理地址里不能有空格".into());
    }
    let (scheme, rest) = text
        .split_once("://")
        .ok_or_else(|| "要写成 http://主机:端口 或 socks5://主机:端口".to_string())?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" => "http",
        "socks5" | "socks5h" => "socks5",
        other => return Err(format!("不支持的代理类型：{other}（只认 http / socks5）")),
    };
    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((auth, hostport)) => (Some(auth), hostport),
        None => (None, rest),
    };
    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| "缺少端口（例如 127.0.0.1:7890）".to_string())?;
    if host.is_empty() {
        return Err("缺少主机名".into());
    }
    if host.contains('/') {
        return Err("主机名里不该有路径".into());
    }
    let port: u16 = port.parse().map_err(|_| format!("端口不是数字：{port}"))?;
    if port == 0 {
        return Err("端口不能是 0".into());
    }
    let (user, password) = match auth {
        None => (None, None),
        Some(auth) => {
            let (user, pass) = match auth.split_once(':') {
                Some((u, p)) => (u, Some(p.to_string())),
                None => (auth, None),
            };
            if user.is_empty() {
                return Err("用户名不能为空".into());
            }
            (Some(user.to_string()), pass)
        }
    };
    Ok(Some(ProxySpec {
        scheme,
        host: host.to_string(),
        port,
        user,
        password,
    }))
}

/// 给子进程用的环境变量（reqwest 认这三个）。
///
/// 为什么走环境变量而不是改每个 client：真正的下载跑在 `dlm.exe` 子进程里，
/// 那一侧有十几处建 client 的地方；让 reqwest 自己读环境是唯一不会漏的路。
pub fn env_pairs(spec: Option<&ProxySpec>) -> Vec<(String, String)> {
    let Some(spec) = spec else {
        return Vec::new();
    };
    let url = spec.to_url();
    vec![
        ("HTTP_PROXY".to_string(), url.clone()),
        ("HTTPS_PROXY".to_string(), url.clone()),
        ("ALL_PROXY".to_string(), url),
    ]
}

static GLOBAL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// GUI 进程自己的抓取/探测用（子进程走环境变量）。
pub fn set_global(url: Option<String>) {
    let _ = GLOBAL.set(url);
}

pub fn global() -> Option<&'static str> {
    GLOBAL.get().and_then(|v| v.as_deref())
}

/// 用给定代理串发一次最小请求，验证"这条路通不通"。
///
/// 只在用户点「测试」时调用；超时给短一点（默认 5 秒），失败信息里只出现脱敏地址。
pub fn probe_via(text: &str, target: &str, timeout: std::time::Duration) -> Result<String, String> {
    let spec = parse_proxy(text)?;
    let mut builder = reqwest::blocking::Client::builder().timeout(timeout);
    if let Some(spec) = &spec {
        builder = builder.proxy(reqwest::Proxy::all(spec.to_url()).map_err(|e| e.to_string())?);
    }
    let client = builder.build().map_err(|e| format!("建客户端失败：{e}"))?;
    let resp = client.get(target).send().map_err(|e| {
        let where_ = spec
            .as_ref()
            .map(|s| s.redacted())
            .unwrap_or_else(|| "直连".into());
        format!("经 {where_} 请求失败：{e}")
    })?;
    Ok(format!("HTTP {}", resp.status().as_u16()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_means_direct_connection_not_an_error() {
        assert_eq!(parse_proxy("").expect("空串合法"), None);
        assert_eq!(parse_proxy("   ").expect("空白合法"), None);
    }

    #[test]
    fn accepts_the_three_documented_shapes() {
        let http = parse_proxy("http://127.0.0.1:7890").unwrap().unwrap();
        assert_eq!(
            (http.scheme, http.host.as_str(), http.port),
            ("http", "127.0.0.1", 7890)
        );

        let socks = parse_proxy("socks5://10.0.0.2:1080").unwrap().unwrap();
        assert_eq!(socks.scheme, "socks5");

        let socksh = parse_proxy("socks5h://10.0.0.2:1080").unwrap().unwrap();
        assert_eq!(socksh.scheme, "socks5", "socks5h 与 socks5 同义");
    }

    #[test]
    fn refuses_shapes_it_cannot_read() {
        for bad in [
            "127.0.0.1:7890",          // 没有 scheme
            "http://127.0.0.1",        // 没端口
            "http://:7890",            // 没主机
            "ftp://127.0.0.1:21",      // 不支持的类型
            "http://127.0.0.1:0",      // 端口 0
            "http://127.0.0.1:70000",  // 端口越界
            "http://127.0.0.1:7890/x", // 带路径
        ] {
            assert!(parse_proxy(bad).is_err(), "{bad} 不该被接受");
        }
    }

    #[test]
    fn credentials_are_kept_but_never_in_the_redacted_form() {
        let p = parse_proxy("http://alice:s3cret@127.0.0.1:7890")
            .unwrap()
            .unwrap();
        assert_eq!(p.user.as_deref(), Some("alice"));
        assert_eq!(p.password.as_deref(), Some("s3cret"));
        assert!(p.to_url().contains("s3cret"), "建 client 时才用完整串");
        let safe = p.redacted();
        assert_eq!(safe, "http://127.0.0.1:7890");
        assert!(
            !safe.contains("s3cret") && !safe.contains("alice"),
            "{safe}"
        );
    }

    #[test]
    fn a_user_without_a_password_is_still_valid() {
        let p = parse_proxy("http://alice@127.0.0.1:7890").unwrap().unwrap();
        assert_eq!(p.user.as_deref(), Some("alice"));
        assert_eq!(p.password, None);
        assert!(!p.redacted().contains("alice"));
    }
}
