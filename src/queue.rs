//! C1 产品层队列（本地 Web UI 的后端）：任务存储、状态、进度、原子写。
//!
//! 队列**不存 Cookie**；认证下载继续走 B2 的一任务一进程入口。这里只接 public 直链。

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::actions::CompletionAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub url: String,
    pub name: String,
    pub referer: Option<String>,
    pub status: TaskStatus,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub log: PathBuf,
    pub created_at: u64,
    pub updated_at: u64,
    /// Unix 秒；`Some` 且未到点 ⇒ 不启动。A3b 的计划任务落在这里。
    pub start_at: Option<u64>,
    /// 配额窗口等待：`Some` 且未到点 ⇒ 不启动；与用户自己的 `start_at` 分开。
    #[serde(default)]
    pub hold_until: Option<u64>,
    /// 完成后动作原始串（`open` / `run:<exe>`）；队列拒绝 `shutdown`。
    #[serde(default)]
    pub on_complete: Option<String>,
    /// 已经自动重试过几轮（P1）。`--max-task-retries` 拿它比上限；用户手动“继续”不重置它，
    /// 只有重新入队（新任务）才从 0 起算 —— 否则手动点一次就能无限绕过上限。
    #[serde(default)]
    pub attempts: u32,
    pub total_len: Option<u64>,
    pub delivered: Option<u64>,
}

/// 自动重试的退避秒数（P1）。`attempts` 是**已经重试过的轮数**（0 = 还没重试过）。
///
/// 三档之后不再自动重试：一轮 30 秒、二轮 120 秒，够覆盖"对端一时抽风"那一类；
/// 再往上多半是永久错误（404/403/域名没了），继续自动重试只是白烧流量与日志。
/// 返回 `None` 时调用方应当判定为真正的 `Failed`，把决定权交回用户。
pub fn retry_backoff(attempts: u32) -> Option<u64> {
    match attempts {
        0 => Some(30),
        1 => Some(120),
        _ => None,
    }
}

/// 队列级全局固定窗口配额（A16）。`window_started_at == 0` = 尚未起算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaState {
    pub limit: u64,
    pub window_secs: u64,
    pub window_started_at: u64,
    pub used: u64,
}

