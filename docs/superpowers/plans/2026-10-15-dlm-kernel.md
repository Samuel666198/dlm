# dlm 内核 v1 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 Rust 实现一个多连接分段下载内核 + CLI，分段调度是零 I/O 的纯状态机，行为可表驱动测试。

**Architecture:** 每段一个持久 worker 线程，领"窗口化子请求"（读满 `WINDOW_BYTES` 或整请求超时即主动断开），把剩余区间交回纯状态机 `Scheduler`；`Scheduler` 唯一决定"谁下哪段区间"，劈分规则是"最长区间的中点"，下限 `MIN_SEGMENT_BYTES`。落盘只有一个 `Mutex<File>`，按偏移写，文件长度随写入自然延长。

**Tech Stack:** rustc 1.95（**必须 msvc 工具链**）、reqwest 0.13.5（blocking + rustls）、clap 4（derive）、serde/serde_json、anyhow、sha2、node（测试对端 harness）。

**Spec:** `docs/superpowers/specs/2026-10-15-dlm-kernel-design.md`（含文末勘误 R1–R5，**正文与勘误冲突时以勘误为准**）

## Global Constraints

- 工具链：`rust-toolchain.toml` 钉 `stable-x86_64-pc-windows-msvc`；**不得改用户全局 rustup default**。
- 依赖只允许：`reqwest = { version = "0.13.5", default-features = false, features = ["blocking", "rustls"] }`、`clap`（features `derive`）、`serde`（features `derive`）、`serde_json`、`anyhow`、`sha2`。不加其它依赖。
- 常量默认值：`PROBE_READ_BYTES=236032`、`MIN_SEGMENT_BYTES=16384`、`DEFAULT_CONNECTIONS=8`、`WINDOW_BYTES=4194304`、`FAST_RETRY_MAX=5`、`SLOW_RETRY_INTERVAL=30s`。前三个的来源强度见 spec §3 与勘误 R1。
- 断点文件 `<目标>.dlm.json` **绝不写入口令**；`http://` + `password` 直接报错退出。
- 写文件一律 tmp + rename 原子替换。
- 单测放模块内 `#[cfg(test)] mod tests`（Rust 惯例），跨模块/真网络验证放 `tests/`。
- 每个任务结尾必须 `cargo test` 全绿再提交；提交信息用 `feat:`/`test:`/`fix:`/`docs:` 前缀。
- 不 push、不建远程仓库（用户没要求）。

## 文件结构

```
dlm/
  rust-toolchain.toml            Task 0
  Cargo.toml                     Task 0
  src/main.rs                    Task 9（组装：cli → probe → engine）
  src/consts.rs                  Task 1
  src/task.rs                    Task 1  任务模型 + 校验
  src/scheduler.rs               Task 2  纯状态机（核心，零 I/O）
  src/store.rs                   Task 3  按偏移落盘
  src/state.rs                   Task 4  断点文件
  src/resume.rs                  Task 5  续传判定
  src/probe.rs                   Task 6  无 Range 试探
  src/fetch.rs                   Task 7  窗口化子请求
  src/engine.rs                  Task 8  worker 线程 + 事件循环
  src/cli.rs                     Task 9  参数 + IDM 斜杠别名
  src/report.rs                  Task 9  一行进度
  tests/harness/probe-server.mjs Task 0（从 idm-recon 复制并按需修）
  tests/integration.rs           Task 6/7/8/10 追加
```

依赖方向：`main → cli, probe, engine → scheduler, fetch, store, state, resume, report`。`scheduler` 不依赖任何 I/O 模块。

---

### Task 0: 工具链钉死 + 依赖基线 + 416 缺陷根因

这一步专门处理 spec 勘误 R2/R3/R4：不钉工具链整个项目编不过，416 不弄明白后面 §7 的集成测试是假承诺。

**Files:**
- Create: `rust-toolchain.toml`, `Cargo.toml`, `src/main.rs`（占位 `fn main() {}`）
- Create: `tests/harness/probe-server.mjs`（复制 `projects/tool/idm-recon/tmp/probe-server.mjs`）
- Create: `tests/harness_api_probe.rs`（临时探针，Task 0 结束后删除）

**Interfaces:**
- Consumes: 无
- Produces: 可编译的空 crate；`probe-server.mjs` 的 `GET /blob`、`GET /sz/<n>`、`GET /flaky`、`GET /noranges` 四个端点（`/noranges` 本任务新增）

- [ ] **Step 1: 钉工具链并验证**

`rust-toolchain.toml`：

```toml
[toolchain]
channel = "stable-x86_64-pc-windows-msvc"
```

`Cargo.toml`：

```toml
[package]
name = "dlm"
version = "0.1.0"
edition = "2024"

[dependencies]
reqwest = { version = "0.13.5", default-features = false, features = ["blocking", "rustls"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
sha2 = "0.11"
```

Run: `cargo build` （不要加 `+toolchain`，靠 rust-toolchain.toml 生效）
Expected: 编译成功。若出现 `dlltool.exe: program not found` ⇒ rust-toolchain.toml 没生效，
用 `rustup show` 确认 active toolchain 是否 `...-msvc`，**不要**去动全局 default。

- [ ] **Step 2: 给 harness 加 `/noranges` 端点**

在 `tests/harness/probe-server.mjs` 的 `http.createServer` 回调里，在 `rng` 解析**之前**插入：

```js
  if (req.url.startsWith("/noranges")) {
    log("request", { ...head, served: "noranges" });
    res.writeHead(200, { "Content-Type": "application/octet-stream",
      "Content-Length": String(Math.min(sendLen, 1024 * 1024)), "ETag": ETAG });
    // 故意不回 Accept-Ranges，且无视 Range：始终从 0 发满 1 MiB
    res.end(BLOB.subarray(0, Math.min(sendLen, 1024 * 1024)));
    return;
  }
```

Run: `node --check tests/harness/probe-server.mjs`
Expected: 无输出（语法通过）。

- [ ] **Step 3: 复现 416 超时并定位**

写 `tests/harness_api_probe.rs`（`#[ignore]` 标记，手工跑）：

```rust
use std::io::Read;
use std::time::Duration;
// 起 harness: node tests/harness/probe-server.mjs 8123 /tmp/h.jsonl
// 跑: cargo test -- --ignored --nocapture harness_probe
#[test]
#[ignore]
fn harness_probe() -> anyhow::Result<()> {
    let url = "http://127.0.0.1:8123/blob";
    for (name, builder) in [
        ("timeout30_keepalive", reqwest::blocking::Client::builder().timeout(Duration::from_secs(30))),
        ("no_timeout_keepalive", reqwest::blocking::Client::builder()),
        ("pool0", reqwest::blocking::Client::builder().pool_max_idle_per_host(0)),
        ("timeout30_pool0", reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30)).pool_max_idle_per_host(0)),
    ] {
        let c = builder.build()?;
        // 关键：模仿真实用法——第一个请求只读 236032 字节就丢弃
        let r = c.get(url).send()?;
        let mut buf = Vec::new();
        r.take(236032).read_to_end(&mut buf)?;
        drop(buf);
        let seg = c.get(url).header("Range", "bytes=1000000-").send()?;
        let s1 = seg.status().as_u16();
        let cr = seg.headers().get("content-range").cloned();
        let r416 = c.get(url).header("Range", "bytes=9999999999-").send();
        println!("{name:24} seg={s1} cr={cr:?} r416={:?}",
                 r416.map(|x| x.status().as_u16()).map_err(|e| e.to_string()));
    }
    Ok(())
}
```

Run: 起 harness 后 `cargo test -- --ignored --nocapture harness_probe`
Expected: 四行输出。**判定**：若 `pool0` 与 `no_timeout_keepalive` 两行打出 `r416=416`
而 `timeout30_keepalive` 打 `operation timed out` ⇒ 根因是"试探请求只读部分 body 就 drop，
脏连接进池后被复用"。据此定稿：试探请求必须带 `.header("Connection", "close")`，
或全局 `pool_max_idle_per_host(0)`。

- [ ] **Step 4: 把结论固化进代码常量与注释**

依 Step 3 的判定，在 `src/consts.rs` 加一条并写明原因（这是不显然的约束，值得注释）：

```rust
/// 试探请求读完预读量后 body 未读干净，该连接不可再复用。
pub const PROBE_USES_CONNECTION_CLOSE: bool = true;
```

`tests/harness_api_probe.rs` 完成使命后删除（它的结论已进注释与 Task 6/7 的实现约束）。

- [ ] **Step 5: 提交**

```bash
git add rust-toolchain.toml Cargo.toml src tests/harness
git commit -m "build: 钉 msvc 工具链、按实测锁定 reqwest feature，并定位 416 复用脏连接问题"
```

---

### Task 1: 常量与任务模型

**Files:**
- Create: `src/consts.rs`, `src/task.rs`
- Modify: `src/main.rs`（加 `mod consts; mod task;`）

**Interfaces:**
- Consumes: 无
- Produces:
  `task::Task { url: String, referer: Option<String>, cookies: Option<String>, data_file: Option<PathBuf>, user: Option<String>, password: Option<String>, dir: PathBuf, name: Option<String>, flags: u32 }`
  `Task::validate(&self) -> anyhow::Result<()>`、`Task::segmentable(&self) -> bool`、`Task::target_path(&self) -> PathBuf`
  `consts::{PROBE_READ_BYTES, MIN_SEGMENT_BYTES, DEFAULT_CONNECTIONS, WINDOW_BYTES, FAST_RETRY_MAX, SLOW_RETRY_INTERVAL_SECS, FLAG_ADD_ONLY, FLAG_IGNORE_MTIME, FLAG_KEEP_STATE}`

- [ ] **Step 1: 写失败的测试**

`src/task.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn t(url: &str) -> Task {
        Task { url: url.into(), referer: None, cookies: None, data_file: None,
               user: None, password: None, dir: ".".into(), name: None, flags: 0 }
    }
    #[test]
    fn rejects_password_over_plain_http() {
        let mut x = t("http://example.com/a.bin");
        x.password = Some("hunter2".into());
        assert!(x.validate().is_err(), "明文 HTTP 带口令必须拒绝，不许降级发送");
    }
    #[test]
    fn allows_password_over_https() {
        let mut x = t("https://example.com/a.bin");
        x.password = Some("hunter2".into());
        assert!(x.validate().is_ok());
    }
    #[test]
    fn post_body_disables_segmentation() {
        let mut x = t("https://example.com/a.bin");
        assert!(x.segmentable());
        x.data_file = Some("body.bin".into());
        assert!(!x.segmentable(), "带 POST 体时 Range 语义不成立，必须单段");
    }
    #[test]
    fn target_path_uses_name_or_url_basename() {
        let mut x = t("https://example.com/dir/big.zip");
        assert_eq!(x.target_path().file_name().unwrap(), "big.zip");
        x.name = Some("renamed.zip".into());
        assert_eq!(x.target_path().file_name().unwrap(), "renamed.zip");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib task::tests`
Expected: 编译失败（`Task` 未定义）。

- [ ] **Step 3: 实现 `consts.rs` 与 `task.rs`**

`src/consts.rs`：

```rust
pub const PROBE_READ_BYTES: u64 = 236_032;
pub const MIN_SEGMENT_BYTES: u64 = 16_384;
pub const DEFAULT_CONNECTIONS: usize = 8;
pub const WINDOW_BYTES: u64 = 4_194_304;
pub const WINDOW_MIN_TIMEOUT_SECS: u64 = 15;
pub const WINDOW_MAX_TIMEOUT_SECS: u64 = 120;
pub const FAST_RETRY_MAX: u32 = 5;
pub const SLOW_RETRY_INTERVAL_SECS: u64 = 30;
pub const STATE_SAVE_MAX_INTERVAL_SECS: u64 = 5;
/// 试探请求读完预读量后 body 未读干净，该连接不可再复用（见 Task 0 Step 3）。
pub const PROBE_USES_CONNECTION_CLOSE: bool = true;
pub const FLAG_ADD_ONLY: u32 = 1 << 0;
pub const FLAG_IGNORE_MTIME: u32 = 1 << 1;
pub const FLAG_KEEP_STATE: u32 = 1 << 2;
```

`src/task.rs`：

```rust
use std::path::PathBuf;
use crate::consts::{FLAG_IGNORE_MTIME, PROBE_READ_BYTES, MIN_SEGMENT_BYTES, DEFAULT_CONNECTIONS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub url: String,
    pub referer: Option<String>,
    pub cookies: Option<String>,
    pub data_file: Option<PathBuf>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub dir: PathBuf,
    pub name: Option<String>,
    pub flags: u32,
}

impl Task {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.url.is_empty(), "缺少 URL");
        let plain_http = self.url.starts_with("http://");
        anyhow::ensure!(!(plain_http && self.password.is_some()),
            "明文 HTTP 不发送口令：请改用 https:// 或去掉 --password");
        anyhow::ensure!(!(plain_http && self.user.is_some() && self.password.is_some()),
            "明文 HTTP 不发送 Basic 认证凭据");
        Ok(())
    }
    pub fn segmentable(&self) -> bool { self.data_file.is_none() }
    pub fn ignore_mtime(&self) -> bool { self.flags & FLAG_IGNORE_MTIME != 0 }
    pub fn target_path(&self) -> PathBuf {
        let name = self.name.clone().unwrap_or_else(|| {
            let tail = self.url.split(['?', '#']).next().unwrap_or(&self.url);
            tail.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("download").to_string()
        });
        self.dir.join(name)
    }
    /// 供 scheduler 初始化用
    pub fn limits(&self) -> (u64, u64, usize) { (PROBE_READ_BYTES, MIN_SEGMENT_BYTES, DEFAULT_CONNECTIONS) }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 4 passed。

- [ ] **Step 5: 提交**

```bash
git add src/consts.rs src/task.rs src/main.rs
git commit -m "feat: 任务模型与常量，含明文 HTTP 拒发口令与 POST 禁分段"
```

---

### Task 2: Scheduler 纯状态机（核心）

**Files:**
- Create: `src/scheduler.rs`
- Modify: `src/main.rs`（`mod scheduler;`）

**Interfaces:**
- Consumes: `consts::MIN_SEGMENT_BYTES`
- Produces:
  `scheduler::Interval { start: u64, end: u64 }`（闭区间，`impl Interval { fn len(&self) -> u64 }`）
  `scheduler::Event { Connected(usize), Progress { worker: usize, abs_end: u64 }, WindowDone { worker: usize }, Failed { worker: usize }, NoRanges }`
  `scheduler::Action { Assign { worker: usize, iv: Interval }, Retighten { worker: usize, iv: Interval }, Idle, Halt }`
  `Scheduler::new(total: u64, min_seg: u64) -> Scheduler`、
  `Scheduler::on(&mut self, ev: Event) -> Vec<Action>`、
  `Scheduler::is_done(&self) -> bool`、`Scheduler::pending_bytes(&self) -> u64`、
  `Scheduler::snapshot_intervals(&self) -> Vec<Interval>`（已完成区间，供 state 落盘）

- [ ] **Step 1: 写表驱动失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn s() -> Scheduler { Scheduler::new(1000, 100) }   // total=1000, min_seg=100

    #[test]
    fn first_connection_gets_whole_file() {
        assert_eq!(s().on(Event::Connected(0)),
            vec![Action::Assign { worker: 0, iv: Interval { start: 0, end: 999 } }]);
    }
    #[test]
    fn second_connection_splits_longest_at_midpoint() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        let acts = sc.on(Event::Connected(1));
        // 老段收紧到上半截之前，新连接从中点起
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { worker: 1, iv }
            if iv.start == 500 && iv.end == 999)), "{acts:?}");
        assert!(acts.iter().any(|a| matches!(a, Action::Retighten { worker: 0, iv }
            if iv.start == 0 && iv.end == 499)), "{acts:?}");
    }
    #[test]
    fn split_uses_remaining_part_of_active_segment() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Connected(1));               // 中点 500
        sc.on(Event::Progress { worker: 1, abs_end: 600 });
        let acts = sc.on(Event::Connected(2));    // 最大区间是 [601,999] 还是 [0,499]？取最长
        let assign = acts.iter().find_map(|a| if let Action::Assign { iv, .. } = a { Some(*iv) } else { None })
            .expect("应有新分配");
        let longest = if 999 - 601 > 499 - 0 { Interval { start: 601, end: 999 } } else { Interval { start: 0, end: 499 } };
        assert_eq!(assign.start, (longest.start + longest.end) / 2 + 1 - (((longest.start + longest.end) / 2 + 1) % 1));
        let mid = (longest.start + longest.end) / 2;
        assert!(assign.start == mid || assign.start == mid + 1, "{assign:?} mid={mid}");
    }
    #[test]
    fn stops_splitting_below_min_segment() {
        let mut sc = Scheduler::new(1000, 400);
        sc.on(Event::Connected(0));               // [0,999]
        let acts = sc.on(Event::Connected(1));    // 半长 500 >= 400 可以劈
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { .. })));
        let acts2 = sc.on(Event::Connected(2));   // 再劈会得到 <400 的段 ⇒ 不劈
        assert!(acts2.iter().all(|a| matches!(a, Action::Idle | Action::Halt)), "{acts2:?}");
    }
    #[test]
    fn window_done_returns_remainder_and_reassigns_without_new_connection() {
        let mut sc = s();
        sc.on(Event::Connected(0));               // [0,999]
        sc.on(Event::Progress { worker: 0, abs_end: 300 });
        sc.on(Event::WindowDone { worker: 0 });
        let acts = sc.on(Event::Connected(0));    // 同一 worker 复用，不再需要新连接
        assert!(acts.iter().any(|a| matches!(a, Action::Assign { worker: 0, iv } if iv.start == 301)), "{acts:?}");
    }
    #[test]
    fn no_ranges_forces_single_segment() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::NoRanges);
        let acts = sc.on(Event::Connected(1));
        assert!(acts.iter().all(|a| matches!(a, Action::Idle)), "退化后不得再派第二段: {acts:?}");
    }
    #[test]
    fn failed_returns_interval_for_requeue() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 200 });
        sc.on(Event::Failed { worker: 0 });
        assert_eq!(sc.pending_bytes(), 799);
    }
    #[test]
    fn done_when_nothing_pending() {
        let mut sc = s();
        sc.on(Event::Connected(0));
        sc.on(Event::Progress { worker: 0, abs_end: 999 });
        sc.on(Event::WindowDone { worker: 0 });
        assert!(sc.is_done());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib scheduler`
