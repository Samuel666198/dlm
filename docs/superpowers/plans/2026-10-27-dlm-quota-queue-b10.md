# dlm A16 配额 · 队列动作 · B10 投递入队 Implementation Plan

**状态**：已交付（2026-10-27；四闸 294/4/113/clippy 0；Edge 156 真浏览器 queue/direct 验收）。
**Spec**：[`../specs/2026-10-27-dlm-quota-queue-b10-design.md`](../specs/2026-10-27-dlm-quota-queue-b10-design.md)
**目标**：按 spec 落三件事——内核字节预算 + 队列全局固定窗口配额、队列 `open`/`run:<exe>`、B10 无 Cookie 浏览器投递进队列（带 Cookie / GUI 不可达回退 B2）。

## Global Constraints

- 四闸 floor：`cargo test --lib`、`cargo test --bins`、`cargo test --test integration`、`cargo clippy --all-targets -- -D warnings`；只许增、不许删、不许改判据方向。数字不写死，跑一遍看 `0 failed`。
- 仍 6 个直接依赖；不新增 crate。`reqwest::blocking` 已在依赖里，宿主直接复用。
- 不 push；不 `checkout --` / `restore` / `reset --hard` / `stash`；撤销只用反向 Edit + md5。
- 报告先写、提交第二；引用代码用符号名，不用行号；点名用例先 `grep`。
- `queue.json` 新字段一律 `#[serde(default)]`；旧文件必须直接可读。
- 队列仍**不存 Cookie**；带 Cookie 的下载走 B2 直连。
- 每个任务结束先跑四闸，再本地提交（仅本地）；不 push。

## File Structure

**Modify**

- `src/cli.rs`：`Cli.max_bytes`、`--max-bytes`、边界校验、`into_config`、Debug。
- `src/engine.rs`：`Config.max_bytes`、`Stop::BudgetReached`、`Report.budget_reached`、预算切片与停止逻辑。
- `src/main.rs`：退出码 3、`finish` 映射、装配层用例。
- `src/report.rs`：`Report` 字面量与测试 helper 补新字段（不新增渲染逻辑）。
- `src/queue.rs`：`QuotaState`、`Task.hold_until`、`Task.on_complete`、校验与 store 方法。
- `src/gui.rs`：`GuiConfig` 配额/端点、runner 记账与切片、`/api/quota`、`/api/add` 返回 id、`TaskView`、UI。
- `src/bin/dlm-gui.rs`：`--quota`、`--quota-window`、`--endpoint`。
- `src/host.rs`：`gui_endpoint`、队列投递、文件名推导、`mode` 回执。
- `tests/integration.rs`：内核预算、队列配额、队列动作、宿主→队列四组端到端。
- `browser/extension/background.js`：显示 `mode`。
- `browser/test/verify-native-download.mjs`：GUI 开着/关着两种投递验收。
- `README.md`、`docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md`、`docs/superpowers/plans/2026-10-21-三路路线图.md`、`docs/evidence/2026-10-21-idm能力对照表.md`：文档同步。
- `docs/evidence/2026-10-27-quota-queue-b10.md`：真实验收记录（新建）。

**Create**

- 本 spec 与 plan（已建）。

---

## Task 0: 开工基线

- 读 `git log --oneline -3`、`git status --porcelain`；确认分支 `sdd/dlm-kernel`、无远程。
- 跑四闸，记录实际输出到本任务报告；若与最近一次基线不一致，先查清再动代码。
- 建 md5 清单（至少 `src/engine.rs`、`src/gui.rs`、`src/queue.rs`、`src/host.rs`、`src/cli.rs`），供撤销变异时复验。
- **验收**：四闸全绿、工作树除既有 CRLF 差异外没有在制品。

---

## Task 1: 内核字节预算 `--max-bytes`（A16 的内核半边）

### Files

`src/cli.rs`、`src/engine.rs`、`src/main.rs`、`src/report.rs`、`tests/integration.rs`

