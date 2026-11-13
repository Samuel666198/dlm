//! `--json` 输出（0.6.0 · 阶段 0）：给 Agent 用的机器可读形状。
//!
//! ## 为什么单开一个模块
//!
//! 给人看的文本可以随时改措辞，给机器看的形状不行 —— 一改就断下游。
//! 所以这一层只做两件事：把结果压成**稳定的字段名**，把失败压成**稳定的错误码**。
//! 任何界面上的文案都不许漏进来。
//!
//! ## 形状
//!
//! 成功：`{"ok":true,"task":{...}}`
//! 失败：`{"ok":false,"error":{"code":"DISK_FULL","message":"…","detail":{…}}}`

use serde::Serialize;
use serde_json::Value;

/// 错误码。**只增不改**：下游靠它分支，改名等于破坏兼容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    BadUrl,
    NoSuchDir,
    DiskFull,
    Duplicate,
    ApprovalExpired,
    ApprovalMismatch,
    NotFound,
    Busy,
    Internal,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::BadUrl => "BAD_URL",
            Code::NoSuchDir => "NO_SUCH_DIR",
            Code::DiskFull => "DISK_FULL",
            Code::Duplicate => "DUPLICATE",
            Code::ApprovalExpired => "APPROVAL_EXPIRED",
            Code::ApprovalMismatch => "APPROVAL_MISMATCH",
            Code::NotFound => "NOT_FOUND",
            Code::Busy => "BUSY",
            Code::Internal => "INTERNAL",
        }
    }

    /// 当前全部错误码 —— 测试拿它防漏写。
    pub const ALL: [Code; 9] = [
        Code::BadUrl,
        Code::NoSuchDir,
        Code::DiskFull,
        Code::Duplicate,
        Code::ApprovalExpired,
        Code::ApprovalMismatch,
        Code::NotFound,
        Code::Busy,
        Code::Internal,
    ];
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

/// 成功信封。
pub fn ok<T: Serialize>(task: T) -> Envelope<T> {
    Envelope {
        ok: true,
        task: Some(task),
        error: None,
    }
}

/// 失败信封。`message` 是给人看的一句话，`detail` 给 Agent 补充判据。
pub fn err(code: Code, message: impl Into<String>, detail: Option<Value>) -> Envelope<()> {
    Envelope {
        ok: false,
        task: None,
        error: Some(ErrorBody {
            code: code.as_str(),
            message: message.into(),
            detail,
        }),
    }
}

/// 压成一行（`--json` 的约定：一个任务一行）。
pub fn to_line<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| {
        format!(r#"{{"ok":false,"error":{{"code":"INTERNAL","message":"序列化失败：{e}"}}}}"#)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct TaskLine {
        id: &'static str,
        status: &'static str,
        bytes_done: u64,
    }

    #[test]
    fn success_line_has_ok_and_task_and_no_error_field() {
        let line = to_line(&ok(TaskLine {
            id: "t_1",
            status: "running",
            bytes_done: 1024,
        }));
        assert!(line.contains(r#""ok":true"#), "{line}");
        assert!(line.contains(r#""id":"t_1""#));
        assert!(!line.contains(r#""error""#), "成功时不该出现 error 字段");
        assert!(!line.contains('\n'), "一行一个任务");
    }

    #[test]
    fn failure_line_carries_a_code_and_no_task() {
        let line = to_line(&err(
            Code::DiskFull,
            "目标磁盘剩余 1.2 GB，任务需要 6.1 GB",
            Some(serde_json::json!({"free": 1_288_490_188u64, "need": 6_550_000_000u64})),
        ));
        assert!(line.contains(r#""ok":false"#));
        assert!(line.contains(r#""code":"DISK_FULL""#));
        assert!(line.contains(r#""free":1288490188"#));
        assert!(!line.contains(r#""task""#), "失败时不该出现 task 字段");
    }

    #[test]
    fn every_code_is_unique_screaming_snake_and_complete() {
        let mut seen = std::collections::BTreeSet::new();
        for code in Code::ALL {
            let s = code.as_str();
            assert!(seen.insert(s), "错误码重复：{s}");
            assert_eq!(s, s.to_ascii_uppercase(), "必须是全大写：{s}");
            assert!(s.chars().all(|c| c.is_ascii_uppercase() || c == '_'), "{s}");
        }
        assert_eq!(seen.len(), Code::ALL.len());
    }
}