Expected: 编译失败（`Scheduler` 未定义）。

- [ ] **Step 3: 实现**

```rust
use crate::consts::MIN_SEGMENT_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval { pub start: u64, pub end: u64 }
impl Interval { pub fn len(&self) -> u64 { self.end - self.start + 1 } }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Connected(usize),
    Progress { worker: usize, abs_end: u64 },
    WindowDone { worker: usize },
    Failed { worker: usize },
    NoRanges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Assign { worker: usize, iv: Interval },
    Retighten { worker: usize, iv: Interval },
    Idle,
    Halt,
}

#[derive(Debug)]
struct Active { iv: Interval, cursor: u64 }   // cursor = 已写到的最高绝对偏移

#[derive(Debug)]
pub struct Scheduler {
    total: u64,
    min_seg: u64,
    queue: Vec<Interval>,      // 未开始的区间
    active: Vec<Option<Active>>,
    workers: Vec<Option<usize>>,
    no_ranges: bool,
    done: Vec<Interval>,
}

impl Scheduler {
    pub fn new(total: u64, min_seg: u64) -> Self {
        Self { total, min_seg, queue: vec![Interval { start: 0, end: total.saturating_sub(1) }],
               active: (0..64).map(|_| None).collect(), workers: vec![], no_ranges: false, done: vec![] }
    }
    pub fn is_done(&self) -> bool { self.active.iter().all(|a| a.is_none()) && self.queue.is_empty() }
    pub fn pending_bytes(&self) -> u64 {
        self.queue.iter().map(|i| i.len()).sum::<u64>()
            + self.active.iter().filter_map(|a| a.as_ref())
                .map(|a| a.iv.end.saturating_sub(a.cursor)).sum::<u64>()
    }
    pub fn snapshot_intervals(&self) -> Vec<Interval> { self.done.clone() }

    fn longest(&mut self) -> Option<Interval> {
        let mut best = self.queue.iter().max_by_key(|i| i.len()).copied();
        for a in self.active.iter().flatten() {
            let tail = Interval { start: a.cursor + 1, end: a.iv.end };
            if tail.len() > 0 && best.map(|b| tail.len() > b.len()).unwrap_or(true) { best = Some(tail); }
        }
        best
    }

    fn take_slot(&mut self) -> Option<usize> {
        if self.workers.is_empty() { return Some(0); }
        (0..self.active.len()).find(|i| self.active[*i].is_none())
    }

    pub fn on(&mut self, ev: Event) -> Vec<Action> {
        match ev {
            Event::NoRanges => { self.no_ranges = true; }
            Event::Progress { worker, abs_end } => {
                if let Some(a) = self.active.get_mut(worker).and_then(|x| x.as_mut()) {
                    a.cursor = a.cursor.max(abs_end);
                }
                return vec![];
            }
            Event::WindowDone { worker } => {
                if let Some(Some(a)) = self.active.get(worker).cloned() {
                    if a.cursor + 1 <= a.iv.end {
                        let rem = Interval { start: a.cursor + 1, end: a.iv.end };
                        if a.cursor >= a.iv.end { /* 已下完 */ } else { self.push_sorted(rem); }
                    }
                    if a.cursor >= a.iv.end { self.done.push(a.iv); }
                    self.active[worker] = None;
                }
            }
            Event::Failed { worker } => {
                if let Some(Some(a)) = self.active.get(worker).cloned() {
                    if a.cursor < a.iv.end { self.push_sorted(Interval { start: a.cursor + 1, end: a.iv.end }); }
                    self.active[worker] = None;
                }
            }
            Event::Connected(w) => {
                if self.no_ranges {
                    // 退化：只允许一段在跑
                    if self.active.iter().any(|a| a.is_some()) { return vec![Action::Idle]; }
                    let iv = match self.queue.pop() { Some(iv) => iv, None => return vec![Action::Idle] };
                    self.active[w] = Some(Active { iv, cursor: iv.start.saturating_sub(1) });
                    return vec![Action::Assign { worker: w, iv }];
                }
                let Some(target) = self.longest() else { return vec![Action::Idle] };
                // 找到正在这个区间上干活的 worker（用于收紧终点）
                let incumbent = self.active.iter().position(|a| match a {
                    Some(x) => x.cursor + 1 == target.start && x.iv.end == target.end, _ => None });
                let mut acts = vec![];
                if let Some(inc) = incumbent {
                    if target.len() / 2 < self.min_seg {
                        acts.push(Action::Idle);       // 太小，不劈：等空闲 worker 直接接手别的区间
                    } else {
                        let mid = (target.start + target.end) / 2;
                        if let Some(Some(x)) = self.active.get_mut(inc) { x.iv.end = mid - 1; }
                        let upper = Interval { start: mid, end: target.end };
                        self.active[w] = Some(Active { iv: upper, cursor: upper.start - 1 });
                        acts.push(Action::Retighten { worker: inc, iv: Interval { start: target.start, end: mid - 1 } });
                        acts.push(Action::Assign { worker: w, iv: upper });
                    }
                } else {
                    self.queue.retain(|i| *i != target);
                    self.active[w] = Some(Active { iv: target, cursor: target.start - 1 });
                    acts.push(Action::Assign { worker: w, iv: target });
                }
                acts.retain(|a| !matches!(a, Action::Idle));
                if acts.is_empty() { vec![Action::Idle] } else { acts }
            }
        }
        vec![]
    }

    fn push_sorted(&mut self, iv: Interval) {
        self.queue.retain(|i| i.start != iv.start);
        self.queue.push(iv);
        self.queue.sort_by_key(|i| std::cmp::Reverse(i.len()));
    }
}
```

实现者注意（这三条会在测试里暴露）：
`MIN_SEGMENT_BYTES` 的默认劈分判据用 `target.len() / 2 < min_seg`；
`cursor` 初值是 `start - 1`，所以 `Progress` 必须传"已写到的最高**绝对偏移**"；
`Active.iv.end` 收紧后，该 worker 的在途请求要按 `Action::Retighten` 改发**带显式终点**的新请求。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib scheduler`
Expected: 9 passed。若 `split_uses_remaining_part_of_active_segment` 因 `min_seg` 边界抖动，
调整该用例的 `Scheduler::new` 参数而不是放宽断言方向。

- [ ] **Step 5: 提交**

```bash
git add src/scheduler.rs src/main.rs
git commit -m "feat: 零 I/O 分段调度器（最长区间中点劈分 + 绝对下限 + 退化单段）"
```

---

### Task 3: store 按偏移落盘

**Files:** Create `src/store.rs`；Modify `src/main.rs`

**Interfaces:** Consumes: 无。Produces: `store::Store::open(path:&Path, truncate:bool) -> anyhow::Result<Store>`、`Store::write_at(&self, offset:u64, bytes:&[u8]) -> anyhow::Result<usize>`、`Store::flush()`、`Store::len() -> anyhow::Result<u64>`、`Store::final_path(&self) -> &Path`。内部 `Mutex<File>`（写者唯一，避免多句柄平台差异）。

- [ ] **Step 1: 失败测试**（写高偏移 ⇒ 文件自然延长、空洞读作零）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn write_at_extends_and_zero_fills() -> anyhow::Result<()> {
        let p = std::env::temp_dir().join("dlm-store-probe.bin");
        let _ = std::fs::remove_file(&p);
        let s = Store::open(&p, true)?;
        s.write_at(0, &[9u8; 10])?;
        let n = s.write_at(1_000_000, &[7u8; 4])?;
        assert_eq!(n, 4);
        assert_eq!(s.len()?, 1_000_004, "长度应随写入偏移自然延长");
        let mut b = vec![0u8; 8];
        { use std::io::{Read, Seek, SeekFrom}; let f = &mut *s.file.lock().unwrap();
          f.seek(SeekFrom::Start(500_000))?; f.read_exact(&mut b)?; }
        assert!(b.iter().all(|x| *x == 0), "未写区域读作零");
        s.flush()?;
        std::fs::remove_file(&p)?;
        Ok(())
    }
    #[test]
    fn open_existing_keeps_content() -> anyhow::Result<()> {
        let p = std::env::temp_dir().join("dlm-store-probe2.bin");
        { let s = Store::open(&p, true)?; s.write_at(0, b"abc")?; }
        let s = Store::open(&p, false)?;
        assert_eq!(s.len()?, 3);
        std::fs::remove_file(&p)?;
        Ok(())
    }
}
```

- [ ] **Step 2: 跑 → 失败**：`cargo test --lib store`，预期未定义。
- [ ] **Step 3: 实现**：`pub struct Store { file: Mutex<File>, path: PathBuf }`，`open` 用
`OpenOptions::new().read(true).write(true).create(true)`，`truncate` 为真时 `.truncate(true)`；
`write_at` 加锁 → `seek(SeekFrom::Start(offset))` → `write_all` → 返回字节数；`flush` = `File::flush`。
测试里需要访问 `file`，故 `pub(crate) file: Mutex<File>`。
- [ ] **Step 4: 跑 → 通过**：`cargo test --lib store`，2 passed。
- [ ] **Step 5: 提交**：`git commit -m "feat: 按偏移落盘（单句柄互斥、长度自然延长）"`

---

### Task 4: state 断点文件

**Files:** Create `src/state.rs`；Modify `src/main.rs`

**Interfaces:** Consumes: `task::Task`。Produces: `state::Snapshot { v:u8, task: TaskView, validator: Validator, segments: Vec<SegView>, stats: Stats, needs_secret: bool, saved_at: String }`、`state::save(path:&Path, snap:&Snapshot)`（tmp+rename）、`state::load(path:&Path) -> anyhow::Result<Option<Snapshot>>`、`state::path_for(target:&Path) -> PathBuf`。`Validator { etag:Option<String>, last_modified:Option<String>, total_len:u64, final_url:String, ignore_mtime:bool }`。

- [ ] **Step 1: 失败测试**（口令绝不落盘）

```rust
#[test]
fn password_never_written_to_state() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("dlm-state-probe");
    std::fs::create_dir_all(&dir)?;
    let mut t = crate::task::Task { url:"https://x/a".into(), referer:None, cookies:None,
        data_file:None, user:Some("u".into()), password:Some("s3cret".into()),
        dir: dir.clone(), name:None, flags:0 };
    let p = state::path_for(&t.target_path());
    let snap = Snapshot::from_task(&t, Validator{ etag:None, last_modified:None, total_len:10,
        final_url:t.url.clone(), ignore_mtime:false }, vec![])?;
    assert!(snap.needs_secret);
    save(&p, &snap)?;
    let raw = std::fs::read_to_string(&p)?;
    assert!(!raw.contains("s3cret"), "断点文件里出现了口令");
    assert!(raw.contains("\"needs_secret\": true"));
    std::fs::remove_file(&p)?;
    t.password = None;
    let snap2 = Snapshot::from_task(&t, snap.validator.clone(), vec![])?;
    assert!(!snap2.needs_secret);
    Ok(())
}
#[test]
fn save_is_atomic_and_roundtrips() -> anyhow::Result<()> { /* save→load 相等；且无 .tmp 残留 */ unimplemented!("实现时补齐断言：load 回来 == snap") }
```

`save_is_atomic_and_roundtrips` 的第二个测试必须在实现该任务时**写成真实断言**（保存、加载、
`assert_eq!(loaded, Some(snap))`、`assert!(!path.with_extension("tmp").exists())`），
不得留 `unimplemented!` 进提交。

- [ ] **Step 2: 跑 → 失败**
- [ ] **Step 3: 实现**：`TaskView` 是 `Task` 的 serde 视图，`password` 字段用 `#[serde(skip)]` 或省略字段本身；
`from_task` 里 `needs_secret = task.password.is_some()`；写盘走 `to_string_pretty` + `*.tmp` + `rename`。
- [ ] **Step 4: 跑 → 通过**
- [ ] **Step 5: 提交**：`git commit -m "feat: 断点文件（原子写、口令不落盘、needs_secret 标记）"`

---

### Task 5: resume 判定

**Files:** Create `src/resume.rs`；Modify `src/main.rs`

**Interfaces:** Consumes: `state::Snapshot`、`task::Task`。Produces: `resume::Decision { Fresh{reason:Option<String>}, Resume(Snapshot), Abort(String) }`、`resume::decide(task:&Task, existing:Option<Snapshot>, probe:&ProbeFacts) -> Decision`。`ProbeFacts { etag:Option<String>, last_modified:Option<String>, total_len:u64, final_url:String, accept_ranges:bool }`（Task 6 的 `ProbeResult` 需 `pub fn facts(&self) -> ProbeFacts`）。

- [ ] **Step 1: 失败测试**（表驱动，全分支）

