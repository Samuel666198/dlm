//! C1–C3 产品层：队列 runner + 本地 HTTP + 内嵌单页 UI。
//!
//! 只绑 127.0.0.1；所有路径（含 `/`）都要求 `?t=<token>`；队列不存 Cookie。
//! runner 是单线程 actor：HTTP 只发命令，runner 拥有队列与子进程。

use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};

use crate::queue::{self, QueueStore, Task, TaskStatus};

#[derive(Debug, Clone)]
pub struct GuiConfig {
    pub queue: PathBuf,
    pub out_dir: PathBuf,
    pub log_dir: PathBuf,
    pub dlm_exe: PathBuf,
    pub concurrency: usize,
    pub port: u16,
    /// 测试可传固定 token；生产默认随机生成。
    pub token: Option<String>,
}

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
}

impl TaskView {
    fn from_task(task: &Task, out_dir: &Path) -> Self {
        let (mut total, mut delivered) = queue::progress(out_dir, &task.name);
        if delivered.is_none() {
            delivered = fs::metadata(queue::target_path(out_dir, &task.name))
                .ok()
                .map(|m| m.len());
        }
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
        }
    }
}

enum Ticket {
    Add { url: String, name: String, referer: Option<String>, start_at: Option<u64> },
    Pause(String),
    Resume(String),
    Remove(String),
}

pub fn run(config: GuiConfig) -> Result<()> {
    fs::create_dir_all(&config.log_dir)
        .with_context(|| format!("建日志目录失败：{}", config.log_dir.display()))?;
    let mut store = QueueStore::load(config.queue.clone())?;
    if store.recover_running() {
        store.save()?;
    }
    let snapshot = Arc::new(Mutex::new(snapshot_of(&store, &config.out_dir)));
    let http_snapshot = Arc::clone(&snapshot);
    let (tx, rx) = mpsc::channel();
    let runner_cfg = config.clone();
    std::thread::Builder::new()
        .name("dlm-gui-runner".into())
        .spawn(move || runner(runner_cfg, store, rx, snapshot))
        .context("启动队列 runner 失败")?;

    let token = config.token.clone().unwrap_or_else(random_token);
    let listener = TcpListener::bind(("127.0.0.1", config.port)).context("绑定 127.0.0.1 失败")?;
    let port = listener.local_addr().context("拿不到监听端口")?.port();
    println!("dlm-gui listening on http://127.0.0.1:{port}/?t={token}");
    io::stdout().flush().ok();

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let token = token.clone();
        let tx = tx.clone();
        let snapshot = Arc::clone(&http_snapshot);
        let log_dir = config.log_dir.clone();
        std::thread::spawn(move || {
            let _ = handle_http(&mut stream, &token, &tx, &snapshot, &log_dir);
        });
    }
    Ok(())
}

fn snapshot_of(store: &QueueStore, out_dir: &Path) -> Vec<TaskView> {
    store.tasks().iter().map(|t| TaskView::from_task(t, out_dir)).collect()
}

fn runner(config: GuiConfig, mut store: QueueStore, rx: Receiver<Ticket>, snapshot: Arc<Mutex<Vec<TaskView>>>) {
    let mut children: HashMap<String, Child> = HashMap::new();
    loop {
        let mut changed = false;
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ticket) => changed |= apply(ticket, &mut store, &mut children, &config),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                for (_, mut child) in children.drain() {
                    let _ = child.kill();
                }
                return;
            }
        }
        changed |= tick(&mut store, &mut children, &config);
        if changed {
            let _ = store.save();
        }
        if let Ok(mut guard) = snapshot.lock() {
            *guard = snapshot_of(&store, &config.out_dir);
        }
    }
}

fn apply(ticket: Ticket, store: &mut QueueStore, children: &mut HashMap<String, Child>, config: &GuiConfig) -> bool {
    match ticket {
        Ticket::Add { url, name, referer, start_at } => {
            if let Err(err) = store.add(&url, &name, referer, start_at, &config.log_dir) {
                eprintln!("dlm-gui: add failed: {err:#}");
                return false;
            }
            true
        }
        Ticket::Pause(id) => {
            let Some(task) = store.find_mut(&id) else { return false };
            if task.status == TaskStatus::Running
                && let Some(mut child) = children.remove(&id)
            {
                let _ = child.kill();
            }
            if matches!(task.status, TaskStatus::Running | TaskStatus::Queued) {
                task.status = TaskStatus::Paused;
                task.pid = None;
                task.updated_at = queue::now_secs();
                return true;
            }
            false
        }
        Ticket::Resume(id) => {
            let Some(task) = store.find_mut(&id) else { return false };
            if matches!(task.status, TaskStatus::Paused | TaskStatus::Failed) {
                task.status = TaskStatus::Queued;
                task.pid = None;
                task.exit_code = None;
                task.updated_at = queue::now_secs();
                return true;
            }
            false
        }
        Ticket::Remove(id) => {
            if let Some(mut child) = children.remove(&id) {
                let _ = child.kill();
            }
            if let Some(task) = store.remove(&id) {
                let _ = fs::remove_file(task.log);
                return true;
            }
            false
        }
    }
}

