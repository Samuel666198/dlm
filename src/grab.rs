//! 站点抓取（0.4.0 · A）：从页面里挑出"可以下"的链接。
//!
//! 这一版只做**纯函数**：给一段 HTML 与一个基准 URL，吐出候选链接；
//! 再用规则（同域 / 通配 / 深度）筛一遍。真正发请求、出清单、入队是下一步 ——
//! 先把"挑哪些"定死，才能保证"抓什么"可复现、可测试。
//!
//! ## 三条纪律
//!
//! - **不跟随脚本链接**：`javascript:` / `mailto:` / `tel:` / `#fragment` 一律丢。
//! - **相对链接按基准解析**：`/a.zip`、`b.zip`、`//cdn/x.zip` 都要能落到绝对 URL。
//! - **通配只有 `*`**：`*.zip`、`/downloads/*`。不做正则 —— 用户写错一个括号
//!   就变成"什么都没抓到"，而 `*` 写错的后果是看得见的多抓/少抓。

use std::collections::BTreeSet;

/// 抓取规则。默认保守：同域、深度 1、无通配（= 全收）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabRules {
    /// 只收与起始页同主机名的链接。
    pub same_host: bool,
    /// 最多跟几层页面（1 = 只看起始页）。
    pub depth: u8,
    /// 命中任一通配才算（空 = 不限制）。
    pub include: Vec<String>,
    /// 命中任一通配就丢弃。
    pub exclude: Vec<String>,
}

