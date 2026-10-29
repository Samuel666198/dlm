//! B2 宿主：在 B1 的 stdio 帧上增加 `download`，spawn dlm 并把 Cookie 走 stdin。
//!
//! 配置解析顺序：`DLM_NATIVE_CONFIG` 环境变量 → `%LOCALAPPDATA%\dlm\native-host\config.json`；
//! 都没有就用默认：dlm.exe 与宿主同目录、下载到 `%USERPROFILE%\Downloads`、日志到
//! `%LOCALAPPDATA%\dlm\native-host\jobs`。测试用 `DLM_NATIVE_CONFIG` 指向临时目录。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::native;
use crate::queue;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub dlm_exe: PathBuf,
    pub download_dir: PathBuf,
    pub log_dir: PathBuf,
    /// B10：`dlm-gui` 写的端点文件；`None` = 不走队列。
    pub gui_endpoint: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    dlm_exe: Option<PathBuf>,
    download_dir: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    gui_endpoint: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct GuiEndpoint {
    port: u16,
    token: String,
}

#[derive(Debug, PartialEq, Eq)]
enum QueueOutcome {
    Queued(String),
    Rejected(String),
    Unavailable,
    TimedOut,
}

impl HostConfig {
    pub fn load() -> Self {
        let config_path = std::env::var_os("DLM_NATIVE_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA")
                    .map(|p| PathBuf::from(p).join("dlm/native-host/config.json"))
            });
        let file = config_path
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<ConfigFile>(&text).ok())
            .unwrap_or_default();

        let host_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let dlm_exe = file.dlm_exe.unwrap_or_else(|| host_dir.join("dlm.exe"));
        let download_dir = file.download_dir.unwrap_or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|p| PathBuf::from(p).join("Downloads"))
                .unwrap_or_else(|| host_dir.join("downloads"))
        });
        let log_dir = file.log_dir.unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|p| PathBuf::from(p).join("dlm/native-host/jobs"))
                .unwrap_or_else(|| host_dir.join("jobs"))
        });
        let gui_endpoint = file.gui_endpoint.or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("dlm/gui/endpoint.json"))
        });
        Self { dlm_exe, download_dir, log_dir, gui_endpoint }
    }
}

/// Windows：别给控制台程序开可见终端。宿主是浏览器拉起的后台进程，弹一个空白终端
/// 会让用户以为“没反应”，关掉窗口还会顺手杀掉正在下载的 dlm。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type Notify = Arc<dyn Fn(Vec<u8>) + Send + Sync>;
type Jobs = Arc<Mutex<HashMap<String, (u32, Option<i64>)>>>;
type Canceled = Arc<Mutex<HashSet<String>>>;

pub struct Host {
    config: HostConfig,
    seq: u64,
    notify: Option<Notify>,
    jobs: Jobs,
    canceled: Canceled,
}

