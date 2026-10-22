# C1–C3 产品层 Implementation Plan

**状态**：已交付（2026-10-22；四闸 `--lib` 278/0、`--bins` 3/0、integration 107/0、clippy 0；Chrome 155 UI 实测）。
**Spec**：[`../specs/2026-10-22-dlm-product-ui-design.md`](../specs/2026-10-22-dlm-product-ui-design.md)
**Goal**：`dlm-gui` 本地 Web UI + 队列，零新 crate；C1 队列/调度、C2 只读壳、C3 交互。

## Global Constraints

- 四闸 floor；不 push；不 `checkout/reset/stash`；报告先写、提交第二。
- 默认依赖仍 6 个；GUI 只用 `std` + 已有 `serde`/`serde_json`。
- 只绑 `127.0.0.1`；token 校验；队列不存 Cookie；日志/UI 不回显敏感值。
- 旧单任务 CLI/引擎判据一条不动；队列是新产品层。

## File Structure

- Create: `src/queue.rs` —— Task/Status/QueueStore/进度读取 + 单测。
- Create: `src/gui.rs` —— runner actor、HTTP、token、UI HTML + 单测（纯函数/解析）。
- Create: `src/bin/dlm-gui.rs` —— 参数解析 + `gui::run`。
- Modify: `src/lib.rs` —— `pub mod queue; pub mod gui;`
- Modify: `tests/integration.rs` —— API 端到端（add → done → file）。
- Create: `browser/test/verify-gui.mjs` —— 真浏览器添加任务、等 done、截图。
- Docs: C spec/plan、README、STATUS、路线图、证据/报告。

## Task 1: queue.rs

- [x] 数据模型 + `QueueStore::load/save/add/pause/resume/remove`；原子写；重启 running→paused。
- [x] `target_path`/`state_path`/`progress`（读 `.dlm.json`）。
- [x] 单测：add/remove、save/load、running→paused、progress 从 fake state 求和。

## Task 2: gui.rs

- [x] `Command` 通道 + Runner 线程（tick 500 ms、并发、reap、start_at、pause/resume/remove）。
- [x] HTTP：token、`/`、`/api/tasks`、`/api/add`、`/api/pause|resume|remove`、`/api/log`。
- [x] UI HTML：列表/进度/状态/日志/添加/按钮；textContent；每秒轮询。
- [x] `run(config)`：生成 token、启动 runner、绑端口、打印 URL、serve 循环。

## Task 3: 验收

- [x] `tests/integration.rs`：启动 `dlm-gui`（`CARGO_BIN_EXE_dlm-gui`），用 reqwest 调 API；
      node harness `/sz/65536`；add → 轮询 done → 文件 65536 B/内容；remove。
- [x] `browser/test/verify-gui.mjs`：启动 `dlm-gui`，CDP 打开页面，填 URL/name，点添加，
      等 done，截图 `docs/evidence/2026-10-22-c3-gui.png`。
- [x] 四闸：lib/bin/integration/clippy。

## Task 4: 文档/提交

- [x] README 产品层章节；STATUS/路线图 C0–C3；证据/报告。
- [x] 提交 `feat: C1–C3 本地 Web UI 与任务队列`。

## 安全/边界

- 队列只接 public 直链；Cookie/认证仍走 B2。
- 日志 API 只读 log 目录；UI 不 innerHTML 插值；token 只在本进程。
- `dlm-gui` 进程退出 = 队列停跑；不做 Windows 服务。
