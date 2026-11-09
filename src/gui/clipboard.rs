//! 剪贴板监视：发现下载链接就提示（**默认关**，设置里可开）。
//!
//! ## 为什么默认关
//!
//! 剪贴板里有密码、验证码、私钥、聊天记录。一个默认开着的程序持续读剪贴板，
//! 用户通常不知道。IDM 默认开，我们在这点上不跟它。
//!
//! ## 怎么读
//!
//! 用 `GetClipboardSequenceNumber()` 判变化：只有序号变了才 `OpenClipboard`，
//! 否则每 300ms 打开一次剪贴板会跟别的程序抢锁。
//!
//! 被占用（`OpenClipboard` 失败）按"没变化"处理，下一轮再试 —— 不重试、
//! 不阻塞、不报错。
//!
//! ## 匹配规则
//!
//! 不用正则（`regex` 不在依赖树里，且 JS 那份 `FILE_LINK_RE` 是对 `pathname`
//! 锚定的，套到整条 URL 上语义会变）。这里用固定扩展名集合手写：
//! 取 URL 的 path 部分、去掉 query，看扩展名是否在白名单里。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 认得的下载扩展名。与 `queue::category_for` 的口径保持一致，另外加上常见分卷。
const EXTENSIONS: &[&str] = &[
    // 压缩
    "zip",
    "rar",
    "7z",
    "tar",
    "gz",
    "bz2",
    "xz",
    "iso",
    "cab",
    "zst",
    "tgz",
    // 文档
    "pdf",
    "doc",
    "docx",
    "xls",
    "xlsx",
    "ppt",
    "pptx",
    "txt",
    "md",
    "epub",
    "mobi",
    "csv",
    // 程序
    "exe",
    "msi",
    "apk",
    "dmg",
    "pkg",
    "deb",
    "rpm",
    "appimage",
    // 媒体
    "mp4",
    "mkv",
    "avi",
    "mov",
    "webm",
    "m4v",
    "mp3",
    "flac",
    "wav",
    "aac",
    "ogg",
    "opus",
    // 镜像与模型
    "img",
    "bin",
    "safetensors",
    "gguf",
    "pt",
    "onnx",
    // 图片
    "jpg",
    "jpeg",
    "png",
    "gif",
    "bmp",
    "webp",
    "tif",
    "tiff",
    "heic",
];

/// 命中的一条链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub url: String,
}

/// 监视线程的句柄。`Drop` 时置停止标志，线程下一轮自己退。
pub struct Watcher {
    stop: Arc<AtomicBool>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// 起一个监视线程。返回 `None` = 这个平台没有实现。
pub fn spawn() -> Option<Watcher> {
    #[cfg(windows)]
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("dlm-clipboard".into())
            .spawn(move || watch_loop(stop_thread))
            .ok()?;
        Some(Watcher { stop })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn watch_loop(stop: Arc<AtomicBool>) {
    use std::ffi::c_void;

    // SAFETY: 只调 user32 的剪贴板 API，句柄都由系统给出并在本函数内释放。
    unsafe {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetClipboardSequenceNumber() -> u32;
            fn OpenClipboard(hwnd: *mut c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn GetClipboardData(format: u32) -> *mut c_void;
            fn IsClipboardFormatAvailable(format: u32) -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalLock(mem: *mut c_void) -> *mut c_void;
            fn GlobalUnlock(mem: *mut c_void) -> i32;
        }

        const CF_UNICODETEXT: u32 = 13;

        let mut last_seq = GetClipboardSequenceNumber();
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(300));

            let seq = GetClipboardSequenceNumber();
            if seq == last_seq {
                continue;
            }
            last_seq = seq;

            // 被别的程序占着就跳过，下一轮再说。
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                continue;
            }
            let text = if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
                let handle = GetClipboardData(CF_UNICODETEXT);
                if handle.is_null() {
                    None
                } else {
                    let ptr = GlobalLock(handle) as *const u16;
                    if ptr.is_null() {
                        None
                    } else {
                        let mut len = 0usize;
                        while *ptr.add(len) != 0 && len < 8192 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(ptr, len);
                        let s = String::from_utf16_lossy(slice);
                        GlobalUnlock(handle);
                        Some(s)
                    }
                }
            } else {
                None
            };
            CloseClipboard();