impl Default for GrabRules {
    fn default() -> Self {
        Self {
            same_host: true,
            depth: 1,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

/// 从 HTML 里抽 `href` / `src`，解析成绝对 URL，去重后按出现顺序返回。
pub fn extract_links(html: &str, base: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in scan_attributes(html) {
        let Some(abs) = resolve(base, &raw) else {
            continue;
        };
        if seen.insert(abs.clone()) {
            out.push(abs);
        }
    }
    out
}

/// 扫出所有 `href="…"` / `src="…"` 的值（单双引号都认，忽略大小写）。
fn scan_attributes(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = Vec::new();
    for name in ["href=", "src="] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(name) {
            let at = from + pos + name.len();
            from = at;
            let quote = match bytes.get(at) {
                Some(b'"') | Some(b'\'') => bytes[at] as char,
                // 无引号属性值：读到空白或 '>' 为止
                Some(_) => {
                    let rest = &html[at..];
                    let value: String = rest
                        .chars()
                        .take_while(|c| !c.is_whitespace() && *c != '>')
                        .collect();
                    if !value.is_empty() {
                        out.push(decode_entities(&value));
                    }
                    continue;
                }
                None => break,
            };
            let start = at + 1;
            let Some(end_rel) = html[start..].find(quote) else {
                continue;
            };
            let value = &html[start..start + end_rel];
            if !value.is_empty() {
                out.push(decode_entities(value));
            }
            from = start + end_rel + 1;
        }
    }
    out
}

/// 最少量的实体解码：`&amp;` 是 URL 里最常见的一个，其它原样保留。
fn decode_entities(value: &str) -> String {
    value.replace("&amp;", "&").replace("&#38;", "&")
}

/// 把链接解析成绝对 `http(s)` URL；不能解析（含脚本/邮件等）返回 `None`。
pub fn resolve(base: &str, link: &str) -> Option<String> {
    let link = link.trim();
    if link.is_empty() || link.starts_with('#') {
        return None;
    }
    let lower = link.to_ascii_lowercase();
    for bad in ["javascript:", "mailto:", "tel:", "data:", "blob:", "about:"] {
        if lower.starts_with(bad) {
            return None;
        }
    }
    if let Some(rest) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
    {
        // 绝对地址：只接受带主机名的
        if rest.is_empty() || rest.starts_with('/') {
            return None;
        }
        return Some(strip_fragment(link));
    }
    let (scheme, host, base_path) = split_url(base)?;
    let joined = if let Some(rest) = link.strip_prefix("//") {
        format!("{scheme}://{rest}")
    } else if link.starts_with('/') {
        format!("{scheme}://{host}{link}")
    } else {
        let dir = match base_path.rfind('/') {
            Some(i) => &base_path[..=i],
            None => "/",
        };
        format!("{scheme}://{host}{dir}{link}")
    };
    Some(strip_fragment(&joined))
}

fn strip_fragment(url: &str) -> String {
    match url.find('#') {
        Some(i) => url[..i].to_string(),
        None => url.to_string(),
    }
}

fn split_url(url: &str) -> Option<(&'static str, String, String)> {
    let lower = url.to_ascii_lowercase();
    let (scheme, rest) = if let Some(r) = lower.strip_prefix("https://") {
        let _ = r;
        ("https", &url[8..])
    } else if lower.starts_with("http://") {
        ("http", &url[7..])
    } else {
        return None;
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let host = rest[..end].to_string();
    if host.is_empty() {
        return None;
    }
    let path = if end < rest.len() && rest.as_bytes()[end] == b'/' {
        let tail = &rest[end..];
        tail.split('?').next().unwrap_or("/").to_string()
    } else {
        "/".to_string()
    };
    Some((scheme, host, path))
}

/// 通配匹配：只有 `*`（匹配任意字符），大小写不敏感。
///
/// 两种写法都认：整条 URL（`https://a/*.zip`）或路径（`/downloads/*`、`*.zip`）。
/// 先按整条 URL 试，不中再按路径试 —— 用户写 `/downloads/*` 时想的是路径。
pub fn matches(pattern: &str, url: &str) -> bool {
    let pat = pattern.trim().to_ascii_lowercase();
    if pat.is_empty() {
        return false;
    }
    let text = url.to_ascii_lowercase();
    if match_core(&pat, &text) {
        return true;
    }
    match path_of(url) {
        Some(path) => match_core(&pat, &path.to_ascii_lowercase()),
        None => false,
    }
}

fn match_core(pat: &str, text: &str) -> bool {
    let mut parts = pat.split('*');
    let Some(first) = parts.next() else {
        return false;
    };
    if !text.starts_with(first) {
        return false;
    }
    let mut cursor = first.len();
    let rest: Vec<&str> = parts.collect();
    for part in &rest {
        if part.is_empty() {
            continue;
        }
        let Some(found) = text[cursor..].find(part) else {
            return false;
        };
        cursor += found + part.len();
    }
    if !pat.ends_with('*')
        && let Some(last) = rest.last()
        && !text.ends_with(last)
    {
        return false;
    }
    true
}

/// URL 的路径部分（含开头的 `/`）；不是 http(s) 就没有。
pub fn path_of(url: &str) -> Option<String> {
    split_url(url).map(|(_, _, path)| path)
}
/// 这条链接该不该留下。
pub fn keep(url: &str, page_host: &str, rules: &GrabRules) -> bool {
    let Some((_, host, _)) = split_url(url) else {
        return false;
    };
    if rules.same_host && !host.eq_ignore_ascii_case(page_host) {
        return false;
    }
    if !rules.include.is_empty() && !rules.include.iter().any(|p| matches(p, url)) {
        return false;
    }
    if rules.exclude.iter().any(|p| matches(p, url)) {
        return false;
    }
    true
}

/// 一条链接被判成什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 看起来是要下的文件。
    File,
    /// 还是页面，可以继续往里走（受深度限制）。
    Page,
}

/// 抓取清单：**只列不下载**。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GrabPlan {
    pub files: Vec<String>,
    pub pages: Vec<String>,
    /// 被丢掉的原因，给人看的：`(url, reason)`
    pub skipped: Vec<(String, String)>,
}

/// 常见可下载扩展名。判"是文件"用它，判"是页面"用 `Page` 兜底。
const FILE_EXTS: &[&str] = &[
    "zip", "7z", "rar", "gz", "tar", "iso", "exe", "msi", "dmg", "pkg", "apk", "pdf", "epub",
    "mp4", "mkv", "avi", "mov", "webm", "flac", "mp3", "wav", "m3u8", "mpd", "bin", "img",
];

/// 按扩展名判定这条链接像文件还是像页面。
pub fn classify(url: &str) -> Kind {
    let path = path_of(url).unwrap_or_default();
    let last = path.rsplit('/').next().unwrap_or("");
    let ext = last.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext {
        Some(ext) if FILE_EXTS.contains(&ext.as_str()) => Kind::File,
        _ => Kind::Page,
    }
}

/// 从"已经抓到的页面"里生成清单（不联网、不建任务）。
///
/// `pages` 是 `(页面 URL, HTML)` 的快照；深度按 BFS 层数算，1 = 只看起始页。
/// 同一链接只出现一次；被规则拒掉的进 `skipped`，并带上人话原因。
pub fn plan(start: &str, pages: &[(String, String)], rules: &GrabRules) -> GrabPlan {
    let mut out = GrabPlan::default();
    let Some(start_host) = host_of(start) else {
        out.skipped
            .push((start.to_string(), "起始地址不是 http(s)".into()));
        return out;
    };
    let mut seen_files: BTreeSet<String> = BTreeSet::new();
    let mut seen_pages: BTreeSet<String> = BTreeSet::new();
    seen_pages.insert(start.to_string());

    // depth=1 只看起始页；每多一层就再往里一圈
    let mut frontier: Vec<(String, String)> = pages
        .iter()
        .filter(|(url, _)| url == start)
        .cloned()
        .collect();
    if frontier.is_empty()
        && let Some((url, html)) = pages.first()
    {
        frontier.push((url.clone(), html.clone()));
    }

    for level in 1..=rules.depth.max(1) {
        let mut next: Vec<(String, String)> = Vec::new();
        for (page_url, html) in &frontier {
            for link in extract_links(html, page_url) {
                if !keep(&link, &start_host, rules) {
                    let reason = skip_reason(&link, &start_host, rules);
                    if !out.skipped.iter().any(|(u, _)| u == &link) {
                        out.skipped.push((link, reason));
                    }
                    continue;
                }
                match classify(&link) {
                    Kind::File => {
                        if seen_files.insert(link.clone()) {
                            out.files.push(link);
                        }
                    }
                    Kind::Page => {
                        if level < rules.depth && seen_pages.insert(link.clone()) {
                            out.pages.push(link.clone());
                            if let Some((u, h)) = pages.iter().find(|(u, _)| u == &link) {
                                next.push((u.clone(), h.clone()));
                            }
                        }
                    }
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    out
}

fn skip_reason(link: &str, start_host: &str, rules: &GrabRules) -> String {
    let host = host_of(link).unwrap_or_default();
    if rules.same_host && !host.eq_ignore_ascii_case(start_host) {
        return format!("不同站点（{host}）");
    }
    if !rules.include.is_empty() && !rules.include.iter().any(|p| matches(p, link)) {
        return "不匹配任何包含规则".into();
    }
    "命中排除规则".into()
}

/// 抓取执行层：拉一个页面，**只收 HTML 文本**，并限制大小。
///
/// 三条边界：
/// - `Content-Type` 不是 `text/*` / `application/xhtml+xml` 就不当页面（避免把 zip 读进内存）
/// - 超过 `max_bytes` 直接报错，不截断 —— 半个页面的链接清单比没有更误导
/// - 只发 GET，不带 Cookie：抓取是**匿名浏览**，不碰用户的登录态
pub fn fetch_text(
    url: &str,
    max_bytes: usize,
    timeout: std::time::Duration,
) -> Result<String, String> {
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .user_agent("dlm-grab/0.1");
    if let Some(proxy) = crate::proxy::global() {
        builder =
            builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| format!("代理不可用：{e}"))?);
    }
    let client = builder.build().map_err(|e| format!("建客户端失败：{e}"))?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(ctype.starts_with("text/") || ctype.contains("xhtml")) {
        return Err(format!("不是网页（Content-Type: {ctype}）"));
    }
    if let Some(len) = resp.content_length()
        && len as usize > max_bytes
    {
        return Err(format!("页面太大（{len} 字节，上限 {max_bytes}）"));
    }
    let body = resp.text().map_err(|e| format!("读响应失败：{e}"))?;
    if body.len() > max_bytes {
        return Err(format!("页面太大（超过 {max_bytes} 字节）"));
    }
    Ok(body)
}

/// 抓取上限：页数、每页字节、单次等待。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_pages: usize,
    pub max_bytes: usize,
    pub timeout: std::time::Duration,
    /// 两次请求之间的停顿 —— 对目标站点最基本的礼貌。
    pub delay: std::time::Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pages: 20,
            max_bytes: 2 * 1024 * 1024,
            timeout: std::time::Duration::from_secs(15),
            delay: std::time::Duration::from_millis(300),
        }
    }
}

