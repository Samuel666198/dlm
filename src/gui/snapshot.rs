//! 视图层：把队列里的 `Task` 投影成 UI/HTTP 能直接吃的形状。
//!
//! 这一层**只读**：不碰队列、不起进程、不发网络。`snapshot_of` 是纯函数，
//! 所以原生 UI 与 HTTP 可以各读一份，互不阻塞（见设计稿第四节）。

use std::path::Path;

use serde::Serialize;

use crate::queue::{self, QueueStore, Task, TaskStatus};

/// 更新状态。
///
/// ## 三个版本号（见 `docs/evidence/2026-11-12-更新功能裁定.md` 第三节）
///
/// | 字段 | 含义 |
/// |---|---|
/// | `current` / `latest` | **发行版** `release`：用户感知的「软件版本」，更新与否只看它 |
/// | `core` / `core_current` | 桌面端三个 exe 的版本，与 `Cargo.toml` 一致 |
/// | `extension` | 扩展版本，与 `manifest.json` 一致 |
///
/// `available` 只由 `release` 决定。`core`/`extension` 是给人看的：
/// 告诉用户「这次更新会不会连带换扩展」。
///
/// ## 清单缺 sha256 ⇒ `available` 但不可安装
///
/// 装进安装目录的东西必须校验摘要，没有摘要的包不许装。但「有新版本」和
/// 「能自动装这个版本」是两件事，不能混成一个布尔：
///
/// | 字段 | 含义 |
/// |---|---|
/// | `available` | 清单里确实有更新的发行版（事实） |
/// | `installable` | 这个包能自动装（有摘要、满足 `min_core`） |
/// | `blocked` | 不可安装的原因，给人看 |
///
/// 混在一起的话，「清单坏了」会显示成「没有更新」——用户永远等不到提示。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateView {
    pub enabled: bool,
    /// 本机发行版版本（`gui::RELEASE`）。
    pub current: String,
    /// 清单里的发行版版本。
    pub latest: Option<String>,
    pub available: bool,
    /// 能不能自动装：有摘要、满足 `min_core`。
    pub installable: bool,
    /// 不可安装的原因（`available` 为真而 `installable` 为假时才有）。
    pub blocked: Option<String>,
    /// 本机桌面端版本（`Cargo.toml`）。
    pub core_current: String,
    /// 清单里的桌面端版本。
    pub core: Option<String>,
    /// 清单里的扩展版本。
    pub extension: Option<String>,
    /// **磁盘上**那份扩展的版本（`%LOCALAPPDATA%\dlm\extension\manifest.json`）。
    ///
    /// updater 把新扩展写到这里；它与 `extension_running` 不相等，就说明
    /// 文件已经就位但浏览器还在跑旧的 —— 那正是「请重载」要说的那句话。
    pub extension_disk: Option<String>,
    /// 浏览器里**正在跑**的扩展版本：扩展连上宿主时自己报的（E16 之后的双向 ping）。
    ///
    /// 与 `extension`（清单里的目标版本）成对出现才有意义：
    /// `extension_running != extension` ⇒ 用户还没重载扩展。
    pub extension_running: Option<String>,
    pub url: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub notes: Option<String>,
    pub min_core: Option<String>,
    pub published_at: Option<String>,
    pub checked_at: Option<u64>,
    pub error: Option<String>,
}

impl UpdateView {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            current: crate::gui::RELEASE.to_string(),
            latest: None,
            available: false,
            installable: false,
            blocked: None,
            core_current: env!("CARGO_PKG_VERSION").to_string(),
            core: None,
            extension: None,
            extension_disk: None,
            extension_running: None,
            url: None,
            sha256: None,
            size: None,
            notes: None,
            min_core: None,
            published_at: None,
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
    /// 落点的绝对路径。
    ///
    /// 详情抽屉要显示它、也要拿它去「打开文件 / 打开文件夹」。**由队列自己算**
    /// （`queue::target_path`），不接受客户端交来的路径 —— 否则就成了"打开任意文件"接口。
    pub path: String,
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
            path: queue::target_path(out_dir, &task.name)
                .to_string_lossy()
                .to_string(),
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