### Steps

1. **CLI**（`src/cli.rs`）
   - `Cli` 增加 `max_bytes: u64`，`#[arg(long, value_name = "BYTES", default_value_t = 0, help = "本次运行最多新交付的字节数（0 = 不限；到量即停并保留断点）")]`。
   - 手写 `Debug` 加上这一格。
   - `Cli::into_config` 写入 `cfg.max_bytes`。
   - 在 `check_boundaries` 那一族里加一支：`--max-bytes > 0` 与 `--data-file` 同给 ⇒ `bail!`（退出码 2），文案点名两者互斥。
2. **引擎**（`src/engine.rs`）
   - `Config` 增加 `pub max_bytes: u64`，`Default` 为 0；`Config` 的 Debug/文档同步。
   - `Stop` 增加 `BudgetReached`。
   - `Report` 增加 `pub budget_reached: bool`；两个 `Report { .. }` 生产构造点都要填（`finish` 按 `Stop::BudgetReached` 填真，`run_whole_once` 填假）。
   - `Engine` 增加 `budget_share: Vec<u64>`、`budget_base: u64`、`budget_exhausted: bool`；`Engine::new` 初始化（`budget_base` 由调用方在新建任务时填入预读落盘字节）。
   - `Engine::dispatch`：
     - `max_bytes == 0` ⇒ 保持现状（`caps[worker] = NO_END_CAP`）。
     - 否则算 `available = max_bytes - (run_delivered + budget_base) - Σbudget_share`，`free_slots = 当前 Idle worker 数`，`share = max(1, available / free_slots)`（`available > 0` 时）。
     - `available == 0` ⇒ `budget_exhausted = true`，不发送、不记录 Dispatched，交回 `None`。
     - 有 share ⇒ `caps[worker] = min(iv.end, iv.start + share - 1)`，`budget_share[worker] = share`。
   - `Engine::on_report`：记账后把 `budget_share[worker]` 归零；若 `run_delivered + budget_base >= max_bytes` ⇒ `budget_exhausted = true`。**已耗尽后只喂事件、不再发新的 `Action::Assign` / `Action::Retighten`**（在途枪照常收尾并交回）。
   - `Engine::drive`：`budget_exhausted` 且 `any_in_flight()` 为假 ⇒ 返回 `Stop::BudgetReached`；若此时 `sched.is_done()` 且盘上已满，`Done` 优先（任务在预算内完成仍算成功）。
   - `run`：新建任务把预读落盘字节写进 `budget_base`（在 `Engine::new` 之后、`drive` 之前直接赋值）；**不要动 `run_delivered`**，它继续只服务速率计算。续传的探针不计数，保持现状。
   - `finish`：`Stop::BudgetReached` 不在 `fatal` 分支；照常 `flush` + `save_state`；`ok = false`；`message = Some("已达到本次运行字节预算（--max-bytes=…）：断点已保留，再次运行会继续")`；`budget_reached = true`。不要删断点、不要跑 sha256。
3. **装配层**（`src/main.rs`）
   - 新增 `const BYTE_BUDGET_REACHED: u8 = 3;`。
   - `finish`：`rep.budget_reached && !ok` ⇒ 3；否则沿用 0/1。
   - 测试 helper `report()` 补 `budget_reached: false`；新增一格断“预算终止映 3、预算内完成仍映 0”。
4. **报告层**（`src/report.rs`）
   - 只补 `Report` 字面量的新字段；`report::outcome` 不改语义（仍只看 `ok`，消息照打）。

### Tests

