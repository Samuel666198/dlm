//! 诊断包：把"出了问题要问什么"一次性收进一个文本文件。
//!
//! ## 为什么是文本，不是 zip
//!
//! 用户要的是「快速导出错误日志，方便排查、反馈」。
//! 我们没有任何压缩库依赖，为了一个导出功能引入一个 zip crate 不划算；
//! 文本文件双击就能看，也能直接贴进聊天窗口 —— 反而更适合"反馈"这个用途。
//!
//! ## 收什么（也只收这些）
//!
//! | 段 | 为什么需要 |
//! |---|---|
//! | 环境 | 版本 / 系统 / 语言 / 是否深色，复现的第一步 |
//! | 设置 | 用户改过什么。**代理密码要脱敏**（只留 host:port） |
//! | 队列摘要 | 哪条任务、什么状态、有没有 video 标记（很多"下错"是旧任务没有标记） |
//! | 任务日志尾部 | 真正的报错原文 |
//! | 外部依赖 | yt-dlp / ffmpeg 找没找到、在哪 —— 视频问题的第一嫌疑 |
//! | 更新日志 | "设置被重置 / 更新失败"这类问题的现场 |
//!
//! ## 不做什么
//!
//! - **不上传**。文件只写到本机目录，路径会回给调用方，由界面显示。
//! - **不脱敏别的**：URL、落点、任务名都是诊断必需的，原样保留。
//!   唯一会主动去掉的是代理里的口令。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 一次导出最多带几个任务的日志。
///
/// 理由：要能看清，不是要全量。最近 6 条覆盖"刚出的问题"，
/// 再多就把文件撑成一屏看不完的东西，反而没人读。
pub const MAX_TASKS: usize = 6;

/// 每个任务日志取最后多少行。
pub const LOG_TAIL_LINES: usize = 60;

/// 代理串脱敏：`http://user:pass@host:port` → `http://***@host:port`。
///
/// 与 `proxy::Redacted` 同一个目的（口令不进日志/错误串），
/// 但那边是给运行期用的，这里是给"导出文件"用的，所以单独一份纯函数，
/// 便于用例直接喂字符串。
pub fn redact_proxy(text: &str) -> String {
    let Some(scheme_end) = text.find("://") else {
        return text.to_string();
    };
    let rest = &text[scheme_end + 3..];
    let Some(at) = rest.find('@') else {
        return text.to_string();
    };
    // `///` 之后、`@` 之前就是 userinfo；只留一个 *** 占位。
    format!("{}://***@{}", &text[..scheme_end], &rest[at + 1..])
}

/// 导出目录：与其它状态文件同处 `%LOCALAPPDATA%\dlm` 下。
pub fn out_dir(base: &Path) -> PathBuf {
    base.join("diagnostics")
}

/// 文件名：`diag-YYYYMMDD-HHMMSS.txt`（本地时间省略到秒就够排序）。
pub fn file_name(now_secs: u64) -> String {
    // 不引入时间库：只要一个能排序、能认出来的名字。
    // 用 `now_secs` 直接当后缀也够，但人对日期更敏感，所以按 UTC 折算成年月日时分秒。
    let secs = now_secs;
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("diag-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.txt")
}

/// Unix 秒 → (年, 月, 日, 时, 分, 秒)，**UTC**。
fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    // 1970-01-01 起算的"民用日"换算（Howard Hinnant 的算法，不需要 chrono）。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d, h, mi, s)
}

/// 读文件尾部若干行；读不到就给一句原因（**不能静默**：空着会让人以为没问题）。
pub fn tail(path: &Path, lines: usize) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let all: Vec<&str> = text.lines().collect();
            let start = all.len().saturating_sub(lines);
            all[start..].join("\n")
        }
        Err(err) => format!("（读不到 {}：{err}）", path.display()),
    }
}

/// 拼一份诊断文本。`sections` 由调用方提供，本函数只负责排版 ——
/// 这样"收什么"留在 GUI 那边（它才知道 queue/log_dir 在哪），
/// 而"怎么排"在这里可被用例单独验。
pub fn render(sections: &[(String, String)]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "dlm 诊断包");
    let _ = writeln!(
        out,
        "由 dlm 自动生成。请把整个文件发给维护者，便于定位问题。"
    );
    let _ = writeln!(out, "（不含上传：文件只留在本机）");
    for (title, body) in sections {
        let _ = writeln!(out);
        let _ = writeln!(out, "===== {title} =====");
        let body = body.trim_end();
        if body.is_empty() {
            let _ = writeln!(out, "（空）");
        } else {
            let _ = writeln!(out, "{body}");
        }
    }
    out
}

/// 把文本写进 `dir`，返回完整路径。
pub fn write(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 代理口令不许出现在导出文件里 —— 这是整个导出功能唯一需要脱敏的地方。
    #[test]
    fn the_proxy_password_never_reaches_the_export() {
        assert_eq!(
            redact_proxy("http://user:s3cret@127.0.0.1:7890"),
            "http://***@127.0.0.1:7890"
        );
        assert_eq!(
            redact_proxy("socks5://alice:pw@host:1080"),
            "socks5://***@host:1080"
        );
        // 没有口令的写法原样保留（改了反而看不出用的是哪个代理）
        assert_eq!(
            redact_proxy("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(redact_proxy(""), "");
        assert_eq!(redact_proxy("不是代理"), "不是代理");
    }

    /// 文件名要能排序，也要认得出来。
    #[test]
    fn the_file_name_is_sortable_and_recognisable() {
        // 用 epoch 0 断言，不依赖我手算的常数：1970-01-01T00:00:00Z
        assert_eq!(file_name(0), "diag-19700101-000000.txt");
        assert_eq!(file_name(86_400), "diag-19700102-000000.txt");
        assert_eq!(file_name(3661), "diag-19700101-010101.txt");
        // 早/晚的排序与字符串序一致
        assert!(file_name(0) < file_name(86_400));
    }

    /// 读不到文件时要**说出原因**，不能静默留空。
    #[test]
    fn a_missing_log_says_why_instead_of_staying_blank() {
        let missing = Path::new("no-such-file-hopefully.log");
        let text = tail(missing, 10);
        assert!(text.contains("读不到"), "{text}");
    }

    /// 排版：标题都在、空内容要显式写「（空）」，不能让人以为是漏了一段。
    #[test]
    fn every_section_is_labelled_and_empty_ones_are_explicit() {
        let text = render(&[
            ("环境".to_string(), "release 0.7.7".to_string()),
            ("任务日志".to_string(), "   ".to_string()),
        ]);
        assert!(text.contains("===== 环境 ====="));
        assert!(text.contains("release 0.7.7"));
        assert!(text.contains("===== 任务日志 ====="));
        assert!(text.contains("（空）"), "空段必须显式标出来：{text}");
    }
}
