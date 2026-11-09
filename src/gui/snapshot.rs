//! 视图层：把队列里的 `Task` 投影成 UI/HTTP 能直接吃的形状。
//!
//! 这一层**只读**：不碰队列、不起进程、不发网络。`snapshot_of` 是纯函数，
//! 所以原生 UI 与 HTTP 可以各读一份，互不阻塞（见设计稿第四节）。

use std::path::Path;

use serde::Serialize;

use crate::queue::{self, QueueStore, Task, TaskStatus};

#[derive(Debug, Clone, Serialize)]
pub struct UpdateView {
    pub enabled: bool,
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub checked_at: Option<u64>,
    pub error: Option<String>,
}

impl UpdateView {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: None,
            available: false,
            url: None,
            notes: None,
            checked_at: None,
            error: None,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct QuotaView {
    pub enabled: bool,
    pub limit: u64,
    pub window_secs: u64,
    pub window_started_at: u64,
    pub used: u64,
    pub resets_at: u64,
}

/// B10 端点文件的形状。`pub` 是为了让集成测试能直接读回自己那份、逐字段对账 ——
/// 它是 `dlm-gui` 与浏览器宿主之间的**外部契约**，不是内部细节。

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: String,
    pub url: String,
    pub name: String,
    pub referer: Option<String>,
    pub status: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub log: String,
    pub progress: f64,
    pub total_len: Option<u64>,
    pub delivered: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub start_at: Option<u64>,
    pub hold_until: Option<u64>,
    pub on_complete: Option<String>,
    /// P1：已经自动重试过的轮数；UI 用它显示“重试 1/2”。
    pub attempts: u32,
    pub category: &'static str,
}

impl TaskView {
    pub(crate) fn from_task(task: &Task, out_dir: &Path) -> Self {
        let (mut total, _) = queue::progress(out_dir, &task.name);
        let delivered = queue::delivered(out_dir, &task.name);
        if total.is_none() && task.status == TaskStatus::Done {
            total = delivered;
        }
        let progress = match task.status {
            TaskStatus::Done => 100.0,
            _ => match (total, delivered) {
                (Some(total), Some(delivered)) if total > 0 => {
                    (delivered as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
                }
                _ => 0.0,
            },
        };
        Self {
            id: task.id.clone(),
            url: task.url.clone(),
            name: task.name.clone(),
            referer: task.referer.clone(),
            status: task.status.as_str().to_string(),
            pid: task.pid,
            exit_code: task.exit_code,
            log: task.log.to_string_lossy().to_string(),
            progress,
            total_len: total,
            delivered,
            created_at: task.created_at,
            updated_at: task.updated_at,
            start_at: task.start_at,
            hold_until: task.hold_until,
            on_complete: task.on_complete.clone(),
            attempts: task.attempts,
            category: queue::category_for(&task.name),
        }
    }
}

pub fn quota_view(store: &QueueStore, now: u64) -> Option<QuotaView> {
    store.quota().map(|q| QuotaView {
        enabled: true,
        limit: q.limit,
        window_secs: q.window_secs.max(1),
        window_started_at: q.window_started_at,
        used: q.used,
        resets_at: if q.window_started_at == 0 {
            now.saturating_add(q.window_secs.max(1))
        } else {
            q.resets_at()
        },
    })
}

pub fn snapshot_of(store: &QueueStore, out_dir: &Path) -> Vec<TaskView> {
    store
        .tasks()
        .iter()
        .map(|t| TaskView::from_task(t, out_dir))
        .collect()
}