/// 真正去抓：从起始页开始 BFS，直到页数上限或深度用尽。
///
/// 与 [`plan`] 的分工：`plan` 只算"给我这些页面能挑出什么"，`scan` 负责"去把页面拿回来"。
/// 抓不到某一页不算失败 —— 记进 `skipped` 继续走，站点上一个 404 不该让整次抓取白跑。
pub fn scan(start: &str, rules: &GrabRules, limits: Limits) -> GrabPlan {
    let Some(start_host) = host_of(start) else {
        let mut out = GrabPlan::default();
        out.skipped
            .push((start.to_string(), "起始地址不是 http(s)".into()));
        return out;
    };
    let mut pages: Vec<(String, String)> = Vec::new();
    let mut queue: std::collections::VecDeque<(String, u8)> = std::collections::VecDeque::new();
    queue.push_back((start.to_string(), 1));
    let mut visited: BTreeSet<String> = BTreeSet::new();
    visited.insert(start.to_string());

    // robots 只读一次；读不到就当没有限制（抓的是公开页面，不因 404 就罢工）
    let mut skipped_by_robots: Vec<String> = Vec::new();
    let robots = origin_of(start)
        .and_then(|o| fetch_text(&format!("{o}/robots.txt"), 256 * 1024, limits.timeout).ok())
        .unwrap_or_default();

    while let Some((url, level)) = queue.pop_front() {
        if pages.len() >= limits.max_pages {
            break;
        }
        if !robots_allows(&robots, &path_of(&url).unwrap_or_else(|| "/".into())) {
            skipped_by_robots.push(url.clone());
            continue;
        }
        if !pages.is_empty() && !limits.delay.is_zero() {
            std::thread::sleep(limits.delay);
        }
        match fetch_text(&url, limits.max_bytes, limits.timeout) {
            Ok(html) => {
                for link in extract_links(&html, &url) {
                    if !keep(&link, &start_host, rules) {
                        continue;
                    }
                    if classify(&link) == Kind::Page
                        && level < rules.depth
                        && visited.insert(link.clone())
                        && queue.len() + pages.len() < limits.max_pages * 4
                    {
                        queue.push_back((link, level + 1));
                    }
                }
                pages.push((url, html));
            }
            Err(reason) => {
                // 起始页失败 = 整次抓取失败；子页面失败只记一笔。
                if pages.is_empty() && url == start {
                    let mut out = GrabPlan::default();
                    out.skipped.push((url, reason));
                    return out;
                }
                // 子页面拉不到不影响已拿到的清单
            }
        }
    }
    let mut out = plan(start, &pages, rules);
    for url in skipped_by_robots {
        out.skipped.push((url, "robots.txt 禁止".into()));
    }
    out
}

