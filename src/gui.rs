//! C1–C3 产品层：队列 runner + 本地 HTTP + 内嵌单页 UI。
//!
//! 只绑 127.0.0.1；所有路径（含 `/`）都要求 `?t=<token>`；队列不存 Cookie。
//! runner 是单线程 actor：HTTP 只发命令，runner 拥有队列与子进程。

use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::actions;
use crate::consts::PROBE_READ_BYTES;
use crate::queue::{self, QueueStore, Task, TaskStatus};

/// 配额切片下限：新建任务首轮预读要落盘并计入配额，切片小于它一定超窗。
const MIN_QUOTA_SLICE: u64 = PROBE_READ_BYTES;

#[derive(Debug, Clone)]
pub struct GuiConfig {
    pub queue: PathBuf,
    pub out_dir: PathBuf,
    pub log_dir: PathBuf,
    pub dlm_exe: PathBuf,
    pub concurrency: usize,
    pub port: u16,
    /// 测试可传固定 token；生产默认随机生成。
    pub token: Option<String>,
    /// `Some(0)` = 显式关闭；`None` = 沿用队列里已存的设置。
    pub quota_limit: Option<u64>,
    pub quota_window_secs: Option<u64>,
    /// B10：浏览器投递发现的端点文件。
    pub endpoint: PathBuf,
    /// C3：更新清单 URL；`None` = 不检查、不发任何外网请求。
    pub update_url: Option<String>,
    /// C3：按分类建子目录（默认关；关时只打标签/筛选）。
    pub category_dirs: bool,
    /// 启动后用 Edge/Chrome 的 `--app` 模式打开 Web UI（窗口外壳，不是原生控件）。
    pub open: bool,
    /// P1：一个任务最多自动重试几轮（0 = 关闭自动重试）。超出的那一轮判 `failed`。
    pub max_task_retries: u32,
    /// P1：退避表（秒）。生产用 [`DEFAULT_RETRY_BACKOFF`]；集成测试注入 `[1, 1]`
    /// 把"重排 → 再跑"真正走完，不用真等两分钟。
    pub retry_backoff: &'static [u64],
    /// P3：跳过单实例探测，强制起一个新进程（测试要并行起多个 GUI 时用）。
    pub force_new_instance: bool,
    /// P3：`--autostart` 起的实例不开浏览器窗口，只监听并写端点文件。
    pub autostart: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateView {
    pub enabled: bool,
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub checked_at: Option<u64>,
    pub error: Option<String>,
}

impl UpdateView {
    fn disabled() -> Self {
        Self {
            enabled: false,
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: None,
            available: false,
            url: None,
            notes: None,
            checked_at: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaView {
    pub enabled: bool,
    pub limit: u64,
    pub window_secs: u64,
    pub window_started_at: u64,
    pub used: u64,
    pub resets_at: u64,
}

/// B10 端点文件的形状。`pub` 是为了让集成测试能直接读回自己那份、逐字段对账 ——
/// 它是 `dlm-gui` 与浏览器宿主之间的**外部契约**，不是内部细节。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: String,
    pub url: String,
    pub name: String,
    pub referer: Option<String>,
    pub status: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub log: String,
    pub progress: f64,
    pub total_len: Option<u64>,
    pub delivered: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub start_at: Option<u64>,
    pub hold_until: Option<u64>,
    pub on_complete: Option<String>,
    /// P1：已经自动重试过的轮数；UI 用它显示“重试 1/2”。
    pub attempts: u32,
    pub category: &'static str,
}

impl TaskView {
    fn from_task(task: &Task, out_dir: &Path) -> Self {
        let (mut total, _) = queue::progress(out_dir, &task.name);
        let delivered = queue::delivered(out_dir, &task.name);
        if total.is_none() && task.status == TaskStatus::Done {
            total = delivered;
        }
        let progress = match task.status {
            TaskStatus::Done => 100.0,
            _ => match (total, delivered) {
                (Some(total), Some(delivered)) if total > 0 => {
                    (delivered as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
                }
                _ => 0.0,
            },
        };
        Self {
            id: task.id.clone(),
            url: task.url.clone(),
            name: task.name.clone(),
            referer: task.referer.clone(),
            status: task.status.as_str().to_string(),
            pid: task.pid,
            exit_code: task.exit_code,
            log: task.log.to_string_lossy().to_string(),
            progress,
            total_len: total,
            delivered,
            created_at: task.created_at,
            updated_at: task.updated_at,
            start_at: task.start_at,
            hold_until: task.hold_until,
            on_complete: task.on_complete.clone(),
            attempts: task.attempts,
            category: queue::category_for(&task.name),
        }
    }
}

enum Ticket {
    Add {
        url: String,
        name: String,
        referer: Option<String>,
        start_at: Option<u64>,
        on_complete: Option<String>,
        reply: Sender<std::result::Result<String, String>>,
    },
    Pause(String),
    Resume(String),
    Remove(String),
    /// P2：打开文件 / 打开所在文件夹。**`mode` 与 `id` 就够了** —— 落点由 runner 从队列自己算，
    /// 不接受客户端交来的路径（见 `handle_http` 里那段注释）。
    Reveal {
        id: String,
        mode: actions::RevealMode,
        reply: Sender<std::result::Result<RevealReply, String>>,
    },
}

/// `/api/reveal` 的回执。`plan` 原样带回 argv，方便调用方（和测试）核对到底要启动什么，
/// 而**不是**让人相信一个 `ok:true`。
#[derive(Debug, Clone, Serialize)]
pub struct RevealReply {
    pub ok: bool,
    pub spawned: bool,
    pub path: String,
    pub program: String,
    pub args: Vec<String>,
}

pub fn run(config: GuiConfig) -> Result<()> {
    // P3 单实例：已经有实例在跑就复用它，不再抢一个端口、也不再起一套 runner。
    // 判据是"端口上真有人认得这个 token"，所以 GUI 被硬杀留下的陈旧端点文件不会挡住启动。
    if !config.force_new_instance
        && let Some(existing) = probe_existing_instance(&config.endpoint)
    {
        println!(
            "dlm-gui already running (pid {}): {}",
            existing.pid, existing.url
        );
        io::stdout().flush().ok();
        if config.open {
            open_app_window(&existing.url);
        }
        return Ok(());
    }
    fs::create_dir_all(&config.log_dir)
        .with_context(|| format!("建日志目录失败：{}", config.log_dir.display()))?;
    let mut store = QueueStore::load(config.queue.clone())?;
    let mut changed = store.recover_running();
    changed |= apply_quota_config(&config, &mut store)?;
    if changed {
        store.save()?;
    }
    let snapshot = Arc::new(Mutex::new(snapshot_of(&store, &config.out_dir)));
    let quota_snapshot = Arc::new(Mutex::new(quota_view(&store, queue::now_secs())));
    let update_snapshot = Arc::new(Mutex::new(UpdateView::disabled()));
    let http_snapshot = Arc::clone(&snapshot);
    let http_quota = Arc::clone(&quota_snapshot);
    let http_update = Arc::clone(&update_snapshot);
    if let Some(url) = config.update_url.clone() {
        let snapshot = Arc::clone(&update_snapshot);
        std::thread::Builder::new()
            .name("dlm-gui-update".into())
            .spawn(move || {
                let view = check_update(&url);
                if let Ok(mut guard) = snapshot.lock() {
                    *guard = view;
                }
            })
            .context("启动更新检查线程失败")?;
    }
    let (tx, rx) = mpsc::channel();
    let runner_cfg = config.clone();
    std::thread::Builder::new()
        .name("dlm-gui-runner".into())
        .spawn(move || runner(runner_cfg, store, rx, snapshot, quota_snapshot))
        .context("启动队列 runner 失败")?;

    let token = config.token.clone().unwrap_or_else(random_token);
    let listener = TcpListener::bind(("127.0.0.1", config.port)).context("绑定 127.0.0.1 失败")?;
    let port = listener.local_addr().context("拿不到监听端口")?.port();
    let endpoint = Endpoint {
        port,
        token: token.clone(),
        pid: std::process::id(),
        started_at: queue::now_secs(),
    };
    if let Err(err) = write_endpoint(&config.endpoint, &endpoint) {
        eprintln!("警告：写端点文件失败：{err:#}（浏览器投递会回退直连）");
    }
    let url = format!("http://127.0.0.1:{port}/?t={token}");
    println!("dlm-gui listening on {url}");
    io::stdout().flush().ok();
    // `--autostart` 只把队列跑起来，不弹浏览器窗口：开机时没人看着屏幕，
    // 而"队列在跑"正是自启唯一要保证的事。
    if config.open && !config.autostart {
        open_app_window(&url);
    }

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let token = token.clone();
        let tx = tx.clone();
        let snapshot = Arc::clone(&http_snapshot);
        let quota_snapshot = Arc::clone(&http_quota);
        let update_snapshot = Arc::clone(&http_update);
        let log_dir = config.log_dir.clone();
        std::thread::spawn(move || {
            let _ = handle_http(
                &mut stream,
                &token,
                &tx,
                &snapshot,
                &quota_snapshot,
                &update_snapshot,
                &log_dir,
            );
        });
    }
    let _ = fs::remove_file(&config.endpoint);
    Ok(())
}

fn apply_quota_config(config: &GuiConfig, store: &mut QueueStore) -> Result<bool> {
    if config.quota_window_secs == Some(0) {
        bail!("参数写错了：--quota-window 至少 1 秒");
    }
    let effective_limit = config
        .quota_limit
        .or(store.quota().map(|q| q.limit))
        .unwrap_or(0);
    if config.quota_window_secs.is_some() && effective_limit == 0 {
        bail!("参数写错了：--quota-window 需要先启用 --quota，或队列里已经存有配额");
    }
    if let Some(limit) = config.quota_limit
        && limit > 0
        && limit < MIN_QUOTA_SLICE
    {
        bail!(
            "参数写错了：--quota 至少 {} 字节（新建任务首轮预读要落盘并计入配额；给少了首轮就超窗）",
            MIN_QUOTA_SLICE
        );
    }
    if config.quota_limit == Some(0) && config.quota_window_secs.is_some() {
        bail!("参数写错了：--quota 0 是关闭配额，不能再给 --quota-window");
    }
    let mut changed = false;
    if let Some(limit) = config.quota_limit {
        let window = config
            .quota_window_secs
            .or_else(|| store.quota().map(|q| q.window_secs))
            .unwrap_or(3600);
        changed |= store.set_quota(limit, window);
    } else if let Some(window) = config.quota_window_secs
        && let Some(q) = store.quota()
    {
        changed |= store.set_quota(q.limit, window);
    }
    Ok(changed)
}

fn quota_view(store: &QueueStore, now: u64) -> Option<QuotaView> {
    store.quota().map(|q| QuotaView {
        enabled: true,
        limit: q.limit,
        window_secs: q.window_secs.max(1),
        window_started_at: q.window_started_at,
        used: q.used,
        resets_at: if q.window_started_at == 0 {
            now.saturating_add(q.window_secs.max(1))
        } else {
            q.resets_at()
        },
    })
}

fn version_key(raw: &str) -> Vec<u64> {
    raw.trim_start_matches(['v', 'V'])
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

fn version_is_newer(current: &str, latest: &str) -> bool {
    version_key(latest) > version_key(current)
}

fn check_update(url: &str) -> UpdateView {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let fail = |error: String| UpdateView {
        enabled: true,
        current: current.clone(),
        latest: None,
        available: false,
        url: None,
        notes: None,
        checked_at: Some(queue::now_secs()),
        error: Some(error),
    };
    let mut builder = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10));
    if reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"))
    {
        builder = builder.no_proxy();
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(err) => return fail(format!("{err}")),
    };
    let response = match client.get(url).send() {
        Ok(response) => response,
        Err(err) => return fail(format!("{err}")),
    };
    if !response.status().is_success() {
        return fail(format!("HTTP {}", response.status()));
    }
    let text = match response.text() {
        Ok(text) => text,
        Err(err) => return fail(format!("读更新清单失败：{err}")),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => return fail(format!("清单 JSON 解析失败：{err}")),
    };
    let Some(latest) = value
        .get("version")
        .or_else(|| value.get("latest"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
    else {
        return fail("更新清单缺 version/latest".to_string());
    };
    UpdateView {
        enabled: true,
        current: current.clone(),
        available: version_is_newer(&current, &latest),
        latest: Some(latest),
        url: value.get("url").and_then(Value::as_str).map(str::to_string),
        notes: value
            .get("notes")
            .and_then(Value::as_str)
            .map(str::to_string),
        checked_at: Some(queue::now_secs()),
        error: None,
    }
}

/// 用浏览器的 app 模式开一个没有地址栏的窗口；找不到 Edge/Chrome 就交给系统默认浏览器。
/// 这不是原生控件，只是把现有 Web UI 装进一个窗口外壳。
fn open_app_window(url: &str) {
    #[cfg(windows)]
    {
        let candidates = [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ];
        if let Some(browser) = candidates.iter().find(|p| Path::new(p).exists())
            && Command::new(browser)
                .arg(format!("--app={url}"))
                .spawn()
                .is_ok()
        {
            return;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if Command::new("open").arg(url).spawn().is_ok() {
            return;
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if Command::new("xdg-open").arg(url).spawn().is_ok() {
            return;
        }
    }
    let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
}

fn write_endpoint(path: &Path, endpoint: &Endpoint) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("建端点目录失败：{}", parent.display()))?;
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "endpoint.json".into());
    let tmp = path.with_file_name(format!("{name}.tmp"));
    fs::write(&tmp, serde_json::to_vec(endpoint)?)
        .with_context(|| format!("写端点临时文件失败：{}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("提交端点文件失败：{}", path.display()))?;
    Ok(())
}

/// 已经在跑的那个实例（P3 单实例）。
///
/// 判据只有一条：**端点文件能读出来，且那个端口上真有一个认得这个 token 的 dlm-gui**。
/// 只读文件不看端口是不行的 —— GUI 被硬杀会留下陈旧端点文件，那种情况必须能正常启动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningInstance {
    pub url: String,
    pub pid: u32,
}

/// 探测是否已有实例在跑；`None` = 没有（含端点文件不存在、读不懂、端口没人听、token 不对）。
///
/// 探测请求带 `t=<token>`，所以"端口上恰好有别的服务"不会被误认成本程序的实例。
pub fn probe_existing_instance(endpoint: &Path) -> Option<RunningInstance> {
    let raw = fs::read(endpoint).ok()?;
    let parsed: Endpoint = serde_json::from_slice(&raw).ok()?;
    let url = format!("http://127.0.0.1:{}/?t={}", parsed.port, parsed.token);
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(800))
        .connect_timeout(Duration::from_millis(300))
        .build()
        .ok()?;
    // 打一个只读、无副作用的路由；200 且 body 是 JSON 数组才算"这是我们自己"。
    let resp = client
        .get(format!(
            "http://127.0.0.1:{}/api/tasks?t={}",
            parsed.port, parsed.token
        ))
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().ok()?;
    serde_json::from_str::<serde_json::Value>(&body).ok()?;
    Some(RunningInstance {
        url,
        pid: parsed.pid,
    })
}

/// 开机自启要写进 `HKCU\...\Run` 的命令行。
///
/// 形状要求只有一条但很硬：**exe 路径含空格时必须整体带引号**，否则 Windows 会把
/// `C:\Program Files\...` 拆成两段，自启项变成一个打不开的坏命令。
/// 这里不用 `format!("\"{}\" ...")` 硬拼而是显式判断，是为了让"带空格"与"不带空格"
/// 两种形状都能被单测逐字比对。
pub fn autostart_command(exe: &Path) -> String {
    let path = exe.display().to_string();
    let quoted = if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path
    };
    format!("{quoted} --autostart")
}

/// `reg.exe` 的参数（P3）：写/删自启项。抽成纯函数便于单测，
/// 也避免把 `"HKCU\..."` 这种带反斜杠的字符串散在调用点里。
pub fn autostart_reg_args(exe: &Path) -> Vec<String> {
    vec![
        "add".into(),
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(),
        "/v".into(),
        AUTOSTART_VALUE.into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        autostart_command(exe),
        "/f".into(),
    ]
}

pub fn autostart_remove_args() -> Vec<String> {
    vec![
        "delete".into(),
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(),
        "/v".into(),
        AUTOSTART_VALUE.into(),
        "/f".into(),
    ]
}

/// 自启项的值名。固定一个，卸载按同名删，不碰这个键下别的值。
pub const AUTOSTART_VALUE: &str = "dlm-gui";

/// 真的写/删自启项。失败原样返回错误（调用方决定是警告还是退出）。
pub fn set_autostart(enabled: bool) -> Result<()> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().context("拿不到 dlm-gui 自身路径")?;
        let args = if enabled {
            autostart_reg_args(&exe)
        } else {
            autostart_remove_args()
        };
        let status = Command::new("reg.exe")
            .args(&args)
            .status()
            .context("调用 reg.exe 失败")?;
        // 删除一个本就不存在的值会退 1；关自启时那不是错误。
        if !status.success() && enabled {
            bail!("写入开机自启失败（reg.exe 退 {:?}）", status.code());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        bail!("开机自启目前只支持 Windows")
    }
}

fn snapshot_of(store: &QueueStore, out_dir: &Path) -> Vec<TaskView> {
    store
        .tasks()
        .iter()
        .map(|t| TaskView::from_task(t, out_dir))
        .collect()
}

fn runner(
    config: GuiConfig,
    mut store: QueueStore,
    rx: Receiver<Ticket>,
    snapshot: Arc<Mutex<Vec<TaskView>>>,
    quota_snapshot: Arc<Mutex<Option<QuotaView>>>,
) {
    let mut children: HashMap<String, Child> = HashMap::new();
    let mut reserved: HashMap<String, u64> = HashMap::new();
    let mut last_delivered: HashMap<String, u64> = HashMap::new();
    loop {
        let mut changed = false;
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ticket) => {
                changed |= apply(
                    ticket,
                    &mut store,
                    &mut children,
                    &mut reserved,
                    &mut last_delivered,
                    &config,
                )
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
            &config,
        );
        if changed {
            let _ = store.save();
        }
        if let Ok(mut guard) = snapshot.lock() {
            *guard = snapshot_of(&store, &config.out_dir);
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
) -> bool {
    match ticket {
        Ticket::Add {
            url,
            name,
            referer,
            start_at,
            on_complete,
            reply,
        } => {
            let category = config.category_dirs.then(|| queue::category_for(&name));
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
            let path = queue::target_path(&config.out_dir, &task.name);
            if !path.is_file() {
                let _ = reply.send(Err("落点文件不在了".to_string()));
                return false;
            }
            let plan = actions::reveal_plan(mode, &path);
            let spawned = match actions::execute(&plan) {
                Ok(()) => true,
                Err(err) => {
                    let _ = reply.send(Err(format!("{err:#}")));
                    return false;
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
enum FinishVerdict {
    /// 下完了（码 0）。
    Done,
    /// 配额到量（码 3）：回队列等窗口，**不计入重试轮数**。
    QuotaWait,
    /// 值得再试（码 1 且还有额度）：等 `backoff` 秒后重排。
    Retry { backoff: u64 },
    /// 判失败，把决定权交回用户。
    Fail,
}

fn finish_verdict(
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

/// P1 默认退避表：第一轮 30 秒、第二轮 120 秒，之后不再自动重试。
pub const DEFAULT_RETRY_BACKOFF: &[u64] = &[30, 120];

/// **失败自动重试的唯一判据**（P1）：给定"这个任务已经重试过几轮"和"配置允许几轮"，
/// 回答"这一次还重不重试、等多久"。
///
/// 两条独立的上限合取，谁先耗尽都停：
/// - `table`（默认 [`DEFAULT_RETRY_BACKOFF`]）—— 退避表走到表尾就没得再试；
/// - `max_task_retries` —— 用户给的轮数上限（`0` 就是关掉自动重试）。
///
/// 抽成纯函数是为了让"退出码 1 到底重排还是判失败"这条决定能被表格钉住，
/// 不用靠起进程、等 30 秒来看。
fn retry_decision(attempts: u32, max_task_retries: u32, table: &[u64]) -> Option<u64> {
    if attempts >= max_task_retries {
        return None;
    }
    queue::retry_backoff(attempts, table)
}

/// 一份额度能切出的单任务上限：够一轮预读才按槽位均分；只剩一份但不够均分时整份给第一个槽位。
fn quota_slice(available: u64, free_slots: usize) -> Option<u64> {
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

/// 构造 `dlm` 子进程参数。抽成纯函数是为了让"配额切片 / 完成后动作到底传没传"能单测。
fn child_args(task: &Task, out_dir: &Path, quota_slice: Option<u64>) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "--url".into(),
        task.url.clone().into(),
        "--out-dir".into(),
        out_dir.as_os_str().to_os_string(),
        "--out-name".into(),
        task.name.clone().into(),
        "--quiet".into(),
    ];
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

fn tick(
    store: &mut QueueStore,
    children: &mut HashMap<String, Child>,
    reserved: &mut HashMap<String, u64>,
    last_delivered: &mut HashMap<String, u64>,
    config: &GuiConfig,
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
        let now = store.delivered(&config.out_dir, &name).unwrap_or(0);
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
        let now = queue::now_secs();
        if let Some(task) = store.find_mut(&id) {
            let verdict = finish_verdict(
                code,
                task.attempts,
                config.max_task_retries,
                config.retry_backoff,
            );
            match verdict {
                FinishVerdict::Done => task.status = TaskStatus::Done,
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
        let next = store
            .tasks()
            .iter()
            .find(|t| {
                t.status == TaskStatus::Queued
                    && t.start_at.is_none_or(|s| s <= now)
                    && t.hold_until.is_none_or(|s| s <= now)
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
        let baseline = store.delivered(&config.out_dir, &task.name).unwrap_or(0);
        let mut command = Command::new(&config.dlm_exe);
        command
            .args(child_args(&task, &config.out_dir, quota_slice))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                children.insert(task.id.clone(), child);
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

fn random_token() -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

fn handle_http(
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
            INDEX_HTML.as_bytes(),
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

fn split_target(target: &str) -> (String, Vec<(String, String)>) {
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

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
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

const INDEX_HTML: &str = include_str!("ui.html");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_target_keeps_path_and_query_pairs() {
        let (path, query) = split_target("/api/log?id=abc&t=deadbeef");
        assert_eq!(path, "/api/log");
        assert!(query.iter().any(|(k, v)| k == "id" && v == "abc"));
        assert!(query.iter().any(|(k, v)| k == "t" && v == "deadbeef"));
    }

    #[test]
    fn find_bytes_finds_the_header_boundary() {
        assert_eq!(
            find_bytes(b"GET / HTTP/1.1\r\n\r\nbody", b"\r\n\r\n"),
            Some(14)
        );
        assert_eq!(find_bytes(b"none", b"\r\n\r\n"), None);
    }

    #[test]
    fn token_is_hex_and_not_constant() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn quota_slice_prefers_one_slot_when_only_one_slice_fits() {
        assert_eq!(
            quota_slice(MIN_QUOTA_SLICE - 1, 1),
            None,
            "不够一轮预读就不派"
        );
        assert_eq!(quota_slice(MIN_QUOTA_SLICE, 1), Some(MIN_QUOTA_SLICE));
        assert_eq!(quota_slice(MIN_QUOTA_SLICE * 2, 2), Some(MIN_QUOTA_SLICE));
        assert_eq!(
            quota_slice(MIN_QUOTA_SLICE + 10, 2),
            Some(MIN_QUOTA_SLICE + 10),
            "均分不够一份时整份给当前槽位，后续槽位看到 0"
        );
    }

    /// P3：自启命令最硬的形状要求 —— 路径含空格必须整体带引号，
    /// 否则 Windows 把 `C:\Program Files\...` 拆成两段，自启项变成打不开的坏命令。
    #[test]
    fn autostart_command_quotes_only_when_the_path_has_spaces() {
        let spaced = autostart_command(Path::new(r"C:\Program Files\dlm\dlm-gui.exe"));
        assert_eq!(
            spaced, r#""C:\Program Files\dlm\dlm-gui.exe" --autostart"#,
            "含空格的路径必须整体加引号"
        );
        assert!(
            spaced.starts_with('"') && spaced.ends_with(" --autostart"),
            "引号要包住整条路径、且 --autostart 在引号外：{spaced}"
        );

        let plain = autostart_command(Path::new(r"C:\dlm\dlm-gui.exe"));
        assert_eq!(
            plain, r"C:\dlm\dlm-gui.exe --autostart",
            "不含空格时不加引号（多余引号同样会让某些解析器犯迷糊）"
        );
        assert!(!plain.contains('"'), "{plain}");
    }

    /// P3：reg 参数逐字钉住 —— 值名、类型、以及"删除用同名"这三件事错了会污染别人的自启项。
    #[test]
    fn autostart_reg_args_write_and_delete_the_same_named_value() {
        let write = autostart_reg_args(Path::new(r"C:\dlm\dlm-gui.exe"));
        assert_eq!(write[0], "add");
        assert_eq!(
            write[1], r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "只碰 HKCU 的 Run 键"
        );
        assert_eq!(write[2], "/v");
        assert_eq!(write[3], AUTOSTART_VALUE);
        assert_eq!(write[4], "/t");
        assert_eq!(write[5], "REG_SZ");
        assert_eq!(write[6], "/d");
        assert_eq!(
            write[7],
            autostart_command(Path::new(r"C:\dlm\dlm-gui.exe"))
        );
        assert_eq!(write[8], "/f", "覆盖同名值，不弹交互提示");

        let remove = autostart_remove_args();
        assert_eq!(remove[0], "delete");
        assert_eq!(remove[1], write[1], "删除必须落在同一个键上");
        assert_eq!(remove[3], write[3], "删除必须用同一个值名");
    }

    /// P3：端点文件读不懂 / 端口没人听 / token 不对，都不能被当成"已有实例"。
    /// 这一格是"陈旧端点文件不阻塞启动"的直接判据。
    #[test]
    fn probe_existing_instance_says_none_for_every_stale_shape() {
        let dir = std::env::temp_dir().join(format!("dlm-instance-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1) 文件不存在。
        assert!(probe_existing_instance(&dir.join("missing.json")).is_none());

        // 2) 文件存在但不是 JSON。
        let garbage = dir.join("garbage.json");
        fs::write(&garbage, b"not json").unwrap();
        assert!(probe_existing_instance(&garbage).is_none());

        // 3) 端口没人听（挑一个刚释放掉的端口）。
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let dead = dir.join("dead.json");
        let endpoint = Endpoint {
            port,
            token: "tok".into(),
            pid: 999_999,
            started_at: 1,
        };
        fs::write(&dead, serde_json::to_vec(&endpoint).unwrap()).unwrap();
        assert!(
            probe_existing_instance(&dead).is_none(),
            "端口没人听时必须回 None（否则 GUI 被硬杀后永远起不来）"
        );

        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn finish_verdict_maps_every_contract_exit_code_to_one_outcome() {
        let table = DEFAULT_RETRY_BACKOFF; // [30, 120]
        assert_eq!(finish_verdict(Some(0), 0, 2, table), FinishVerdict::Done);
        assert_eq!(
            finish_verdict(Some(3), 0, 2, table),
            FinishVerdict::QuotaWait,
            "码 3 是配额到量，走等待而不是重试"
        );
        assert_eq!(
            finish_verdict(Some(3), 5, 2, table),
            FinishVerdict::QuotaWait,
            "重试额度用完也不影响配额等待：那是两件事"
        );
        assert_eq!(
            finish_verdict(Some(2), 0, 2, table),
            FinishVerdict::Fail,
            "码 2 是参数错，重试必然同样失败"
        );
        assert_eq!(
            finish_verdict(Some(1), 0, 2, table),
            FinishVerdict::Retry { backoff: 30 }
        );
        assert_eq!(
            finish_verdict(Some(1), 1, 2, table),
            FinishVerdict::Retry { backoff: 120 }
        );
        assert_eq!(
            finish_verdict(Some(1), 2, 2, table),
            FinishVerdict::Fail,
            "两轮用完就判失败"
        );
        assert_eq!(
            finish_verdict(Some(1), 0, 0, table),
            FinishVerdict::Fail,
            "上限 0 = 关掉自动重试"
        );
        assert_eq!(
            finish_verdict(None, 0, 2, table),
            FinishVerdict::Fail,
            "被信号打死（没有退出码）不猜，判失败"
        );
        assert_eq!(
            finish_verdict(Some(7), 0, 2, table),
            FinishVerdict::Fail,
            "契约外的退出码不重试"
        );
    }

    /// P1 的判据表：退避表与用户上限两条合取，谁先耗尽都停。
    #[test]
    fn retry_decision_stops_at_whichever_limit_runs_out_first() {
        let table = DEFAULT_RETRY_BACKOFF;
        // 默认上限 2：两轮都能重试，第三轮停。
        assert_eq!(
            retry_decision(0, 2, table),
            Some(30),
            "第 1 轮：30 秒后重排"
        );
        assert_eq!(
            retry_decision(1, 2, table),
            Some(120),
            "第 2 轮：120 秒后重排"
        );
        assert_eq!(retry_decision(2, 2, table), None, "第 3 轮：判 failed");

        // 上限 0 = 关掉自动重试：第一轮就不重试，而不是"先等 30 秒再判失败"。
        assert_eq!(retry_decision(0, 0, table), None, "上限 0 必须当场判失败");

        // 上限 1：只有第一轮。
        assert_eq!(retry_decision(0, 1, table), Some(30));
        assert_eq!(retry_decision(1, 1, table), None);

        // 上限比退避表长：退避表那一侧先耗尽，不会出现"等 0 秒"或负数。
        assert_eq!(retry_decision(2, 99, table), None, "退避表只有两档");
        assert_eq!(retry_decision(u32::MAX, u32::MAX, table), None);
    }

    /// `attempts` 必须原样进 `TaskView`：UI 的“重试 1/2”靠它，别在视图层被吞掉。
    #[test]
    fn task_view_carries_the_retry_attempt_count() {
        let task = Task {
            id: "1".into(),
            url: "https://example.com/a.bin".into(),
            name: "a.bin".into(),
            referer: None,
            status: TaskStatus::Queued,
            pid: None,
            exit_code: Some(1),
            log: "a.log".into(),
            created_at: 0,
            updated_at: 0,
            start_at: None,
            hold_until: Some(123),
            on_complete: None,
            attempts: 1,
            total_len: None,
            delivered: None,
        };
        let view = TaskView::from_task(&task, Path::new("D:/dl"));
        assert_eq!(view.attempts, 1);
        assert_eq!(view.hold_until, Some(123), "等待到点的时间要带出去");
    }

    #[test]
    fn child_args_carry_the_completion_action_and_the_quota_slice() {
        let mut task = Task {
            id: "1".into(),
            url: "https://example.com/a.bin".into(),
            name: "a.bin".into(),
            referer: Some("https://example.com/".into()),
            status: TaskStatus::Queued,
            pid: None,
            exit_code: None,
            log: "a.log".into(),
            created_at: 0,
            updated_at: 0,
            start_at: None,
            hold_until: None,
            on_complete: None,
            attempts: 0,
            total_len: None,
            delivered: None,
        };
        let plain: Vec<String> = child_args(&task, Path::new("D:/dl"), None)
            .into_iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(
            plain
                .windows(2)
                .any(|w| w[0] == "--referer" && w[1] == "https://example.com/")
        );
        assert!(!plain.iter().any(|s| s == "--on-complete"));
        assert!(!plain.iter().any(|s| s == "--max-bytes"));

        task.on_complete = Some("open".into());
        let with_action: Vec<String> = child_args(&task, Path::new("D:/dl"), Some(4096))
            .into_iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(
            with_action
                .windows(2)
                .any(|w| w[0] == "--on-complete" && w[1] == "open")
        );
        assert!(
            with_action
                .windows(2)
                .any(|w| w[0] == "--max-bytes" && w[1] == "4096")
        );
    }

    #[test]
    fn version_comparison_is_numeric_by_dotted_segment() {
        assert!(!version_is_newer("0.1.0", "0.1.0"));
        assert!(version_is_newer("0.1.0", "0.1.1"));
        assert!(
            version_is_newer("0.1.0", "0.10.0"),
            "10 > 1，不是字符串比较"
        );
        assert!(version_is_newer("0.9.9", "0.10.0"));
        assert!(version_is_newer("0.9.0", "v1.0"));
        assert!(!version_is_newer("1.0.0", "0.99.99"), "远端更旧不算新版本");
        assert!(!version_is_newer("1.0.0", "not-a-version"));
    }

    #[test]
    fn update_check_reads_a_local_manifest() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = r#"{"version":"99.0.0","url":"https://example.invalid/dl","notes":"test"}"#;
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let view = check_update(&format!("http://127.0.0.1:{port}/update.json"));
        handle.join().unwrap();
        assert!(view.enabled, "{view:?}");
        assert_eq!(view.latest.as_deref(), Some("99.0.0"));
        assert!(view.available, "99.0.0 应比当前版本新：{view:?}");
        assert_eq!(view.url.as_deref(), Some("https://example.invalid/dl"));
        assert_eq!(view.notes.as_deref(), Some("test"));
        assert!(view.error.is_none(), "{view:?}");
        assert!(view.checked_at.is_some());
    }

    #[test]
    fn endpoint_file_contains_the_port_and_token() {
        let dir = std::env::temp_dir().join(format!("dlm-endpoint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("endpoint.json");
        let endpoint = Endpoint {
            port: 51234,
            token: "tok".into(),
            pid: 7,
            started_at: 9,
        };
        write_endpoint(&path, &endpoint).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["port"], 51234);
        assert_eq!(value["token"], "tok");
        assert_eq!(value["pid"], 7);
        assert_eq!(value["started_at"], 9);
        let _ = fs::remove_dir_all(dir);
    }
}
