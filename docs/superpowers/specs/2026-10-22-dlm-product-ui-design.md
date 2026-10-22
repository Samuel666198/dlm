# dlm 产品层（C0–C3）设计（2026-10-22）

**C0 裁定：本地 Web UI，零新 crate。** 不选 eframe/Tauri/Win32：它们要么破"6 个直接依赖"、要么引入 webview/构建链；
本地 Web UI 用 `std::net` 手写一个小 HTTP 服务，复用现有浏览器验收通道，默认依赖仍是 6 个。代价：没有原生窗口、
要自己写 HTTP/UI；换来的是可测试、可移植、不破 v1 依赖上限。

## 1. 范围

- **C1 队列**：多任务、并发上限、排队/暂停/恢复/删除、起始时间（A3b 移入）、持久化到
  `%LOCALAPPDATA%\dlm\queue.json`。
- **C2 只读壳**：任务列表、状态、进度、日志。
- **C3 交互**：添加 URL/名称/Referer/可选起始时间、暂停、恢复、删除、查看日志。
- 一个二进制 `dlm-gui`：runner 线程 + 本地 HTTP + 内嵌单页 UI；进程活着 = 调度器活着。
- **不做**：原生窗口、Firefox/Opera 扩展、B3 媒体嗅探、A5 IDM 断点兼容、队列任务的 Cookie/认证。
  认证下载继续走 B2 的一任务一进程入口；队列只接 public 直链。

## 2. 队列模型

```json
{
  "next_id": 3,
  "tasks": [
    {
      "id": "1729...-1",
      "url": "https://example.com/file.bin",
      "name": "file.bin",
      "referer": null,
      "status": "queued|running|paused|done|failed",
      "pid": null,
      "exit_code": null,
      "log": "...\\jobs\\<id>.log",
      "created_at": 1729...,
      "updated_at": 1729...,
      "start_at": null,
      "total_len": null,
      "delivered": null
    }
  ]
}
```

- 原子写：`queue.json.tmp` → rename；重启时把 `running` 改回 `paused`（进程已不在），由用户/自动恢复。
- 任务不存 Cookie；`referer` 原样存（可能含敏感 query，作为已登记边界）。
- 进度：读目标旁 `<target>.dlm.json` 的 `validator.total_len` + `segments` 求和；状态 running 时轮询。
- 起始时间：`start_at`（Unix 秒）为 `Some` 且未到点 ⇒ 不启动；到点自动入 running。UI 用
  `<input type="datetime-local">` 转 epoch，Rust 只比较整数。

## 3. Runner

- 单线程 actor：拥有 `QueueStore`、`HashMap<id, Child>`、命令通道、只读快照 `Arc<Mutex<Vec<TaskView>>>`。
- 每 500 ms tick：reap `try_wait`；更新 done/failed/exit_code；启动 queued 且到点的任务直到并发上限。
- 启动 dlm：`--url <url> --out-dir <root> --out-name <name> --quiet [--referer]`；stdout/stderr 进日志；
  stdin null。**不带 Cookie**。
- 暂停：kill 当前 Child（dlm 的断点保留），状态 paused；恢复：重新入 queued，dlm 从断点续传。
- 删除：kill，删任务与日志；目标文件保留。
- 失败：spawn 失败/非零退出 ⇒ failed；日志保留。
- 目标名字在添加时校验：非空、无 `/` `\`、不是 `.`/`..`（与 dlm 的落点闸同向，不替代它）。

## 4. HTTP + UI

- `TcpListener` 绑 `127.0.0.1:<port>`（`--port 0` 选临时端口）；打印
  `http://127.0.0.1:<port>/?t=<token>`。
- **token**：进程启动时随机生成；所有 `/` 与 `/api/*` 都要求 `?t=` 匹配；只绑回环。
- API：
  - `GET /api/tasks` → 任务快照（含 progress/log）。
  - `POST /api/add` `{url,name,referer?,start_at?}`。
  - `POST /api/pause|resume|remove` `{id}`。
  - `GET /api/log?id=` → 日志尾部（≤64 KiB，只读 log 目录内路径）。
- UI：单页内嵌 HTML/JS；每秒轮询；列表/进度条/状态/日志；添加表单；按钮；无外部资源/CDN。
- 安全：只绑 127.0.0.1；token 防 CSRF；不存 Cookie；日志不回显 Referer/Cookie；UI 不做 innerHTML 插值
  （用 textContent）。

## 5. 验收

- 四闸：`--lib` 队列/资产单测；`--bins` 新增 `dlm-gui` 0 测试；integration 新增一条 API 端到端；
  clippy 0。
- 浏览器：Edge/Chrome 打开 `dlm-gui` 页面，添加一个本地 URL，等 done，截图/日志。
- 手动：暂停/恢复/删除/起始时间各一次；不做自动化也能用 API 覆盖。

## 6. 非目标 / 后续

- 原生 GUI、托盘、开机自启、队列 Cookie、B3 媒体、A5 兼容都不在本轮。
- 队列的"进程活着 = 调度器活着"是产品层事实，不是 Windows 服务。