```rust
#[test]
fn decides_across_all_validator_combinations() {
    let base = ProbeFacts { etag: Some("E".into()), last_modified: Some("L".into()),
        total_len: 100, final_url: "https://x/a".into(), accept_ranges: true };
    let snap = |v: Validator| Snapshot::stub_for_test(v, vec![]);
    // 1 无断点 ⇒ Fresh
    assert!(matches!(decide(&task(), None, &base), Decision::Fresh{..}));
    // 2 ETag 变 ⇒ Fresh 且带原因
    let d = decide(&task(), Some(snap(val("E2","L",100,"https://x/a"))), &base);
    assert!(matches!(d, Decision::Fresh{reason:Some(_)}), "{d:?}");
    // 3 只有 Last-Modified 变 + ignore_mtime ⇒ Resume
    assert!(matches!(decide(&task(), Some(snap(val("E","L2",100,"https://x/a")))), Decision::Resume(_)) ,
        "task() 需带 FLAG_IGNORE_MTIME");
    // 4 只有 Last-Modified 变、未开 ignore ⇒ Fresh
    assert!(matches!(decide(&task_plain(), Some(snap(val("E","L2",100,"https://x/a")))), Decision::Fresh{..}));
    // 5 总长变 ⇒ Fresh
    assert!(matches!(decide(&task_plain(), Some(snap(val("E","L",99,"https://x/a")))), Decision::Fresh{..}));
    // 6 needs_secret 且本次没给口令 ⇒ Abort
    let mut s = snap(val("E","L",100,"https://x/a")); s.needs_secret = true;
    assert!(matches!(decide(&task_plain(), Some(s), &base), Decision::Abort(_)));
    // 7 服务器不支持 Range 但断点多段 ⇒ Fresh
    let mut b = base.clone(); b.accept_ranges = false;
    assert!(matches!(decide(&task_plain(), Some(snap_multi(val("E","L",100,"https://x/a"), 2))), Decision::Fresh{..}));
}
```

- [ ] **Step 2: 跑 → 失败**
- [ ] **Step 3: 实现**：判定顺序 = ①`needs_secret && task.password.is_none() ⇒ Abort`；
②`final_url != probe.final_url ⇒ Fresh{原因}`；③ETag 两者都有则只比 ETag，ETag 任一缺失则比 `last_modified`
（除非 `ignore_mtime`）；④`total_len` 不等 ⇒ Fresh；⑤`!accept_ranges && 段数>1 ⇒ Fresh`；⑥否则 `Resume`。
- [ ] **Step 4: 跑 → 通过**
- [ ] **Step 5: 提交**：`git commit -m "feat: 续传判定，含 If-Range 前提与 needs_secret 中止"`

---

### Task 6: probe 试探

**Files:** Create `src/probe.rs`；Modify `src/main.rs`；Append `tests/integration.rs`

**Interfaces:** Consumes: `task::Task`、`consts::{PROBE_READ_BYTES, PROBE_USES_CONNECTION_CLOSE}`。Produces: `probe::ProbeResult { facts: ProbeFacts, prelude: Vec<u8> }`、`probe::run(task:&Task, agent:&reqwest::blocking::Client, prelude_max:u64) -> anyhow::Result<ProbeResult>`

- [ ] **Step 1: 集成测试（真发 HTTP 到 harness）**

```rust
// tests/integration.rs
#[test]
fn probe_reads_prelude_and_reports_ranges_supported() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;                 // 见 Step 2
    let t = dlm::task::Task { url: format!("{base}/blob"), dir: "D:/x".into(), ..harness::blank_task() };
    let r = dlm::probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;
    assert!(r.facts.accept_ranges, "harness 明确回 Accept-Ranges: bytes");
    assert_eq!(r.facts.total_len, 48 * 1024 * 1024);
    assert_eq!(r.prelude.len() as u64, PROBE_READ_BYTES);
    assert_eq!(r.facts.etag.as_deref(), Some("\"probe-f675558c108f46fc\""));
    Ok(())
}
#[test]
fn probe_detects_server_without_ranges() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let t = dlm::task::Task { url: format!("{base}/noranges"), ..harness::blank_task() };
    let r = dlm::probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;
    assert!(!r.facts.accept_ranges);
    Ok(())
}
```

- [ ] **Step 2: 写 `tests/harness/mod.rs`**：用 `std::process::Command::new("node")` 起
`tests/harness/probe-server.mjs <port> <logpath>`，轮询端口可连后返回 `ServerGuard`，
`Drop` 时 kill。`client()` 建 reqwest blocking client：
`.timeout(Duration::from_secs(30)).connect_timeout(Duration::from_secs(3)).tcp_nodelay(true)`，
并且**试探路径必须带 `Connection: close`**（Task 0 Step 3 的结论）。
- [ ] **Step 3: 跑 → 失败**：`cargo test --test integration`，未定义。
- [ ] **Step 4: 实现 `probe.rs`**：`GET` 无 Range，若 `task.segmentable()` 则加
`header(Range, format!("bytes=0-{}", prelude_max-1))` 以**限量而不是靠 drop**（这样连接是干净读满的，
规避 Task 0 的脏连接问题）；取 `accept-ranges/etag/last-modified`，`total_len` 优先从
`content-range` 的 `/N` 解析，退化为 `content-length`；body 读进 `prelude`。
带 `Connection: close`。把 `prelude` 交给 engine 落盘到偏移 0。
- [ ] **Step 5: 跑 → 通过**：`cargo test --test integration`，2 passed。
- [ ] **Step 6: 提交**：`git commit -m "feat: 无 Range 试探取回长度/ETag/预读，退化路径可辨"`

---

### Task 7: fetch 窗口化子请求

**Files:** Create `src/fetch.rs`；Modify `src/main.rs`；Append `tests/integration.rs`

**Interfaces:** Consumes: `scheduler::Interval`、`task::Task`、`store::Store`、`consts::{WINDOW_BYTES, WINDOW_MIN_TIMEOUT_SECS, WINDOW_MAX_TIMEOUT_SECS}`。Produces:
`fetch::Outcome { Advanced { bytes: u64 }, Complete, Stalled, Failed(FailKind) }`、
`fetch::FailKind { Http(u16), Transport, ValidatorChanged }`、
`fetch::run_window(agent:&Client, task:&Task, iv:Interval, store:&Store, rate_bps:u64) -> anyhow::Result<Outcome>`

- [ ] **Step 1: 失败测试**（三条行为）

```rust
#[test]
fn window_writes_at_offset_and_stops_at_window_edge() -> anyhow::Result<()> {
    let (_s, base) = harness::spawn()?;
    let (store, path) = harness::store("w1.bin")?;
    let t = dlm::task::Task { url: format!("{base}/blob"), ..harness::blank_task() };
    let iv = Interval { start: 1_000_000, end: 1_000_000 + WINDOW_BYTES * 2 };
    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000)?;
    assert!(matches!(out, Outcome::Advanced { bytes } if bytes == WINDOW_BYTES), "{out:?}");
    assert_eq!(store.len()?, 1_000_000 + WINDOW_BYTES + 1);
    assert!(harness::bytes_at(&path, 1_000_000, 4) == vec![0u8,0,0,0] || true); // 内容由 harness blob 决定
    harness::require_block_alignment(&path, 1_000_000, WINDOW_BYTES)?;   // 断言真内容：块号对齐
    Ok(())
}
#[test]
fn range_start_open_ended_but_respects_explicit_end() -> anyhow::Result<()> { /* 末段带 end ⇒ 请求头必须含终点 */ unimplemented!("实现时写真实断言：抓 harness 日志断言 range 字符串") }
#[test]
fn flaky_server_caps_length_and_reports_stalled() -> anyhow::Result<()> { /* /flaky 只承认 4MiB：请求超过该长度的区间 ⇒ 收到的字节 < 期望，返回 Advanced 而非 Complete */ unimplemented!("实现时写真实断言") }
```

后两个测试在本任务内**必须写成真实断言**再提交（读 harness 写的 jsonl 或比对字节），
不允许把 `unimplemented!` 带进仓库。

- [ ] **Step 2: 跑 → 失败**
- [ ] **Step 3: 实现**：请求头 = `Range: bytes={start}-{end_if_known}`（`iv.end == total-1` 时发 `bytes={start}-`，
对齐 IDM 的开放终点观察）；`.timeout(窗口预算 = max(WINDOW_MIN, min(WINDOW_MAX, WINDOW_BYTES/rate*2)))`；
带 9 项契约映射出的头：`Referer`、`Cookie`、`Authorization: Basic`（仅 https）、`Accept-Encoding: identity`、
`Accept: */*`；用 `impl Read` 流式读，每 64 KiB `store.write_at(cursor, chunk)` 一次，
累计到 `WINDOW_BYTES` 或读到 `iv.end` 为止；读到 EOF 且 cursor==iv.end ⇒ `Complete`；
请求失败/超时 ⇒ `Stalled`（超时）或 `Failed`；`416` ⇒ 直接返回 `Complete`（spec §6.4）；
`status` 非 206（在要求 Range 时）⇒ `Failed(ValidatorChanged)`。
- [ ] **Step 4: 跑 → 通过**
- [ ] **Step 5: 提交**：`git commit -m "feat: 窗口化子请求，含 9 项契约头、416 与降级判定"`

---

### Task 8: engine 主循环

**Files:** Create `src/engine.rs`；Modify `src/main.rs`；Append `tests/integration.rs`

**Interfaces:** Consumes: 上述全部。Produces: `engine::Config { workers:usize, min_seg:u64, max_retries:Option<u32>, verify_sha256:Option<String>, resume:Decision }`、`engine::run(task:&Task, cfg:Config) -> anyhow::Result<Report>`、`engine::Report { bytes_total:u64, duplicated:u64, retries:u32, elapsed:Duration, ok:bool }`

- [ ] **Step 1: 失败测试**（端到端 + 事件回放一致性 + 完整性）

```rust
#[test]
fn full_download_is_byte_exact_and_ranges_match_scheduler() -> anyhow::Result<()> {
    let (_s, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng1")?;
    let t = dlm::task::Task { url: format!("{base}/sz/{}", 8 * 1024 * 1024),
        dir: dir.clone(), ..harness::blank_task() };
    let rep = engine::run(&t, engine::Config {
        workers: 4, min_seg: MIN_SEGMENT_BYTES, max_retries: None,
        verify_sha256: Some(harness::expected_sha(8 * 1024 * 1024)),
        resume: resume::Decision::Fresh { reason: None } })?;
    assert!(rep.ok, "{rep:?}");
    assert_eq!(harness::sha256_of(&t.target_path()), harness::expected_sha(8 * 1024 * 1024));
    // 事件回放一致性：把 harness 日志里的 Range 序列喂给"重放器"
    let observed = harness::ranges_from_log(&rep.event_log_path)?;   // Vec<(start,end)>
    let predicted = harness::replay(observed.clone(), 8 * 1024 * 1024, MIN_SEGMENT_BYTES);
    assert_eq!(predicted.errors, 0, "有 Range 不是任何合法劈分: {:?}", predicted.errors);
    Ok(())
}
#[test]
fn resume_after_kill_does_not_redownload_done_segments() -> anyhow::Result<()> { /* 跑一半 kill worker → 重跑 → 断言第二段请求的 Range 起点 == 上次已写偏移 */ unimplemented!("实现时写真实断言") }
#[test]
fn no_ranges_falls_back_to_single_worker_and_is_correct() -> anyhow::Result<()> { /* /noranges：结果字节正确且并发数实际=1 */ unimplemented!("实现时写真实断言") }
```

- [ ] **Step 2: 跑 → 失败**
- [ ] **Step 3: 实现**：`mpsc::channel::<SchedulerMsg>`；worker 线程 `loop { recv(Assign) → run_window → send(Event) }`；
主线程持有 `Scheduler`，把事件转成 `Action` 派给对应 worker 通道；维护 `rate_bps` 的滑动平均喂给超时预算；
按 `STATE_SAVE_MAX_INTERVAL_SECS` + 关键事件触发 `state::save`；`stats.duplicated` 累加"被丢弃的已收字节"；
Ctrl-C 用 `tokio`? **不用**——v1 不做信号处理，改为：主循环检查 `ctrlc` 不可用则靠 `SIGINT` 默认终止 + 每次 `save` 原子落盘 ⇒
**本任务只保证"任意时刻被杀都能续"**，`unimplemented` 的那条 kill 测试就是这个的验收。
- [ ] **Step 4: 跑 → 通过**
- [ ] **Step 5: 提交**：`git commit -m "feat: worker 线程池 + 事件循环 + 状态保存节奏"`

---

### Task 9: CLI 与进度输出

**Files:** Create `src/cli.rs`, `src/report.rs`；Modify `src/main.rs`

**Interfaces:** Consumes: `task::Task`、`engine::{Config, run}`。Produces: `cli::parse_from<I:IntoIterator<Item=OsString>>(args:I) -> anyhow::Result<Cli>`、`Cli::into_task(self) -> Task`、`report::render(&ProgressLine)`

- [ ] **Step 1: 失败测试**（别名展开必须等价）

```rust
#[test]
fn idm_style_slash_aliases_expand_to_same_task() -> anyhow::Result<()> {
    let posix = parse_from(["dlm", "--url", "https://x/a.bin", "--out-dir", "D:/d", "--out-name", "b.bin"])?.into_task()?;
    let idm   = parse_from(["dlm", "/d", "https://x/a.bin", "/p", "D:/d", "/f", "b.bin"])?.into_task()?;
    assert_eq!(posix, idm);
    assert_eq!(posix.target_path(), PathBuf::from("D:/d").join("b.bin"));
}
#[test]
fn flag_bits_map_to_task_flags() -> anyhow::Result<()> {
    let c = parse_from(["dlm", "/d", "https://x/a", "/n", "/a", "--keep-state"])?;
    let t = c.into_task()?;
    assert_eq!(t.flags & FLAG_ADD_ONLY, FLAG_ADD_ONLY);
    assert_eq!(t.flags & FLAG_KEEP_STATE, FLAG_KEEP_STATE);
}
#[test]
fn plain_http_with_password_fails_at_parse_time() {
    let r = parse_from(["dlm", "--url", "http://x/a", "--password", "p"]).and_then(|c| c.into_task());
    assert!(r.is_err());
}
```

- [ ] **Step 2: 跑 → 失败**
- [ ] **Step 3: 实现**：`parse_from` 先把首现的 `/d /p /f /n /q /a /h` 就地替换成对应 `--长名`（`/q` → `--quiet`、
`/a` → `--add-only`、`/h` 接受但仅置位、`/n` → 静默位），再交给 clap；`--connections`、`--referer`、`--cookie`、
`--data-file`、`--user`、`--password`、`--ignore-mtime`、`--no-resume`、`--sha256`、`--max-retries`、`--keep-state`、`--quiet`。
`report::render` 一行 `\r 45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4  dup 5.5%`。
- [ ] **Step 4: 跑 → 通过**
- [ ] **Step 5: 提交**：`git commit -m "feat: CLI（POSIX 为主 + IDM 斜杠别名）与一行进度"`

---

### Task 10: 验收跑通 spec §7 四场景

**Files:** Append `tests/integration.rs`；Create `docs/evidence/`（一次性实测记录）

**Interfaces:** Consumes: 全部。Produces: 四条场景测试 + 一份实测记录文档。

- [ ] **Step 1: 写四场景测试**

```rust
#[test] fn scenario_a_fullspeed_48mib_is_byte_exact() -> anyhow::Result<()> { /* /blob 全速, workers=8, sha 比对 */ unimplemented!("本步写真实断言：engine::run + sha256 比对") }
#[test] fn scenario_b_throttled_splits_at_midpoint_with_floor() -> anyhow::Result<()> { /* /slow, 断言存在 [1/2,1/4,1/8] 起点且最小段 >= MIN_SEGMENT_BYTES */ unimplemented!("本步写真实断言") }
#[test] fn scenario_c_flaky_resumes_without_redownload() -> anyhow::Result<()> { /* /flaky 中途断，重跑，断言 dup 字节 < 5% 总量 */ unimplemented!("本步写真实断言") }
#[test] fn scenario_d_no_ranges_single_connection_correct() -> anyhow::Result<()> { /* /noranges，断言并发实际为 1 且字节正确 */ unimplemented!("本步写真实断言") }
```

