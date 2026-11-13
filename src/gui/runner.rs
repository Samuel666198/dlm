//! 队列 runner：单线程 actor，拥有队列与子进程。
//!
//! HTTP 与原生 UI 都只发 `Ticket`，谁都不直接碰队列。这样两条入口共用一套状态机，
//! 不会出现"UI 里暂停了但 HTTP 看还是 running"。

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::actions;
use crate::queue::{self, QueueStore, Task, TaskStatus};

use super::snapshot::{QuotaView, TaskView, quota_view, snapshot_of};
use super::{GuiConfig, MIN_QUOTA_SLICE, RevealReply, RuntimeTuning, Ticket};
pub(crate) fn run(
    config: GuiConfig,
    mut store: QueueStore,
    rx: Receiver<Ticket>,
    snapshot: Arc<Mutex<Vec<TaskView>>>,
    quota_snapshot: Arc<Mutex<Option<QuotaView>>>,
    tuning: RuntimeTuning,
) {
    let mut children: HashMap<String, Child> = HashMap::new();

    let mut reserved: HashMap<String, u64> = HashMap::new();
    let mut last_delivered: HashMap<String, u64> = HashMap::new();
    // E18：运行时参数由设置投影而来，`ApplyRuntime` 会就地替换。
    // 之前这里是裸的 `per_host: usize`（调用方硬编码 8），连接数与限速压根没往下传。
    let mut tuning = tuning;
    // 跨任务的 host 连接预算（勘误 E4）：这是改 scheduler.rs 做不到的那一层。
    let mut gate = HostGate::new(tuning.per_host);
    loop {
        let mut changed = false;
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ticket) => {
                // `ApplyRuntime` 不碰队列，所以它不走 `apply`（那条路返回"队列变没变"）。
                // 换 tuning 与 gate 都在这儿做，且**立刻生效**：下一轮 `tick` 派发时就用新值。
                if let Ticket::ApplyRuntime(next) = &ticket {
                    tuning = next.clone();
                    gate.set_per_host(next.per_host);
                    changed = true;
                } else {
                    changed |= apply(
                        ticket,
                        &mut store,
                        &mut children,
                        &mut reserved,
                        &mut last_delivered,
                        &config,
                        &tuning,
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                for (_, mut child) in children.drain() {
                    let _ = child.kill();
                }
                return;
            }
        }
        changed |= tick(
            &mut store,
            &mut children,
            &mut reserved,
            &mut last_delivered,
            &mut gate,
            &config,
            &tuning,
        );
        if changed {
            let _ = store.save();
        }
        if let Ok(mut guard) = snapshot.lock() {
            *guard = snapshot_of(&store, &tuning.out_dir);
        }
        if let Ok(mut guard) = quota_snapshot.lock() {
            *guard = quota_view(&store, queue::now_secs());
        }
    }
}

