//! C1 产品层队列（本地 Web UI 的后端）：任务存储、状态、进度、原子写。
//!
//! 队列**不存 Cookie**；认证下载继续走 B2 的一任务一进程入口。这里只接 public 直链。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

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
    pub total_len: Option<u64>,
    pub delivered: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QueueFile {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    tasks: Vec<Task>,
}

#[derive(Debug)]
pub struct QueueStore {
    path: PathBuf,
    next_id: u64,
    tasks: Vec<Task>,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
            Err(err) => return Err(err).with_context(|| format!("读队列文件失败：{}", path.display())),
        };
        Ok(Self { path, next_id: file.next_id.max(1), tasks: file.tasks })
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
            fs::create_dir_all(parent).with_context(|| format!("建队列目录失败：{}", parent.display()))?;
        }
        let file = QueueFile { next_id: self.next_id, tasks: self.tasks.clone() };
        let text = serde_json::to_string_pretty(&file).context("序列化队列失败")?;
        let tmp = tmp_path(&self.path);
        fs::write(&tmp, text).with_context(|| format!("写队列临时文件失败：{}", tmp.display()))?;
        // `rename` 在本机（Windows）会替换已存在的目标；先删目标会把"原子替换"拆成
        // "删掉旧文件 + 再改名"两步，崩在中间就丢队列。
        fs::rename(&tmp, &self.path).with_context(|| format!("提交队列文件失败：{}", self.path.display()))?;
        Ok(())
    }

    pub fn add(
        &mut self,
        url: &str,
        name: &str,
        referer: Option<String>,
        start_at: Option<u64>,
        log_dir: &Path,
    ) -> Result<Task> {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            bail!("只接受 http/https URL");
        }
        validate_name(name)?;
        let now = now_secs();
        let seq = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let id = format!("{}-{seq}", now_millis());
        let task = Task {
            id: id.clone(),
            url: url.to_string(),
            name: name.to_string(),
            referer: referer.filter(|s| !s.trim().is_empty()),
            status: TaskStatus::Queued,
            pid: None,
            exit_code: None,
            log: log_dir.join(format!("{id}.log")),
            created_at: now,
            updated_at: now,
            start_at,
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

pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("文件名不能为空");
    }
    if name == "." || name == ".." {
        bail!("文件名不能是 . 或 ..");
    }
    if name.chars().any(|c| c == '/' || c == '\\' || c == ':' || c.is_control()) {
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

/// 从断点读 `(total_len, delivered)`；没有/坏文件交 `(None, None)`。
pub fn progress(out_dir: &Path, name: &str) -> (Option<u64>, Option<u64>) {
    let Ok(text) = fs::read_to_string(state_path(out_dir, name)) else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (None, None);
    };
    let total = v.get("validator").and_then(|x| x.get("total_len")).and_then(|x| x.as_u64());
    let delivered = v
        .get("segments")
        .and_then(|x| x.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let start = row.get("start")?.as_u64()?;
                    let end = row.get("end")?.as_u64()?;
                    if end >= start { Some(end - start + 1) } else { None }
                })
                .sum::<u64>()
        });
    (total, delivered)
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "queue.json".into());
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
            .add("https://example.com/a.bin", "a.bin", Some("https://example.com/".into()), None, &dir)
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
        let task = store.add("https://example.com/a.bin", "a.bin", None, None, &dir).unwrap();
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
        assert!(store.add("file:///x", "a.bin", None, None, &dir).is_err());
        assert!(store.add("https://example.com/a", "../a", None, None, &dir).is_err());
        assert!(store.add("https://example.com/a", "a/b", None, None, &dir).is_err());
        assert!(store.add("https://example.com/a", "a:b", None, None, &dir).is_err());
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
    fn remove_returns_the_task() {
        let (dir, mut store) = temp_queue("remove");
        let task = store.add("https://example.com/a.bin", "a.bin", None, None, &dir).unwrap();
        assert_eq!(store.remove(&task.id).unwrap().id, task.id);
        assert!(store.find(&task.id).is_none());
        let _ = fs::remove_dir_all(dir);
    }
}