上面五个 `unimplemented!` 只是骨架占位——**该任务的 Step 1 就是把它们全部换成真实断言**，
不允许以 `unimplemented!` 状态提交。

- [ ] **Step 2: 跑 `cargo test`**（unit + integration 全量）
Expected: 全绿。任一场景红 ⇒ 停下修，不许放宽判据（改测试输入可以，改断言方向不行）。

- [ ] **Step 3: 与 IDM 行为对照（参考，不作门禁）**

同一条 harness 上跑 `MSYS2_ARG_CONV_EXCL='*' "/c/Program Files (x86)/Internet Download Manager/IDMan.exe" /n /d "http://127.0.0.1:8077/slow" /p <dir> /f cmp.bin`，
把两边 Range 序列并排写进 `docs/evidence/idm-vs-dlm-<日期>.md`，
开头必须标注：本机 IDM 构件已被第三方修改、且环回链路观测强度有限。

- [ ] **Step 4: 提交**：`git commit -m "test: spec §7 四场景验收 + 与 IDM 的 Range 序列对照"`

---

## 自检（写完后回看 spec）

- **覆盖**：spec §2 九模块 → Task 1–9；§3 常量 → Task 1；§4 断点结构与安全边界 → Task 4；
  §5 CLI → Task 9；§6 七条错误处理 → Task 5（续传冲突）、7（416/降级/超时）、8（重试与保存）、9（校验失败退出码）；
  §7 三层测试 → Task 2/6/8/10；§8 不做项未进任何任务；§9 依赖 → Task 0。
- **缺口一处，故意留白并说明**：spec §6.6 "Ctrl-C 优雅收尾"在 Task 8 降级为"任意时刻被杀都能续"，
  因为不引入信号处理依赖。若执行时要严格实现 §6.6，需要加 `ctrlc` crate —— 与"只允许这 6 个依赖"冲突，
  **需要用户批准才能扩依赖**。
- **占位符**：全文出现的 `unimplemented!` 都在其所在任务的步骤里被明确要求替换成真实断言后方可提交；无 TBD/TODO。
- **类型一致性**：`Interval/Event/Action/ProbeFacts/Decision/Outcome/Config/Report` 的字段名与签名在定义任务与引用任务间一致
  （`Decision::Fresh{reason}`、`Outcome::Advanced{bytes}`、`Event::Progress{worker, abs_end}`）。

---

## 勘误（Task 0 及其修复轮的实测推翻，任务正文不回改，以此节为准）

同一约定见 spec `docs/superpowers/specs/2026-10-15-dlm-kernel-design.md` §10（R1–**R5**，R5 取代 R4）。
证据：`.superpowers/sdd/2026-10-15-dlm-kernel/task-0-report.md` §3–§4 与其后的「修复轮」（F1–F6）、
「修复轮 2」（G1–G3）与「修复轮 3」（H1–H4）三节。

**E1：Task 0 Step 5 的 `build:` 前缀与计划自身的约束清单冲突。** Global Constraints 第 21 行只允许
`feat:`/`test:`/`fix:`/`docs:`。冲突时**约束清单赢**：已落地的 `70f4d7f` 不回改（底线是"只加新提交、不 amend"），
它的更正提交用 `fix:`，并在提交信息里把被证伪的措辞纠正过来（见 E2/R5）。

**E2：`PROBE_USES_CONNECTION_CLOSE` 整条删除，不带给 Task 1。** 该常量取值 `false`、又不门控任何代码，
是"为一个不会发生的场景留的开关"；它的知识承载在 spec R5 与 `tests/harness/probe-server.mjs` 的注释里。
Task 0 Step 4 建的 `src/consts.rs`（未进模块树，删除后 `cargo build`/`cargo test` 实测仍绿）已随修复轮删掉，
Task 1 自己新建 `src/consts.rs`。跟着作废的引用：
- Task 1 Step 3 代码块：删掉 `PROBE_USES_CONNECTION_CLOSE` 那一行与它上面的文档注释行，其余常量**原样实现**。
  代码块现在写的是 13 条，去掉这条后是 **12 条**；而 Task 1 的 `Produces` 行只列了 9 条（少列了
  `WINDOW_MIN_TIMEOUT_SECS`、`WINDOW_MAX_TIMEOUT_SECS`、`STATE_SAVE_MAX_INTERVAL_SECS` 等）——**以 Step 3 代码块为准**。
- Task 6 `Interfaces` 的 `Consumes` 去掉 `consts::PROBE_USES_CONNECTION_CLOSE`。
- Task 6 Step 2 末句"试探路径必须带 `Connection: close`"、Step 4 末句"带 `Connection: close`"，以及
  Task 6 Step 3 判定段（第 151–152 行）给出的两条补救，**全部作废**：Task 6/7 不得加 `Connection: close`，
  也不得设 `pool_max_idle_per_host(0)`（依据见 R5；加了不致错，只是每条试探多拆一次连接，并把
  "复用连接上打 416"这条真实路径从测试覆盖里关掉）。

**E3：Task 6 增补一条集成用例——同一连接上"读满的 206"后接"416"必须双双完成。** 形状：一个客户端先请求
`bytes=0-<n>` 并把 body **读到 EOF**（连接干净入池），再在**同一连接**上发 `Range: bytes=9999999999-`，
断言拿到 `416` + `Content-Range: bytes */50331648` 且不卡。这条用例是 harness `res.end()` 修复的守卫：
一旦有人把推流循环改回"写满只 log 不 `res.end()`"，它立刻变红（Task 0 的原始病症就是这样复现的）。

**E4：`slow` 标志此前是死代码，Task 0 修复轮 2 已把限速做实；Task 10 的有节拍对端用 `GET /sz/<字节数>s`。**
一句话根因：`push()` 收尾写成 `if (!res.write(c)) res.once("drain", push); else if (slow) setTimeout(push, 200);`
—— 64 KiB 的写在这台机器上让 `res.write()` 返回 `false`（限速流自己造成 backpressure），恢复只从 `drain`
分支回来，而那条分支**不带任何延时**地直接 `push()`，于是 `slow` 只在"写得进去"这一支生效、实际从不生效。
本条**取代**这条勘误原先写下的前提（"`/slow` 约每 200 ms 发 64 KiB ⇒ 整块 48 MiB 要 ~157 s"）：那句话当时不成立，
现在成立。

- 修前实测（port 8151，`target/g2-before.jsonl`）：`/sz/524288s`（8 块）服务端 `request`→`response_done`
  **9 ms**、客户端 38 ms；`/sz/1048576s`（16 块）**1 ms**、客户端 5 ms；修复轮 1 另测 `/slow` 整块 48 MiB **114 ms**。
  节拍若生效应分别是 1600 ms / 3200 ms / ~154 s ⇒ 差两个数量级，"限速"名不副实。
- 修后实测（port 8152/8153，`target/g2-after.jsonl`、`target/g2-after2.jsonl`）。改法只一行：drain 分支与
  "写得进去"分支统一走 `resume()`，`slow` 时 `setTimeout(push, 200)`、否则 `setImmediate(push)`；64 KiB chunk 与
  200 ms 都没动，`res.end()` 及其上方注释原样保留。`/sz/524288s` 服务端 **1639 / 1624 ms**（期望 8×200=1600）；
  `/sz/1048576s` **3463 ms**（期望 16×200=3200）；带 `Range` 的有节拍 206（`/sz/524288s` + `bytes=65536-393215`，
  5 块）**1023 ms**（期望 5×200=1000）；`/slow` 逐块到达客户端的时刻为 0 / 216 / 420 / 623 / 824 ms（此后客户端
  主动断链，没有等满 154 s）。客户端计时比服务端跨度少约一个节拍：最后一块写完后还要 sleep 一次才走到 `res.end()`。
- 非限速路径**没有变慢**（同一会话内对照）：`/sz/524288` 服务端 2 ms → 1 ms；`/flaky`（64 块、非限速）17 ms → 12 ms；
  `/sz/1024` 与 `/blob` 的 206 / 416、HEAD、`/noranges` 的状态码与响应头（含 `Content-Range`、`Accept-Ranges`
  该缺失处仍缺失）以及 `request` / `response` / `response_done` / `conn_close_side` 记录形状逐项不变。
  E3 的守卫形状（同连接读满 206 → 再打 416）仍 1 ms 通过；把那个 206 换成有节拍的 body（`/sz/196608s` 读满）后
  同连接再打 416 也通，只是第二个请求要**多等最多一个节拍**（`res.end()` 排在最后一次 sleep 之后）——
  Task 7/10 在复用连接上排后续请求时要把这 200 ms 计入预期。
- **Task 10 用哪个形状**：`GET /sz/<字节数>s`，字节数取 65536 的整数倍，期望时长 ≈ `(字节数/65536)×200 ms`
  （`/sz/196608s` ≈ 0.6 s、`/sz/524288s` ≈ 1.6 s、`/sz/1048576s` ≈ 3.2 s）；要窗口/偏移形状就同时带 `Range`，
  206 同样按节拍出块。`/slow` 现在是货真价实的 ~154 s 整块（48 MiB = 768 块），**不要**拿它做有界等待的测试对端。
- **本 harness 不提供"服务端自己停在 body 中间、永远不发完"的对端**：本轮只让 `slow` 的节拍生效，服务器写满
  仍旧必然收尾（`res.end()`，理由见 `push()` 上方那段注释）。停在中间目前只能由**客户端**断链造成（实测：
  `socket.destroy()` 之后服务端记 `conn_close_side ... pending=true`），那与"对端慢慢把字节送过来"不是同一种病症。
  spec §7 场景 2 若需要后者，得另给 harness 加一个"发够 N 块就挂住不 finish"的端点——那是**控制方另行裁定**的事，
  不在本轮范围内自行发明。**（本条末段已被 E5 取代：那样的对端在修复轮 3 已落地为 `GET /drop/<n>`；
  本条关于节拍的实测与"`slow` 该怎么用"的结论不受影响，不在此重述。）**

**E5：`/flaky` 从不打断 body（它只是把服务长度改成 4 MiB）；spec §7 场景 3 的"中途断"对端现在是 `GET /drop/<n>`。**
一句话根因：`/flaky` 在 harness 里唯一的作用是 `wantSz = ... : (flaky ? 4 * 1024 * 1024 : SIZE)`，之后仍走同一段
推流并 `res.end()` 收尾——它给的是"**一个更短的完整文件**"，不是"中途断"。
实测证据（修复轮 3，改动前的 harness 在 port 8161）：`GET /flaky` ⇒ `advertised-cl=4194304 received=4194304 err=-`；
`GET /drop/100000` 当时不被识别为特殊路由、按普通 blob 服务 ⇒ `advertised-cl=50331648 received=50331648 err=-`。
⇒ 本 harness 此前**没有任何对端能产出"响应比 `Content-Length` 短"**：fetch 的"body 短于 Content-Length ⇒ Failed"
分支、以及成功判据 ③"掐断后能续且不重下已完成段"都没有可测的对端；拿 `/flaky` 写"中途断"断言会**碰巧通过**
（它是一次完整响应，续传路径根本没机会出现）。

- 新增路由 `GET /drop/<n>`（`tests/harness/probe-server.mjs`，Task 0 修复轮 3）：`n` 计的是**本条响应实际写出的
  body 字节数**（不是文件偏移），可与 `Range` 组合。响应头一律按现有代码路径如实算（诚实的 `Content-Length`、
  `Content-Range`、`Accept-Ranges: bytes`、`ETag`、`Last-Modified`），body 写到第 `n` 字节后 `res.socket.destroy()`、
  永不 `res.end()`。实测形状（port 8163，逐字数据见报告「修复轮 3」）：无 `Range` ⇒ `advertised-cl=50331648 /
  received=100000`；带 `Range: bytes=65536-393215` ⇒ `advertised-cl=327680 / received=100000 /
  Content-Range: bytes 65536-393215/50331648`。客户端侧三种读法各报其形状：undici `TypeError: terminated`
  ← `SocketError: other side closed [UND_ERR_SOCKET]`；curl `curl: (18) end of response with 227680 bytes missing`；
  裸 socket 收到 **FIN（不是 RST）**。边界：`n >= 本条响应总长` ⇒ 不截断、正常收尾；`n=0` ⇒ 头发完、body 0 字节。
- **Task 10 的"掐断"场景（成功判据 ③ 与 `scenario_c_flaky_resumes_without_redownload`）必须改用 `/drop/<n>`**，
  不再用 `/flaky`：先 `GET /drop/<n>` 带 `Range: bytes=<lo>-<hi>` 只拿到 `<n>` 字节，再 `GET` 带
  `Range: bytes=<lo+n>-<hi>` 续，断言已完成段不重下。实测这条形状可跑：`bytes=0-327679` 截在 100000 后，
  续传 `bytes=100000-327679` 得 227680 字节，两段拼接与一次读完的 327680 字节**逐字节相同**。
  `/flaky` 仍然是合法的**尺寸变化**端点（4 MiB 完整体）——要测"服务端只承认一个比原始文件短的长度"照旧用它，
  只是它不承担"中途断"的语义，别拿它验证截断/续传。
- 日志：截断记成**新事件类型** `response_drop`（`{conn,url,status,bytes,chunks,expected,drop_at,range,total}`），
  一条 HTTP 请求仍然只有顶部那一条 `request` 记录（实测整轮 38 条请求 ⇒ 38 条 `request`，按
  `(conn,reqNoOnConn)` 去重后仍是 38 ⇒ 没有 F1 那类重复计数）；断链一侧照旧 `conn_close_side ... pending=true`。
- 与 `res.end()` 纪律的关系：其它所有路径的收尾行为一字未动。截断路径不构成回归，因为它把 socket 一起拆了，
  这条连接不可能再服务第二个请求（实测：同连接上的第二个请求立刻 `ECONNRESET`，不是 30 s 挂住；
  E3 的守卫形状——同连接"读满的 206"→"416"——仍 1 ms/0 ms 双双通过）。

**E6：Task 1 必须新建 `src/lib.rs`（简报的 `Files` 行只写了 `src/consts.rs`、`src/task.rs` 与"Modify: `src/main.rs`"，漏了它）。**
不是风格选择，是两处硬需求没有别的落地方式，且第一处就写在 Task 1 自己的步骤里：

- Task 1 Step 2/Step 4 的命令是 `cargo test --lib task::tests` / `cargo test --lib`。bin-only 的 crate 跑这条命令
  **连编译都不进**，实测（临时移走 `src/lib.rs` 后重跑）：`error: no library targets found in package \`dlm\``，退出码 101。
  简报预期的 RED 是"`Task` 未定义"，没有 lib target 时连那个 RED 都拿不到。
- Tasks 6/7/8/10 的 `tests/integration.rs`（本计划第 722–740 行、第 762 行起、第 809 行起、第 893 行起）写的是
  `dlm::task::Task { .. }`、`dlm::probe::run(..)` 这样的路径。`tests/` 下的集成测试是**另一个 crate**，
  只能经由 lib target 导入 `dlm::…`；写在 `src/main.rs` 里的 `mod task;` 对它们不可见（bin crate 不是依赖）。

