# dlm A16 配额恢复 · 队列完成后动作 · B10 浏览器投递入队（设计）

**状态**：已实现（2026-10-27；四闸 294/4/113/clippy 0；Edge 156 真浏览器 queue/direct 验收）
**上游**：路线图 [`../plans/2026-10-21-三路路线图.md`](../plans/2026-10-21-三路路线图.md)、能力对照表 [`../../evidence/2026-10-21-idm能力对照表.md`](../../evidence/2026-10-21-idm能力对照表.md)
**用户裁定（2026-10-27）**：
1. A16 配额＝**全局配额、固定窗口**（队列级，不是每任务）。
2. 队列完成后动作只允许 `open` 与 `run:<exe>`，**`shutdown` 不进队列**。
3. B10 带 Cookie 的浏览器投递**继续走 B2 直连**；只有无 Cookie 的公开直链进队列（保持“队列不存 Cookie”）。
4. `dlm-gui` 没在运行时，浏览器投递**回退直连 B2**，不自动拉起 GUI。

**约束**：仍 6 个直接依赖（`clap`、`reqwest`、`serde`、`serde_json`、`anyhow`、`sha2`）；不 push；四闸为 floor（只许增、不许删、不许改判据方向）；新功能先 spec → plan → 实现。

---

## 1. 范围

本轮只做三件事：

- **A16**：把“配额到量暂停、择时恢复”落成队列级的全局固定窗口配额；内核增加一次运行的字节预算与退出码 3。
- **E4 补全**：队列任务支持完成后动作 `open` / `run:<exe>`。
- **B10**：浏览器投递无 Cookie 时进入队列；带 Cookie 或 GUI 不可达时回退 B2 直连。

**非目标**：滚动窗口、自然日配额、每任务独立配额、队列 `shutdown`、队列 Cookie/认证、自动拉起 `dlm-gui`、B3 媒体嗅探、A5 IDM 断点兼容、Windows 服务/开机自启。

---

## 2. A16 全局配额

### 2.1 记账口径

- **全局**：一个 `dlm-gui` 进程内所有队列任务共享同一份额度；不是每任务一份。
- **固定窗口**：`window_secs` 从本窗口**第一次派发带配额的任务**起算；没有任务时窗口不流逝。到点后整窗重置。不是滚动窗口。
- **计“新交付字节”**：按任务断点覆盖量的增量记账（`queue::progress` 的 `delivered` 差值），不是线上总流量。
  - 新建任务首轮的探针预读**计入**（它确实落盘并进入覆盖集）。
  - 续传时的探针预读**不计入**（按既有裁定，它既不落盘也不进账）。
  - `Retighten` 造成的重复流量**不计入**（既有口径：`duplicated` 不是账单，且 A4 后全速档已降到 0.63%）。
- **停止粒度**：内核按 `--max-bytes` 停，越过预算的那一块最多一整块 `CHUNK_BYTES`（64 KiB）；并发 N 时最坏超出 N×64 KiB。这是文档化边界，不是无界。队列切片下限取 `consts::PROBE_READ_BYTES`（约 236032 B），否则新建任务的首轮预读一落盘就会超出窗口。

### 2.2 内核 `--max-bytes`

新增 CLI 选项：

```
--max-bytes <BYTES>   本次运行最多新交付的字节数（0 = 不限；到量即停并保留断点）
```

- 落点：`cli::Cli.max_bytes: u64`（默认 0），`Cli::into_config` 写入 `engine::Config.max_bytes: u64`。
- **与 `--data-file` 互斥**：带体任务是一次不可续的 POST，字节预算无处停；`--max-bytes` 配 `--data-file` 在参数层拒绝（退出码 2），不进 `engine::run`。
- **执行**：`Engine` 在派发每一枪时把预算切片折成该枪的终点帽：
  - 已有 `run_delivered` 是**本次运行**新交付字节（不含上一轮 `stats.bytes_delivered`），它继续只服务速率计算；新建任务的预读落盘字节另记一格 `budget_base`，**不并进 `run_delivered`**（否则会污染速率分母）。
  - 每个 worker 额外记一格 `budget_share`；派发时 `available = max_bytes - (run_delivered + budget_base) - Σbudget_share`，`share = available / 空闲槽位数`（`available > 0` 时至少给 1 字节，舍入超出 ≤ 空闲槽位数−1 字节）。
  - 该枪的 `fetch::EndCap` 设为 `min(iv.end, iv.start + share - 1)`；`run_window` 既有的“越过帽最多再落一块”语义保持不变。
  - `available == 0` 时不再派发新枪；等在途枪全部交回后返回 `Stop::BudgetReached`。若此时任务其实已经完成，`Done` 优先，退出码 0。