fn tick(store: &mut QueueStore, children: &mut HashMap<String, Child>, config: &GuiConfig) -> bool {
    let mut changed = false;
    let mut finished: Vec<(String, Option<i32>)> = Vec::new();
    for (id, child) in children.iter_mut() {
        match child.try_wait() {
            Ok(Some(status)) => finished.push((id.clone(), status.code())),
            Ok(None) => {}
            Err(_) => finished.push((id.clone(), None)),
        }
    }
    for (id, code) in finished {
        children.remove(&id);
        if let Some(task) = store.find_mut(&id) {
            task.status = if code == Some(0) { TaskStatus::Done } else { TaskStatus::Failed };
            task.exit_code = code;
            task.pid = None;
            task.updated_at = queue::now_secs();
            changed = true;
        }
    }

    let now = queue::now_secs();
    while children.len() < config.concurrency {
        let next = store
            .tasks()
            .iter()
            .find(|t| t.status == TaskStatus::Queued && t.start_at.is_none_or(|s| s <= now))
            .cloned();
        let Some(task) = next else { break };
        let log = match fs::File::create(&task.log) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("dlm-gui: create job log failed: {err}");
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Failed;
                    t.updated_at = now;
                }
                changed = true;
                continue;
            }
        };
        let log_err = match log.try_clone() {
            Ok(file) => file,
            Err(err) => {
                eprintln!("dlm-gui: clone job log failed: {err}");
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Failed;
                    t.updated_at = now;
                }
                changed = true;
                continue;
            }
        };
        let mut command = Command::new(&config.dlm_exe);
        command
            .arg("--url")
            .arg(&task.url)
            .arg("--out-dir")
            .arg(&config.out_dir)
            .arg("--out-name")
            .arg(&task.name)
            .arg("--quiet");
        if let Some(referer) = &task.referer {
            command.arg("--referer").arg(referer);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                children.insert(task.id.clone(), child);
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Running;
                    t.pid = Some(pid);
                    t.updated_at = now;
                }
                changed = true;
            }
            Err(err) => {
                eprintln!("dlm-gui: spawn dlm failed: {err}");
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Failed;
                    t.updated_at = now;
                }
                changed = true;
            }
        }
    }
    changed
}

fn random_token() -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