- `src/cli.rs`：`max_bytes_maps_into_config_and_is_refused_with_data_file`。
- `src/engine.rs`：单 worker、小预算、131072 字节桩 ⇒ `report.budget_reached == true`、`!report.ok`、落盘 ≤ 预算 + `CHUNK_BYTES`、断点文件存在。
- `src/main.rs`（`--bins`）：`the_byte_budget_maps_to_exit_three_and_completion_still_maps_to_zero`。
- `tests/integration.rs`：`the_binary_stops_at_the_byte_budget_and_resumes_to_completion`——`/sz/131072`、`--connections 1 --max-bytes 65536` ⇒ 退 3、目标文件约 65536 B、`.dlm.json` 在；再跑不带预算 ⇒ 退 0、文件 131072 B 且内容正确。
- `tests/integration.rs`：`max_bytes_with_data_file_is_a_parameter_error`——退 2、对端零请求。

### Acceptance

- 只有“预算到量且没下完”退 3；任务在预算内完成仍退 0；其他退出码语义不变。
- 预算停止时断点保留、可续传；`--data-file` 与 `--max-bytes` 同给退 2。
- 四闸全绿。

### Notes

- 预算按**本次运行新交付字节**计，不是文件总长；续传任务从 0 起算。
- 终点帽沿用 `fetch::EndCap`，`run_window` 既有“越过帽最多再落一块”的语义不动。
- 这一发只碰内核与 CLI，不碰队列。

---

## Task 2: 队列全局固定窗口配额（A16 的队列半边）

### Files

`src/queue.rs`、`src/gui.rs`、`src/bin/dlm-gui.rs`、`tests/integration.rs`

### Steps

1. **数据模型**（`src/queue.rs`）
   - 新增 `pub struct QuotaState { limit, window_secs, window_started_at, used }`（`Serialize`/`Deserialize`/`Copy`）。
   - `QueueFile` 增加 `#[serde(default)] quota: Option<QuotaState>`。
   - `Task` 增加 `#[serde(default)] pub hold_until: Option<u64>`；`add` 初始化为 `None`。
   - `QueueStore` 增加：`quota()`、`set_quota(limit, window_secs)`、`quota_tick(now)`、`quota_charge(bytes)`、`quota_hold(id, until)`、`quota_next_reset()`、`delivered(out_dir, name)`。
   - `set_quota(0, _)` ⇒ 清空 `quota` 与所有 `hold_until`；改额度/窗口 ⇒ `used = 0`、`window_started_at = 0`，下次派发起算。
   - `quota_tick` 在 `window_started_at == 0` 时不动；第一次派发带配额的任务时才把 `window_started_at` 设为 `now`。
   - `delivered(out_dir, name)`：断点可读就用覆盖量，断点已删就用目标文件长度；`TaskView` 与配额采样共用这一处。
2. **CLI**（`src/bin/dlm-gui.rs`）
   - `--quota <BYTES>`、`--quota-window <SECS>`，都用 `Option<u64>` 区分“没给”与显式 0。
   - 生效额度＝CLI 给了就用 CLI，否则用 `queue.json` 里存的。
   - `--quota-window` 给值但生效额度为 0 ⇒ 参数错误 2；`0 < quota < consts::PROBE_READ_BYTES` ⇒ 参数错误 2；窗口给 0 ⇒ 参数错误 2。
   - 启用配额且窗口为 0 ⇒ 默认 3600。
   - 启动时：有 `--quota` ⇒ 覆盖 `queue.json`；配置写回 `queue.json`。