fn apply(
    ticket: Ticket,
    store: &mut QueueStore,
    children: &mut HashMap<String, Child>,
    reserved: &mut HashMap<String, u64>,
    last_delivered: &mut HashMap<String, u64>,
    config: &GuiConfig,
    tuning: &RuntimeTuning,
) -> bool {
    match ticket {
        // 在 `run` 里就地处理（要动 `tuning` 与 `gate`，两者都不在这条路的参数里）。
        // 这里必须是显式分支：漏了它穷举就不全，编译器会红 —— 这正是想要的。
        Ticket::ApplyRuntime(_) => false,
        Ticket::Add {
            url,
            name,
            referer,
            start_at,
            on_complete,
            reply,
        } => {
            let category = tuning.category_dirs.then(|| queue::category_for(&name));
            match store.add(
                &url,
                &name,
                referer,
                start_at,
                on_complete,
                category,
                &config.log_dir,
            ) {
                Ok(task) => {
                    let _ = reply.send(Ok(task.id));
                    true
                }
                Err(err) => {
                    eprintln!("dlm-gui: add failed: {err:#}");
                    let _ = reply.send(Err(format!("{err:#}")));
                    false
                }
            }
        }
        Ticket::Pause(id) => {
            let Some(task) = store.find_mut(&id) else {
                return false;
            };
            if task.status == TaskStatus::Running
                && let Some(mut child) = children.remove(&id)
            {
                let _ = child.kill();
            }
            reserved.remove(&id);
            last_delivered.remove(&id);
            if matches!(task.status, TaskStatus::Running | TaskStatus::Queued) {
                task.status = TaskStatus::Paused;
                task.pid = None;
                task.updated_at = queue::now_secs();
                return true;
            }
            false
        }
        Ticket::Resume(id) => {
            let Some(task) = store.find_mut(&id) else {
                return false;
            };
            if matches!(task.status, TaskStatus::Paused | TaskStatus::Failed) {
                task.status = TaskStatus::Queued;
                task.pid = None;
                task.exit_code = None;
                task.hold_until = None;
                task.updated_at = queue::now_secs();
                return true;
            }
            false
        }
        Ticket::Remove(id) => {
            if let Some(mut child) = children.remove(&id) {
                let _ = child.kill();
            }
            reserved.remove(&id);
            last_delivered.remove(&id);
            if let Some(task) = store.remove(&id) {
                let _ = fs::remove_file(task.log);
                return true;
            }
            false
        }
        Ticket::Reveal { id, mode, reply } => {
            // 落点**只能**由队列里的任务名推出：`name` 在入队时已过 `validate_name`
            // （不含 `/ \ :`），所以 `out_dir.join(name)` 不会越出目录。
            // 这条路线里没有任何一格接受客户端交来的路径 —— 那是通用"打开任意文件"接口，不做。
            let Some(task) = store.find_mut(&id) else {
                let _ = reply.send(Err("任务不存在".to_string()));
                return false;
            };
            if task.status != TaskStatus::Done {
                let _ = reply.send(Err("任务还没下完".to_string()));
                return false;
            }
            let path = queue::target_path(&tuning.out_dir, &task.name);
            if !path.is_file() {
                let _ = reply.send(Err("落点文件不在了".to_string()));
                return false;
            }
            let plan = actions::reveal_plan(mode, &path);
            // 门禁/测试里不许真弹资源管理器窗口：落点是用例的临时目录，用完就删，
            // 窗口会立刻变成「位置不可用」对话框留在用户桌面上。回执照实说 `spawned: false`。
            let spawned = if actions::shell_suppressed() {
                false
            } else {
                match actions::execute(&plan) {
                    Ok(()) => true,
                    Err(err) => {
                        let _ = reply.send(Err(format!("{err:#}")));
                        return false;
                    }
                }
            };
            let _ = reply.send(Ok(RevealReply {
                ok: true,
                spawned,
                path: path.to_string_lossy().to_string(),
                program: plan.program.to_string_lossy().to_string(),
                args: plan
                    .args
                    .iter()
                    .map(|a| a.to_string_lossy().to_string())
                    .collect(),
            }));
            false
        }
    }
}

/// 一个任务跑完之后该怎么处置（P1）。抽成纯函数与枚举，是为了让"退出码 → 状态"这张表
/// 能被单元用例逐格钉住：退出档是 CLI 契约，改错一格就是让用户白等两轮或永远等不到重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishVerdict {
    /// 下完了（码 0）。
    Done,
    /// 配额到量（码 3）：回队列等窗口，**不计入重试轮数**。
    QuotaWait,
    /// 值得再试（码 1 且还有额度）：等 `backoff` 秒后重排。
    Retry { backoff: u64 },
    /// 判失败，把决定权交回用户。
    Fail,
}

pub(crate) fn finish_verdict(
    code: Option<i32>,
    attempts: u32,
    max_task_retries: u32,
    table: &[u64],
) -> FinishVerdict {
    match code {
        Some(0) => FinishVerdict::Done,
        Some(3) => FinishVerdict::QuotaWait,
        // 码 2 是参数层（clap 用法错误或我们的边界校验）：重试必然同样失败。
        Some(2) => FinishVerdict::Fail,
        // 码 1 是"跑到结论但没下完"，也是唯一值得自动再试的一档。
        Some(1) => match retry_decision(attempts, max_task_retries, table) {
            Some(backoff) => FinishVerdict::Retry { backoff },
            None => FinishVerdict::Fail,
        },
        // 其它非零码（含被信号打死时的 `None`）不在契约里：不猜，判失败。
        _ => FinishVerdict::Fail,
    }
}

