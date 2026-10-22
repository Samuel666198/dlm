//! B2 宿主：在 B1 的 stdio 帧上增加 `download`，spawn dlm 并把 Cookie 走 stdin。
//!
//! 配置解析顺序：`DLM_NATIVE_CONFIG` 环境变量 → `%LOCALAPPDATA%\dlm\native-host\config.json`；
//! 都没有就用默认：dlm.exe 与宿主同目录、下载到 `%USERPROFILE%\Downloads`、日志到
//! `%LOCALAPPDATA%\dlm\native-host\jobs`。测试用 `DLM_NATIVE_CONFIG` 指向临时目录。

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::native;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub dlm_exe: PathBuf,
    pub download_dir: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    dlm_exe: Option<PathBuf>,
    download_dir: Option<PathBuf>,
    log_dir: Option<PathBuf>,
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
        Self { dlm_exe, download_dir, log_dir }
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
            "job": job,
            "log": log_path.to_string_lossy(),
        }))
        .expect("json! serializable")
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
        }
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
        let log = reply["log"].as_str().expect("log path");
        assert!(std::path::Path::new(log).exists(), "job log should exist: {log}");
        let _ = fs::remove_dir_all(&dir);
    }
}