3. **Runner**（`src/gui.rs`）
   - `GuiConfig` 增加 `quota_limit: Option<u64>`、`quota_window_secs: Option<u64>`（或启动时已折进 store，二选一，保持一处来源）。
   - 新状态 `reserved: HashMap<String, u64>`、`last_delivered: HashMap<String, u64>`。
   - `tick` 顺序：
     1. 采样 running 任务：`QueueStore::delivered` 的差值 ⇒ `quota_charge`；更新 `last_delivered`。
     2. reap：再补一次差值（成功收尾会删断点，`delivered` 退回读文件长度）；`reserved.remove`；退出码 3 ⇒ `Queued`，`exit_code = Some(3)`，按剩余额度决定 `hold_until`；0 ⇒ `Done`；其他 ⇒ `Failed`。
     3. `quota_tick(now)`：推进窗口、清零 `used`、释放到点的 `hold_until`。
     4. 派发：对空闲槽位，跳过 `start_at`/`hold_until` 未到的任务；配额启用时若 `window_started_at == 0` 先设为 `now`，再算 `available = limit - used - Σreserved`、`free_slots = concurrency - children.len()`、`share = available / free_slots`。
        - `share >= MIN_QUOTA_SLICE`（取 `consts::PROBE_READ_BYTES`）⇒ 子进程加 `--max-bytes <share>`，`reserved.insert`，`last_delivered.insert`。
        - `0 < available < MIN_QUOTA_SLICE` ⇒ 本拍不派。
        - `MIN_QUOTA_SLICE <= available < free_slots * MIN_QUOTA_SLICE` ⇒ 把 `available` 整份给当前槽位（后续槽位看到 0）。
        - 配额未启用 ⇒ 不传 `--max-bytes`，行为与今天一致。
   - 采样差值用 `saturating_sub`；进度回退（例如目标文件被截断/从头重下）时只更新样本、不扣减 `used`。
   - 暂停/删除：移除 `reserved` / `last_delivered`；`hold_until` 在恢复时清空。
   - 保存：沿用“有变化才 save”；窗口推进、charge、hold、reserved 变化都算变化。
   - 提取纯函数 `quota_next_reset(now, quota)` 与 `quota_slice(available, free_slots)` 供单测。
   - 在 `gui.rs` 定义 `const MIN_QUOTA_SLICE: u64 = consts::PROBE_READ_BYTES;`，不要在 CLI 校验与派发两处各写一个字面量。
4. **API/UI**（`src/gui.rs`）
   - `TaskView` 增加 `hold_until`；`/api/tasks` 仍是数组。
   - 新增 `GET /api/quota`（token 必需），返回 `{enabled,limit,window_secs,window_started_at,used,resets_at}`；未启用 `{enabled:false}`。
   - UI 顶部配额摘要；任务行显示“配额等待至 HH:MM”；每秒拉一次 `/api/quota`。

### Tests

- `queue::tests`：`old_queue_without_quota_still_loads`、`quota_window_resets_and_releases_holds`、`quota_charge_and_hold_roundtrip`。
- `gui::tests`：`quota_slice_prefers_one_slot_when_only_one_slice_fits`、`quota_next_reset_advances_whole_windows`。
- `tests/integration.rs`：`gui_quota_stops_and_resumes_across_a_window`——`--quota 65536 --quota-window 2 --concurrency 1`，`/sz/131072`；先观察到任务退出码 3 / `hold_until`，窗口重置后自动续传至 `done`，文件 131072 B，日志含预算消息。
- `tests/integration.rs`：`gui_quota_windows_are_validated`——`--quota 1024 --quota-window 0` 退 2。

### Acceptance

- 全局共享一份额度；到量任务回队列等待，窗口重置后自动续传。
- `dlm-gui` 重启后 `quota.used` 与 `window_started_at` 仍在；旧 `queue.json` 可直接读。
- 配额关闭时行为与今天逐字节一致。
- 四闸全绿。

### Notes

- 配额按断点覆盖量记账；暂停时在途未落进断点的字节不计入，续传时重新计入。
- 停止粒度 ≤ 每 worker 一块；README 照实写 N×64 KiB 上界。
- 不与 `--max-speed` 混用；队列暂不透传限速。

---

## Task 3: 队列完成任务动作 `open` / `run:<exe>`

### Files

`src/queue.rs`、`src/gui.rs`、`tests/integration.rs`

### Steps