impl Host {
    pub fn new(config: HostConfig) -> Self {
        Self {
            config,
            seq: 0,
            notify: None,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            canceled: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn with_notify(mut self, notify: Notify) -> Self {
        self.notify = Some(notify);
        self
    }

    pub fn serve_stdio() -> io::Result<()> {
        let config = HostConfig::load();
        let out = Arc::new(std::sync::Mutex::new(io::stdout()));
        let writer = out.clone();
        let host = Host::new(config).with_notify(Arc::new(move |bytes| {
            if let Ok(mut guard) = writer.lock() {
                let _ = native::write_frame(&mut *guard, &bytes);
            }
        }));
        let mut stdin = io::stdin().lock();
        let mut host = host;
        while let Some(frame) = native::read_frame(&mut stdin)? {
            let reply = host.handle(&frame);
            native::write_frame(&mut *out.lock().expect("stdout lock"), &reply)?;
        }
        Ok(())
    }

    pub fn serve<R: Read, W: Write>(&mut self, mut input: R, mut output: W) -> io::Result<()> {
        while let Some(frame) = native::read_frame(&mut input)? {
            let reply = self.handle(&frame);
            native::write_frame(&mut output, &reply)?;
        }
        Ok(())
    }

    pub fn handle(&mut self, payload: &[u8]) -> Vec<u8> {
        let value: Value = match serde_json::from_slice(payload) {
            Ok(v) => v,
            Err(_) => return native::respond(payload),
        };
        match value.get("op").and_then(Value::as_str) {
            Some("download") => self.download(&value),
            Some("cancel") => self.cancel(&value),
            _ => native::respond(payload),
        }
    }

    fn download(&mut self, value: &Value) -> Vec<u8> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
        let Some(url) = text("url") else {
            return error("download needs a url");
        };
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return error("only http/https urls are accepted");
        }
        let name = text("name");
        let referer = text("referer");
        let cookie = text("cookie");
        let hls = value.get("hls").and_then(Value::as_bool).unwrap_or(false);
        let tab = value.get("tab").and_then(Value::as_i64);
        let display_name = name.map(str::to_string).unwrap_or_else(|| derive_name(url));

        // B10：无 Cookie 且队列可达 ⇒ 交给队列（队列的落点是 `dlm-gui --out-dir`）。
        // 带 Cookie 的下载仍走 B2 直连：队列不存 Cookie。
        // B3b：HLS 强制直连 —— 队列没有 --hls，也不能把不可续传的 HLS 伪装成普通文件。
        if cookie.is_none() && !hls {
            match self.try_queue(url, name, referer) {
                QueueOutcome::Queued(id) => {
                    return serde_json::to_vec(&json!({
                        "op": "accepted",
                        "mode": "queue",
                        "id": id,
                        "name": display_name,
                        "tab": tab,
                    }))
                    .expect("json! serializable");
                }
                QueueOutcome::Rejected(message) => return error(&message),
                QueueOutcome::TimedOut => return error("队列响应超时，未重复下载"),
                QueueOutcome::Unavailable => {}
            }
        }

        if let Err(err) = fs::create_dir_all(&self.config.download_dir) {
            eprintln!("dlm-native-host: create download dir failed: {err}");
            return error("could not create download dir");
        }
        if let Err(err) = fs::create_dir_all(&self.config.log_dir) {
            eprintln!("dlm-native-host: create log dir failed: {err}");
            return error("could not create log dir");
        }

        self.seq = self.seq.saturating_add(1);
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let job = format!("{secs}-{}", self.seq);
        let log_path = self.config.log_dir.join(format!("{job}.log"));
        let log = match fs::File::create(&log_path) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("dlm-native-host: create job log failed: {err}");
                return error("could not create job log");
            }
        };
        let log_err = match log.try_clone() {
            Ok(file) => file,
            Err(err) => {
                eprintln!("dlm-native-host: clone job log failed: {err}");
                return error("could not create job log");
            }
        };

        let mut args: Vec<std::ffi::OsString> = vec![
            "--url".into(),
            url.into(),
            "--out-dir".into(),
            self.config.download_dir.clone().into(),
        ];
        if let Some(name) = name {
            args.push("--out-name".into());
            args.push(name.into());
        }
        if let Some(referer) = referer {
            args.push("--referer".into());
            args.push(referer.into());
        }
        if cookie.is_some() {
            args.push("--cookie-stdin".into());
        }
        if hls {
            args.push("--hls".into());
        }

        let mut command = Command::new(&self.config.dlm_exe);
        command
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log_err))
            .stdin(if cookie.is_some() { Stdio::piped() } else { Stdio::null() });
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                eprintln!("dlm-native-host: spawn dlm failed: {err}");
                return error("could not start dlm");
            }
        };
        if let Some(cookie) = cookie
            && let Some(mut stdin) = child.stdin.take()
            && (stdin.write_all(cookie.as_bytes()).is_err() || stdin.write_all(b"\n").is_err())
        {
            let _ = child.kill();
            return error("could not hand cookie to dlm");
        }

        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.insert(job.clone(), (child.id(), tab));
        }

        if let Some(stdout) = child.stdout.take() {
            let job = job.clone();
            let name = display_name.clone();
            let notify = self.notify.clone();
            let mut log = log;
            std::thread::spawn(move || {
                let mut reader = stdout;
                let mut acc: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];
                let mut last_emit = Instant::now() - Duration::from_secs(2);
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    for &byte in &buf[..n] {
                        if byte == b'\r' || byte == b'\n' {
                            if !acc.is_empty() {
                                handle_dlm_line(
                                    &String::from_utf8_lossy(&acc),
                                    &mut log,
                                    &job,
                                    &name,
                                    tab,
                                    &notify,
                                    &mut last_emit,
                                );
                                acc.clear();
                            }
                        } else {
                            acc.push(byte);
                        }
                    }
                }
                if !acc.is_empty() {
                    handle_dlm_line(
                        &String::from_utf8_lossy(&acc),
                        &mut log,
                        &job,
                        &name,
                        tab,
                        &notify,
                        &mut last_emit,
                    );
                }
            });
        }

        {
            let notify = self.notify.clone();
            let job = job.clone();
            let name = display_name.clone();
            let log_path = log_path.clone();
            let jobs = self.jobs.clone();
            let canceled = self.canceled.clone();
            std::thread::spawn(move || {
                let (ok, code) = match child.wait() {
                    Ok(status) => (status.success(), status.code()),
                    Err(_) => (false, None),
                };
                if let Ok(mut map) = jobs.lock() {
                    map.remove(&job);
                }
                let was_canceled = canceled.lock().map(|mut set| set.remove(&job)).unwrap_or(false);
                let frame = json!({
                    "op": "done",
                    "job": job,
                    "tab": tab,
                    "name": name,
                    "ok": ok,
                    "code": code,
                    "canceled": was_canceled,
                    "log": log_path.to_string_lossy(),
                });
                if let Some(notify) = notify
                    && let Ok(bytes) = serde_json::to_vec(&frame)
                {
                    notify(bytes);
                }
            });
        }

        serde_json::to_vec(&json!({
            "op": "accepted",
            "mode": "direct",
            "job": job,
            "tab": tab,
            "name": display_name,
            "log": log_path.to_string_lossy(),
        }))
        .expect("json! serializable")
    }

    fn cancel(&self, value: &Value) -> Vec<u8> {
        let Some(job) = value.get("job").and_then(Value::as_str) else {
            return error("cancel needs a job");
        };
        let found = self.jobs.lock().ok().and_then(|jobs| jobs.get(job).copied());
        let Some((pid, tab)) = found else {
            return serde_json::to_vec(&json!({
                "op": "cancel",
                "job": job,
                "ok": false,
                "message": "未知任务或已结束",
            }))
            .expect("json! serializable");
        };
        if let Ok(mut set) = self.canceled.lock() {
            set.insert(job.to_string());
        }
        #[cfg(windows)]
        {
            let mut command = Command::new("taskkill");
            command
                .args(["/PID", &pid.to_string(), "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            let ok = command.status().map(|status| status.success()).unwrap_or(false);
            if !ok && let Ok(mut set) = self.canceled.lock() {
                set.remove(job);
            }
            serde_json::to_vec(&json!({
                "op": "cancel",
                "job": job,
                "tab": tab,
                "ok": ok,
                "message": if ok { "已停止，断点保留" } else { "停止失败" },
            }))
            .expect("json! serializable")
        }
        #[cfg(not(windows))]
        {
            let _ = tab;
            serde_json::to_vec(&json!({
                "op": "cancel",
                "job": job,
                "ok": false,
                "message": "当前平台不支持停止",
            }))
            .expect("json! serializable")
        }
    }

    fn try_queue(&self, url: &str, name: Option<&str>, referer: Option<&str>) -> QueueOutcome {
        let Some(path) = &self.config.gui_endpoint else { return QueueOutcome::Unavailable };
        let Ok(text) = fs::read_to_string(path) else { return QueueOutcome::Unavailable };
        let Ok(endpoint) = serde_json::from_str::<GuiEndpoint>(&text) else {
            return QueueOutcome::Unavailable;
        };
        let name = name
            .filter(|n| queue::validate_name(n).is_ok())
            .map(str::to_string)
            .unwrap_or_else(|| derive_name(url));
        let client = match reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(5))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                eprintln!("dlm-native-host: queue client failed: {err}");
                return QueueOutcome::Unavailable;
            }
        };
        let body = json!({
            "url": url,
            "name": name,
            "referer": referer,
            "start_at": null,
        });
        let response = client
            .post(format!("http://127.0.0.1:{}/api/add?t={}", endpoint.port, endpoint.token))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send();
        match response {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().unwrap_or_default();
                if status != reqwest::StatusCode::OK {
                    return QueueOutcome::Unavailable;
                }
                match serde_json::from_str::<Value>(&text) {
                    Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => {
                        let Some(id) = value.get("id").and_then(Value::as_str) else {
                            return QueueOutcome::Rejected("队列接受了任务但没有返回 id".to_string());
                        };
                        QueueOutcome::Queued(id.to_string())
                    }
                    Ok(value) => QueueOutcome::Rejected(
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("队列拒绝了任务")
                            .to_string(),
                    ),
                    Err(err) => QueueOutcome::Rejected(format!("队列响应无法解析：{err}")),
                }
            }
            Err(err) if err.is_connect() => QueueOutcome::Unavailable,
            Err(err) if err.is_timeout() => QueueOutcome::TimedOut,
            Err(err) => {
                eprintln!("dlm-native-host: queue request failed: {err}");
                QueueOutcome::Unavailable
            }
        }
    }
}

