//! `dlm-mcp`：把 dlm 的本地控制口包成 **MCP 工具**（0.6.0 阶段 5）。
//!
//! ## 它是什么
//!
//! 一个极小的 MCP server（stdio + 每行一条 JSON-RPC）。它不下载任何东西、不碰队列文件，
//! 只做一件事：把工具的调用**转成对 `dlm-gui` 控制口的 HTTP 请求**。
//!
//! 这样分层有三个好处：
//! - 队列只有一个写入者（runner），MCP 不会与界面抢状态；
//! - token 与端口由 `dlm-gui` 生成并写进 `endpoint.json`，本程序**自己去找**，
//!   用户不用把 token 抄进任何配置文件；
//! - 「Agent 下发要确认」那条规则（`agent_confirm`）在 GUI 侧执行，绕不过去。
//!
//! ## 端点从哪来
//!
//! 1. `--endpoint <路径>`；
//! 2. 环境变量 `DLM_ENDPOINT`；
//! 3. 默认 `%LOCALAPPDATA%\dlm\gui\endpoint.json`（与 `dlm-gui` 的默认值一致）。
//!
//! ## 工具
//!
//! | 工具 | 干什么 |
//! |---|---|
//! | `dlm_add` | 加一个下载；要确认时返回票据 id，而不是任务 id |
//! | `dlm_ticket` | 查票据状态（`pending_approval` / `approved` / `rejected` / `expired`） |
//! | `dlm_list` | 列队列（名字、状态、进度） |
//! | `dlm_status` | 配额与限速现状 |

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

const SERVER_NAME: &str = "dlm-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 与 `dlm-gui` 支持的那版一致：客户端按这个版本协商，不认识就自己降级。
const PROTOCOL_VERSION: &str = "2024-11-05";
const ENDPOINT_ENV: &str = "DLM_ENDPOINT";

#[derive(Debug, Clone, serde::Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
}

fn main() {
    let explicit = resolve_endpoint_path(std::env::args().skip(1).collect());
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            eprintln!("dlm-mcp: 建 HTTP 客户端失败：{err}");
            std::process::exit(1);
        }
    };

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(err) => {
                // 解析不了就没法知道 id；MCP 允许这样直接丢掉。
                eprintln!("dlm-mcp: 收到非 JSON 行（{err}），忽略");
                continue;
            }
        };
        if let Some(reply) = handle(&msg, explicit.as_deref(), &client) {
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
        }
    }
}

/// 端点文件路径：`--endpoint` > `DLM_ENDPOINT` > 默认。
fn resolve_endpoint_path(args: Vec<String>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--endpoint" {
            return it.next().map(PathBuf::from);
        }
        if let Some(rest) = arg.strip_prefix("--endpoint=") {
            return Some(PathBuf::from(rest));
        }
    }
    std::env::var_os(ENDPOINT_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|base| {
                PathBuf::from(base)
                    .join("dlm")
                    .join("gui")
                    .join("endpoint.json")
            })
        })
}

fn read_endpoint(explicit: Option<&std::path::Path>) -> Result<Endpoint, String> {
    let path = explicit
        .map(|p| p.to_path_buf())
        .or_else(|| resolve_endpoint_path(Vec::new()))
        .ok_or_else(|| "找不到端点文件：请先启动 dlm，或用 --endpoint 指定".to_string())?;
    let bytes = std::fs::read(&path)
        .map_err(|err| format!("读不到端点文件 {}（{err}）：请先启动 dlm", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|err| format!("端点文件内容不合法：{err}"))
}

/// 一条 JSON-RPC 消息 → 一条回应（通知类返回 `None`）。
fn handle(
    msg: &Value,
    explicit: Option<&std::path::Path>,
    client: &reqwest::blocking::Client,
) -> Option<String> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    // 没有 id 的是通知（`notifications/initialized` 之类）：不回。
    let id = id?;

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_specs() })),
        "tools/call" => match call_tool(msg, explicit, client) {
            Ok(text) => Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            })),
            Err(err) => Ok(json!({
                "content": [{ "type": "text", "text": err }],
                "isError": true,
            })),
        },
        other => Err(format!("不认识的方法：{other}")),
    };

    let body = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": message },
        }),
    };
    Some(body.to_string())
}