1. **数据模型**（`src/queue.rs`）
   - `Task` 增加 `#[serde(default)] pub on_complete: Option<String>`。
   - `QueueStore::add` 增加 `on_complete: Option<String>` 参数；旧调用点同步。
   - 新增 `pub fn validate_on_complete(raw: &str) -> Result<Option<String>>`：
     - 用 `actions::CompletionAction::from_str` 解析；
     - `Open` / `Run(_)` ⇒ `Ok(Some(raw.to_string()))`；
     - `Shutdown { .. }` ⇒ `bail!("队列不支持 shutdown，请在 CLI 用 --on-complete shutdown:<秒数>")`；
     - 解析错误原样 `bail!`。
2. **API**（`src/gui.rs`）
   - `Ticket::Add` 增加 `on_complete: Option<String>`。
   - `/api/add` 读取 `on_complete` 字符串，先 `queue::validate_on_complete`，失败返回 `{"ok":false,"error":...}`，不入队。
   - 成功返回 `{"ok":true,"id":...}`（顺带给 B10 用；Task 4 依赖）。
3. **Runner**（`src/gui.rs`）
   - 提取纯函数 `fn child_args(task: &Task, out_dir: &Path, quota_slice: Option<u64>) -> Vec<OsString>`；`tick` 用它构造 `Command`。
   - `on_complete` 存在 ⇒ 追加 `--on-complete <raw>`；配额切片存在 ⇒ 追加 `--max-bytes <share>`（与 Task 2 合并）。
4. **UI**（`src/gui.rs` 的 `INDEX_HTML`）
   - `TaskView` 增加 `on_complete`（原始串）。
   - 添加表单增加一个 `select`：无 / 打开 / 运行程序；选“运行程序”时显示一个文本输入（exe 路径）。
   - 提交时带 `on_complete`；用 `textContent`，不插 HTML。
   - 任务行显示动作（可选，显示原始串即可）。

### Tests

- `queue::tests`：`queue_accepts_open_and_run_but_rejects_shutdown`、`on_complete_survives_a_queue_roundtrip`。
- `gui::tests`：`child_args_carry_the_completion_action_and_the_quota_slice`（有/无两臂）。
- `tests/integration.rs`：`gui_rejects_shutdown_completion_action`——`/api/add` 传 `shutdown:60` ⇒ `ok:false`，队列没有新任务，且没有任何关机动作。
- 不启动 `explorer.exe` / 真实程序做动作；执行面由既有 CLI 用例覆盖。

### Acceptance

- `open` / `run:<exe>` 可从 UI/API 入队并传给子进程；`shutdown` 被拒。
- 旧 `queue.json` 无该字段可读；动作失败不改变任务结果。
- 四闸全绿。

---

## Task 4: B10 浏览器投递进队列

### Files

`src/gui.rs`、`src/bin/dlm-gui.rs`、`src/host.rs`、`tests/integration.rs`、`browser/extension/background.js`、`browser/test/verify-native-download.mjs`

### Steps

1. **端点文件**（`src/gui.rs` / `src/bin/dlm-gui.rs`）
   - `GuiConfig` 增加 `endpoint: PathBuf`；`--endpoint <FILE>`，默认 `%LOCALAPPDATA%\dlm\gui\endpoint.json`。
   - 绑定端口、生成 token 后原子写 `{port,token,pid,started_at}`（tmp + rename）；目录不存在则创建。
   - 正常退出删除；写失败只警告，不阻止启动。
2. **宿主配置**（`src/host.rs`）
   - `HostConfig` 增加 `gui_endpoint: Option<PathBuf>`；默认 `%LOCALAPPDATA%\dlm\gui\endpoint.json`；测试可注入。