裁定与落地形状（只改模块声明的落点，别的结构不动）：新建 `src/lib.rs`，内容就是 `pub mod consts;` + `pub mod task;`，
后续任务往这里逐条追加。于是**本计划所有 "Modify: `src/main.rs`（`mod X;`）" 一律读作 "Modify: `src/lib.rs`（`pub mod X;`）"**
（第 178/315/574/625/671/715/762/809/853 行；`pub` 是必需的，`mod` 私有则 `dlm::task::Task` 在集成测试里不可见）。
`src/main.rs` 保持占位 `fn main() {}` 直到 Task 9 真正组装——同名 lib + bin 并存是 cargo 支持的形状，
`cargo test` 会同时跑 `unittests src\lib.rs` 与 `unittests src\main.rs` 两个二进制（实测后者 0 tests，不报错）。

不受本条影响的部分：E2 关于"Step 3 代码块优先于 `Produces` 摘要行"的裁定照旧生效，Task 1 实际落地 **12 条常量**
（`grep -c "^pub const" src/consts.rs` = 12；与代码块那 13 行逐字 diff，只差被删的 `PROBE_USES_CONNECTION_CLOSE`
及其上方的文档注释行，其余顺序与字面值不变）。Task 1 落地后的门禁实测：`cargo test --lib` = 4 passed、
`cargo test`（lib + bin + doc 全目标）全绿、`cargo clippy --all-targets -- -D warnings` 零输出。

**E7：Task 2 的评审修复轮改掉了 `scheduler` 的四处对外形状 —— Task 4/7/8 引用简报 `Produces` 时以本条为准（任务正文不回改）。**
证据全在 `src/scheduler.rs`（行号取修复轮提交后的文件）与 `.superpowers/sdd/2026-10-15-dlm-kernel/task-2-report.md`
的「修复轮（I1/I2/I3）」节；判据来源是 spec `docs/superpowers/specs/2026-10-15-dlm-kernel-design.md` §2.2（第 51–52 行
「段表退化为单段」）、§6.4（第 112 行「416 → 该段直接标完成，不重试」）、§7.1（第 122–124 行六条必测用例的第六行
「416 后该段标完成」）与 §7.2（第 127–130 行 sha256 门槛 + "已完成段不重下"）。

- **`Event` 增一个 `SegmentComplete { worker: usize }`（`src/scheduler.rs:70`；分支 `:170`；落账 `seal()` `:203`）。
  本计划第 321 行列的五种事件现在是六种。Task 7 第 800 行的「`416` ⇒ 直接返回 `Complete`」到调度器这一层必须映射成
  `Event::SegmentComplete{worker}`，不再折成 `WindowDone`。** 但这条映射**不是无条件的**（2026-10-16 追加，Task 7 派发时以本条为准）：
  我们的每个区间都是由 probe 拿到的 `total_len` 推出来的，所以**收到 416 本身就证明服务端长度在我们脚下变了**
  —— 若 416 里的 `N` 等于我们的 `total_len`，合法区间的 `start` 必然 `< N`，那个 416 根本发不出去。
  故 Task 7 的判据 = 读 416 响应的 `Content-Range: bytes */N`：**`N != total_len` ⇒ 发 `Failed`**（让 §6.1/§6.2 的重试上限真介入，
  并最终明确报"服务端长度变化"，而不是悄悄产出一个短文件）；**只有 `N == total_len` 才允许 `SegmentComplete`**。
  `SegmentComplete` 这个事件本身仍然保留：spec §7.1 第 6 条要在单元层可表达、可测，而活锁已由"不再退回队列"根除。 语义 = "这个 worker 的区间就此收尾，不需要交回任何字节"：
  release 路径的显式分支，整段并进 `done`，**不看 `written`**、不产生队列项。为什么必须新增而不是复用现有形状：
  ① 416 折成零字节 `WindowDone` 时 `release()`（`:186-195`）会把**整段**推回队列 ⇒ "再派→再 416→再回队"的无界 HTTP 活锁，
  而 spec §6.1/§6.2 的快速 5 次 / 慢速 30 s 封顶只挂在 `Failed` 上，这条路径永远接不上；
  ② 用现有形状硬凑"416 ⇒ 完成"只有一条捷径 —— `Progress{abs_end: iv.end}` + `WindowDone`，那会把**从未落盘的字节**
  记成"已确认落盘"，直接污染 Task 4 的断点文件与 §7.2 的 sha256 门槛（拿数据损坏换掉活锁，不可接受）。
  代价要说清楚：被 `SegmentComplete` 封存的那一段里可能含本地没有的字节，这是 §6.4 裁定本身（Range 越界的字节拿不到），
  不是新漏；Task 4/8 按"归属已定、不再下载"理解它。**会撞的具体闸（2026-10-16 追加，Task 8 派发时以本条为准）**：
  spec §6 末条（设计稿第 116 行，"没给 sha256 则只校验『已写字节数 == total_len』"）与 Task 4 的 `written` 字段在封存之后会对得上号却**含义不同** ——
  `snapshot_intervals()` 是"归属已定的字节"，不是"磁盘上真有的字节"。所以 T8 的收尾判定必须把这两个量分开算
  （落盘量取自 `store.len()` 与 `Progress` 交回的字节，覆盖集取自 `snapshot_intervals()`），
  **不许拿覆盖集当已写量去过那道"已写字节数 == total_len"的闸**。钉住的用例：
  `segment_complete_marks_a_zero_progress_interval_done_and_never_requeues`（`:639`）、
  `segment_complete_seals_its_interval_exactly_once_and_is_not_redispatched`（`:654`）、
  对照行 `zero_progress_window_done_still_requeues_the_whole_interval`（`:674`，钉"两个事件不得合并"）；
  随机不变量用例的事件流里也加进了这条（`:755` 的 `rng.next(4)`）。

- **`Action::Halt` 删除：本计划第 322 行的四种动作现在是三种（`Assign`/`Retighten`/`Idle`，`src/scheduler.rs:77-84`）。**
  它没有任何生产者 —— spec §6.6 的 Ctrl-C 在本计划里已降级（第 927–929 行自检段），Task 7/8 都不产出它，
  留着唯一的效果是让两条"停劈"地板断言写成 `matches!(a, Idle | Halt)`；现已收紧为 `all(Idle)`
  （`src/scheduler.rs:426`、`:550`，测试侧 `track()` 同步 `:819`）。
  **Task 8 若真要"停发新请求"的通道，得自己提出新形状，不要复活 `Halt` 这个名字。**

- **退化模式（spec §2.2「段表退化为单段」）做实了两条，Task 7/8 对"退化后按什么顺序派"的假设随之改变。**
  ① 派发判据换成**起点最低**：`first_pending()`（`src/scheduler.rs:218`）在退化分支生效（`:243`），"最长优先"只留在正常模式
  （`longest_pending()` `:211`）。原行为下队列 `{[201,499],[700,999]}` 会先派 [700,999]（更长），而对端不认 Range 时只会从
  字节 0 交出整个 body，`fetch::run_window` 按 `iv.start` 落盘 ⇒ 内容写进中段、文件错位；POST 降级（§2.2 第一条）走的是同一条边。
  **随之而来的上游义务（Task 7 必须实现，本计划第 795–801 行没写）**：对端不认 Range 时 `iv.start > 0` 的行是真会出现的
  （`src/scheduler.rs:590` 的 `[201,499]`、`:505` 的 `[401,999]` 就是），所以 `run_window` 收到 200 响应时必须**先丢掉 body 的前
  `iv.start` 个字节**，再从 `iv.start` 起落盘。调度器给不出这条信息 —— 只要已确认字节挡在前面，它所能产出的任何区间形状都躲不开这一跳。
  ② `NoRanges` 到达即**塌表**：`collapse_pending()`（`:317`，入口 `:157`）把无人认领的区间合并成一段，退化期交回的尾巴
  也并回那一段（`:309`）。只合并**相邻/重叠**者 —— 中间隔着已确认或在途字节的不合并，否则会把已落盘字节重算成待下载
  （不变量 `done + pending <= total` 会当场红）。所以"单段"的准确说法是"表里相邻的空位不重复登记、每次只派一段"。
  ③ 上一轮裁定 B 的读法②保留：在途段**不被打断**，按各自（已收紧的）终点自然收尾。钉住的用例：
  `degraded_dispatch_is_start_ascending`（`:579`）、`no_ranges_collapses_the_pending_table_into_one_segment`（`:610`）。

- **`snapshot_intervals()` 返回的是"已确认字节的极小覆盖集"（升序、相邻/重叠已合并、两两不重叠），不是 IDM 的分段边界列表。**
  本计划第 326 行那句"已完成区间，供 state 落盘"要按这个措辞读，第 627 行 Task 4 的 `SegView { start, end, written, state }`
  （**这四个字段的写法已被 E9 取代：实际落地是 `{ start, end }`**）
  **不得假设"一条 = 一个曾经派出去过的分段"**：一段被劈过再交回，这里只剩合并后的一条；`SegmentComplete` 封存的段也照样
  出现在里面（其 `written` 请按上面那条理解）。这是上一轮报告 §5 顾虑 2 的正式化，实现见 `src/scheduler.rs:148`
  与 `push_done()`（`:333` 附近，合并逻辑）。

- **E8：`Store::flush()` 不是 `File::flush` —— 本计划第 616 行那句"`flush` = `File::flush`"已被实测推翻，Task 8 按本条派发。**
  `std::fs::File::flush` 对文件在 Windows 与 Unix **都是文档写明的空操作**（std 原文："Since a `File` structure doesn't contain
  any buffers, this function is currently a no-op on Unix and Windows"）。Task 8 的收尾会写 `store.flush()?`，读者当真是"字节落盘了"
  —— 名字承诺了实现给不起的东西。**裁定取"让名字为真"：`flush()` 现调 `File::sync_all()`**（std 自带、Windows 上即 `FlushFileBuffers`、
  不加依赖），错误按兄弟方法一样 `with_context` 上抛。落地见 `src/store.rs:131-136`，文档 `:110-127`。
  三条随之而来的下游义务（计划正文没有，Task 8 的 brief 必须写）：
  ① **只在任务收尾调一次，禁止进 `run_window` 的每块循环** —— 实测每 64 KiB 调一次是 2.3–7.3 ms/块，而纯写只要 15–38 µs/块，
  白送 60–480 倍吞吐；收尾那一次付 2.4–3.2 ms（4 MiB 实测 3.4–17.7 ms）。
  ② **不许在 writer 还活着的时候调** —— `sync_all` 期间持有那把全局写锁，会把并发的 `write_at`/`len()` 一起卡住整个 syscall 时长
  （偶发 8.3–228 ms）。所以 T8 的顺序必须是"先 join 全部 worker，再 flush"，取消/异常出口也不例外。
  ③ **它不是断电级保证**，`flush` 的文档已明写"进了设备"不等于"断电不丢"（设备写缓存另算）；任何"断电安全"的说法都不许由这句调用衍生。
  本轮同时把 `len()` 的性能守卫从 `avg_micros < 5_000` **收紧**到 `< 500`（实测均值 3 µs、加载下最差 6 µs、余量 83 倍），
  报告 §4 那句"< 500 µs"现在与代码一致了；断言只变严、无任何放宽。

- **E9：`SegView` 落地只有 `{ start, end }` —— 本计划第 627 行写的 `{ start, end, written, state }` 两字段作废（Task 4 执行期定，配套设计稿勘误 R6）。**
  裁决理由三条，都可在码上核：① 设计稿第 81 行自己的示例 `{start:0, end:236032, written:236032}` 里 `written` **没有定义**
  （既不是包含式字节数 236033，也不是相对偏移），没有语义可保；② `state` 恒为 `"done"`，是常量字段；
  ③ E7/T2-D 之后 `Event::SegmentComplete` 会把**从未落盘**的字节记进覆盖集，留着 `written` 就是让断点文件对磁盘内容说谎 ——
  而"落盘量"的真相归 `store.len()`（E8/E7），不归本文件。落盘名以 `src/state.rs` 为准：`stats` 是
  `bytes_delivered` / `bytes_duplicated` / `retries`，**本计划第 843 行那句 `stats.duplicated` 按 `bytes_duplicated` 读**（Task 8 派发时必须这样写）。
  配套政策（设计稿 R6 已陈述一次，这里只指路）：五个结构体一律 `deny_unknown_fields` 且无 `#[serde(default)]` ⇒
  新增字段是破坏性变更，必须同时抬 `v`。
  **另加一条 `Task::validate()` 的新规则（评审实证：口令防的是"字段"而不是"值"）**：`--url https://alice:s3cret@x/a.bin`
  会把凭据原样落进 `task.url` 与 `validator.final_url` 两处，而 `state` 模块删不掉 URL（续传要用）。
  故 userinfo 形态的凭据在 `validate()` 里**直接拒绝**，错误信息指使用 `--user` / `--password`。
  这样 E7/T2-D 之后"口令不落盘"这条硬规定才真的成立，且它住在唯一的校验闸口里（`from_task` 已经调 `validate()`，磁盘边界自动受保护）。
  代价：真要用 userinfo URL 的人得改写成两个参数 —— 而这正是我们想要的可见性。

- **E10：Task 5 Step 3 第 ③ 句作废，按"两步并列"判读（依据设计稿 §6.3 第 110–111 行；落地见 `src/resume.rs:208-230`，实证为简报自己的分支 4）。**
  （行号按 `8b2921f` 复核：①b「请求本身变了」插进来之后整体后移 45 行，原先写的 167–185 已失效。措辞里的两步一律写作"第一步/第二步"，
  以免与 Step 3 自己的规则编号 ①–⑥ 撞名。）
  原句「ETag 两者都有则只比 ETag，ETag 任一缺失则比 `last_modified`（除非 `ignore_mtime`）」按字面实现与同计划第 692 行自相矛盾：
  那一支两边 ETag 相等、只有 Last-Modified 变了，字面读法判 `Resume`，而分支 4 与 §6.3 都判 `Fresh`。准确规则是两步，
  **顺序执行、互不替代**：
  **第一步 ETag 步**：两边都有且不相等 ⇒ `Fresh{原因}`；`--ignore-mtime` 对这一条**没有效力**（§6.3 括号那句「ETag 变了仍重来」）。
  任一边缺 ETag ⇒ 本步**不作判断**，继续下一步（不是"跳过整个 ③"）。
  **第二步 Last-Modified 步**：本次任务带 `FLAG_IGNORE_MTIME` 时**整步跳过**；否则把两个 `Option<String>` **整体**比对 ——
  值不等、或一边有一边没有，都算不符 ⇒ `Fresh{原因}`；两边都缺 ⇒ 视为相等，放行到 ④。
  边界一句：两把验证子全缺时 ③ 无事可做，续传的安全性**只剩**规则 ② 的最终 URL 与规则 ④ 的长度，且这条链路上
  `If-Range`（设计稿第 67 行）根本发不出去 —— 手里没有验证子可带。Task 5 Step 1 的分支 3/4 **不动**，它们就是本条裁定的依据。