impl QuotaState {
    pub fn resets_at(&self) -> u64 {
        self.window_started_at
            .saturating_add(self.window_secs.max(1))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QueueFile {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    tasks: Vec<Task>,
    #[serde(default)]
    quota: Option<QuotaState>,
}

#[derive(Debug)]
pub struct QueueStore {
    path: PathBuf,
    next_id: u64,
    tasks: Vec<Task>,
    quota: Option<QuotaState>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl QueueStore {
    pub fn load(path: PathBuf) -> Result<Self> {
        let file = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<QueueFile>(&text)
                .with_context(|| format!("解析队列文件失败：{}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => QueueFile::default(),
            Err(err) => {
                return Err(err).with_context(|| format!("读队列文件失败：{}", path.display()));
            }
        };
        Ok(Self {
            path,
            next_id: file.next_id.max(1),
            tasks: file.tasks,
            quota: file.quota,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn find(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    pub fn quota(&self) -> Option<QuotaState> {
        self.quota
    }

    /// 设/改额度。`limit == 0` 关闭并清掉所有等待；改额度或窗口会把本窗账归零、下次派发起算。
    pub fn set_quota(&mut self, limit: u64, window_secs: u64) -> bool {
        if limit == 0 {
            let had_holds = self.tasks.iter().any(|t| t.hold_until.is_some());
            let changed = self.quota.is_some() || had_holds;
            self.quota = None;
            for task in &mut self.tasks {
                task.hold_until = None;
            }
            return changed;
        }
        let window_secs = window_secs.max(1);
        let changed = self
            .quota
            .is_none_or(|q| q.limit != limit || q.window_secs != window_secs);
        if !changed {
            return false;
        }
        self.quota = Some(QuotaState {
            limit,
            window_secs,
            window_started_at: 0,
            used: 0,
        });
        for task in &mut self.tasks {
            task.hold_until = None;
        }
        true
    }

    /// 推进固定窗口：跨过整窗就清零并释放到点的等待。没有起算（`window_started_at == 0`）时不动。
    pub fn quota_tick(&mut self, now: u64) -> bool {
        let Some(mut quota) = self.quota else {
            return false;
        };
        if quota.window_started_at == 0 {
            return false;
        }
        let window = quota.window_secs.max(1);
        let end = quota.window_started_at.saturating_add(window);
        if now < end {
            return false;
        }
        let elapsed = now - quota.window_started_at;
        let windows = (elapsed / window).max(1);
        quota.window_started_at = quota
            .window_started_at
            .saturating_add(windows.saturating_mul(window));
        quota.used = 0;
        self.quota = Some(quota);
        for task in &mut self.tasks {
            if task.hold_until.is_some_and(|until| until <= now) {
                task.hold_until = None;
            }
        }
        true
    }

    pub fn quota_charge(&mut self, bytes: u64) -> bool {
        let Some(quota) = self.quota.as_mut() else {
            return false;
        };
        if bytes == 0 {
            return false;
        }
        quota.used = quota.used.saturating_add(bytes);
        true
    }

    pub fn quota_hold(&mut self, id: &str, until: u64) -> bool {
        let Some(task) = self.find_mut(id) else {
            return false;
        };
        if task.hold_until == Some(until) {
            return false;
        }
        task.hold_until = Some(until);
        true
    }

    /// 下一次整窗重置的时刻；还没起算 ⇒ `None`。
    pub fn quota_next_reset(&self) -> Option<u64> {
        self.quota
            .filter(|q| q.window_started_at > 0)
            .map(|q| q.resets_at())
    }

    /// 一个任务的覆盖量：断点可读就用段表；成功收尾会删断点，退回读目标文件长度。
    /// `TaskView` 与配额采样共用这一处，避免两套口径。
    pub fn delivered(&self, out_dir: &Path, name: &str) -> Option<u64> {
        delivered(out_dir, name)
    }

    /// 第一次派发带配额的任务时起算本窗（没有任务时窗口不流逝）。
    pub fn quota_start(&mut self, now: u64) -> bool {
        let Some(quota) = self.quota.as_mut() else {
            return false;
        };
        if quota.window_started_at != 0 {
            return false;
        }
        quota.window_started_at = now;
        true
    }

    /// 重启恢复：上次进程已不在，`running` 一律当 `paused`（dlm 断点还在，可恢复）。
    pub fn recover_running(&mut self) -> bool {
        let now = now_secs();
        let mut changed = false;
        for task in &mut self.tasks {
            if task.status == TaskStatus::Running {
                task.status = TaskStatus::Paused;
                task.pid = None;
                task.updated_at = now;
                changed = true;
            }
        }
        changed
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("建队列目录失败：{}", parent.display()))?;
        }
        let file = QueueFile {
            next_id: self.next_id,
            tasks: self.tasks.clone(),
            quota: self.quota,
        };
        let text = serde_json::to_string_pretty(&file).context("序列化队列失败")?;
        let tmp = tmp_path(&self.path);
        fs::write(&tmp, text).with_context(|| format!("写队列临时文件失败：{}", tmp.display()))?;
        // `rename` 在本机（Windows）会替换已存在的目标；先删目标会把"原子替换"拆成
        // "删掉旧文件 + 再改名"两步，崩在中间就丢队列。
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("提交队列文件失败：{}", self.path.display()))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        url: &str,
        name: &str,
        referer: Option<String>,
        start_at: Option<u64>,
        on_complete: Option<String>,
        category: Option<&str>,
        log_dir: &Path,
    ) -> Result<Task> {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            bail!("只接受 http/https URL");
        }
        validate_name(name)?;
        let stored_name = match category.filter(|c| !c.is_empty()) {
            Some(category) => {
                ensure!(
                    category
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "分类目录名不合法"
                );
                format!("{category}/{name}")
            }
            None => name.to_string(),
        };
        let now = now_secs();
        let seq = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let id = format!("{}-{seq}", now_millis());
        let task = Task {
            id: id.clone(),
            url: url.to_string(),
            name: stored_name,
            referer: referer.filter(|s| !s.trim().is_empty()),
            status: TaskStatus::Queued,
            pid: None,
            exit_code: None,
            log: log_dir.join(format!("{id}.log")),
            created_at: now,
            updated_at: now,
            start_at,
            hold_until: None,
            on_complete,
            attempts: 0,
            total_len: None,
            delivered: None,
        };
        self.tasks.push(task.clone());
        Ok(task)
    }

    pub fn remove(&mut self, id: &str) -> Option<Task> {
        let pos = self.tasks.iter().position(|t| t.id == id)?;
        Some(self.tasks.remove(pos))
    }
}

/// 队列只接受 `open` / `run:<exe>`；`shutdown` 会一次关掉整台机器，队列可能同时完成多个任务 ⇒ 拒绝。
pub fn validate_on_complete(raw: &str) -> Result<Option<String>> {
    match CompletionAction::from_str(raw) {
        Ok(CompletionAction::Open | CompletionAction::Run(_)) => Ok(Some(raw.to_string())),
        Ok(CompletionAction::Shutdown { .. }) => {
            bail!("队列不支持 shutdown，请在 CLI 用 --on-complete shutdown:<秒数>")
        }
        Err(err) => bail!("{err}"),
    }
}

/// 按文件名扩展名归类（C3）。只影响 UI 标签/筛选，不改变落盘目录。
pub fn category_for(name: &str) -> &'static str {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return "other";
    };
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" | "ts"
        | "m2ts" | "3gp" => "video",
        "mp3" | "flac" | "wav" | "aac" | "m4a" | "ogg" | "opus" | "wma" | "aiff" => "audio",
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" => "archive",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "epub"
        | "mobi" | "csv" => "document",
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "tif" | "tiff" | "heic" => {
            "image"
        }
        "exe" | "msi" | "apk" | "dmg" | "pkg" | "deb" | "rpm" | "appimage" => "program",
        _ => "other",
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("文件名不能为空");
    }
    if name == "." || name == ".." {
        bail!("文件名不能是 . 或 ..");
    }
    if name
        .chars()
        .any(|c| c == '/' || c == '\\' || c == ':' || c.is_control())
    {
        bail!("文件名不能含 / \\ : 或控制字符");
    }
    Ok(())
}

pub fn target_path(out_dir: &Path, name: &str) -> PathBuf {
    out_dir.join(name)
}

pub fn state_path(out_dir: &Path, name: &str) -> PathBuf {
    PathBuf::from(format!("{}.dlm.json", target_path(out_dir, name).display()))
}

/// 覆盖量：断点可读就用段表；成功收尾会删断点，退回读目标文件长度。
pub fn delivered(out_dir: &Path, name: &str) -> Option<u64> {
    let (_, covered) = progress(out_dir, name);
    covered.or_else(|| {
        fs::metadata(target_path(out_dir, name))
            .ok()
            .map(|m| m.len())
    })
}

/// 从断点读 `(total_len, delivered)`；没有/坏文件交 `(None, None)`。
pub fn progress(out_dir: &Path, name: &str) -> (Option<u64>, Option<u64>) {
    let Ok(text) = fs::read_to_string(state_path(out_dir, name)) else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (None, None);
    };
    let total = v
        .get("validator")
        .and_then(|x| x.get("total_len"))
        .and_then(|x| x.as_u64());
    let delivered = v.get("segments").and_then(|x| x.as_array()).map(|rows| {
        rows.iter()
            .filter_map(|row| {
                let start = row.get("start")?.as_u64()?;
                let end = row.get("end")?.as_u64()?;
                if end >= start {
                    Some(end - start + 1)
                } else {
                    None
                }
            })
            .sum::<u64>()
    });
    (total, delivered)
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "queue.json".into());
    path.with_file_name(format!("{name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_queue(tag: &str) -> (PathBuf, QueueStore) {
        let dir = std::env::temp_dir().join(format!("dlm-queue-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("queue.json");
        let store = QueueStore::load(path.clone()).unwrap();
        (dir, store)
    }

    #[test]
    fn add_save_load_roundtrip() {
        let (dir, mut store) = temp_queue("roundtrip");
        let task = store
            .add(
                "https://example.com/a.bin",
                "a.bin",
                Some("https://example.com/".into()),
                None,
                None,
                None,
                &dir,
            )
            .unwrap();
        store.save().unwrap();
        let loaded = QueueStore::load(store.path().to_path_buf()).unwrap();
        let got = loaded.find(&task.id).expect("task survives");
        assert_eq!(got.url, "https://example.com/a.bin");
        assert_eq!(got.name, "a.bin");
        assert_eq!(got.status, TaskStatus::Queued);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn running_becomes_paused_on_recovery() {
        let (dir, mut store) = temp_queue("recover");
        let task = store
            .add(
                "https://example.com/a.bin",
                "a.bin",
                None,
                None,
                None,
                None,
                &dir,
            )
            .unwrap();
        store.find_mut(&task.id).unwrap().status = TaskStatus::Running;
        store.find_mut(&task.id).unwrap().pid = Some(1234);
        assert!(store.recover_running());
        assert_eq!(store.find(&task.id).unwrap().status, TaskStatus::Paused);
        assert_eq!(store.find(&task.id).unwrap().pid, None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_bad_urls_and_names() {
        let (dir, mut store) = temp_queue("bad");
        assert!(
            store
                .add("file:///x", "a.bin", None, None, None, None, &dir)
                .is_err()
        );
        assert!(
            store
                .add(
                    "https://example.com/a",
                    "../a",
                    None,
                    None,
                    None,
                    None,
                    &dir
                )
                .is_err()
        );
        assert!(
            store
                .add("https://example.com/a", "a/b", None, None, None, None, &dir)
                .is_err()
        );
        assert!(
            store
                .add("https://example.com/a", "a:b", None, None, None, None, &dir)
                .is_err()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn progress_sums_segments_and_reads_total() {
        let (dir, _store) = temp_queue("progress");
        let state = state_path(&dir, "a.bin");
        fs::write(
            &state,
            r#"{"v":1,"validator":{"total_len":100},"segments":[{"start":0,"end":9},{"start":20,"end":29}]}"#,
        )
        .unwrap();
        assert_eq!(progress(&dir, "a.bin"), (Some(100), Some(20)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn category_rules_are_extension_based_and_default_to_other() {
        let table: &[(&str, &str)] = &[
            ("movie.MP4", "video"),
            ("song.flac", "audio"),
            ("pack.tar.gz", "archive"),
            ("manual.pdf", "document"),
            ("photo.jpeg", "image"),
            ("setup.exe", "program"),
            ("file.bin", "other"),
            ("no-extension", "other"),
            ("trailing.", "other"),
        ];
        for (name, want) in table {
            assert_eq!(category_for(name), *want, "{name}");
        }
    }

    #[test]
    fn remove_returns_the_task() {
        let (dir, mut store) = temp_queue("remove");
        let task = store
            .add(
                "https://example.com/a.bin",
                "a.bin",
                None,
                None,
                None,
                None,
                &dir,
            )
            .unwrap();
        assert_eq!(store.remove(&task.id).unwrap().id, task.id);
        assert!(store.find(&task.id).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn old_queue_without_quota_or_actions_still_loads() {
        let (dir, _store) = temp_queue("old");
        let path = dir.join("queue.json");
        fs::write(
            &path,
            r#"{"next_id":2,"tasks":[{"id":"1-1","url":"https://x/a","name":"a.bin","referer":null,
                "status":"paused","pid":null,"exit_code":null,"log":"x.log","created_at":1,
                "updated_at":1,"start_at":null,"total_len":null,"delivered":null}]}"#,
        )
        .unwrap();
        let loaded = QueueStore::load(path).unwrap();
        assert!(loaded.quota().is_none());
        assert_eq!(loaded.tasks().len(), 1);
        assert!(loaded.tasks()[0].hold_until.is_none());
        assert!(loaded.tasks()[0].on_complete.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn quota_window_resets_and_releases_holds() {
        let (dir, mut store) = temp_queue("quota");
        let task = store
            .add(
                "https://example.com/a.bin",
                "a.bin",
                None,
                None,
                None,
                None,
                &dir,
            )
            .unwrap();
        assert!(store.set_quota(1_000, 10));
        assert!(store.quota_start(100));
        assert!(store.quota_charge(600));
        assert!(store.quota_hold(&task.id, 110));
        assert!(!store.quota_tick(105), "窗口未到不许清零");
        assert_eq!(store.quota().unwrap().used, 600);
        assert!(store.quota_tick(110), "到点要推进窗口");
        let q = store.quota().unwrap();
        assert_eq!((q.window_started_at, q.used), (110, 0));
        assert_eq!(
            store.find(&task.id).unwrap().hold_until,
            None,
            "到点释放等待"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn quota_charge_and_disable_roundtrip() {
        let (dir, mut store) = temp_queue("quota-off");
        let task = store
            .add(
                "https://example.com/a.bin",
                "a.bin",
                None,
                None,
                None,
                None,
                &dir,
            )
            .unwrap();
        assert!(!store.quota_charge(5), "没启用时不计账");
        assert!(store.set_quota(1_000, 10));
        assert!(store.quota_start(100));
        assert!(store.quota_charge(5));
        assert_eq!(store.quota().unwrap().used, 5);
        assert!(store.quota_hold(&task.id, 110));
        assert!(store.set_quota(0, 10), "关闭是变化");
        assert!(store.quota().is_none());
        assert_eq!(
            store.find(&task.id).unwrap().hold_until,
            None,
            "关闭要清等待"
        );
        assert!(!store.set_quota(0, 10), "重复关闭不再算变化");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn quota_survives_a_queue_roundtrip() {
        let (dir, mut store) = temp_queue("quota-roundtrip");
        assert!(store.set_quota(4_194_304, 3600));
        assert!(store.quota_start(100));
        assert!(store.quota_charge(123));
        store.save().unwrap();
        let loaded = QueueStore::load(dir.join("queue.json")).unwrap();
        let q = loaded.quota().expect("quota survives");
        assert_eq!(
            (q.limit, q.window_secs, q.window_started_at, q.used),
            (4_194_304, 3600, 100, 123)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn queue_accepts_open_and_run_but_rejects_shutdown() {
        assert_eq!(
            validate_on_complete("open").unwrap().as_deref(),
            Some("open")
        );
        assert_eq!(
            validate_on_complete("run:C:\\tools\\scan.exe")
                .unwrap()
                .as_deref(),
            Some("run:C:\\tools\\scan.exe")
        );
        let err = validate_on_complete("shutdown:60").unwrap_err().to_string();
        assert!(err.contains("shutdown"), "{err}");
        assert!(validate_on_complete("wat").is_err());
    }

    #[test]
    fn category_dirs_prefix_the_stored_name_without_letting_users_choose_it() {
        let (dir, mut store) = temp_queue("category-dir");
        let task = store
            .add(
                "https://example.com/a.mp4",
                "a.mp4",
                None,
                None,
                None,
                Some("video"),
                &dir,
            )
            .unwrap();
        assert_eq!(task.name, "video/a.mp4");
        assert_eq!(
            target_path(&dir, &task.name),
            dir.join("video").join("a.mp4")
        );
        assert!(
            store
                .add(
                    "https://example.com/a",
                    "a",
                    None,
                    None,
                    None,
                    Some("../evil"),
                    &dir
                )
                .is_err()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn on_complete_survives_a_queue_roundtrip() {
        let (dir, mut store) = temp_queue("action");
        let task = store
            .add(
                "https://example.com/a.bin",
                "a.bin",
                None,
                None,
                Some("open".to_string()),
                None,
                &dir,
            )
            .unwrap();
        store.save().unwrap();
        let loaded = QueueStore::load(dir.join("queue.json")).unwrap();
        assert_eq!(
            loaded.find(&task.id).unwrap().on_complete.as_deref(),
            Some("open")
        );
        let _ = fs::remove_dir_all(dir);
    }
}