3. **宿主决策**
   - `download` 先判 `cookie` 是否非空：非空 ⇒ 现有直连路径，回复加 `mode:"direct"`。
   - 无 Cookie ⇒ `try_queue(url, name, referer)`：
     - 读端点文件；缺失 ⇒ `Unavailable`。
     - `reqwest::blocking::Client`，`no_proxy()`，连接/总超时 5s，POST `http://127.0.0.1:<port>/api/add?t=<token>`，body `{url,name,referer,start_at:null}`。
     - 200 `{ok:true,id}` ⇒ `Queue(id)`；200 `{ok:false,error}` ⇒ `Rejected(error)`；连接被拒/其他非 200 ⇒ `Unavailable`；**连接成功但超时** ⇒ `TimedOut`。
   - `Queue` ⇒ `{op:"accepted",mode:"queue",id}`；`Rejected` ⇒ `{op:"error",message}`；`Unavailable` ⇒ 走直连，`mode:"direct"`；`TimedOut` ⇒ `{op:"error",message:"队列响应超时，未重复下载"}`（队列可能已收下，直连会重复）。
   - **落点**：队列模式由 `dlm-gui --out-dir` 决定；宿主自己的 `download_dir` 只用于直连回退，回复里的 `mode` 要能让用户看出走的是哪条路。
4. **文件名推导**
   - 扩展不传 `name`；用 `reqwest::Url` 取 path 最后一段。
   - 空段或 `queue::validate_name` 不过 ⇒ `download.bin`。
   - 不做百分号解码（与 dlm 落点名裁定一致）。
5. **扩展显示**（`browser/extension/background.js`）
   - 在 `submitTask` 的回执分支里显示 `mode`：`queue` ⇒ “已加入队列”；`direct` ⇒ “已直接下载”；缺省按 direct。
6. **浏览器验收**（`browser/test/verify-native-download.mjs`）
   - 第一发：起 `dlm-gui`（临时队列/端点），投递无 Cookie 链接 ⇒ 页面显示“已加入队列”，轮询 `/api/tasks` 至 `done`。
   - 第二发：关掉 GUI，再投一次 ⇒ 显示“已直接下载”，文件落盘。
   - 截图/日志进 `docs/evidence/2026-10-27-quota-queue-b10.md`。

### Tests

- `host::tests`：`derive_name_from_url_shapes`、`host_sends_cookieless_downloads_to_the_queue`（本地 stub HTTP 返回 `{ok:true,id}`）、`host_keeps_cookie_downloads_on_the_direct_path`、`host_falls_back_to_direct_when_the_queue_endpoint_is_unreachable`、`host_returns_queue_rejection_without_falling_back`。
- `gui::tests`：`endpoint_file_contains_the_port_and_token`（临时目录，断言 JSON 三格）。
- `tests/integration.rs`：`host_to_queue_end_to_end`——起 `dlm-gui`（临时队列/端点），直接调 `Host::handle` 发无 Cookie `download`，轮询 `/api/tasks` 至 `done`，断言文件存在、`queue.json` 不含 `cookie` 字段。
- 浏览器验收不在四闸内，单独记录。

### Acceptance

- GUI 开着、无 Cookie ⇒ 进队列；有 Cookie ⇒ 直连；GUI 不可达 ⇒ 直连；队列明确拒绝 ⇒ 返回错误、不直连。
- 队列文件与日志没有 Cookie；端点文件只含 port/token/pid/时间。
- 四闸全绿。

### Notes

- 不新增监听端口；复用 `dlm-gui` 的环回 HTTP。
- 不自动拉起 GUI；宿主不制造长期后台进程。
- 端点文件是同用户可读的 token 文件；与“能直接 spawn dlm”同一信任边界，README 写明。

---

## Task 5: 文档、证据与收口

### Files

`README.md`、`docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md`、`docs/superpowers/plans/2026-10-21-三路路线图.md`、`docs/evidence/2026-10-21-idm能力对照表.md`、`docs/evidence/2026-10-27-quota-queue-b10.md`

### Steps

1. **README**
   - 退出码表加 `3`（预算到量、可续传）。
   - 选项表加 `--max-bytes`。
   - `dlm-gui` 选项表加 `--quota`、`--quota-window`、`--endpoint`。
   - 队列章节：全局配额、`/api/quota`、配额等待、`open`/`run:<exe>`、`shutdown` 不进队列。
   - B10 章节：无 Cookie 进队列、带 Cookie 直连、GUI 不可达回退；端点文件与 token 边界。
   - 已知限制：配额按覆盖量记账、暂停/重启的漏记、N×64 KiB 超出、探针与重复流量不计入。