- **E11：Task 6 落地时把简报的"试探永远先发"拆成了两条入口 —— Task 7/8 引用简报 `Produces` 时以本条为准（任务正文不回改）。**
  ① **`probe::run` 对带 `data_file` 的任务在上网之前就硬拒**（`src/probe.rs:190` 的 `ensure!`）。理由不是风格：
  按简报字面用 GET 不带 body 去试探，收回来的事实描述的是"我们从没发过的那一发"（同一 URL 的 GET 与 POST 是两个资源，
  长度/验证子/状态码都可能不同）；而照简报"POST 带 body 去试探"等于**把用户的表单提交两次**，是对别人服务器的真实副作用。
  ⇒ 该形状的事实由 `fetch::run_whole` 从**这一枪自己的响应头**收割（Task 7 已落地），Task 8 的引擎必须**跳过 probe**，
  不许把这条 `Err` 当"网络故障"重试。
  ② **`Authorization` 的闸住在 `with_contract_headers`（`src/probe.rs:366`）里，按 scheme 挡**，不在 `Task::validate()`：
  `validate()` 只管"明文 HTTP + 口令"这一对；`--user` 单独出现时仍会在 https 上正常发头。
  评审另核 `reqwest-0.13.5/src/redirect.rs:239-251`：https→http 降级重定向时 reqwest 自己摘掉 `Authorization`/`Cookie`，
  所以"只查 `task.url` 的 scheme"不成立漏洞。
  ③ **`accept_ranges` 是带否决的合取式，不是设计稿 `:52` 那句字面上的"或"**：
  `有证据 && !(我们发了 Range && 状态不是 206)`。五格真值表钉在单测里。
  关键反例（我派发时写错的那格）：**对端省了 `Accept-Ranges` 头但老老实实回了 206** ⇒ 仍算 `true`（能分段），
  用 `accepts_bytes && (!range_sent || got_206)` 会把这种诚实对端误判成不能分段，白丢并行、换不到任何正确性。
  ④ **`206` 的长度绝不回落 `Content-Length`**：206 的 `Content-Length` 是**窗口**不是全长，拿它当 `total_len` 会让
  调度器把整份文件砍成一小截还自洽（预读字节数恰好等于它，测试全绿）。只有 `200` 那一支的 `Content-Length` 才是"对端真给了多少"。
  ⑤ **`final_url` 落地时要过 userinfo 闸**（`probe::LANDED_CREDENTIALS` 一份常量、三处引用）：服务器可以用一条带凭据的
  重定向把凭据送进我们的原因串与断点文件；①b 与 ② 两侧都取自同一个函数才不会不一致。Task 7 的 `run_whole` 同形。
  ⑥ 简报外新增的 `416 + bytes */0`（零长度文件）那一支**保留**：拒绝下载空文件才是缺陷。
  **转派给 Task 8 的两条**：`resume.rs:136` 那句字段注释（"服务器是否广告 `Accept-Ranges: bytes`"）在 ③ 之后**已经不准**，
  它是**行为加权**的；以及"试探之后连接可复用"**只在认 Range 的对端成立**，降级那一支不回池（`src/probe.rs:23-27`，
  注意那句是**推理不是实测**）⇒ 引擎侧当约束用，不许当已验证性质写进断言。

- **E12：Task 7 落地时改掉/补上的对外形状 —— Task 8/9 引用简报 `Produces` 时以本条为准（任务正文不回改）。**
  ① **签名多了 `total_len`**：`run_window(agent, task, iv, store, rate_bps, total_len)`。简报的 `Produces` 行没有它，
  但 416 的判据没有它就无法表达（见 ②）。另有一条简报里没有的入口 `run_whole(agent, task, store, rate_bps) -> (Outcome, Option<ProbeFacts>)`。
  ② **`416` 不是"看到就完事"**：读 `Content-Range: bytes */N`，**`N == total_len` 才** ⇒ `Complete`（该段封存），
  `N` 取不到或不等 ⇒ `Failed(ValidatorChanged)`（服务端长度在我们脚下变了，交给重试上限）。可达形状是 `iv.start >= total_len`
  （续传时区间起点已落在 EOF 上），不是 `iv.start == total_len` —— 两者必须能被区分，见本轮钉的用例。
  ③ **body 比区间短绝不返回 `Complete`**：交过字节 ⇒ `Advanced { bytes }`；一个字节都没落地 ⇒ `Failed(Transport)`。
  把 `Complete` 用在这里会**永远封存一段没下完的区间**（`SegmentComplete` 不查盘上字节，E7/E9）。
  ④ **`Stalled` 的定义按设计稿 R7**：预算到期**且零进度**才算；"活着但慢"的对端只要交过一个字节就走 `Advanced`。
  reqwest blocking 的 `.timeout()` 是**整请求**期限（实测 `tests/integration.rs:1650`），不是静默期限。
  ⑤ **降级那一支（发了 Range 却答 200）在 64 KiB 循环内流式丢掉前 `iv.start` 个字节**再落盘；
  这里绝不许出现 `Vec::with_capacity(iv.start)`（那等于把整个 body 吸进内存）。
  ⑥ **窗口上界只算一次**：`stop = min(iv.end+1, iv.start+WINDOW_BYTES)`，`bytes` 与 `store.len()` 两条断言都由它派生，
  差一字节就一起红。`write_at` 返回 `Err` 时**往上抛**、绝不记进 `Advanced`（T3 裁定：那一块真落了几字节无从知道）。
  ⑦ **`FailKind::Transport` 带原因**（本轮修复落地）：`Transport(String)`，取 `reqwest`/`io::Error` 的 source 链首句，
  压成单行、上限 200 字符，**绝不回显 body 字节**。Task 8 要把它交给用户 —— 折成裸枚举就是"网络错了"这种没法行动的消息。
  URL 天然不带凭据（`validate()` 拒绝 userinfo），所以这一串可以直接上桌。
  ⑧ **POST 那一枪的功能封顶**（设计稿 R7）：`run_whole` 是"一发请求读完全部"，最多只能拿到一个窗口预算内到达的字节，
  而**重发这一枪就是二次提交表单**（E11① 的理由）⇒ Task 8 不许靠重发续完，跑不完就如实报未完成并以非零码退出。

- **E13：Task 8 的简报有三处接口是断的（我在 T7 收口时通读六模块对外形状量出来的），派发前按本条改；任务正文不回改。**
  ① **`Scheduler` 没有"把断点喂回来"的门。** 它只有 `new(total, min_seg)`（`src/scheduler.rs:127`）＋ `on/ is_done/ pending_bytes/ snapshot_intervals`；
  `state::SegView::interval()` 的注释（"Task 8 若要把断点喂回调度器"）**假设的门不存在** ⇒ `Decision::Resume(snap)` 到了引擎手里只能**重下**（要么就是再造一本覆盖账，E7 明令禁止）。
  ⇒ Task 8 必须给 `scheduler` 加一个播种入口（名字建议 `resume(total, min_seg, done: &[Interval]) -> Self`），**内部走既有的 `seal()` 同一条路**，
  并配一条**等价性用例**：`resume(seed)` 的后续行为 == `new()` 之后把那批区间逐个封存的后续行为（同一事件流下动作序列逐条相同）。
  这条是"新模块改旧模块"，评审时按契约变更对待，不许只加不测。
  ② **`Config { …, resume: Decision }` 这个形状站不住。** `Decision::Fresh{reason}` 里**没有任何事实**，而引擎必须知道
  `total_len`（喂 `Scheduler` 与 `run_window` 的第 6 个参数）、`accept_ranges`（要不要发 `NoRanges`）与验证子（存断点要用）；
  这三样只在 `ProbeFacts` 里，而 `probe::run` 又必须被调（它拒绝 `data_file` 那一支，E11①）。把 probe 放在 CLI 里 ⇒ Task 9 那个"解析参数＋打进度行"的任务
  凭空背上网络与断点文件两套账，且 `Decision::Abort` 在 CLI 与引擎之间无家可归。
  ⇒ **裁定：引擎自己走完整条链** `state::load → probe（`data_file` 跳过）→ resume::decide → 执行`；
  `Config` 去掉 `resume`，改为 `{ workers, min_seg, max_retries, verify_sha256, force_fresh, keep_state }`
  （`force_fresh` 就是 spec §4 的 `--no-resume`，`keep_state` 就是 spec §4 的"完成后不删断点"）。
  **本条写下五分钟后自纠一处**：`keep_state` **不进 `Config`** —— 它已经是 `Task::flags` 里的 `FLAG_KEEP_STATE`（`src/consts.rs:12`），
  而 `flags` 会随 `TaskView` 进断点文件；两处都能读＝两本账。⇒ 引擎从 `task.flags & FLAG_KEEP_STATE` 读，`Config` 只留
  `{ workers, min_seg, max_retries, verify_sha256, force_fresh }`。`force_fresh` 保持不进 `flags`（`--no-resume` 是"这一次怎么办"，不是"这个任务是什么"）。
  `Abort` ⇒ `engine::run` 返回 `Err(原因)` 且**不碰断点文件**（T5 转派第 ④ 条）；"跑完了但没跑全"⇒ `Ok(Report{ ok: false, … })`，非零退出码归 Task 9。
  ③ **Step 1 那条"事件回放一致性"的实现路径与设计稿 §7.2 相反。** 简报写的是 `harness::replay(observed_ranges, total, min_seg)` 造一条 `predicted` 再比 ——
  §7.2 明令"**不能直接拿『预测序列』对比，因为劈分时机取决于真实时序**"。要做的是**录事件**：`Report` 带一条本次运行喂给调度器的事件序列
  （简报第 823 行的 `rep.event_log_path` 因此作废 —— 引擎在生产里不写日志文件，别为测试造一条），测试把**同一串事件**回放给一个全新 `Scheduler`，
  断言它给出的 `Assign{iv}`/`Retighten{iv}` 与引擎**实际派发**的区间逐条相等，再与 node 侧日志里**线上真收到的 `Range`** 对齐。
  两道链接各有独立价值：前者抓"引擎没照调度器办事"，后者抓"引擎说派了但线上不是那个 Range"。
  ④ 三条配套裁定：**退避不许造假时钟** —— 把 `spec §6.1/§6.2` 的退避抽成纯函数 `next_retry_delay(attempt, max_retries) -> Option<Duration>`（`1,2,4,8,16` 后转 `30 s` 无限；
  `max_retries` 封顶 ⇒ `None` ⇒ 整任务失败），表驱动单测零睡眠，另配**一条真睡 1 秒**的集成用例证明退避真的接进了循环；
  **完成判定分两本账**（E7 追加段）—— `sched.is_done()` 且 `store.len() == total_len` 才算完，覆盖集字节数**永远不许**当"已写字节数"过 §6.7 的闸，
  给了 `--sha256` 就读真文件算（`sha2 = "0.11"` 早在 `Cargo.toml` 里，不是新依赖），不匹配则**保留文件与断点**并报错；
  **`data_file` 那一支既不读也不写断点文件** —— POST 不可续（E12⑧），若给它存一份段表就等于凭空造出一条"下轮会判 `Resume` 却永远执行不了"的假路径。
  ⑤ 夹具天花板要现在就拆：进程内窄桩上限是 `MAX_CONNECTIONS = 8` × `PER_CONNECTION = 4`（`tests/harness/mod.rs:463/:464`）⇒ 多 worker 的引擎用例一律用 node 对端（无上限），
  并**授权把 `PER_CONNECTION` 抬到 64**（T7 复审登记的挂账：现有两条用例正好把 4 格占满，再补一发会以"30 秒客户端超时"失败而不是断言红）。抬完之后既有断言一条不许改。

- **E14：`T8-R1` 已裁定 —— `Retighten` 那笔重复流量是**可省的实现缺口**，不是结构性代价（2026-10-17 控制员实测，数据在下面）。**
  阶段4 报的是"8 MiB 文件重复下载 8,206,900 字节 ≈ 2× 流量，且这是 scheduler/fetch 形状之间的结构性缺口，我改不动"。
  我按"别人的观察也要先复量"的规矩做了一次一次性取证（临时 `tests/tmp_probe.rs`，跑完即删、未进提交），拿 node 服务器自己记的 `response_done.bytes` 当线上真相：
  **实测 `served = 12,655,972` / 文件 `8,388,608` ⇒ 真多打出去 4,267,364 字节 = 1.51×，不是 2×**；
  而引擎报的 `duplicated = 8,461,668` **比线上真实浪费多算了约一倍** ⇒ 那个计数器现在量的是"落在收紧后区间之外的写字节"，
  与它的名字（`Stats::bytes_duplicated`，`state.rs` 写的是"重复下载被丢弃的字节"）**不是一回事**。
  顺带把 `T8-R2` 钉死：18 条 `request` = probe 那一枪 + 17 条 `Dispatched` ⇒ **`dispatched()` 与线上 `Range` 是一对一，比对时只须剔掉 probe 自己那条 `bytes=0-236031`**。
  机制（看清楚才知道怎么省）：第一条派出去的区间是 `[236032, 8388607]`，它的 `end+1 == total_len` ⇒ 按 E12① 发的是**开放终点 `bytes=236032-`**；
  对端因此有权一路把尾巴全推过来，而调度器同期把这一段**反复收紧**（`tightened` 里同一段被收了 4 次，终点 4312319 → 2274175 → 1255103 → 745567），
  incumbent 手上那一枪既没人能打断它、读到的又超出收紧后的范围 ⇒ 那些字节**真在盘上、却永远进不了覆盖集**，随后由别人重下一遍。
  **裁定：两条一起改，都要有测试。**
  ① **乙 —— 给在途那一枪一个"终点帽"**：每个 worker 一格 `Arc<AtomicU64>`，`Retighten` 到达时主线程把帽写成新终点；
  `run_window` 的 64 KiB 循环每块查一次帽，`cursor > 帽` 就**停手**（走已有那条 `Advanced{bytes}`）。
  形状约束：帽**只降不升**（用 `fetch_max`），初值是这一枪自己算出来的 `stop` ⇒ `run_window` 再多一个参数，
  **E12① 那条签名以本条为准再改一次**。不许改成"每块 sleep"或"另起一条线程读 socket"，也不许引入 `tokio`。
  ② **戊 —— 开放终点那条规则的"终点"是窗口，不是文件尾巴**：发 `bytes=start-` 的条件从 `iv.end+1 >= total_len`
  改成 **`min(iv.end, iv.start + WINDOW_BYTES - 1) + 1 >= total_len`** ⇒ 文件真正那一尾仍发开放终点（IDM 的观察保留，E12① 不作废），
  但**不再让对端替我们流一段我们注定用不上的 body**。
  ③ **`bytes_duplicated` 的口径必须跟着改成"真浪费"或如实改名**，二选一由实现者定并写清理由：
  **甲**按"被第二次请求的字节"计（那要有线上证据，不许拿覆盖集倒推）；或如实把它注释成"落在归属范围之外、**可能**被重下的字节"（承认它是个上界而不是账单）。
  判据要能红：同一形状下 `served` 与 `bytes_total` 的差值须落在"每次收紧 ≤ 一个 64 KiB 块"的量级（阶段4 的 4.27 MiB 就是不合格的形状）。
  **不许**为了测试好看把它折成 0。
  ④ 这两条落在 `src/fetch.rs`（T7 已评审收口的模块）⇒ 算**契约变更**：改动连同它自己的单测一起过 T8 的任务级评审，
  且既有 158 条单测与 60 条集成用例的**判据方向一条不许动**。