/// 把 dlm 的一行 stdout 分流：进度行转成 `op:"progress"`（每 400 ms 最多一发），其余写回任务日志。
fn handle_dlm_line(
    text: &str,
    log: &mut fs::File,
    job: &str,
    name: &str,
    tab: Option<i64>,
    notify: &Option<Notify>,
    last_emit: &mut Instant,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if let Some((percent, done, total, speed)) = parse_progress(text) {
        if last_emit.elapsed() < Duration::from_millis(400) {
            return;
        }
        *last_emit = Instant::now();
        if let Some(notify) = notify {
            let eta = if speed > 0.0 {
                ((total.saturating_sub(done)) as f64 / speed).round() as u64
            } else {
                0
            };
            let frame = json!({
                "op": "progress",
                "job": job,
                "tab": tab,
                "name": name,
                "percent": percent,
                "done": done,
                "total": total,
                "speed": speed,
                "eta": eta,
            });
            if let Ok(bytes) = serde_json::to_vec(&frame) {
                notify(bytes);
            }
        }
    } else {
        let _ = writeln!(log, "{text}");
    }
}

/// `45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4  dup 5.5%` ⇒ `(45.2, done, total, speed)`（字节/秒）。
fn parse_progress(line: &str) -> Option<(f64, u64, u64, f64)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 6 {
        return None;
    }
    let percent = tokens[0].strip_suffix('%')?.parse::<f64>().ok()?;
    let done_mib = tokens[1].parse::<f64>().ok()?;
    let total_mib = tokens[2].strip_prefix("MiB/")?.parse::<f64>().ok()?;
    if tokens[3] != "MiB" || !tokens[5].starts_with("MiB/s") {
        return None;
    }
    let speed_mib = tokens[4].parse::<f64>().ok()?;
    const MIB: f64 = 1_048_576.0;
    Some((
        percent,
        (done_mib * MIB) as u64,
        (total_mib * MIB) as u64,
        speed_mib * MIB,
    ))
}

