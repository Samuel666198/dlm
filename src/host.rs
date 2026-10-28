//! B2 宿主：在 B1 的 stdio 帧上增加 `download`，spawn dlm 并把 Cookie 走 stdin。
//!
//! 配置解析顺序：`DLM_NATIVE_CONFIG` 环境变量 → `%LOCALAPPDATA%\dlm\native-host\config.json`；
//! 都没有就用默认：dlm.exe 与宿主同目录、下载到 `%USERPROFILE%\Downloads`、日志到
//! `%LOCALAPPDATA%\dlm\native-host\jobs`。测试用 `DLM_NATIVE_CONFIG` 指向临时目录。

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

pub struct Host {
    config: HostConfig,
    seq: u64,
}

impl Host {
    pub fn new(config: HostConfig) -> Self {
        Self { config, seq: 0 }
    }

    pub fn serve_stdio() -> io::Result<()> {
        let config = HostConfig::load();
        let mut host = Host::new(config);
        host.serve(io::stdin().lock(), io::stdout().lock())
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
        if value.get("op").and_then(Value::as_str) == Some("download") {
            return self.download(&value);
        }
        native::respond(payload)
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
            "--quiet".into(),
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
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .stdin(if cookie.is_some() { Stdio::piped() } else { Stdio::null() });
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

        serde_json::to_vec(&json!({
            "op": "accepted",
            "mode": "direct",
            "job": job,
            "log": log_path.to_string_lossy(),
        }))
        .expect("json! serializable")
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