- **结论**：`Report` 新增 `budget_reached: bool`；`Stop::BudgetReached` 不是“盘坏了”，`finish` 照常 `flush` + `save_state`（断点必须保留），`ok = false`，`message` 写“已达到本次运行字节预算（--max-bytes=N）：断点已保留，再次运行会继续”。
- **退出码**：`main` 新增 `BYTE_BUDGET_REACHED = 3`。仅当 `Report.budget_reached && !Report.ok` 时返回 3；任务在预算内完成仍返回 0。
- **不做**：不把预算写进断点、不跨运行累计；每次运行独立。配额窗口的账在队列层。

### 2.3 队列模型

`src/queue.rs`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaState {
    pub limit: u64,            // 每窗字节数；0 = 未启用
    pub window_secs: u64,      // 固定窗口长度，最小 1（生产建议 >= 60）
    pub window_started_at: u64,// Unix 秒；0 = 尚未起算
    pub used: u64,             // 本窗已记账字节
}
```

- `QueueFile` 增加 `#[serde(default)] quota: Option<QuotaState>`。
- `queue::Task` 增加 `#[serde(default)] hold_until: Option<u64>`：配额窗口等待，与用户自己的 `start_at` 分开；`start_at` 语义不变。
- `QueueStore` 增加：
  - `quota(&self) -> Option<QuotaState>`
  - `set_quota(&mut self, limit: u64, window_secs: u64)`（limit 为 0 ⇒ 清空配额与所有 `hold_until`；改额度/窗口 ⇒ `used = 0`、`window_started_at = 0`，下次派发起算）
  - `quota_tick(&mut self, now: u64) -> bool`（`window_started_at == 0` 时不动；否则推进窗口、`used = 0`、释放到点的 `hold_until`）
  - `quota_charge(&mut self, bytes: u64)`
  - `quota_hold(&mut self, id: &str, until: u64)`
  - `delivered(&self, out_dir: &Path, name: &str) -> Option<u64>`：断点可读就用覆盖量，断点已删就用目标文件长度（成功收尾会删 `.dlm.json`）。`TaskView` 与配额采样共用这一处，避免两套口径。
- **迁移**：旧 `queue.json` 没有 `quota` / `hold_until` / `on_complete` ⇒ `serde(default)` 读成 `None`，行为与今天一致。不升版本号。

### 2.4 调度与记账

`src/gui.rs` 的 runner：

- 新状态：`reserved: HashMap<String, u64>`（每个 running 任务预占的切片）、`last_delivered: HashMap<String, u64>`（上次采样到的覆盖量）。
- 每拍 `tick`：
  1. **收样本**：对每个 running 任务读 `QueueStore::delivered(...)`，`delta = now - last_delivered`，`quota_charge(delta)`，更新样本。
  2. **收官**：reap 子进程时再补一次差值（此时断点可能已被成功收尾删掉，`delivered` 会退回读文件长度）；`reserved.remove(id)`。
     - 退出码 `3` ⇒ 任务回到 `Queued`，`exit_code = Some(3)`；若当前 `available < MIN_QUOTA_SLICE` 则 `hold_until = next_reset`，否则 `None`（下一拍可立即再派）。
     - 退出码 `0` ⇒ `Done`；其他 ⇒ `Failed`（与今天一致）。
  3. **窗口**：`quota_tick(now)`；`next_reset = window_started_at + window_secs`（若已过期，按整窗推进到未来）。
  4. **派发**：对每个空闲槽位取一个 `Queued` 且 `start_at <= now`、`hold_until <= now` 的任务。
     - 配额未启用：不传 `--max-bytes`，行为与今天一致。
     - 配额启用：`available = limit - used - Σreserved`，`free_slots = concurrency - children.len()`，`share = available / free_slots`。
     - `share >= MIN_QUOTA_SLICE`（取 `consts::PROBE_READ_BYTES`：新建任务首轮预读要落盘并计入配额，切片小于它就一定超窗）⇒ 子进程参数追加 `--max-bytes <share>`，`reserved.insert(id, share)`。
     - `0 < available < MIN_QUOTA_SLICE` ⇒ 本拍不派，等窗口重置。
     - `MIN_QUOTA_SLICE <= available < free_slots * MIN_QUOTA_SLICE` ⇒ 把 `available` 整份给当前这一个槽位（后续槽位看到 `available = 0`，自然不派）。