/// **失败自动重试的唯一判据**（P1）：给定"这个任务已经重试过几轮"和"配置允许几轮"，
/// 回答"这一次还重不重试、等多久"。
///
/// 两条独立的上限合取，谁先耗尽都停：
/// - `table`（默认 [`DEFAULT_RETRY_BACKOFF`]）—— 退避表走到表尾就没得再试；
/// - `max_task_retries` —— 用户给的轮数上限（`0` 就是关掉自动重试）。
///
/// 抽成纯函数是为了让"退出码 1 到底重排还是判失败"这条决定能被表格钉住，
/// 不用靠起进程、等 30 秒来看。
pub(crate) fn retry_decision(attempts: u32, max_task_retries: u32, table: &[u64]) -> Option<u64> {
    if attempts >= max_task_retries {
        return None;
    }
    queue::retry_backoff(attempts, table)
}

/// 一份额度能切出的单任务上限：够一轮预读才按槽位均分；只剩一份但不够均分时整份给第一个槽位。
pub(crate) fn quota_slice(available: u64, free_slots: usize) -> Option<u64> {
    if available < MIN_QUOTA_SLICE {
        return None;
    }
    let free = free_slots.max(1) as u64;
    let per_slot = available / free;
    Some(if per_slot >= MIN_QUOTA_SLICE {
        per_slot
    } else {
        available
    })
}

/// 构造 `dlm` 子进程参数。抽成纯函数是为了让"配额切片 / 完成后动作 / **连接数与限速**
/// 到底传没传"能单测。
///
/// **E18**：`--connections` 与 `--max-speed` 曾经根本没出现在这张表里 ——
/// 设置面板能改能存，子进程却永远跑 `DEFAULT_CONNECTIONS` 且不限速。
/// 现在两者都从 [`RuntimeTuning`] 来，且**总是显式传**：
/// 不传时 `dlm` 用自己的默认值，那个默认值与设置面板显示的值是两本账。
pub(crate) fn child_args(
    task: &Task,
    out_dir: &Path,
    quota_slice: Option<u64>,
    tuning: RuntimeTuning,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "--url".into(),
        task.url.clone().into(),
        "--out-dir".into(),
        out_dir.as_os_str().to_os_string(),
        "--out-name".into(),
        task.name.clone().into(),
        "--quiet".into(),
        "--connections".into(),
        tuning.connections.max(1).to_string().into(),
    ];
    // 限速 0 = 不限，那正是 `dlm` 的默认；传 0 与不传等价，但显式传能让
    // "设置里的 0"与"忘了传"在 argv 上区分开，排查时省一轮。
    if tuning.max_speed_kib > 0 {
        args.push("--max-speed".into());
        args.push(tuning.max_speed_bytes().to_string().into());
    }
    if let Some(referer) = &task.referer {
        args.push("--referer".into());
        args.push(referer.into());
    }
    if let Some(action) = &task.on_complete {
        args.push("--on-complete".into());
        args.push(action.into());
    }
    if let Some(slice) = quota_slice {
        args.push("--max-bytes".into());
        args.push(slice.to_string().into());
    }
    args
}

/// 跨任务的 host 连接预算（勘误 E4）。
///
/// 打包成结构体是因为它有两个必须一起维护的映射：已用量、每个任务占了多少。
/// 分开传会让 `tick` 的参数超过 clippy 的 7 个上限，也容易漏掉归还。
pub(crate) struct HostGate {
    /// 每个 host 已占用的连接数。
    pub usage: crate::gui::host_budget::HostUsage,
    /// 每个在跑任务占了这个 host 几条，结束时按同数归还。
    pub per_task: HashMap<String, (String, usize)>,
    /// 同一 host 的连接上限。0 = 不限。
    pub per_host: usize,
}

impl HostGate {
    pub fn new(per_host: usize) -> Self {
        Self {
            usage: HashMap::new(),
            per_task: HashMap::new(),
            per_host,
        }
    }

    /// 这个 host 还能不能再起一单要 `want` 条连接的任务。
    pub fn allows(&self, url: &str, want: usize) -> bool {
        let used = self
            .usage
            .get(&crate::gui::host_budget::host_of(url))
            .copied()
            .unwrap_or(0);
        crate::gui::host_budget::can_start(used, want, self.per_host)
    }