2. **STATUS**：追加 2026-10-27 一节，记录交付、四闸实测、未做项。
3. **路线图**：A16 标已交付；C3 的“完成后动作（队列）”标已交付；B10 标已交付；剩余 B3/A5、分类规则、更新检查照旧。
4. **能力对照表**：§10 追加增量——A16 已覆盖（全局配额）、E4 队列动作已覆盖、B10 本地队列入口已覆盖、A15 无限重试仍未做。
5. **证据**：`docs/evidence/2026-10-27-quota-queue-b10.md` 记录：
   - 内核预算集成用例的退出码与文件大小；
   - 队列配额窗口的 stop→resume 日志；
   - 浏览器两发（queue/direct）的截图与日志；
   - 明确写“四闸数字随发漂移，跑一遍看 0 failed”。
6. **最终复核**：四闸 + `git status` + `git diff --stat`；确认没有改判据方向、没有新增依赖、没有 push。

### Acceptance

- README/STATUS/路线图/能力表与实际行为一致；证据文件里的数字都来自真跑。
- 四闸全绿；工作树只含本轮预期改动；未 push。

---

## 顺序与依赖

1. **Task 0** 基线。
2. **Task 1** 内核预算（Task 2 依赖退出码 3）。
3. **Task 2** 队列配额（依赖 Task 1；与 Task 3 都改 `gui.rs`，必须串行）。
4. **Task 3** 队列动作（依赖 Task 2 的 `child_args` 形状；顺带把 `/api/add` 返回 id 定下来）。
5. **Task 4** B10（依赖 `/api/add` 返回 id 与 `GuiConfig.endpoint`）。
6. **Task 5** 文档与收口。

**不要并发派发 2/3/4**：三个任务都碰 `src/gui.rs`，并发会互相覆盖。Task 1 与 Task 3/4 也不建议并发（`cli.rs`/`engine.rs`/`gui.rs` 有交叉阅读成本）。

## 检查点与回滚

- 每个 Task 结束：四闸 → 写报告 → 本地提交（仅本地，不 push）。
- 若某 Task 失败：保留工作树，先 `cp` 到 `.superpowers/sdd/` 并两侧 md5 复验；不要 `git checkout --` / `reset --hard` / `stash`。
- 变异验证只在当前树上做，结论过夜前重跑。

## 完成定义

- spec §7 的 5 条验收全部有可指到的用例或真实验收记录。
- 四闸全绿（floor 不降）。
- `queue.json` 旧文件可直接读；旧 CLI/GUI 调用方式不变。
- README/STATUS/路线图/能力表/证据五处同步。
- 未 push；没有新增直接依赖。

## 交付附记（2026-10-27）

- 计划 Task 2 的 `--quota 65536` / `/sz/131072` 与 spec 的"配额下限取 `PROBE_READ_BYTES`"冲突：
  131072 B 文件在预读阶段就被整份读回并落盘，引擎没有可停的枪。实际集成用例改为
  `--quota 262144 --quota-window 2` + `/sz/524288`，日志能看到至少一轮预算停住。
- Task 1 的 `/sz/131072` + `--max-bytes 65536` 同理；实际用 `/sz/1048576` + `--max-bytes 524288`。
- B10 浏览器验收用新脚本 `browser/test/verify-queue-download.mjs`：B2 的页面带 `sid=42` Cookie，
  天然走直连，无法同时演"无 Cookie 入队"；新脚本用无 Cookie 页面覆盖 queue → 杀 GUI → direct 两种模式。
- `queue::set_quota` 只在额度/窗口真的变化时重置窗口；同参数重启保留 `used`/`window_started_at`。
- 队列任务日志改为 append，跨窗口/暂停保留历史（否则预算停止那一轮的结论话会被下一轮覆盖）。