- **暂停/删除**：从 `reserved` / `last_delivered` 移除；暂停不额外记账，已落进断点的部分由样本记过。
- **重启**：`recover_running` 仍把 `running → paused`；`reserved` 是进程内状态，重启清空；`quota.used` 与 `window_started_at` 从 `queue.json` 读回。
- **保存节奏**：沿用现有 runner 的“有变化才 `store.save()`”；配额窗口推进、charge、hold 都算变化。

### 2.5 API 与 UI

- `GET /api/quota?t=<token>` ⇒
  ```json
  {"enabled":true,"limit":41943040,"window_secs":3600,"window_started_at":1761600000,"used":12345678,"resets_at":1761603600}
  ```
  未启用时 `{"enabled":false}`。
- `/api/tasks` **保持数组**（既有集成用例按数组解析）；`TaskView` 增加 `hold_until`、`on_complete`。
- `/api/add` 返回值从 `{"ok":true}` 扩成 `{"ok":true,"id":"..."}`（向后兼容；B10 需要 id）。
- UI：
  - 顶部一行配额摘要：`配额 12.3 / 40.0 MiB · 37 分钟后重置`；未启用时显示“配额未启用”。
  - 任务行在 `hold_until` 存在时显示“配额等待至 HH:MM”。
  - 添加表单不新增配额字段（配额是全局的）。

### 2.6 CLI（`dlm-gui`）

```
--quota <BYTES>          每窗口字节上限（0 = 关闭；不给 = 沿用队列里已存的设置）
--quota-window <SECS>    固定窗口秒数（默认 3600；给了 --quota 0 时忽略）
```

- 用 `Option<u64>` 区分“没给”与“显式 0”：没给 ⇒ 沿用 `queue.json`；`Some(0)` ⇒ 关闭并清 `hold_until`。
- 生效额度＝CLI 给了就用 CLI，否则用 `queue.json` 里存的。`--quota-window` 给值但生效额度为 0 ⇒ 参数错误 2。
- `0 < quota < PROBE_READ_BYTES` ⇒ 参数错误 2（小于一个预读块的额度会让新建任务首轮就超窗；要么关掉，要么给足）。窗口给 0 ⇒ 参数错误 2。
- 启用配额后**第一次派发**时把 `window_started_at` 设为当前时刻；没有任务时窗口不流逝。
- 配置写入 `queue.json`；重启不带参数也保持配额。改额度/窗口会把 `used` 清零并等下次派发起算。

### 2.7 边界与已接受代价

- 配额按**断点覆盖量**记账：暂停/杀进程时，最后一拍之后还在途、尚未写进断点的字节不计入本窗口；续传时它们会重新下载并计入下一窗口。
- `dlm-gui` 崩溃/重启：最多漏记最后一拍（500 ms）到崩溃之间的增量；重启后 `used` 以最后一次保存为准。**宁可少记不重复记**。
- 系统时钟大幅回拨会让固定窗口延后；不做单调时钟修正（与 `start_at` 同一立场）。
- 配额不是限速；要同时控带宽仍用 CLI 的 `--max-speed`，队列暂不透传该选项。
- 超出预算的上界是并发的整块数（N×64 KiB），README 照实写。

---

## 3. 队列完成后动作

### 3.1 数据模型

- `queue::Task` 增加 `#[serde(default)] on_complete: Option<String>`；存储原始动作串（`"open"` / `"run:C:\\tools\\scan.exe"`），与 CLI 的 `--on-complete` 同形。
- 旧队列文件没有该字段 ⇒ `None`。

### 3.2 校验

`queue::validate_on_complete(raw: &str) -> Result<Option<String>>`：

- 用 `actions::CompletionAction::from_str` 做唯一解析器；
- `CompletionAction::Open` ⇒ 放行；
- `CompletionAction::Run(_)` ⇒ 放行（路径非空由既有解析器保证）；
- `CompletionAction::Shutdown { .. }` ⇒ 拒绝，错误文案“队列不支持 shutdown，请在 CLI 用 --on-complete shutdown:<秒数>”；
- 其他 ⇒ 既有解析器的错误原样交回。

`/api/add` 接受 `on_complete`（可选字符串）；校验失败返回 `{"ok":false,"error":"..."}`，不入队。

### 3.3 执行

- runner 构造子进程参数时，若有 `on_complete` 就追加 `--on-complete <raw>`；`dlm` 自己在下载成功且校验通过后执行动作。
- 动作失败只由 `dlm` 打警告、不改退出码；队列照常标记 `Done`，日志里能看到警告。
- 下载失败时 `dlm` 不执行动作（既有 `main` 语义）。