fn derive_name(url: &str) -> String {
    let fallback = || "download.bin".to_string();
    let Ok(parsed) = reqwest::Url::parse(url) else { return fallback() };
    let Some(segment) = parsed.path_segments().and_then(|mut s| s.next_back()) else {
        return fallback();
    };
    let segment = segment.trim();
    if segment.is_empty() || queue::validate_name(segment).is_err() {
        fallback()
    } else {
        segment.to_string()
    }
}

fn error(message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"op": "error", "message": message})).expect("json! serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &Path) -> HostConfig {
        HostConfig {
            dlm_exe: std::env::var_os("COMSPEC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cmd.exe")),
            download_dir: dir.join("dl"),
            log_dir: dir.join("logs"),
            gui_endpoint: None,
        }
    }

    fn endpoint_file(dir: &Path, port: u16) -> PathBuf {
        let path = dir.join("endpoint.json");
        fs::write(
            &path,
            serde_json::json!({"port": port, "token": "tok", "pid": 1, "started_at": 1}).to_string(),
        )
        .unwrap();
        path
    }

    fn one_shot_stub(response: &'static str) -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        });
        (port, handle)
    }

    #[test]
    fn ping_and_hello_still_go_through_native_respond() {
        let mut host = Host::new(config(&std::env::temp_dir()));
        let pong: Value = serde_json::from_slice(&host.handle(br#"{"op":"ping"}"#)).unwrap();
        assert_eq!(pong["op"], "pong");
        assert_eq!(pong["host"], "com.dlm.host");
    }

    #[test]
    fn download_rejects_non_http_urls() {
        let mut host = Host::new(config(&std::env::temp_dir()));
        let reply: Value = serde_json::from_slice(&host.handle(br#"{"op":"download","url":"file:///x"}"#)).unwrap();
        assert_eq!(reply["op"], "error");
    }

    #[test]
    fn download_spawns_the_configured_program_and_returns_a_job() {
        let dir = std::env::temp_dir().join(format!("dlm-b2-host-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut host = Host::new(config(&dir));
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/x","name":"x.bin"}"#),
        )
        .unwrap();
        assert_eq!(reply["op"], "accepted", "{reply}");
        assert_eq!(reply["mode"], "direct", "没有端点文件 ⇒ 回退直连：{reply}");
        let log = reply["log"].as_str().expect("log path");
        assert!(std::path::Path::new(log).exists(), "job log should exist: {log}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_reports_when_the_download_process_exits() {
        let dir = std::env::temp_dir().join(format!("dlm-b2-done-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (tx, rx) = std::sync::mpsc::channel();
        let mut host = Host::new(config(&dir)).with_notify(Arc::new(move |bytes| {
            let _ = tx.send(bytes);
        }));
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/x","name":"x.bin"}"#),
        )
        .unwrap();
        assert_eq!(reply["op"], "accepted", "{reply}");
        let frame = rx.recv_timeout(Duration::from_secs(15)).expect("done frame");
        let done: Value = serde_json::from_slice(&frame).unwrap();
        assert_eq!(done["op"], "done", "{done}");
        assert_eq!(done["job"], reply["job"]);
        assert!(done["ok"].is_boolean(), "done 必须带退出结论：{done}");
        assert_eq!(done["log"], reply["log"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_lines_are_parsed_and_other_lines_are_not() {
        let (percent, done, total, speed) =
            parse_progress("45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4  dup 5.5%").expect("progress line");
        assert!((percent - 45.2).abs() < 1e-9);
        assert_eq!(total, 48 * 1_048_576);
        assert!((done as i64 - (21.7 * 1_048_576.0) as i64).abs() <= 1);
        assert!((speed - 3.1 * 1_048_576.0).abs() < 1.0);
        assert!(parse_progress("下载已完成  目标：x  全长 4 B").is_none());
        assert!(parse_progress("").is_none());
    }

    #[test]
    fn cancel_unknown_job_is_refused() {
        let mut host = Host::new(config(&std::env::temp_dir()));
        let reply: Value = serde_json::from_slice(&host.handle(br#"{"op":"cancel","job":"nope"}"#)).unwrap();
        assert_eq!(reply["op"], "cancel");
        assert_eq!(reply["ok"], false);
    }

    #[test]
    fn derive_name_from_url_shapes() {
        assert_eq!(derive_name("https://x/a.bin"), "a.bin");
        assert_eq!(derive_name("https://x/"), "download.bin");
        assert_eq!(derive_name("https://x/dir/"), "download.bin");
        assert_eq!(derive_name("https://x/a%20b.zip"), "a%20b.zip", "不做百分号解码");
        assert_eq!(derive_name("https://x/a:b"), "download.bin", "冒号会被队列名闸拒");
        assert_eq!(derive_name("https://x/a?x=1"), "a");
    }

    #[test]
    fn host_sends_cookieless_downloads_to_the_queue() {
        let (port, handle) = one_shot_stub(r#"{"ok":true,"id":"q-1"}"#);
        let dir = std::env::temp_dir().join(format!("dlm-b10-queue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut host = Host::new(HostConfig {
            gui_endpoint: Some(endpoint_file(&dir, port)),
            dlm_exe: dir.join("no-such-dlm.exe"),
            ..config(&dir)
        });
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/a.bin"}"#),
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(reply["op"], "accepted", "{reply}");
        assert_eq!(reply["mode"], "queue", "{reply}");
        assert_eq!(reply["id"], "q-1");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_keeps_cookie_downloads_on_the_direct_path() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("dlm-b10-cookie-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut host = Host::new(HostConfig {
            gui_endpoint: Some(endpoint_file(&dir, port)),
            ..config(&dir)
        });
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/a.bin","cookie":"sid=1"}"#),
        )
        .unwrap();
        assert_eq!(reply["mode"], "direct", "带 Cookie 必须直连：{reply}");
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err(), "带 Cookie 时不许碰队列端点");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_falls_back_to_direct_when_the_queue_endpoint_is_unreachable() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dir = std::env::temp_dir().join(format!("dlm-b10-down-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut host = Host::new(HostConfig {
            gui_endpoint: Some(endpoint_file(&dir, port)),
            ..config(&dir)
        });
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/a.bin"}"#),
        )
        .unwrap();
        assert_eq!(reply["mode"], "direct", "端点连不上要回退：{reply}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hls_downloads_skip_the_queue_and_stay_direct() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("dlm-b3b-hls-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut host = Host::new(HostConfig {
            gui_endpoint: Some(endpoint_file(&dir, port)),
            ..config(&dir)
        });
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/a.m3u8","hls":true}"#),
        )
        .unwrap();
        assert_eq!(reply["mode"], "direct", "HLS 必须直连：{reply}");
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err(), "HLS 不许碰队列端点");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_returns_queue_rejection_without_falling_back() {
        let (port, handle) = one_shot_stub(r#"{"ok":false,"error":"nope"}"#);
        let dir = std::env::temp_dir().join(format!("dlm-b10-reject-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut host = Host::new(HostConfig {
            gui_endpoint: Some(endpoint_file(&dir, port)),
            dlm_exe: dir.join("no-such-dlm.exe"),
            ..config(&dir)
        });
        let reply: Value = serde_json::from_slice(
            &host.handle(br#"{"op":"download","url":"https://example.invalid/a.bin"}"#),
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(reply["op"], "error", "{reply}");
        assert_eq!(reply["message"], "nope", "队列明确拒绝时不许回退直连：{reply}");
        let _ = fs::remove_dir_all(&dir);
    }
}