/// 工具清单。参数 schema 手写：只有四个工具，引一个 schema 生成器不值得。
fn tool_specs() -> Value {
    json!([
        {
            "name": "dlm_add",
            "description": "把一个 http(s) 链接加进 dlm 的下载队列。若 dlm 里开着「Agent 下发要确认」，这里会返回票据 id 而不是任务 id —— 那时请回到对话里问用户一句，得到同意后每隔几秒用 dlm_ticket 查状态。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "下载链接，必须 http/https" },
                    "name": { "type": "string", "description": "保存的文件名；留空则从 URL 推" },
                    "referer": { "type": "string", "description": "来源页；有些站点防盗链需要" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "dlm_ticket",
            "description": "查一张下载票据的状态：pending_approval（等人点允许）/ approved（已入队，带回 task_id）/ rejected / expired。",
            "inputSchema": {
                "type": "object",
                "properties": { "ticket": { "type": "string", "description": "dlm_add 返回的票据 id" } },
                "required": ["ticket"]
            }
        },
        {
            "name": "dlm_list",
            "description": "列出 dlm 队列里的任务：名字、状态、进度。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "dlm_status",
            "description": "dlm 此刻的配额与限速情况。",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call_tool(
    msg: &Value,
    explicit: Option<&std::path::Path>,
    client: &reqwest::blocking::Client,
) -> Result<String, String> {
    let name = msg
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let args = msg
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let endpoint = read_endpoint(explicit)?;
    let base = format!("http://127.0.0.1:{}", endpoint.port);
    let token = endpoint.token;

    match name {
        "dlm_add" => {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if url.is_empty() {
                return Err("url 不能为空".to_string());
            }
            let mut body = json!({ "url": url });
            if let Some(n) = args.get("name").and_then(Value::as_str) {
                body["name"] = json!(n);
            }
            if let Some(r) = args.get("referer").and_then(Value::as_str) {
                body["referer"] = json!(r);
            }
            http_call(
                client,
                client
                    .post(format!("{base}/api/agent/add?t={token}"))
                    // `.json()` 要 reqwest 的 `json` 特性（本项目只开了 blocking+rustls），
                    // 所以自己序列化并带 Content-Type。
                    .header("content-type", "application/json")
                    .body(body.to_string()),
            )
        }
        "dlm_ticket" => {
            let ticket = args
                .get("ticket")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if ticket.is_empty() {
                return Err("ticket 不能为空".to_string());
            }
            http_call(
                client,
                client.get(format!("{base}/api/agent/ticket?t={token}&id={ticket}")),
            )
        }
        "dlm_list" => {
            let raw = http_call(client, client.get(format!("{base}/api/tasks?t={token}")))?;
            Ok(summarize_tasks(&raw))
        }
        "dlm_status" => http_call(client, client.get(format!("{base}/api/quota?t={token}"))),
        other => Err(format!("没有这个工具：{other}")),
    }
}

fn http_call(
    _client: &reqwest::blocking::Client,
    req: reqwest::blocking::RequestBuilder,
) -> Result<String, String> {
    let resp = req
        .send()
        .map_err(|err| format!("连不上 dlm-gui 的控制口：{err}（它开着吗？）"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("控制口返回 {status}：{text}"));
    }
    Ok(text)
}

/// 队列 JSON → 给人（和模型）看的几行。
///
/// 直接把 30KB 的 JSON 塞回对话没有意义：这里只保留"还要多久、卡在哪"够用的那几列。
fn summarize_tasks(raw: &str) -> String {
    let Ok(tasks) = serde_json::from_str::<Vec<Value>>(raw) else {
        return raw.to_string();
    };
    if tasks.is_empty() {
        return "队列是空的。".to_string();
    }
    let mut out = String::new();
    for t in &tasks {
        let name = t.get("name").and_then(Value::as_str).unwrap_or("?");
        let status = t.get("status").and_then(Value::as_str).unwrap_or("?");
        let progress = t.get("progress").and_then(Value::as_f64).unwrap_or(0.0);
        let attempts = t.get("attempts").and_then(Value::as_u64).unwrap_or(0);
        out.push_str(&format!("{name} — {status} {progress:.0}%"));
        if attempts > 0 {
            out.push_str(&format!("（重试 {attempts} 次）"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, id: u32) -> String {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        handle(&msg, None, &reqwest::blocking::Client::new()).expect("有 id 的请求必须有回应")
    }

    #[test]
    fn initialize_reports_the_tools_capability() {
        let reply: Value = serde_json::from_str(&call("initialize", 1)).unwrap();
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        assert_eq!(reply["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    /// 通知（没有 id）不回 —— 回了就是协议错误，客户端会当成一条新请求。
    #[test]
    fn notifications_get_no_reply() {
        let msg = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(handle(&msg, None, &reqwest::blocking::Client::new()).is_none());
    }

    #[test]
    fn every_tool_declares_an_input_schema() {
        let reply: Value = serde_json::from_str(&call("tools/list", 2)).unwrap();
        let tools = reply["result"]["tools"].as_array().expect("tools 是数组");
        assert_eq!(tools.len(), 4);
        for t in tools {
            assert!(t["name"].is_string(), "{t}");
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        // `dlm_add` 必须要求 url：缺了它 Agent 会拿一个空链接去撞。
        let add = tools.iter().find(|t| t["name"] == "dlm_add").unwrap();
        assert_eq!(add["inputSchema"]["required"][0], "url");
    }

    #[test]
    fn an_unknown_method_answers_with_an_error_not_a_crash() {
        let reply: Value = serde_json::from_str(&call("nope/nothing", 3)).unwrap();
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn the_endpoint_path_prefers_the_flag_then_the_env() {
        let with_flag = resolve_endpoint_path(vec!["--endpoint".into(), "D:\\x.json".into()]);
        assert_eq!(with_flag, Some(PathBuf::from("D:\\x.json")));
        let eq = resolve_endpoint_path(vec!["--endpoint=D:\\y.json".into()]);
        assert_eq!(eq, Some(PathBuf::from("D:\\y.json")));
    }

    #[test]
    fn a_missing_endpoint_file_says_what_to_do() {
        let err = read_endpoint(Some(std::path::Path::new(
            "Z:\\definitely\\not\\here\\endpoint.json",
        )))
        .unwrap_err();
        assert!(err.contains("请先启动 dlm"), "实际：{err}");
    }

    #[test]
    fn the_task_summary_keeps_the_useful_columns() {
        let raw = json!([
            {"name": "a.zip", "status": "running", "progress": 42.4, "attempts": 0},
            {"name": "b.iso", "status": "queued", "progress": 0.0, "attempts": 2},
        ])
        .to_string();
        let out = summarize_tasks(&raw);
        assert!(out.contains("a.zip — running 42%"), "{out}");
        assert!(out.contains("（重试 2 次）"), "重试要说出来：{out}");
        assert_eq!(summarize_tasks("[]"), "队列是空的。");
    }
}
