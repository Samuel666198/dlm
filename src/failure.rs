//! 失败的专业说法：**错误码 + 一句说明 +（可选）处理建议**。
//!
//! ## 为什么单开一层
//!
//! 2026-11-28 用户提了两条，指向同一件事：
//!
//! - 「日志的『dlm 跑不起来』太口语化」
//! - 「不是有错误码➕解释的吗，用这个专业的报错方式」
//!
//! 于是把"怎么描述一次失败"收成一处：
//!
//! ```text
//! 错误 [TOOL_MISSING]：未找到 yt-dlp。
//! 处理：安装后确保它在 PATH，或设置 DLM_YTDLP 指向可执行文件。
//! ```
//!
//! 三条硬要求：
//!
//! 1. **码与 `--json` 同源**（都用 [`crate::json_out::Code`]）——
//!    界面上一句说法、机器读一句说法，但码必须是同一个，否则排查时对不上。
//! 2. 码**只增不改**（`Code` 的既有约定）。
//! 3. 说明写"发生了什么"，建议写"怎么办"；不写口语，也不写猜测的原因。

use std::fmt;

use crate::json_out::Code;

/// 一次失败的完整描述。
#[derive(Debug, Clone)]
pub struct Failure {
    pub code: Code,
    /// 一句话：发生了什么。**句末带句号**，与"处理"那行区分开。
    pub message: String,
    /// 怎么办。`None` 表示没什么可建议的（比如上游给的原文已经够清楚）。
    pub hint: Option<String>,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Failure {
    /// 只写说明那一句 —— `anyhow` 的 `{:#}` 会把整条链串起来，
    /// 这里不该再重复码（码由 [`format_human`] 负责出现在人看的位置）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// 从 `anyhow::Error` 里把 [`Failure`] 找出来（穿过 `.context()` 包过的层）。
pub fn find(err: &anyhow::Error) -> Option<&Failure> {
    err.downcast_ref::<Failure>()
        // `anyhow` 的 downcast 只看最外层；`.context()` 之后要把链走一遍。
        .or_else(|| err.chain().find_map(|e| e.downcast_ref::<Failure>()))
}

/// 给人看的两行（或三行）。**没有 [`Failure`] 时退回 `INTERNAL`**，绝不空着。
pub fn format_human(err: &anyhow::Error) -> String {
    match find(err) {
        Some(f) => match &f.hint {
            Some(hint) => format!("错误 [{}]：{}\n处理：{hint}", f.code.as_str(), f.message),
            None => format!("错误 [{}]：{}", f.code.as_str(), f.message),
        },
        None => format!("错误 [{}]：{err:#}", Code::Internal.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 码、说明、建议三段都要出现在人看的输出里 —— 少一段就等于让用户自己猜。
    #[test]
    fn a_human_failure_carries_the_code_the_reason_and_the_next_step() {
        let err: anyhow::Error = Failure::new(Code::ToolMissing, "未找到 yt-dlp。")
            .with_hint("安装 yt-dlp 后确保它在 PATH，或设置 DLM_YTDLP。")
            .into();
        let text = format_human(&err);
        assert!(text.contains("[TOOL_MISSING]"), "{text}");
        assert!(text.contains("未找到 yt-dlp。"), "{text}");
        assert!(text.contains("处理："), "{text}");
        assert!(text.contains("DLM_YTDLP"), "{text}");
        // 不许出现口语化的开头
        assert!(!text.contains("跑不起来"), "{text}");
    }

    /// 没有建议时就不该凭空造一行「处理：」。
    #[test]
    fn no_hint_means_no_hint_line() {
        let err: anyhow::Error = Failure::new(Code::NotAFile, "该地址返回的是网页。").into();
        let text = format_human(&err);
        assert!(text.contains("[NOT_A_FILE]"), "{text}");
        assert!(!text.contains("处理："), "没有建议时不该出现这一行：{text}");
    }

    /// 普通 `anyhow` 错误也要有码 —— 否则同一处输出会出现"带码"和"不带码"两种形态。
    #[test]
    fn a_plain_error_falls_back_to_internal_and_still_has_a_code() {
        let err = anyhow::anyhow!("磁盘写入失败");
        let text = format_human(&err);
        assert!(text.contains("[INTERNAL]"), "{text}");
        assert!(text.contains("磁盘写入失败"), "{text}");
    }

    /// 被 `.context()` 包过一层也要认出来（否则加了上下文码就丢了）。
    #[test]
    fn context_does_not_hide_the_code() {
        let err = anyhow::Error::new(Failure::new(Code::DiskFull, "磁盘空间不足。"))
            .context("写断点文件失败");
        let text = format_human(&err);
        assert!(text.contains("[DISK_FULL]"), "{text}");
    }
}