### 3.4 安全边界

- `run:<exe>` 是**本机任意程序执行**能力，与 CLI 相同；队列 UI 仍只绑 `127.0.0.1` 且所有路径要求 token。能拿到 token 的本机进程本来就能直接跑 `dlm`，不引入新的信任边界。
- `shutdown` 不进队列：队列可能一次完成多个任务，关机副作用不可控。
- README 写明这条边界。

---

## 4. B10 浏览器投递进队列

### 4.1 端点文件

`dlm-gui` 绑定端口并生成 token 后，写：

```
%LOCALAPPDATA%\dlm\gui\endpoint.json
{"port":51234,"token":"...","pid":1234,"started_at":1761600000}
```

- 原子写（tmp + rename）；目录不存在则创建。
- 新增 `--endpoint <FILE>`（默认上面的路径）；`GuiConfig.endpoint: PathBuf`。
- 写失败只打警告、不阻止 GUI 启动；此时浏览器投递回退直连。
- 进程正常退出时删除；被硬杀留下的陈旧文件由宿主用“连不上 ⇒ 回退”容忍，不依赖文件一定新鲜。
- 同一用户应只跑一个 `dlm-gui`：后启动的会覆盖端点文件，先启动的队列仍在跑但浏览器不再投给它。

### 4.2 宿主决策

`src/host.rs` 的 `Host::download`：

| 条件 | 行为 | 回复 |
|---|---|---|
| 消息里有非空 `cookie` | 走现有 B2 直连（`spawn dlm`，Cookie 走 stdin） | `{op:"accepted",mode:"direct",job,log}` |
| 无 Cookie，端点文件可读且 `/api/add` 返回 200 `{ok:true}` | 交给队列 | `{op:"accepted",mode:"queue",id}` |
| 无 Cookie，队列返回 200 `{ok:false,error}` | 不下载，队列是权威 | `{op:"error",message}` |
| 无 Cookie，端点缺失或**连接被拒** | 回退直连 B2（GUI 没在运行） | `{op:"accepted",mode:"direct",job,log}` |
| 无 Cookie，**连接成功但超时** | 不直连（队列可能已收下，直连会重复下载） | `{op:"error",message:"队列响应超时，未重复下载"}` |
| 无 Cookie，其他非 200（含 token 不符） | 回退直连（该请求没有被队列收下） | `{op:"accepted",mode:"direct",job,log}` |

- HTTP 客户端用已有的 `reqwest::blocking`，`no_proxy()`，连接/总超时 5 秒；只访问 `127.0.0.1`。
- **落点**：队列模式由 `dlm-gui --out-dir` 决定；宿主自己的 `download_dir` 只用于直连回退。回复里用 `mode` 说明走的是哪条路，避免用户以为落点还在浏览器下载目录。
- `HostConfig` 增加 `gui_endpoint: Option<PathBuf>`；默认 `%LOCALAPPDATA%\dlm\gui\endpoint.json`，测试可注入临时路径。
- `/api/add` 的 body 用 `{url,name,referer,start_at:null}`；队列不接收 Cookie。

### 4.3 文件名推导

扩展不传 `name`。队列要求非空且不含 `/ \ :` 与控制字符：

- 用 `reqwest::Url` 取 path 的最后一段；
- 空段或校验不过 ⇒ `download.bin`；
- 不做百分号解码（与 dlm 的落点名裁定一致）。

### 4.4 扩展显示

`browser/extension/background.js` 把回复里的 `mode` 带进页面状态：

- `queue` ⇒ “已加入队列”；
- `direct` ⇒ “已直接下载”；
- `error` ⇒ 现有错误显示。

协议字段向后兼容（旧宿主没有 `mode` 时按 direct 显示）。

### 4.5 安全边界

- 端点文件含 token，位于用户目录；同用户进程可读，这与“能直接 spawn dlm”同一信任边界。网页读不到本地文件。
- 队列仍**不存 Cookie**；带 Cookie 的下载继续走 B2，Cookie 只走 stdin，不进命令线与队列文件。
- 不新增监听端口：复用 `dlm-gui` 已有的环回 HTTP。
- 不自动拉起 GUI：GUI 没开就回退直连，避免宿主制造长期后台进程。

---

## 5. 兼容与迁移