/// `robots.txt` 的最小判定：只看 `User-agent: *` 段落里的 `Disallow`。
///
/// 刻意只做"最小"：
/// - 不认 `Allow` 的优先级（`Allow` 优先规则各站实现不一，宁可少抓）
/// - 不认 `Crawl-delay`（我们自己的 `delay` 已经够慢）
/// - 认不出的一律当"允许"，因为抓取本来就是公开页面
pub fn robots_allows(robots: &str, path: &str) -> bool {
    let mut in_star = false;
    let mut matched_any_agent = false;
    for raw in robots.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "user-agent" => {
                in_star = value == "*";
                if in_star {
                    matched_any_agent = true;
                }
            }
            "disallow" if in_star => {
                if value.is_empty() {
                    continue; // 空 Disallow = 全允许
                }
                let prefix = value.trim_end_matches('*');
                if path.starts_with(prefix) {
                    return false;
                }
            }
            _ => {}
        }
    }
    let _ = matched_any_agent;
    true
}

/// `scheme://host` —— robots 的位置。
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, host, _) = split_url(url)?;
    Some(format!("{scheme}://{host}"))
}

/// 页面主机的取法（抓取时用来判同域）。
pub fn host_of(url: &str) -> Option<String> {
    split_url(url).map(|(_, host, _)| host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_links_resolve_against_the_base() {
        assert_eq!(
            resolve("https://a.example/dir/page.html", "x.zip").as_deref(),
            Some("https://a.example/dir/x.zip")
        );
        assert_eq!(
            resolve("https://a.example/dir/page.html", "/root.zip").as_deref(),
            Some("https://a.example/root.zip")
        );
        assert_eq!(
            resolve("https://a.example/dir/page.html", "//cdn.example/y.zip").as_deref(),
            Some("https://cdn.example/y.zip")
        );
        assert_eq!(
            resolve("https://a.example/dir/page.html", "https://b.example/z.zip").as_deref(),
            Some("https://b.example/z.zip")
        );
    }

    #[test]
    fn script_and_mail_links_are_dropped() {
        for bad in [
            "javascript:void(0)",
            "mailto:a@b.c",
            "tel:+1",
            "#top",
            "data:text/html,x",
        ] {
            assert!(resolve("https://a.example/", bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn extraction_reads_href_and_src_and_dedupes() {
        let html = r#"
            <a href="/a.zip">A</a>
            <a href='/b.zip'>B</a>
            <img src="c.png">
            <a href="javascript:void(0)">x</a>
            <a href="/a.zip#frag">dup</a>
        "#;
        let got = extract_links(html, "https://a.example/dir/");
        assert_eq!(
            got,
            vec![
                "https://a.example/a.zip".to_string(),
                "https://a.example/b.zip".to_string(),
                "https://a.example/dir/c.png".to_string(),
            ]
        );
    }

    #[test]
    fn glob_only_understands_star() {
        assert!(matches("*.zip", "https://a/x.zip"));
        assert!(!matches("*.zip", "https://a/x.rar"));
        assert!(matches("/downloads/*", "https://a/downloads/1.bin"));
        assert!(!matches("/downloads/*", "https://a/other/1.bin"));
        assert!(matches("*media*/v.mp4", "https://a/media/2024/v.mp4"));
        assert!(!matches("", "https://a/x"));
    }

    #[test]
    fn rules_filter_by_host_include_and_exclude() {
        let rules = GrabRules {
            same_host: true,
            depth: 1,
            include: vec!["*.zip".into(), "*.mp4".into()],
            exclude: vec!["*sample*".into()],
        };
        assert!(keep("https://a.example/x.zip", "a.example", &rules));
        assert!(
            !keep("https://b.example/x.zip", "a.example", &rules),
            "异域"
        );
        assert!(
            !keep("https://a.example/x.rar", "a.example", &rules),
            "类型不符"
        );
        assert!(
            !keep("https://a.example/sample.zip", "a.example", &rules),
            "排除优先"
        );
    }

    #[test]
    fn plan_lists_files_and_pages_without_downloading_anything() {
        let start = "https://a.example/dir/";
        let pages = vec![
            (
                start.to_string(),
                r#"<a href="/f/1.zip">1</a><a href="/f/2.mp4">2</a><a href="more.html">more</a>
                   <a href="https://b.example/x.zip">off</a><a href="/f/1.zip">dup</a>"#
                    .to_string(),
            ),
            (
                "https://a.example/dir/more.html".to_string(),
                r#"<a href="/f/3.rar">3</a>"#.to_string(),
            ),
        ];
        let rules = GrabRules {
            depth: 2,
            ..GrabRules::default()
        };
        let got = plan(start, &pages, &rules);

        assert_eq!(
            got.files,
            vec![
                "https://a.example/f/1.zip".to_string(),
                "https://a.example/f/2.mp4".to_string(),
                "https://a.example/f/3.rar".to_string(),
            ],
            "去重、跨层收文件"
        );
        assert_eq!(
            got.pages,
            vec!["https://a.example/dir/more.html".to_string()]
        );
        assert!(
            got.skipped
                .iter()
                .any(|(u, r)| u.contains("b.example") && r.contains("不同站点")),
            "跨站要点名原因：{:?}",
            got.skipped
        );
    }

    #[test]
    fn depth_one_never_walks_into_sub_pages() {
        let start = "https://a.example/";
        let pages = vec![(
            start.to_string(),
            r#"<a href="p.html">p</a><a href="x.zip">x</a>"#.to_string(),
        )];
        let got = plan(start, &pages, &GrabRules::default());
        assert!(got.pages.is_empty(), "depth=1 不该把子页面排进清单");
        assert_eq!(got.files, vec!["https://a.example/x.zip".to_string()]);
    }

    #[test]
    fn classify_uses_the_extension_and_defaults_to_page() {
        assert_eq!(classify("https://a/x.ZIP"), Kind::File);
        assert_eq!(classify("https://a/x.tar.gz"), Kind::File);
        assert_eq!(classify("https://a/page.html"), Kind::Page);
        assert_eq!(classify("https://a/"), Kind::Page);
    }

    /// 起一个只回一次的极简 HTTP 服务，返回 (url, JoinHandle)。
    fn serve_once(
        ctype: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (format!("http://{addr}/"), handle)
    }

    #[test]
    fn fetch_text_takes_html_and_refuses_binary() {
        let (url, h) = serve_once("text/html; charset=utf-8", "<a href=\"/a.zip\">a</a>");
        let body = fetch_text(&url, 1 << 20, std::time::Duration::from_secs(5));
        h.join().expect("server");
        let body = body.expect("html 应该拿到");
        assert!(body.contains("a.zip"));
        assert_eq!(extract_links(&body, &url), vec![format!("{}a.zip", url)]);

        let (url2, h2) = serve_once("application/zip", "PK\x03\x04");
        let got = fetch_text(&url2, 1 << 20, std::time::Duration::from_secs(5));
        h2.join().expect("server");
        assert!(got.is_err(), "二进制不该当成页面读进内存");
    }

    #[test]
    fn fetch_text_refuses_a_page_over_the_byte_cap() {
        let (url, h) = serve_once("text/html", "<html>0123456789</html>");
        let got = fetch_text(&url, 8, std::time::Duration::from_secs(5));
        h.join().expect("server");
        assert!(got.is_err(), "超过上限必须报错，不能截断后当完整页面");
    }
    /// 起一个能回多页的极简服务：`routes` 是 (路径, Content-Type, body)。
    fn serve_pages(routes: &'static [(&'static str, &'static str, &'static str)]) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for _ in 0..(routes.len() + 4) {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .to_string();
                let (ctype, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, c, b)| (*c, *b))
                    .unwrap_or(("text/html", "not found"));
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://{addr}/")
    }

    #[test]
    fn scan_walks_pages_and_collects_files() {
        let base = serve_pages(&[
            (
                "/",
                "text/html",
                r#"<a href="/p2.html">2</a><a href="/a.zip">a</a>"#,
            ),
            ("/p2.html", "text/html", r#"<a href="/b.rar">b</a>"#),
        ]);
        let limits = Limits {
            max_pages: 5,
            max_bytes: 65536,
            timeout: std::time::Duration::from_secs(5),
            delay: std::time::Duration::ZERO,
        };
        let rules = GrabRules {
            depth: 2,
            ..GrabRules::default()
        };
        let got = scan(&base, &rules, limits);
        assert!(
            got.files.iter().any(|u| u.ends_with("a.zip")),
            "{:?}",
            got.files
        );
        assert!(
            got.files.iter().any(|u| u.ends_with("b.rar")),
            "{:?}",
            got.files
        );
    }

    #[test]
    fn a_dead_start_page_is_reported_and_yields_nothing() {
        let limits = Limits {
            delay: std::time::Duration::ZERO,
            timeout: std::time::Duration::from_millis(300),
            ..Limits::default()
        };
        let got = scan("http://127.0.0.1:1/", &GrabRules::default(), limits);
        assert!(got.files.is_empty());
        assert!(!got.skipped.is_empty(), "起始页失败必须有原因");
    }
    #[test]
    fn robots_disallow_blocks_that_path_only() {
        let robots = "User-agent: *\nDisallow: /private/\nDisallow: /tmp\n\nUser-agent: Googlebot\nDisallow: /\n";
        assert!(!robots_allows(robots, "/private/x.zip"));
        assert!(!robots_allows(robots, "/tmpfile"));
        assert!(robots_allows(robots, "/public/x.zip"));
        assert!(robots_allows("", "/anything"), "没有 robots = 全允许");
        assert!(
            robots_allows("User-agent: *\nDisallow:\n", "/x"),
            "空 Disallow = 全允许"
        );
    }

    #[test]
    fn origin_is_scheme_and_host_only() {
        assert_eq!(
            origin_of("https://a.example/dir/x.html").as_deref(),
            Some("https://a.example")
        );
        assert!(origin_of("ftp://a/x").is_none());
    }
    #[test]
    fn host_of_rejects_non_http() {
        assert_eq!(host_of("https://a.example/x").as_deref(), Some("a.example"));
        assert!(host_of("file:///x").is_none());
    }
}
