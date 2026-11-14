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

/// HTTP 线程要读的那几个槽。打成一包是因为它们**总是一起传**，且参数表已经到顶了。
pub(crate) struct HttpCtx {
    pub snapshot: Arc<Mutex<Vec<TaskView>>>,
    pub quota: Arc<Mutex<Option<QuotaView>>>,
    pub update: Arc<Mutex<UpdateView>>,
    pub log_dir: PathBuf,
    /// Agent 下发的确认闸门（见 `tickets`）。
    pub gate: Arc<Mutex<super::AgentGate>>,
}

pub(crate) fn handle_http(
    stream: &mut TcpStream,
    token: &str,
    tx: &Sender<Ticket>,
    ctx: &HttpCtx,
) -> io::Result<()> {
    let snapshot = &ctx.snapshot;
    let quota_snapshot = &ctx.quota;
    let update_snapshot = &ctx.update;
    let log_dir: &Path = &ctx.log_dir;
    let gate = &ctx.gate;
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
        ("POST", "/api/ext-hello") => {
            // 扩展连上宿主时，把自己**正在跑**的 `manifest.version` 报上来，
            // 宿主转这一发。桌面端据此判断「用户到底重载了扩展没有」——
            // 扩展是「加载已解压」，浏览器不许程序化替换，只能靠它自己说。
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
            let version = value
                .get("version")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(version) = version else {
                return respond(
                    stream,
                    400,
                    "application/json",
                    br#"{"ok":false,"error":"missing version"}"#,
                );
            };
            if let Ok(mut guard) = update_snapshot.lock() {
                guard.extension_running = Some(version.to_string());
            }
            eprintln!("dlm-gui: 扩展上报运行版本 {version}");
            respond(stream, 200, "application/json", br#"{"ok":true}"#)
        }
        // ---- Agent 接入（0.6.0 阶段 5）----
        //
        // 握手：`dlm-mcp` 是单独下载的，启动第一件事就是问这一句 ——
        // 界面太旧时它能说出"请升级 dlm"，而不是抛一句 404。
        ("GET", "/api/agent/hello") => {
            let confirm = gate.lock().map(|g| g.confirm).unwrap_or(true);
            let body = serde_json::to_vec(&json!({
                "ok": true,
                "name": "dlm-gui",
                "api": super::AGENT_API,
                "confirm": confirm,
            }))
            .unwrap_or_default();
            respond(stream, 200, "application/json", &body)
        }
        //
        // 与 `/api/add` 的区别只有一条：`agent_confirm` 开着时不直接建任务，而是挂一张
        // 待确认票据 —— 本地控制口挡得住别的机器，挡不住本机的 Agent。
        ("POST", "/api/agent/add") => {
            let fields = match parse_add_fields(&request.body) {
                Ok(f) => f,
                Err(err) => {
                    return respond(
                        stream,
                        400,
                        "application/json",
                        serde_json::to_vec(&json!({"ok": false, "error": err}))
                            .unwrap_or_default()
                            .as_slice(),
                    );
                }
            };
            let confirm = gate.lock().map(|g| g.confirm).unwrap_or(true);
            if !confirm {
                let body = send_add(tx, fields);
                return respond(stream, 200, "application/json", &body);
            }
            let ticket = {
                let Ok(mut g) = gate.lock() else {
                    return respond(
                        stream,
                        200,
                        "application/json",
                        br#"{"ok":false,"error":"gate busy"}"#,
                    );
                };
                g.tickets.request(
                    &fields.url,
                    &fields.name,
                    fields.referer.clone(),
                    queue::now_secs(),
                )
            };
            let body = serde_json::to_vec(&json!({
                "ok": true,
                "state": "pending_approval",
                "ticket": ticket,
                "message": "已在 dlm 里请求确认：请回到对话里问一句，人点了「允许」才会真正入队",
            }))
            .unwrap_or_default();
            respond(stream, 200, "application/json", &body)
        }
        ("GET", "/api/agent/ticket") => {
            let id = request
                .query
                .iter()
                .find(|(k, _)| k == "id")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let state = gate.lock().ok().and_then(|g| g.tickets.state(&id).cloned());
            let body = match state {
                Some(state) => {
                    serde_json::to_vec(&json!({"ok": true, "ticket": id, "state": state}))
                        .unwrap_or_default()
                }
                None => serde_json::to_vec(
                    &json!({"ok": false, "error": "没有这张票据（可能已被清理或程序重启过）"}),
                )
                .unwrap_or_default(),
            };
            respond(stream, 200, "application/json", &body)
        }
        ("POST", "/api/add") => {
            let fields = match parse_add_fields(&request.body) {
                Ok(f) => f,
                Err(err) => {
                    let body =
                        serde_json::to_vec(&json!({"ok": false, "error": err})).unwrap_or_default();
                    return respond(stream, 200, "application/json", &body);
                }
            };
            let body = send_add(tx, fields);
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

/// `/api/add` 与 `/api/agent/add` 共用的入参。
///
/// 抽出来是为了让"Agent 走的那条路"和"扩展走的那条路"用**同一套解析与校验**：
/// 两条路各写一份的话，早晚会有一条能建出另一条建不出的任务（比如绕过名字校验）。
pub(crate) struct AddFields {
    pub url: String,
    pub name: String,
    pub referer: Option<String>,
    pub start_at: Option<u64>,
    pub on_complete: Option<String>,
}

/// 解析并校验入参。校验规则与 `/api/add` 以前内联的那份完全一致。
pub(crate) fn parse_add_fields(body: &[u8]) -> Result<AddFields, String> {
    let value: Value = serde_json::from_slice(body).map_err(|_| "bad json".to_string())?;
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
        return Err("only http/https".to_string());
    }
    queue::validate_name(&name).map_err(|err| err.to_string())?;
    let on_complete = match on_complete {
        // `validate_on_complete` 自己就返回 `Option<String>`（空/无动作 ⇒ None）。
        Some(raw) => queue::validate_on_complete(&raw).map_err(|err| err.to_string())?,
        None => None,
    };
    Ok(AddFields {
        url,
        name,
        referer,
        start_at,
        on_complete,
    })
}

/// 把入参交给 runner，等它的回执，拼出 `/api/add` 的响应体。
pub(crate) fn send_add(tx: &Sender<Ticket>, fields: AddFields) -> Vec<u8> {
    let (reply, reply_rx) = mpsc::channel();
    if tx
        .send(Ticket::Add {
            url: fields.url,
            name: fields.name,
            referer: fields.referer,
            start_at: fields.start_at,
            on_complete: fields.on_complete,
            reply,
        })
        .is_err()
    {
        return br#"{"ok":false,"error":"queue unavailable"}"#.to_vec();
    }
    match reply_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(id)) => serde_json::to_vec(&json!({"ok": true, "id": id})).unwrap_or_default(),
        Ok(Err(err)) => serde_json::to_vec(&json!({"ok": false, "error": err})).unwrap_or_default(),
        Err(_) => br#"{"ok":false,"error":"queue busy"}"#.to_vec(),
    }
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
