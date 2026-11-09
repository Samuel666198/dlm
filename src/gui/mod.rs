//! 产品层：队列 runner + 本地 HTTP + 原生 UI（本轮正在从浏览器 UI 迁往 egui）。
//!
//! 子模块各管一件事：
//! - [`snapshot`]：队列 → 视图的只读投影（纯函数）
//! - [`runner`]：单线程 actor，拥有队列与子进程
//! - [`http`]：本地 HTTP，只服务浏览器扩展
//!
//! 只绑 127.0.0.1；所有 HTTP 路径都要求 `?t=<token>`；队列不存 Cookie。

pub mod app;
pub mod clipboard;
pub mod http;
pub mod runner;
pub mod settings;
pub mod snapshot;

#[cfg(test)]
mod tests;

use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions;
use crate::consts::PROBE_READ_BYTES;
use crate::gui::app::theme;
use crate::queue::{self, QueueStore};

pub use snapshot::{QuotaView, TaskView, UpdateView, quota_view, snapshot_of};

/// P1 默认退避表：第一轮 30 秒、第二轮 120 秒，之后不再自动重试。
pub const DEFAULT_RETRY_BACKOFF: &[u64] = &[30, 120];
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
    /// 只起 runner + HTTP，**不初始化 eframe**。集成测试必须用它 —— 否则每跑一次
    /// 测试就弹十几个真窗口，无 GPU 的机器直接红掉（勘误 E2）。
    pub headless: bool,
    pub autostart: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: u64,
}

pub(crate) enum Ticket {
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
    let (tx, rx): (Sender<Ticket>, _) = mpsc::channel();
    let runner_cfg = config.clone();
    // UI 与 runner 共享同一份快照句柄：runner 写，UI 读。
    let ui_snapshot = Arc::clone(&snapshot);
    let ui_quota = Arc::clone(&quota_snapshot);
    let ui_update = Arc::clone(&update_snapshot);
    std::thread::Builder::new()
        .name("dlm-gui-runner".into())
        .spawn(move || runner::run(runner_cfg, store, rx, snapshot, quota_snapshot))
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

    // HTTP accept 循环始终在独立线程里：headless 时它是唯一的服务，
    // 将来接 eframe 时主线程要留给窗口事件循环（勘误 E1）。
    let http_listener = listener;
    let http_token = token.clone();
    let http_tx = tx.clone();
    let http_log_dir = config.log_dir.clone();
    std::thread::Builder::new()
        .name("dlm-gui-http".into())
        .spawn(move || {
            serve_http(
                http_listener,
                http_token,
                http_tx,
                http_snapshot,
                http_quota,
                http_update,
                http_log_dir,
            );
        })
        .context("启动 HTTP 线程失败")?;

    if config.headless {
        // 勘误 E2：集成测试只跑 runner + HTTP。主线程停在这里直到进程被杀，
        // 与之前的行为一致 —— 测试靠 kill 结束。
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    // 非 headless：起原生窗口。主线程归 eframe 的事件循环，
    // HTTP 已经在 serve_http 线程里跑（勘误 E1）。
    let ui_tx = tx.clone();

    let autostart = config.autostart;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size(theme::WINDOW_DEFAULT)
            .with_min_inner_size(theme::WINDOW_MIN)
            .with_title("dlm")
            .with_icon(window_icon())
            // `--autostart` 纯托盘：开机时没人看屏幕，只保证队列在跑（P3）。
            .with_visible(!config.autostart),
        ..Default::default()
    };
    eframe::run_native(
        "dlm",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            // 中文字形：default_fonts 只有拉丁，不装这个满屏方框。
            let cjk = app::fonts::install(&cc.egui_ctx);
            eprintln!(
                "dlm-gui: 中文字体 = {}{}",
                app::fonts::resolved_font_name().unwrap_or("（无，中文将显示为方框）"),
                if cjk { "" } else { " [装载失败]" }
            );
            let settings_dir = config
                .queue
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let mut app = app::App::new(ui_snapshot, ui_quota, ui_update, ui_tx, settings_dir);
            // `--autostart` 纯托盘：等 20 帧（≈330ms）窗口稳定后再藏起来。
            // 太早发 `Visible(false)` 会被随后的窗口初始化覆盖（实测）。
            app.start_hidden = if autostart { 20 } else { 0 };
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("起窗口失败：{e}"))?;

    let _ = fs::remove_file(&config.endpoint);
    Ok(())
}

/// HTTP accept 循环。抽成独立函数，headless 与将来的窗口模式共用一份。
#[allow(clippy::too_many_arguments)]
fn serve_http(
    listener: TcpListener,
    token: String,
    tx: Sender<Ticket>,
    snapshot: Arc<Mutex<Vec<TaskView>>>,
    quota_snapshot: Arc<Mutex<Option<QuotaView>>>,
    update_snapshot: Arc<Mutex<UpdateView>>,
    log_dir: PathBuf,
) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let token = token.clone();
        let tx = tx.clone();
        let snapshot = Arc::clone(&snapshot);
        let quota_snapshot = Arc::clone(&quota_snapshot);
        let update_snapshot = Arc::clone(&update_snapshot);
        let log_dir = log_dir.clone();
        std::thread::spawn(move || {
            let _ = http::handle_http(
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

pub fn random_token() -> String {
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

// ---------------------------------------------------------------- UI 辅助

/// UI 线程发 `Ticket::Add` 时用的丢弃式回执通道。
///
/// UI 不能阻塞等回执（会死锁：runner 要先处理完 ticket 才会回），所以给一个
/// 立刻丢弃的接收端。任务是否成功由快照里的状态体现，不靠这个回执。
pub(crate) fn discard_reply() -> Sender<std::result::Result<String, String>> {
    let (tx, _rx) = mpsc::channel();
    tx
}

/// 窗口图标：与托盘同一份像素。
///
/// `egui::IconData` 要 RGBA 与尺寸；`tray_icon::Icon` 不暴露像素，所以这里
/// 重新生成一份（32×32，成本可忽略）。
fn window_icon() -> eframe::egui::IconData {
    const SIZE: u32 = 32;
    let accent = [0xC0u8, 0x63, 0x40, 0xFF];
    let white = [0xFFu8, 0xFF, 0xFF, 0xFF];
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            let r = 7i32;
            let dx = if (x as i32) < r {
                r - x as i32
            } else if x as i32 > SIZE as i32 - 1 - r {
                x as i32 - (SIZE as i32 - 1 - r)
            } else {
                0
            };
            let dy = if (y as i32) < r {
                r - y as i32
            } else if y as i32 > SIZE as i32 - 1 - r {
                y as i32 - (SIZE as i32 - 1 - r)
            } else {
                0
            };
            if dx * dx + dy * dy > r * r {
                continue;
            }
            rgba[i..i + 4].copy_from_slice(&accent);
            let cx = SIZE as i32 / 2;
            let in_shaft = (x as i32 - cx).abs() <= 1 && (9..=18).contains(&(y as i32));
            let in_head = {
                let d = y as i32 - 17;
                (0..=6).contains(&d) && (x as i32 - cx).abs() <= 6 - d
            };
            if in_shaft || in_head {
                rgba[i..i + 4].copy_from_slice(&white);
            }
        }
    }
    eframe::egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}