- `queue.json`：新增字段全部 `#[serde(default)]`；旧文件直接可读。
- `endpoint.json`：不存在 ⇒ 直连。
- CLI：`dlm` 新增 `--max-bytes`；`dlm-gui` 新增 `--quota` / `--quota-window` / `--endpoint`；旧调用方式不变。
- 退出码：新增 3；0/1/2 语义不变。
- 既有判据一条不放宽；新增用例只增不减。

---

## 6. 测试策略

**内核（`src/engine.rs` / `src/cli.rs` / `src/main.rs` / `tests/integration.rs`）**

- CLI：`--max-bytes` 进 `Config.max_bytes`；与 `--data-file` 同给 ⇒ 参数错误 2。
- 引擎单测：单 worker、注入小预算 ⇒ 停在 `Stop::BudgetReached`，`Report.budget_reached` 为真，落盘字节 ≤ 预算 + 一块。
- 集成：`--connections 1 --max-bytes 524288` 对 `/sz/1048576` ⇒ 退 3、断点在、目标文件落在预算 + 一块之内；再跑一次不带预算 ⇒ 退 0、文件 1048576 B。（原稿写 65536/131072，但 131072 在预读阶段就被整份读回，没有可停的枪；改为高于 `PROBE_READ_BYTES` 的组合。）
- 装配层：`main::finish` 把 `budget_reached && !ok` 映成 3；任务在预算内完成仍映 0。

**队列（`src/queue.rs` / `src/gui.rs` / `tests/integration.rs`）**

- 单测：旧 `queue.json` 无新字段可读；`quota_tick` 推进窗口并清零；`quota_charge`/`quota_hold` 往返；切片分配函数（含“剩余只够一个槽位”的分支）。
- 集成：`dlm-gui --quota 262144 --quota-window 2 --concurrency 1` 添加 `/sz/524288` ⇒ 先出现退出码 3 / `hold_until`，窗口重置后自动续传至 `done`，文件 524288 B。（原稿 65536 低于 `PROBE_READ_BYTES`，按本 spec §2.6 的参数下限会被拒。）
- 负数/边界：`--quota-window 0` 配启用配额 ⇒ 参数错误；`--quota 0` ⇒ 关闭并清 `hold_until`。

**队列动作**

- 单测：`open` / `run:` 放行，`shutdown:60` 拒绝，未知值拒绝；`Task` 往返保留 `on_complete`。
- 单测：runner 的子进程参数构造（提取纯函数）在 `on_complete` 存在时包含 `--on-complete`，不存在时不包含。
- 集成：`/api/add` 传 `shutdown:60` ⇒ `ok:false` 且队列里没有新任务（不执行关机）。不启动 `explorer.exe` 做真实动作。

**B10**

- 单测：端点文件解析；`derive_name` 的四种形状（正常末段、空段、含冒号、含百分号）；无 Cookie + 可达 ⇒ 队列；有 Cookie ⇒ 直连；不可达 ⇒ 直连；队列 `ok:false` ⇒ 错误。
- 集成：起 `dlm-gui`（临时队列/端点），直接调 `Host::handle` 发一条无 Cookie 的 `download`，轮询 `/api/tasks` 至 `done`，断言落盘文件存在且 `queue.json` 里没有 Cookie 字段。
- 浏览器验收（不在四闸内）：扩 `browser/test/verify-native-download.mjs`，GUI 开着时断言 `mode=queue`；关了 GUI 再投一次，断言 `mode=direct`。

**四闸**：`cargo test --lib`、`cargo test --bins`、`cargo test --test integration`、`cargo clippy --all-targets -- -D warnings`；floor，不写死数字。

---

## 7. 验收标准

1. `dlm --max-bytes` 到量即停、断点保留、退出码 3；再跑能续完；`--data-file` 同给退 2。
2. 队列启用配额后，全局共享一份额度；到量任务回到队列等待，窗口重置后自动续传；重启 `dlm-gui` 后配额与 `used` 仍在。
3. 队列任务可带 `open` / `run:<exe>`；`shutdown` 被拒；动作失败不改任务结果。
4. GUI 开着、无 Cookie 的浏览器投递进队列；有 Cookie 或 GUI 不可达时回退直连；两种路径都不把 Cookie 写进队列文件或日志。
5. 旧 `queue.json` 无需手工迁移；既有四闸全绿。

---

## 8. 不做

- 滚动窗口、自然日配额、每任务配额。
- 队列 `shutdown`、队列 Cookie/认证、自动拉起 GUI。
- 配额透传 `--max-speed`、按域名/分类配额、配额提醒通知。
- B3 媒体嗅探、A5 IDM 断点兼容、Windows 服务/开机自启。