### E14 附记（2026-10-17 复量后对本条的更正 —— 我这条勘误自己的两个数是错的，按 append 不回改正文）

  **量具错了两代**：① 我原先加的 `Σ response_done.bytes` 只统计"读完/正常收尾"的那几枪，**半路被丢掉的响应一条没算**；
  ② 换成 `Σ (response_done + conn_close_side)` 又会**重复计数**，因为正常收尾的响应**两条都记**（`res.on("close")` 每条响应都发一条）。
  唯一诚实的线上口径是 **`Σ conn_close_side.sent`**（服务端每条响应推上网线的字节，含客户端没收的）。用这一把尺子同一形状复量前后：

  | 形状 | 阶段4 线上多打 | 阶段5A 线上多打 | 阶段4 `duplicated` | 5A `duplicated` |
  |---|---|---|---|---|
  | 8 MiB × 4 worker | 8,465,356（**×2.009**） | 6,221,236（×1.742） | 8,334,284 | 2,341,183 |
  | 8 MiB × 8 worker | 13,571,780（×2.618） | 6,875,250（×1.820） | 13,047,492 | 2,450,123 |
  | 48 MiB × 4 worker | 21,044,506（×1.418） | 12,465,707（×1.248） | 11,410,714 | 4,762,358 |
  | 48 MiB × 8 worker | 34,947,830（×1.694） | 27,878,619（×1.554） | 25,051,894 | 3,976,460 |

  ⇒ **本条正文里那两句要按这张表作废**：
  (a) "真流量 1.51×，不是 2×" —— 错。按 `conn_close_side` 口径，阶段4 在 8 MiB/4 worker 上就是 **2.009×**，
      **实现者原先那句"≈2× 流量"是对的，我的"更正"反而把它改错了**。
  (b) "`duplicated` 比真实浪费虚高约一倍" —— 也错。同一形状下 `duplicated = 8,334,284` 与线上多打 `8,465,356` **差 1.6%**，
      阶段4 这一格基本就是一张真账单。反倒是 **5A 之后它开始低报**（2.34 MiB vs 6.22 MiB）：
      乙把浪费从"我们读走又丢掉"挪到了"对端推上来而我们没接"，后半截**不在这本账里**，引擎也没有独立证据去数它。
  ⇒ **结论不变但理由换了**：乙+戊 仍是该做的（线上多打在四种形状上分别降 27% / 49% / 41% / 20%，读走的字节降得更多），
  而 ③ 的"如实标注口径"比原先想的更要紧 —— 已落地的 `Stats::bytes_duplicated` 注释里那句
  "线上真实多打 4.3 MiB（`response_done` 口径）"引用的是**我这把坏尺子**，要按这张表改成 `conn_close_side` 口径的两个数。
  **残余没消掉的那一截**（5A 之后仍 1.25×–1.74×）成因已被实现者量清：帽只能拦"帽落下之后还要读的字节"，
  拦不到"调度器决定收紧时那一枪早已读过去的字节"，因为调度器只在一枪**结束时**才从 `Progress` 知道它读到哪儿；
  同一条在途尾巴被反复劈（实测一段被收 4 次）就把这一截乘了出来。要压到"每笔 ≤ 一块"得改 §2.1 那条**核心劈分规则**
  （例如"已被收紧过的在途段不再作为劈分候选"）—— 那是 IDM 观察行为，**已登记为 `T8-R5` 等用户点头，v1 先不自作主张**。

## E15：E13① 的"动作序列逐条相同"按字面**做不到**，裁定接受收窄并改钉三条可观测性质（T8 评审 A2，2026-10-17）

E13① 原文要求：`resume(seed)` 与"`new()` 之后把那批区间逐个封存"在**同一事件流下动作序列逐条相同**。评审逐行核后判这句话的前提是错的，我采纳：

1. **公开事件里根本没有"封任意区间"这把通道** —— 唯一会封段的是 `SegmentComplete`，而它只对"该 worker 当前手上那一枪的区间"生效。
   所以 arbitrary 的种子**喂不进** `new()`，"逐个封存"这条参照路径在多洞种子上并不存在 —— 我原先把它当成可达的参照，是我的错。
2. **两边的队列粒度天然不同**：真跑出来的队列由 `push_pending` 建立，它只按 `start` 去重、**从不合并** ⇒ "挣来的"队列带着历史碎片
   （评审复算的形状：62/63 字节的小段），而门给出的是极小补集（一条 190 字节的洞）。粒度一不同，`longest_pending` 在第一个 `Connected`
   就合法地分岔 —— 这不是实现的缺陷，是两种形状各自都对。

**裁定（不许再拿"动作逐条相同"当验收）**：等价性改钉成这三条，缺一不算过：
① **洞的集合相等**（按字节集比，不是比条数、也不是比总长 —— 只比条数会同时放过"漏一个洞"与"多并一个洞"）；
② **种子里的字节一次都不许被重新派出**（`dispatched ∩ seed = ∅`，这条才是"不重下已完成段"的正身）；
③ 每一步的 `pending_bytes()` / `is_done()` / `snapshot_intervals()` 两边一致，且两臂各自跑 `check_invariants`。
**也不许**为了让①之外多拿到"动作逐条相同"而给 `push_pending` 加补集归一 —— 那会改掉 Task 2 已评审的派发策略与它名下 40 条用例的形状。

**顺带一条方法教训（评审 A1 的成因，值得常驻）**：门的输入夹法 `clamp_to_file` 被测试的**参照实现 `cover()` 原样抄了一份**
⇒ 参照与被测在**同一条规则**上不独立，于是那一类缺陷（整条越过 EOF 的种子被夹成"一个已确认的幽灵字节"，
`is_done()` 为真而那个字节从没被派过）当场变成看不见。**凡是"参照实现 / 预测器"型夹具，与被测共享的每一行都要写明它在哪条轴上独立、
在哪条轴上继承**；默认要求：参照**不许**抄被测正在被检验的那条规则，宁可写更笨的显式表。

## E16：退化模式（对端不认 Range）**不许按窗重请求** —— 那一支的流量是平方级的（T8 评审 B-I-2①，2026-10-17 控制员裁定 R-B2）

**现状（读代码得到的形状，不是测出来的）**：`src/fetch.rs:625` 把每一枪的读上界夹成一个窗口
（`stop = min(iv.end + 1, iv.start + WINDOW_BYTES)`），而 `:631` 在 `degraded`（我们发了 Range 而对端答
`200`）时先**流掉 `iv.start` 个字节**再落盘。两条叠加的结果：区间每往前挪一个窗口，就重新要一遍"从文件头
到那里"的 body，读完窗口就中途弃响应 ⇒ 每个窗口都付一次"从头到现在"的前缀。

**算术代价**（`WINDOW_BYTES = 4 MiB`、`total_len = 48 MiB` ⇒ 12 个窗口）：丢弃的前缀之和
= `4 MiB × (0 + 1 + … + 11) = 264 MiB`，加上真用到的 48 MiB ⇒ **≈312 MiB ≈ 6.5×**。
⚠ 这组数是**推出来的，不是测出来的**，写进任何注释时都要带着这个身份。今天**没有任何用例跑得进去**：
harness 的 `/noranges` 只有 1 MiB < 一个窗口 ⇒ 只有一枪、前缀为 0 ⇒ 平方项根本不出现。
⇒ **这条既是实现缺口，也是覆盖缺口，两者要一起补。**

**裁定 R-B2**：降级那一支必须**一枪下完剩余整条 body**，不再按窗切。随之三条义务：
① 预算（超时）按**整条剩余体量**算，不许沿用窗口那份 —— `reqwest` 的 `.timeout()` 是**整请求期限**（**R7**），
48 MiB 用窗口预算会在半路把自己掐断；
② 被掐断（超时 / 对端收线）时照旧交回 `Advanced { bytes }` 并让调度器重新决定尾巴，**不许**把"没下完"
报成完成（`completion_verdict` 那两本账的闸原样有效）；
③ 若实现仍保留任何"丢弃的前缀"，那批字节是**真线上代价**：要么进 `bytes_duplicated`（要有线上证据），
要么在 `state.rs` 的字段文档里写明它没进账 —— **不许拿覆盖集倒推**（**E14③**，另见 **E17**）。

**必带一条今天会红的用例**（否则这条裁定没有证据）：需要一个能服务**大于一个窗口**的"不认 Range"对端
（node 新路由 `/noranges/sz/<n>`，或把 `ScriptedPeer` 接成"200 + 整条 body"的形状）。判据落在对端自己记的
`conn_close_side.sent` 求和上：`Σ sent ≤ 剩余 body + 试探预读 + 一个 CHUNK_BYTES`。按现状（多窗）这一条
应当**红到数倍**，改完应当绿。造不出这个形状就照实上报，**不许**把它降级成"退化模式能跑完"那种恒真断言。

## E17：`run` 里那笔"kill + 续传重下账"的前提是假的 ⇒ 整笔撤掉（T8 评审 B-I-1，控制员裁定 R-B1）

**被撤的代码**：`src/engine.rs:457-461`
`stats.bytes_duplicated += on_disk − overlap_with(&seed, on_disk)`，连带 helper `overlap_with`（`:533`）
与它的算术用例 `overlap_with_counts_only_bytes_that_are_both_confirmed_and_on_disk`（`:1698`）**一起删**
—— 撤了账就没人用它，留半截就是死代码（同一条清理规矩用在 `Action::Halt` 上，账本 `Ruling (T2-C)`）。

**为什么它是假的**：这笔账把"盘上有、覆盖集里没有"读成"上一轮写过、本轮必然重下"。可是
`Store::write_at` 一延长文件，中间没写过的洞**读起来是零、也照样算进 `len()`**；而多 worker 的中点劈分
**天生**制造洞（`src/scheduler.rs:306-320` 把 `[mid, end]` 交给另一个 worker ⇒ `mid` 以下就是洞）。
举例：已确认 `[0, 3M] ∪ [12M, 16M]`、EOF 在 16M ⇒ 它记 9M，其中 **5M 是从没下载过的洞**。
更糟的是这一笔**会落盘并被下一轮重新加载**（本轮实测核过两处出口：`src/engine.rs:1000` 的
`snap.stats = self.stats` 写进去、`:519` 的 `stats: snap.stats` 读回来）⇒ 反复被杀能把一个远大于
`bytes_total` 的数字滚起来。**这正是 E14③ 明令禁止的"拿覆盖集倒推"。**

**顺带作废我自己的一条旧裁定**：Task 2 的自裁 ②（账本 `## Task 2` 节）写"在途段的已写前缀不进断点 ⇒
这批字节必须计入 `stats.duplicated`"。**该义务撤回**：在只拿得到"覆盖集 + `store.len()`"这两个量的引擎里，
它没有诚实的算法 —— 唯一能算出它的方式就是这条刚被撤的倒推。改成**如实标注**（E14③ 允许的另一支）：

- `state.rs::Stats::bytes_duplicated` 的文档要写明一处**已知的少报**：任务在任意时刻被杀，上一轮在途窗口
  已落盘却没进段表的那批字节会被下一轮重下，而它**不在**这个字段里。上界是**算术推出来的**
  `WINDOW_BYTES × workers`（来自 `run_window` "每枪至多一个窗口"这条形状），不是实测数字，标注时须带出处。
- 用例不许为了让这笔账"看得见"而放宽任何既有判据，也不许新造第三本账。

**为什么不是反过来修**：想把洞算准，只有一条路 —— 让断点文件也记"写过但未确认"的半段账。那是给
`state::Snapshot` 加第三本账，与 **E7** 的"两本账"裁定正面冲突；而 spec §7.2 的"已完成段不重下"本来
只约束**已完成**段，为在途半段付这个复杂度不值。

### E17 附记（落地那一发 `59dcf70` 算出来的两处更正，append 不回改正文）

① **正文那个例子的算术是错的，而更正之后结论更硬**。我写"已确认 `[0,3M] ∪ [12M,16M]`、EOF 16M ⇒ 它记 9M，
其中 **5M** 是从没下载过的洞"。按**含端点**语义重算：`on_disk = 16M`，覆盖集 `= (3M + 1) + 4M = 7M + 1`
⇒ 那一笔是 `16M − (7M + 1) = 9M − 1`；而洞 `[3M+1, 12M−1]` 的长度恰好也是 `9M − 1`
⇒ **那一笔整笔都是洞，一个字节都不是重下**（我原先"其中 5M"是没算就拆出来的数）。撤账的结论不变，
只是它比我写下的更该撤。⇒ 这是本项目**第五次**把没算过的数写进文档，且**这次连"推论"都不是**，是凭印象拆数。
② **引用锚点漂移**：正文引的 `src/scheduler.rs:306-320` 已不在那儿，`split_tail` 现在是 **`:314-330`**。
⇒ 顺带定一条纪律：**锚点宁可用符号名（`split_tail`）也不用行号** —— 8F-1 新写的 `bytes_duplicated` 字段文档
就是按符号名引的。本会话已经为腐烂的行号返工三次（`task-7-report.md` 的 RED 锚点、E7 的两处引用、这条）。
③ 落地时另一处措辞矛盾也一并修掉：`state.rs` 字段文档第一句还留着 E14③ 的"上界式"说法，而后文按 E14 附记
写的是"既不是上界也不是下界"（评审 A 的 Minor 之一）⇒ 统一成后者，并写明"E14③ 当时给的'标注成上界'这个
选项已被复量否掉"。**留给最终评审的同类项**：`state.rs:1124` 附近若还有"账单式"残留措辞，一并扫。

## E19：T8 任务级评审（第 1 发，E12–E17 义务逐条对账）的三条结论（2026-10-17，HEAD `83d8501`，全文见 `.superpowers/sdd/2026-10-15-dlm-kernel/t8-review-E12-E17.md`）

判决面：E12–E17 加 spec §6/§2.1/§2.2 共 38 条义务，**落实 35 / 部分 1 / 未落实 2**；无实现面回归（该发是只读评审，
基线 166 lib / 76 integration / clippy 0 由控制员在同一棵树亲量）。下面三条是要带进 T9/T10 的。

① **E16 那条"必带用例"的判据被操作化成了"枪数"，不是裁定点名的 `Σ conn_close_side.sent`** —— 这是**仪器决定的，不是偷懒**：
E16 的用例 `a_degraded_peer_is_fetched_in_one_body_shot_not_one_per_window` 跑的是**进程内** `spawn_scripted_peer`，
它按拍记录的是 `(Range 头, 时刻)`（`tests/harness/mod.rs:1597`），**根本没有 `sent` 这个计数器**；
`conn_close_side` 只在 node 那台 `probe-server.mjs:141` 上记。⇒ **裁定的那把尺子在这台对端上不存在**。
控制员已复量这一格**有牙**：把 `src/fetch.rs:519` 退化支的窗口夹装回去（`.min(iv.start + WINDOW_BYTES)`）⇒
该用例红在 `tests/integration.rs:3069`，`left: 4 / right: 2`，失败消息把四条线上 Range 整条打出来
（`bytes=0-236031` / `bytes=236032-` / `bytes=4430336-` / `bytes=8624640-`）⇒ 平方级流量那一件事被"请求条数 + 每条的线上区间"
**等价地**钉住了（`IgnoresRange` 的对端每多一枪就多推一遍"从文件头到那里"，条数与区间已知 ⇒ 体量是它们的算术后果）。
⇒ **本条把 E16 的"必带一条判据为 `Σ conn_close_side.sent ≤ …`"正式改判成"枪数等式 + 线上区间到 EOF + 全文 sha256"**，
理由与变异证据就是上面这段。若日后要给进程内对端加 `sent` 计数，那是**加固**，不是补漏。
（同族提醒：`src/fetch.rs:3120-3124` 自陈"摘掉 `Windowed` 的窗口夹时臂二不红"——那是**另一格**，
要验退化支必须动 `ToEof` 那一支，别跑错边。）

② **spec §6.2 后半句"在进度行显示 `slow-retry xN`"没有任何数据面**（该发评审用 `grep -rni "slow.retry" src/ tests/`
只命中 `consts.rs:8` 的常量与 `engine.rs:169/:182` 的退避表）。引擎只在**结束时**返回 `Report`，
`retries` 是全程总数、不含"是否已进入 30 s 慢重试/第几轮"；运行中唯一的外界观察是断点文件，而 `data_file` 那一支
**按 E13④ 根本不写断点**。⇒ **Task 9 计划里那行 `…  x4  dup 5.5%` 三样里约两样现在拿不到**（`dup %` 与 `slow-retry`）。
这条**不阻塞 T8 收口**（四场景都不看进度行），但**派 T9 之前必须有裁定**：
（甲）给引擎加一个只读运行态快照（共享原子量：已交付字节、`bytes_duplicated`、慢重试轮次），由引擎已有的轮询那一拍更新；
（乙）T9 的进度行**砍掉** `dup %` 与 `slow-retry xN` 两格并改 spec §5 的格式。
我的倾向是**甲**，理由：§5 那行格式是用户会看见的产品表面，砍格子等于把承诺改掉；而甲的成本是一个窄面小轮。
**等用户点头**（与 `T8-R5/R6/R7`、`ctrlc` 同批）。它与 STATUS 里那条"`engine::run` 阻塞、无回调 ⇒ 进度行没有 transport"
是同一件事的两侧：**先要有出口，出口里还得有数**。