fn handle_http(
    stream: &mut TcpStream,
    token: &str,
    tx: &Sender<Ticket>,
    snapshot: &Arc<Mutex<Vec<TaskView>>>,
    log_dir: &Path,
) -> io::Result<()> {
    let request = match read_request(stream) {
        Ok(req) => req,
        Err(err) => {
            let _ = respond(stream, 400, "text/plain; charset=utf-8", err.to_string().as_bytes());
            return Ok(());
        }
    };
    if !request.query.iter().any(|(k, v)| k == "t" && v == token) {
        return respond(stream, 403, "text/plain; charset=utf-8", b"forbidden");
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => respond(stream, 200, "text/html; charset=utf-8", INDEX_HTML.as_bytes()),
        ("GET", "/api/tasks") => {
            let tasks = snapshot.lock().map(|g| g.clone()).unwrap_or_default();
            let body = serde_json::to_vec(&tasks).unwrap_or_else(|_| b"[]".to_vec());
            respond(stream, 200, "application/json", &body)
        }
        ("POST", "/api/add") => {
            let value: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return respond(stream, 400, "application/json", br#"{"ok":false,"error":"bad json"}"#),
            };
            let url = value.get("url").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let name = value.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let referer = value.get("referer").and_then(Value::as_str).map(str::to_string);
            let start_at = value.get("start_at").and_then(Value::as_u64);
            let lower = url.to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                return respond(stream, 200, "application/json", br#"{"ok":false,"error":"only http/https"}"#);
            }
            if let Err(err) = queue::validate_name(&name) {
                let body = serde_json::to_vec(&json!({"ok": false, "error": err.to_string()})).unwrap();
                return respond(stream, 200, "application/json", &body);
            }
            let _ = tx.send(Ticket::Add { url, name, referer, start_at });
            respond(stream, 200, "application/json", br#"{"ok":true}"#)
        }
        ("POST", "/api/pause") | ("POST", "/api/resume") | ("POST", "/api/remove") => {
            let value: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return respond(stream, 400, "application/json", br#"{"ok":false,"error":"bad json"}"#),
            };
            let Some(id) = value.get("id").and_then(Value::as_str) else {
                return respond(stream, 400, "application/json", br#"{"ok":false,"error":"missing id"}"#);
            };
            let ticket = match request.path.as_str() {
                "/api/pause" => Ticket::Pause(id.to_string()),
                "/api/resume" => Ticket::Resume(id.to_string()),
                _ => Ticket::Remove(id.to_string()),
            };
            let _ = tx.send(ticket);
            respond(stream, 200, "application/json", br#"{"ok":true}"#)
        }
        ("GET", "/api/log") => {
            let Some(id) = request.query.iter().find(|(k, _)| k == "id").map(|(_, v)| v.as_str()) else {
                return respond(stream, 400, "text/plain; charset=utf-8", b"missing id");
            };
            let log = snapshot
                .lock()
                .ok()
                .and_then(|g| g.iter().find(|t| t.id == id).map(|t| PathBuf::from(&t.log)));
            let Some(log) = log else {
                return respond(stream, 404, "text/plain; charset=utf-8", b"not found");
            };
            let Ok(base) = log_dir.canonicalize() else {
                return respond(stream, 404, "text/plain; charset=utf-8", b"not found");
            };
            let Ok(real) = log.canonicalize() else {
                return respond(stream, 404, "text/plain; charset=utf-8", b"no log yet");
            };
            if !real.starts_with(&base) {
                return respond(stream, 403, "text/plain; charset=utf-8", b"forbidden");
            }
            let mut data = fs::read(&real).unwrap_or_default();
            if data.len() > 64 * 1024 {
                data = data[data.len() - 64 * 1024..].to_vec();
            }
            respond(stream, 200, "text/plain; charset=utf-8", &data)
        }
        _ => respond(stream, 404, "text/plain; charset=utf-8", b"not found"),
    }
}

struct Request {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_bytes(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request headers too large"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "request ended before headers"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    if method.is_empty() || target.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad request line"));
    }
    let (path, query) = split_target(&target);
    let mut content_length = 0usize;
    for line in lines {
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 64 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(Request { method, path, query, body })
}

fn split_target(target: &str) -> (String, Vec<(String, String)>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let pairs = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();
    (path.to_string(), pairs)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="zh"><head><meta charset="utf-8"><title>dlm 任务队列</title>
<style>
:root{color-scheme:light dark;font-family:system-ui,Segoe UI,Microsoft YaHei,sans-serif}
body{max-width:980px;margin:24px auto;padding:0 16px;background:#f6f7f9;color:#1b1f24}
h1{font-size:20px;margin:0 0 16px}
form{display:grid;grid-template-columns:2fr 1fr 1fr 1fr auto;gap:8px;margin-bottom:16px}
input,button{font:13px/1.4 inherit;padding:8px 10px;border:1px solid #c9ced6;border-radius:6px;background:#fff}
button{background:#1565c0;color:#fff;border:0;cursor:pointer}
button.secondary{background:#e8edf5;color:#1b1f24}
.task{background:#fff;border:1px solid #e1e5ea;border-radius:8px;padding:12px;margin-bottom:10px}
.title{font-weight:600;margin-bottom:4px}
.url{font:12px/1.4 ui-monospace,Consolas,monospace;color:#4b5563;word-break:break-all}
.progress{height:8px;background:#e8edf5;border-radius:4px;margin:8px 0 4px;overflow:hidden}
.progress > div{height:100%;background:#1565c0;transition:width .3s}
.meta{font-size:12px;color:#6b7280}
.actions{display:flex;gap:6px;margin-top:8px}
pre{background:#0f172a;color:#dbeafe;padding:12px;border-radius:8px;overflow:auto;max-height:280px;font:12px/1.4 ui-monospace,Consolas,monospace}
@media (max-width:760px){form{grid-template-columns:1fr 1fr}.task{padding:10px}}
</style></head><body>
<h1>dlm 任务队列</h1>
<form id="add">
  <input id="url" placeholder="https://example.com/file.bin" required>
  <input id="name" placeholder="文件名（自动从 URL 填）" required>
  <input id="referer" placeholder="Referer（可选）">
  <input id="start" type="datetime-local" title="起始时间（可选）">
  <button type="submit">添加</button>
</form>
<div id="tasks"></div>
<h2 style="font-size:16px">日志</h2>
<pre id="log">点任务上的“日志”查看。</pre>
<script>
const token = new URLSearchParams(location.search).get('t') || '';
const api = (path, opts) => fetch(path + (path.includes('?') ? '&' : '?') + 't=' + encodeURIComponent(token), opts);
const $ = (id) => document.getElementById(id);
const urlInput = $('url'), nameInput = $('name');
urlInput.addEventListener('change', () => {
  if (nameInput.value) return;
  try {
    const u = new URL(urlInput.value);
    const last = decodeURIComponent(u.pathname.split('/').filter(Boolean).pop() || 'download.bin');
    nameInput.value = last;
  } catch {}
});
async function act(id, op) {
  await api('/api/' + op, {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({id})});
  load();
}
async function showLog(id) {
  const r = await api('/api/log?id=' + encodeURIComponent(id));
  $('log').textContent = await r.text();
}
function row(t) {
  const box = document.createElement('div'); box.className = 'task';
  const title = document.createElement('div'); title.className = 'title';
  title.textContent = t.name + ' — ' + t.status + (t.start_at ? '（计划）' : '');
  const url = document.createElement('div'); url.className = 'url'; url.textContent = t.url;
  const progress = document.createElement('div'); progress.className = 'progress';
  const bar = document.createElement('div'); bar.style.width = Math.max(0, Math.min(100, t.progress || 0)) + '%';
  progress.appendChild(bar);
  const meta = document.createElement('div'); meta.className = 'meta';
  meta.textContent = (t.delivered || 0) + ' / ' + (t.total_len == null ? '?' : t.total_len) + ' B' +
    (t.pid ? ' · pid ' + t.pid : '') + (t.exit_code == null ? '' : ' · exit ' + t.exit_code);
  const actions = document.createElement('div'); actions.className = 'actions';
  const mk = (label, fn, secondary) => {
    const b = document.createElement('button'); b.textContent = label;
    if (secondary) b.className = 'secondary';
    b.addEventListener('click', fn); actions.appendChild(b);
  };
  if (t.status === 'running' || t.status === 'queued') mk('暂停', () => act(t.id, 'pause'), true);
  if (t.status === 'paused' || t.status === 'failed') mk('恢复', () => act(t.id, 'resume'));
  mk('日志', () => showLog(t.id), true);
  mk('删除', () => act(t.id, 'remove'), true);
  box.append(title, url, progress, meta, actions);
  return box;
}
async function load() {
  try {
    const r = await api('/api/tasks');
    if (!r.ok) throw new Error(await r.text());
    const tasks = await r.json();
    const box = $('tasks'); box.replaceChildren();
    if (!tasks.length) { const p = document.createElement('p'); p.textContent = '队列为空'; box.appendChild(p); }
    for (const t of tasks) box.appendChild(row(t));
  } catch (e) {
    $('log').textContent = '刷新失败：' + e.message;
  }
}
$('add').addEventListener('submit', async (e) => {
  e.preventDefault();
  const body = {
    url: urlInput.value.trim(),
    name: nameInput.value.trim(),
    referer: $('referer').value.trim() || null,
    start_at: $('start').value ? Math.floor(new Date($('start').value).getTime() / 1000) : null,
  };
  const r = await api('/api/add', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
  const j = await r.json();
  if (!j.ok) { alert(j.error || '添加失败'); return; }
  e.target.reset();
  load();
});
setInterval(load, 1000);
load();
</script></body></html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_target_keeps_path_and_query_pairs() {
        let (path, query) = split_target("/api/log?id=abc&t=deadbeef");
        assert_eq!(path, "/api/log");
        assert!(query.iter().any(|(k, v)| k == "id" && v == "abc"));
        assert!(query.iter().any(|(k, v)| k == "t" && v == "deadbeef"));
    }

    #[test]
    fn find_bytes_finds_the_header_boundary() {
        assert_eq!(find_bytes(b"GET / HTTP/1.1\r\n\r\nbody", b"\r\n\r\n"), Some(14));
        assert_eq!(find_bytes(b"none", b"\r\n\r\n"), None);
    }

    #[test]
    fn token_is_hex_and_not_constant() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