            if let Some(text) = text
                && let Some(hit) = detect(&text)
            {
                // 目前只把命中记到 stderr；UI 侧的提示条在下一步接。
                eprintln!("dlm-gui: 剪贴板发现下载链接 {}", hit.url);
            }
        }
    }
}

/// 从一段文本里找出第一条像下载链接的东西。
///
/// 认 http/https，且 path 的扩展名在白名单里。整段文本前后可能有空白或说明文字。
pub fn detect(text: &str) -> Option<Hit> {
    let trimmed = text.trim();
    // 整段就是一个 URL 的情况最常见。
    if let Some(hit) = detect_one(trimmed) {
        return Some(hit);
    }
    // 否则在文本里找 http(s):// 的起点，从那里截到下一个分隔符。
    //
    // 不用 `split_whitespace`：中文标点后面常直接贴 URL（"下载地址：https://…"），
    // 那样切不开，整段 token 会因为前缀不是 http 而被拒。
    let mut search = trimmed;
    while let Some(idx) = search.find("http") {
        let tail = &search[idx..];
        let lower = tail.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            let end = tail
                .find(|c: char| is_url_terminator(c))
                .unwrap_or(tail.len());
            if let Some(hit) = detect_one(&tail[..end]) {
                return Some(hit);
            }
        }
        search = &search[idx + 4..];
    }
    None
}

/// URL 到此为止：空白、引号、尖括号，以及全角标点。
///
/// 全角那组是实测出来的：中文文本里 `：`、`，`、`。` 后面紧跟 URL 很常见，
/// 而它们不是 ASCII 空白，`split_whitespace` 不认。
fn is_url_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '<' | '>' | '：' | '，' | '。' | '、' | '）' | '】' | '「' | '」'
        )
}

fn detect_one(raw: &str) -> Option<Hit> {
    let s = raw.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
    let lower = s.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    // 去掉 query 与 fragment 再看扩展名 —— 与 content.js 对 pathname 的判断等价。
    let without_query = s.split(['?', '#']).next().unwrap_or(s);
    let ext = without_query.rsplit('.').next()?.to_ascii_lowercase();
    if ext.is_empty() || ext.contains('/') {
        return None;
    }
    if EXTENSIONS.contains(&ext.as_str()) {
        Some(Hit { url: s.to_string() })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_file_url_is_a_hit() {
        let hit = detect("https://example.com/a/b/file.zip").unwrap();
        assert_eq!(hit.url, "https://example.com/a/b/file.zip");
    }

    /// query 里带 ".zip" 不该命中 —— 扩展名只看 path。
    /// 这正是 JS 那份正则套到整条 URL 上会出错的地方。
    #[test]
    fn an_extension_in_the_query_is_not_a_hit() {
        assert!(detect("https://example.com/page?name=a.zip").is_none());
    }

    #[test]
    fn a_query_after_a_real_extension_still_hits() {
        let hit = detect("https://example.com/file.zip?token=abc").unwrap();
        assert!(hit.url.starts_with("https://example.com/file.zip"));
    }

    #[test]
    fn surrounding_text_and_quotes_are_tolerated() {
        assert!(detect("下载地址：https://x.test/a.7z 提取码 1234").is_some());
        assert!(detect("\"https://x.test/a.iso\"").is_some());
        assert!(detect("<https://x.test/a.pdf>").is_some());
    }

    #[test]
    fn non_download_links_are_ignored() {
        assert!(detect("https://example.com/index.html").is_none());
        assert!(detect("https://example.com/").is_none());
        assert!(detect("just some text").is_none());
        assert!(
            detect("ftp://example.com/a.zip").is_none(),
            "只认 http/https"
        );
        assert!(detect("").is_none());
    }

    #[test]
    fn model_and_image_weights_are_recognised() {
        // 用户实际会复制的类型。
        assert!(detect("https://cdn.test/model.safetensors").is_some());
        assert!(detect("https://cdn.test/model.gguf").is_some());
        assert!(detect("https://cdn.test/pic.jpg").is_some());
    }

    #[test]
    fn case_insensitive_extension() {
        assert!(detect("https://x.test/A.ZIP").is_some());
        assert!(detect("https://x.test/a.Mp4").is_some());
    }
}