    /// E18：设置改了每 host 上限，就地替换。
    ///
    /// **只换上限，不动已用量与 per_task**：已占的连接是真实存在的，
    /// 清掉它们会让"归还"算错（归还时按 `per_task` 里记的数扣）。
    /// 新上限在**下一次** `allows` 判定时生效 —— 已经在跑的任务不受影响，
    /// 这是对的：改设置不该把正在下载的任务踢掉。
    pub fn set_per_host(&mut self, per_host: usize) {
        self.per_host = per_host;
    }

    pub fn take(&mut self, task_id: &str, url: &str, want: usize) {
        let host = crate::gui::host_budget::host_of(url);
        crate::gui::host_budget::acquire(&mut self.usage, &host, want);
        self.per_task.insert(task_id.to_string(), (host, want));
    }

    pub fn give_back(&mut self, task_id: &str) {
        if let Some((host, want)) = self.per_task.remove(task_id) {
            crate::gui::host_budget::release(&mut self.usage, &host, want);
        }
    }
}

fn tick(
    store: &mut QueueStore,
    children: &mut HashMap<String, Child>,
    reserved: &mut HashMap<String, u64>,
    last_delivered: &mut HashMap<String, u64>,
    gate: &mut HostGate,
    config: &GuiConfig,
    tuning: &RuntimeTuning,
) -> bool {
    let mut changed = false;
    // 1) 采样：按覆盖量增量记配额。先采再 reap，子进程退出前最后一截也算得到。
    let running: Vec<(String, String)> = store
        .tasks()
        .iter()
        .filter(|t| children.contains_key(&t.id))
        .map(|t| (t.id.clone(), t.name.clone()))
        .collect();
    let mut charged = false;
    for (id, name) in running {
        let now = store.delivered(&tuning.out_dir, &name).unwrap_or(0);
        let previous = last_delivered.insert(id.clone(), now).unwrap_or(now);
        if now > previous {
            charged |= store.quota_charge(now - previous);
        }
    }
    changed |= charged;

    // 2) 收官：退出码 3 = 预算到量，任务回队列等下一个窗口；其余照旧。
    let mut finished: Vec<(String, Option<i32>)> = Vec::new();
    for (id, child) in children.iter_mut() {
        match child.try_wait() {
            Ok(Some(status)) => finished.push((id.clone(), status.code())),
            Ok(None) => {}
            Err(_) => finished.push((id.clone(), None)),
        }
    }
    for (id, code) in finished {
        children.remove(&id);
        reserved.remove(&id);
        last_delivered.remove(&id);
        // 任务结束，把 host 预算还回去。
        gate.give_back(&id);
        let now = queue::now_secs();
        if let Some(task) = store.find_mut(&id) {
            let verdict = finish_verdict(
                code,
                task.attempts,
                config.max_task_retries,
                config.retry_backoff,
            );
            match verdict {
                FinishVerdict::Done => {
                    task.status = TaskStatus::Done;
                    if tuning.scan_after_done {
                        crate::gui::scan::spawn_scan(
                            &queue::target_path(&tuning.out_dir, &task.name),
                            &task.log,
                        );
                    }
                }
                // 码 3 走配额等待，不是重试：`attempts` 不动，否则配额轮会吃掉重试额度。
                FinishVerdict::QuotaWait => task.status = TaskStatus::Queued,
                FinishVerdict::Retry { backoff } => {
                    task.status = TaskStatus::Queued;
                    task.hold_until = Some(now.saturating_add(backoff));
                    task.attempts = task.attempts.saturating_add(1);
                }
                FinishVerdict::Fail => {
                    task.status = TaskStatus::Failed;
                    task.hold_until = None;
                }
            }
            task.exit_code = code;
            task.pid = None;
            task.updated_at = now;
            changed = true;
        }
    }

    // 3) 窗口推进（固定窗口，到点清零并释放等待）。
    let now = queue::now_secs();
    changed |= store.quota_tick(now);

    // 上一轮因预算退出、而本窗剩余仍不足一轮预读的任务：明确进入配额等待，别每 500 ms 空转。
    if let Some(quota) = store.quota() {
        let reserved_total = reserved.values().copied().fold(0u64, u64::saturating_add);
        let available = quota
            .limit
            .saturating_sub(quota.used)
            .saturating_sub(reserved_total);
        if available < MIN_QUOTA_SLICE
            && let Some(reset) = store.quota_next_reset()
        {
            let waiting: Vec<String> = store
                .tasks()
                .iter()
                .filter(|t| {
                    t.status == TaskStatus::Queued
                        && t.exit_code == Some(3)
                        && t.hold_until.is_none()
                        && t.start_at.is_none_or(|s| s <= now)
                })
                .map(|t| t.id.clone())
                .collect();
            for id in waiting {
                changed |= store.quota_hold(&id, reset);
            }
        }
    }

    // 4) 派发。
    while children.len() < config.concurrency {
        // E18：连接数从设置来，不再是编译期常量。
        let want = connections_per_task(tuning);
        let next = store
            .tasks()
            .iter()
            .find(|t| {
                t.status == TaskStatus::Queued
                    && t.start_at.is_none_or(|s| s <= now)
                    && t.hold_until.is_none_or(|s| s <= now)
                    // 按 host 的连接预算（勘误 E4）：同 host 上已占用的 + 这一单
                    // 要用的超过上限，就跳过它去看下一个（可能是别的 host）。
                    && gate.allows(&t.url, want)
            })
            .cloned();
        let Some(task) = next else { break };
        let quota_slice = if let Some(quota) = store.quota() {
            if quota.window_started_at == 0 {
                changed |= store.quota_start(now);
            }
            let reserved_total = reserved.values().copied().fold(0u64, u64::saturating_add);
            let available = quota
                .limit
                .saturating_sub(quota.used)
                .saturating_sub(reserved_total);
            let free_slots = (config.concurrency - children.len()).max(1);
            let Some(slice) = quota_slice(available, free_slots) else {
                break;
            };
            Some(slice)
        } else {
            None
        };

        let log = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&task.log)
        {
            Ok(file) => file,
            Err(err) => {
                eprintln!("dlm-gui: create job log failed: {err}");
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Failed;
                    t.updated_at = now;
                }
                changed = true;
                continue;
            }
        };
        let log_err = match log.try_clone() {
            Ok(file) => file,
            Err(err) => {
                eprintln!("dlm-gui: clone job log failed: {err}");
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Failed;
                    t.updated_at = now;
                }
                changed = true;
                continue;
            }
        };
        let baseline = store.delivered(&tuning.out_dir, &task.name).unwrap_or(0);
        let mut command = Command::new(&config.dlm_exe);
        command
            .args(child_args(
                &task,
                &tuning.out_dir,
                quota_slice,
                tuning.clone(),
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                children.insert(task.id.clone(), child);
                // 记下这一单占了这个 host 几条，结束时按同数归还。
                gate.take(&task.id, &task.url, want);
                if let Some(slice) = quota_slice {
                    reserved.insert(task.id.clone(), slice);
                }
                last_delivered.insert(task.id.clone(), baseline);
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Running;
                    t.pid = Some(pid);
                    t.exit_code = None;
                    t.updated_at = now;
                }
                changed = true;
            }
            Err(err) => {
                eprintln!("dlm-gui: spawn dlm failed: {err}");
                reserved.remove(&task.id);
                last_delivered.remove(&task.id);
                if let Some(t) = store.find_mut(&task.id) {
                    t.status = TaskStatus::Failed;
                    t.updated_at = now;
                }
                changed = true;
            }
        }
    }
    changed
}

/// 一个任务会开几条连接。
///
/// **E18 之前**这里返回 `consts::DEFAULT_CONNECTIONS`（写死 8），因为队列不透传
/// `--connections`。现在从 [`RuntimeTuning`] 取，设置面板改了立刻生效。
///
/// 这个数必须与 `child_args` 传给子进程的 `--connections` **是同一个值** ——
/// 否则 host 预算按 8 算、实际开 16 条，每 host 上限就形同虚设。
/// 两处都读同一个 `tuning.connections`，这条一致性由构造方式保证，不需要再对账。
fn connections_per_task(tuning: &RuntimeTuning) -> usize {
    tuning.connections.max(1)
}

/// 给 `gui::tests` 用的只读投影。
///
/// 存在的唯一理由：E18 的用例要断言「argv 里的 `--connections` 与 host 预算
/// 用的是同一个值」。把 `connections_per_task` 直接 `pub(crate)` 也能测，
/// 但那会让这个内部函数在整个 crate 里可见 —— 加一层命名明确的测试口，
/// 以后有人想在生产代码里调它会先看到 `_for_test` 三个字。
#[cfg(test)]
pub(crate) fn connections_per_task_for_test(tuning: &RuntimeTuning) -> usize {
    connections_per_task(tuning)
}
