//! 本地 HTTP：只服务浏览器扩展与 CLI，不服务原生 UI。
//!
//! 原生 UI 直接读 `snapshot`，不绕这一层打自己的服务（设计稿第四节）。
//! 只绑 127.0.0.1；所有路径（含 `/`）都要求 `?t=<token>`。

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::actions;
use crate::queue;

use super::Ticket;
use super::snapshot::{QuotaView, TaskView, UpdateView};

/// 这个页面**不是**应用界面 —— 原生窗口才是。
///
/// 它只给浏览器扩展与人工排查用：说明队列在哪、窗口怎么找回来。
/// 原来那份 719 行的 `ui.html` 已随原生 UI 上线删除（设计稿第 10 步）。
pub const LANDING_HTML: &str = include_str!("landing.html");

pub(crate) fn handle_http(
    stream: &mut TcpStream,
    token: &str,
    tx: &Sender<Ticket>,
    snapshot: &Arc<Mutex<Vec<TaskView>>>,
    quota_snapshot: &Arc<Mutex<Option<QuotaView>>>,
    update_snapshot: &Arc<Mutex<UpdateView>>,
    log_dir: &Path,
) -> io::Result<()> {
    let request = match read_request(stream) {
        Ok(req) => req,
        Err(err) => {
            let _ = respond(
                stream,
                400,
                "text/plain; charset=utf-8",
                err.to_string().as_bytes(),
            );
            return Ok(());
        }
    };
    if !request.query.iter().any(|(k, v)| k == "t" && v == token) {
        return respond(stream, 403, "text/plain; charset=utf-8", b"forbidden");
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => respond(
            stream,
            200,
            "text/html; charset=utf-8",
            LANDING_HTML.as_bytes(),
        ),
        ("GET", "/api/tasks") => {
            let tasks = snapshot.lock().map(|g| g.clone()).unwrap_or_default();
            let body = serde_json::to_vec(&tasks).unwrap_or_else(|_| b"[]".to_vec());
            respond(stream, 200, "application/json", &body)
        }
        ("GET", "/api/quota") => {
            let quota = quota_snapshot.lock().ok().and_then(|g| g.clone());
            let body = match quota {
                Some(q) => {
                    serde_json::to_vec(&q).unwrap_or_else(|_| br#"{"enabled":false}"#.to_vec())
                }
                None => br#"{"enabled":false}"#.to_vec(),
            };
            respond(stream, 200, "application/json", &body)
        }
        ("GET", "/api/update") => {
            let update = update_snapshot
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| UpdateView::disabled());
            let body =
                serde_json::to_vec(&update).unwrap_or_else(|_| br#"{"enabled":false}"#.to_vec());
            respond(stream, 200, "application/json", &body)
        }
        ("POST", "/api/add") => {
            let value: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => {
                    return respond(
                        stream,
                        400,
                        "application/json",
                        br#"{"ok":false,"error":"bad json"}"#,
                    );
                }
            };
            let url = value
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let referer = value
                .get("referer")
                .and_then(Value::as_str)
                .map(str::to_string);
            let start_at = value.get("start_at").and_then(Value::as_u64);
            let on_complete = value
                .get("on_complete")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let lower = url.to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                return respond(
                    stream,
                    200,
                    "application/json",
                    br#"{"ok":false,"error":"only http/https"}"#,
                );
            }
            if let Err(err) = queue::validate_name(&name) {
                let body =
                    serde_json::to_vec(&json!({"ok": false, "error": err.to_string()})).unwrap();
                return respond(stream, 200, "application/json", &body);
            }
            let on_complete = match on_complete {
                Some(raw) => match queue::validate_on_complete(&raw) {
                    Ok(action) => action,
                    Err(err) => {
                        let body =
                            serde_json::to_vec(&json!({"ok": false, "error": err.to_string()}))
                                .unwrap();
                        return respond(stream, 200, "application/json", &body);
                    }
                },
                None => None,
            };
            let (reply, reply_rx) = mpsc::channel();
            if tx
                .send(Ticket::Add {
                    url,
                    name,
                    referer,
                    start_at,
                    on_complete,
                    reply,
                })
                .is_err()
            {
                return respond(
                    stream,
                    200,
                    "application/json",
                    br#"{"ok":false,"error":"queue unavailable"}"#,
                );
            }
            let body = match reply_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(id)) => serde_json::to_vec(&json!({"ok": true, "id": id})).unwrap(),
                Ok(Err(err)) => serde_json::to_vec(&json!({"ok": false, "error": err})).unwrap(),
                Err(_) => br#"{"ok":false,"error":"queue busy"}"#.to_vec(),
            };
            respond(stream, 200, "application/json", &body)
        }
        ("POST", "/api/pause") | ("POST", "/api/resume") | ("POST", "/api/remove") => {
            let value: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => {
                    return respond(
                        stream,
                        400,
                        "application/json",
                        br#"{"ok":false,"error":"bad json"}"#,
                    );
                }
            };
            let Some(id) = value.get("id").and_then(Value::as_str) else {
                return respond(
                    stream,
                    400,
                    "application/json",
                    br#"{"ok":false,"error":"missing id"}"#,
                );
            };
            let ticket = match request.path.as_str() {
                "/api/pause" => Ticket::Pause(id.to_string()),
                "/api/resume" => Ticket::Resume(id.to_string()),
                _ => Ticket::Remove(id.to_string()),
            };
            let _ = tx.send(ticket);
            respond(stream, 200, "application/json", br#"{"ok":true}"#)
        }
        ("POST", "/api/reveal") => {
            let value: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => {
                    return respond(
                        stream,
                        400,
                        "application/json",
                        br#"{"ok":false,"error":"bad json"}"#,
                    );
                }
            };
            let Some(id) = value.get("id").and_then(Value::as_str) else {
                return respond(
                    stream,
                    400,
                    "application/json",
                    br#"{"ok":false,"error":"missing id"}"#,
                );
            };
            let raw_mode = value.get("mode").and_then(Value::as_str).unwrap_or("open");
            let mode = match raw_mode.parse::<actions::RevealMode>() {
                Ok(m) => m,
                Err(err) => {
                    let body = serde_json::to_vec(&json!({"ok": false, "error": err})).unwrap();
                    return respond(stream, 400, "application/json", &body);
                }
            };
            // 客户端交来的 `path` / `name` 之类字段**一律不看**：落点只按 task id 从队列算。
            let (reply, reply_rx) = mpsc::channel();
            if tx
                .send(Ticket::Reveal {
                    id: id.to_string(),
                    mode,
                    reply,
                })
                .is_err()
            {
                return respond(
                    stream,
                    200,
                    "application/json",
                    br#"{"ok":false,"error":"queue unavailable"}"#,
                );
            }
            let body = match reply_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(reply)) => serde_json::to_vec(&reply).unwrap(),
                Ok(Err(err)) => serde_json::to_vec(&json!({"ok": false, "error": err})).unwrap(),
                Err(_) => br#"{"ok":false,"error":"queue busy"}"#.to_vec(),
            };
            respond(stream, 200, "application/json", &body)
        }
        ("GET", "/api/log") => {
            let Some(id) = request
                .query
                .iter()
                .find(|(k, _)| k == "id")
                .map(|(_, v)| v.as_str())
            else {
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers too large",
            ));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request ended before headers",
            ));
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad request line",
        ));
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
    Ok(Request {
        method,
        path,
        query,
        body,
    })
}

pub(crate) fn split_target(target: &str) -> (String, Vec<(String, String)>) {
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

pub(crate) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
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