③ **`E8①` 被两处注释读成了"全二进制只有一个 flush 调用点"，而它说的是"收尾调一次、绝不进每块循环"** ——
`engine.rs` 里两条互斥路线（`run_whole_once` 与主循环收尾）各有一处，**互为反例**，于是话是假的而行为是对的。
控制员已改口（`4073005`）：两处都改写成"E8① 的判据是频率与位置，不是唯一调用点；每条路线至多一次"。
危害写在注释里的那句：T9/T10 照原话会把"`data_file` 那一支也被主循环的 flush 覆盖"当成真。
同批修掉 **E11 转派给 Task 8 的第一条**（一直漏着）：`src/resume.rs` 的 `accept_ranges` 字段文档还写着
"`Accept-Ranges: bytes` 吗"，而 E11③/T6 评审 I1 早已把它定成**行为加权的合取**（头 + 这次真按 Range 答 206）
⇒ 契约面上的这一句会把 T9 读回头像。**另：E17 附记③ 留给最终评审的那个同类项已消解** —— 该发评审核过
`state.rs:1124` 附近是测试里的 spec 示例 JSON，没有"账单式"残留措辞。

## E20：Task 9 派发前必须改掉/补齐的九处（控制员预检 + T8→T9 派发面评审 F ＋ 8F-5 处置；**编号顺延，原先"预留 E18"那一档没有发出，不再回填**，2026-10-17）

Task 9 的简报（`:851-890`）是**写代码之前**拟的，下面这些已按现状代码复量过。**每一条都要抄进 T9 的 dispatch；与本节冲突时以本节为准。**

① **进度行没有 transport，而且缺的不只是出口**。`engine::run` 阻塞到结束才返回 `Report`（`Config` 里没有回调位、`main.rs` 是空壳），`:885` 那行渲染格式**没有数据来源**。且按 E19② 复量：**光有加出口还不够**——`dup %` 与 `slow-retry xN` 这两格今天**连数都没有**（全树只有 `consts::SLOW_RETRY_INTERVAL_SECS` 与退避表，`Report::retries` 是全程总数、不含"是否已进入慢重试/第几轮"）。⇒ **待用户裁（甲＝给引擎加运行态只读快照；乙＝砍这两格并改 spec §5 格式）**；裁完才能派 T9。倾向甲的理由与"若加快照需要哪些字段"那 8 条，见 `.superpowers/sdd/2026-10-15-dlm-kernel/t8-review-report-surface.md`。⚠ 其中一条是坑：**瞬时速率不许由 CLI 拿 `Report::elapsed` 自己平均**（那是全程平均，慢启动段会把它算低）。

② **别名表以 recon 取证 C3 为权威，`:853` 那张映射是反的**。计划把 `/q` 映射成 `--quiet`；取证表是 `/n` ＝静默、`/q` ＝下完退出、`/a` ＝只入队。⇒ v1 里 **`/q` 与 `/h` 只做"接受并忽略"并写明为什么**（它们的语义属于 GUI 侧的队列/前台行为，内核没有对应物），**`/n` 才是 `--quiet`**。

③ **`--quiet` 那一住不住 `Task.flags`**：spec §5 的位义只定义了 bit0/bit1/bit2 ⇒ 静默位住在 `Cli`，**不许塞进 `Task.flags`**（塞进去会改变断点文件里那份 flags 的比对语义）。

④ **`FLAG_ADD_ONLY`（bit0）必须由 CLI 在 `engine::run` 之前短路**。复量：`grep -rn "FLAG_ADD_ONLY" src/ tests/` 只有 `consts.rs` 的定义与 `resume.rs` 测试里当任意位用的两处 ⇒ **引擎零实现、零用例**，且 `run` 的文档已明写"本函数不看这一位"（8F-5 的 F4）。spec §5 的语义是"打印任务摘要后退出，一个字节都不许多打"⇒ **进了 `run` 就一定会打字节**，所以这一位在 CLI 侧消费。摘要里**不许出现口令**。

⑤ **`Task` 上没有任何常数访问器可填 `Config`**（原计划可能引导这一读法）。`limits()` 那一族已整条删除（E19③ 后继，8F-5 的 C1 + 控制员补刀 `b29b7ee`）：段大小取 `Config::min_seg`、连接数取 `Config::workers`，**别去 `Task` 上找**。照那个已删的三元组填会把 `--connections` / `--min-seg` **静默吞成默认值**——这就是当初删它的原因，别把它发明回来。

⑥ **`prelude_max`（试探预读上限）没有 CLI 选项**。spec §5 那十二个长选项里没有任何一个能改 `PROBE_READ_BYTES`；`probe::run` 的文档原先那句"Task 9 允许覆盖"是**假功能**，已作废（`b29b7ee`）。⇒ T9 **不许**自己发明 `--probe-bytes`。

⑦ **退出码只有两档，且判据是 `!ok` 而不是 `message.is_some()`**。`Report::message` 的契约是双向的（8F-5 的 F2）：`ok = true` 时也可以有 message（收尾删断点删不掉那一支），**写成"`message` 有值就非零退出"会把一次成功下载报成失败**。⇒ 非零码的**唯一**判据是 `!ok`（外加 `run` 返回 `Err`）。⚠ 想做 §6 那五类码（完成/校验不符/续传冲突中止/网络失败/落盘失败）目前**只能靠匹配 `message` 字符串**——引擎侧没有错误类别枚举；`StoreFailed`/`StateFailed` 走 `Err` 那一支还会**丢掉整份 `Report`**（已挂账）。v1 的处置：**0/1 两档**，摘要里把 `ok`、`fresh_reason`、`message` 三样都打出来。

⑧ **`retries` 与 `duplicated` 是跨断点文件累积的账**（8F-5 的 F3），不是"本次运行"的量 ⇒ CLI 打印时要照"这个断点生命周期一共…"说，别印成"本次"。例外：`--data-file` 那一支恒 0（既不读也不写断点，E13④）。

⑨ **两条打印纪律**：`Report::trace` 是**每事件一条、无上限**的调试载体 ⇒ CLI **不许**往 stdout 打它（`--quiet` 之外也不许）；最终落点没有 `Report` 字段，CLI 打 `task.target_path()` 即可——`Store` 不改名（`store.rs` 的 `final_path` 用例写明"Task 8/9 按它报最终文件"），两者同值是构造上的事实。另：`--sha256` 的**形状校验（64 位十六进制）归 CLI**，引擎侧 `hash_matches` 只做大小写不敏感等值、给错长度只会得到"不符"而不是"参数写错了"。

**留给 T9 之后那一发测试轮的具名欠账**（8F-5 的 F2 申报，控制员裁定登记而不造假夹具）：
`ok == true` + `message == Some(..)` 这一格**今天没有用例**——造它需要断点文件先被成功写出、随后 `fs::remove_file`
以一个非 `NotFound` 的错误失败，现成形状只有"另一个句柄占着该文件"这类 **Windows 共享冲突竞态**，没有确定形状可铺。
⇒ 要补的话得先给 `Engine` 开一个可注入的"删除失败"接缝（那是设计变更，不是测试技巧）。在此之前，
这一格只在 `Report::message` 的文档里如实标注，**不许拿恒真夹具凑数**（本项目已四次把这种夹具当证据）。

## E21：用户裁定（2026-10-17，"按推荐"）⇒ **E20① 走甲**，附 `RunSnapshot` 的契约与 T9 的切法

**裁定一览（编号沿用上一条消息里摊给用户的那八项）**

| # | 裁定 | 落点 |
|---|---|---|
| ① | **甲**：给引擎加运行态只读快照 + 回调出口 ⇒ E20① 关闭，T9 解锁 | 本节下半部＝派 9A 的判据 |
| ② | `T8-R5` 残余 **1.25×–1.74×** 重复流量：**v1 不动**，登记为已知残余，T10 实测后再判 | `state.rs` 的 `bytes_duplicated` 文档已写明，不重复登记 |
| ③ | `T8-R6` 收紧过程无观测量：**接受**，不为它开 `fetch` 每块上报 | 与本节"`--data-file` 那一支 0 次 tick"是同一条理由 |
| ④ | `T8-R7` 整请求 120 s 上界：**接受**（那是 spec R7 的形状） | 已写成已知残余，不动 |
| ⑤ | **`ctrlc` 不加**（依赖锁死 6 个 crate）⇒ spec §6.6 的"非零码退出"照旧留白 | STATUS 待裁清单划掉 |
| ⑥ | **未答**：idm-recon 那台 probe-server 的同源缺陷（不 `res.end()`、`/flaky` 从不断线）。这一项我没给推荐，"按推荐"不覆盖它 ⇒ **挂着，不动那边仓库** | 留给最终评审那一次 |
| ⑦ | T10 **不跑 `IDMan.exe` 并排对照**（默认不做） | 派 T10 时照此 |
| ⑧ | `git push` **等全分支最终评审之后**，且要单独点头 | 分支仍是本地 |

**甲的契约（9A 的验收判据；每条都可单独判红）**

```rust
// src/engine.rs —— 新增，不动任何其他形状
pub struct RunSnapshot {           // Copy、八格，渲染 spec §5 那一行 + §6.2 的 slow-retry 所需的最小闭包
    pub total_len: u64,            // 全长（`ProbeFacts::total_len`，多 worker 那一支恒已知）
    pub covered_bytes: u64,        // 已确认进覆盖集的字节
    pub run_delivered: u64,        // **本次运行**交出去落盘的字节
    pub run_bytes_duplicated: u64, // **本次运行**的 dup 增量（与上一格同拍取，`dup %` 的分子）
    pub instant_bytes_per_sec: u64,// **本次运行**这一拍的瞬时速率
    pub active_workers: usize,     // 这一拍 `WorkerState::Busy` 的条数（真在途）
    pub workers_total: usize,      // `cfg.workers`
    pub slow_retry_rounds: u32,    // 0 ⇒ 还没进慢重试；≥1 ⇒ 已排下第 N 次 30 s 慢重试
}
pub type OnTick = Arc<dyn Fn(&RunSnapshot) + Send + Sync>;
// Config 增一字段 pub on_tick: Option<OnTick>，Default = None。
```

1. **`covered_bytes = total_len.saturating_sub(sched.pending_bytes())`**。**不给 `scheduler` 加新 API**：`pending_bytes()` 的定义已经是"没进覆盖集的字节"，而 `scheduler.rs:1136` 那条等式 `pending_bytes() == total - covered`（`git blame` 实测＝`4a105e4`，8F-1）已经把它钉住了。寿命：**含上一轮续传来的字节**（文件级百分比本来就该按整文件算）。
2. ⚠ **`total_len` 不是 `Option`，而且不许拿 0 当"未知"哨兵**（可达性复量过的两条）：`probe` 那侧**拒绝**"206 说不出可用总长"（spec §2 注 11 / `probe.rs` 那条），所以多 worker 那一支的全长恒已知；`total_len == 0` 是**真的空文件**（`bytes */0` 那一格有用例）⇒ `report` 侧唯一的义务是**不许除零、不许印 `NaN%`**，这一格是活分支，写法要照它来。
3. `run_delivered` 直接用引擎已有的那个量（`Engine::run_delivered`）；`run_bytes_duplicated = stats.bytes_duplicated.saturating_sub(plan.stats.bytes_duplicated)`，起点＝`plan_from_decision` 交回的那份跨轮账。⇒ **这一对必须同一拍取**（分子分母不同源就是假数）。⚠ 它的寿命与 `Report::retries`/`Report::duplicated`（E20⑧ 的跨轮账）**故意不同**，两处不许互相"对齐"，文档各自写清。
4. `instant_bytes_per_sec = (run_delivered − 上一拍的 run_delivered) × 1_000_000 / 这一拍与上一拍的间隔微秒`，间隔下限 1 ms。⚠ **不许**把 `running_rate_bytes_per_sec(run_delivered, started.elapsed())` 交出去当显示速率——那是**全程平均**（慢启动段会把它算低，E20① 点名的坑）。那一支服务的是 `fetch::window_budget` 的分子，**两个数不许共用**。
5. `active_workers` 数的是 `WorkerState::Busy`。⚠ **不是** `Engine::any_in_flight()`（那个把 `Backoff` 也算进去，用途是"还有没有活没干完"，判据不同）。慢重试期间 `active_workers` 会是 0 而 `slow_retry_rounds ≥ 1` ⇒ 这一对组合是诚实的，**不许为好看把 Backoff 算进 active**。
6. `slow_retry_rounds = workers.map(|w| w.attempt.saturating_sub(FAST_RETRY_MAX)).max()`。`WorkerCtl::attempt` 的语义是"这条失败链已经排下来几次重试、一次成功就归零"。⇒ 写文档时必须说明**取的是跨 worker 最大值**，既不是"全场总轮数"也不是"每条链各几轮"；§6.2 只承诺一个 `slow-retry xN`，**每 worker 的 attempt 不在 v1 交付面里**（YAGNI）。
7. **节拍：零新计时器、零新线程、零新依赖**。挂在主循环已有的那一拍上（`LOOP_POLL = 1 s`，`last_tick` 每拍更新）：**每拍至多调一次**，收尾那一拍再调一次终值。⚠ **绝不进 `translate`/事件处理那每一块的循环**（与 E8① 同族判据），也不许为它开第二个线程。
8. **`--data-file` 那一支 0 次 tick**（`run_whole_once` 整程阻塞在一发请求里，本函数按 E8② **不开线程** ⇒ 中途没有任何引擎侧能读的量；要造一个就是 ③ 刚裁"不做"的那件事）。`on_tick` 的文档必须写明"哪些路有 tick、哪些没有"，CLI 不许把它当必然存在的进度源——那条路的权威只有 `Report`。
9. `Config` 是 `Clone` ⇒ 字段必须是 `Arc`；`RunSnapshot` 是 `Copy`、按引用交 ⇒ 只有闭包需要 `Send + Sync`，**快照本身不跨线程**。回调在**引擎线程里同步执行** ⇒ 文档写明"实现者不许在回调里做长阻塞 I/O，打印一行是预期用法"。
10. 命名：spec §2 模块表那行 `report | tick(&Snapshot)` 的 `Snapshot` 与已实现的 `state::Snapshot`（磁盘断点体）撞名 ⇒ 实现用 **`RunSnapshot`**、`report::tick(&RunSnapshot)`。这是命名落地选择，不改 spec 的承诺形状。

**9A 的验收下限**：`Config::default().on_tick == None` ⇒ **三闸一字不变（166 lib / 76 integration / clippy `-D warnings` 退出码 0）**，**既有断言一条都不许改**（撞闸就改入参，不改判据方向——本项目的老规矩）。新用例至少三格：(a) `None` 时一次都不调（用 `Arc<AtomicUsize>` 计数，不许拿打印当证据）；(b) 有 sink 时调用次数 ≥ 1 且 ≤ `运行秒数上取整 + 2`（节拍上界，防"每块都打"）；(c) 速率那一格在两拍之间交 X 字节、间隔 Y ms 时等于 `X×1e6/Y` 的整数商，**并且不等于全程平均**（这一格就是坑 4 的活体针：拿 `run_delivered/started.elapsed()` 当期望值它就红）。
**切法**：9A＝引擎面一条（上面 1–10），9B＝`cli.rs`/`report.rs`/`main.rs` 一条。**不按文件面切**（四发按文件面切的死了两发）。






