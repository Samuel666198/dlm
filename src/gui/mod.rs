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
pub mod host_budget;
pub mod http;
pub mod naming;
pub mod runner;
pub mod scan;
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
use crate::updater;

pub use snapshot::{QuotaView, TaskView, UpdateView, quota_view, snapshot_of};

/// P1 默认退避表：第一轮 30 秒、第二轮 120 秒，之后不再自动重试。
pub const DEFAULT_RETRY_BACKOFF: &[u64] = &[30, 120];
/// 配额切片下限：新建任务首轮预读要落盘并计入配额，切片小于它一定超窗。
const MIN_QUOTA_SLICE: u64 = PROBE_READ_BYTES;

/// 本机**发行版**版本：用户感知的「软件版本」。
///
/// 与 `Cargo.toml` 的 `core` 版本分开（见 `docs/evidence/2026-11-12-更新功能裁定.md`）：
/// 用户不在乎「内核 0.1.0 变 0.1.1」，只在乎「这个软件更新了」。
/// 更新判断只比这一个字段。
///
/// **发版时这个常量要跟着改**，否则刚装上的新版本会认为自己还是旧的，
/// 每启动一次就提示一次「有更新」。
pub const RELEASE: &str = "0.3.1";

/// 默认更新清单地址。
///
/// 放在 `main` 分支的仓库根：raw.githubusercontent 走 CDN、不吃 GitHub API 配额
/// （未认证 API 只有 60 次/小时，多开几次或调试时很容易打满）。
pub const DEFAULT_UPDATE_URL: &str =
    "https://raw.githubusercontent.com/Samuel666198/dlm/main/latest.json";

/// 启动后延迟多久再查更新（秒）。
///
/// 启动已经 573 ms 超目标（E15），检查放在独立线程里不阻塞主线程；再加一层延迟是为了
/// 让冷启动的第一秒只做「把窗口画出来」这一件事。副作用也有用：集成测试起 GUI 后
/// 往往一两秒内就杀掉，于是根本不会发这次外网请求。
pub const UPDATE_CHECK_DELAY_SECS: u64 = 10;

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
    /// 决定「启动多久之后才去查更新」：自动检查用 [`UPDATE_CHECK_DELAY_SECS`]
    /// （别跟首帧抢时间），**显式给了 `--update-url` 就是 0** ——
    /// 那时是有人在专门验证更新链路，等十秒只会让验证变慢。
    pub update_check_delay_secs: u64,
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
    /// E18：设置保存后通知 runner 换一份**运行时**参数。
    ///
    /// 只带 runner 真正用得上的三项，不带整份 `Settings`：
    /// 界面偏好（关窗收托盘之类）住在 UI 线程，传过来只会让 runner 多一份没人读的副本。
    /// 没有这一条时，用户改完设置必须重启程序才生效 —— 那等于设置界面在说谎。
    ApplyRuntime(RuntimeTuning),
}

/// runner 侧的运行时参数：从 `Settings` 投影出来，**只含影响子进程的那几项**。
///
/// 单独成一个结构而不是直接传 `Settings`：`Settings` 里还有界面偏好与剪贴板开关，
/// 那些归 UI 线程；runner 拿着它们只会诱导后来的人从错误的地方读状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeTuning {
    /// 每个任务交给 `dlm --connections` 的连接数。
    pub connections: usize,
    /// 全局限速，KiB/s（0 = 不限），转成 `--max-speed` 的字节/秒。
    pub max_speed_kib: u64,
    /// 队列层的每 host 连接预算。
    pub per_host: usize,
    /// 新任务的落点目录。
    ///
    /// **设置里改了立刻生效**（走 `Ticket::ApplyRuntime`），所以它必须住在这里 ——
    /// 以前 runner 只读启动时那份 `config.out_dir`，用户在设置里改了目录要重启才生效，
    /// 那等于设置界面在说谎。
    ///
    /// 优先级：`settings.out_dir`（用户显式设的）> `--out-dir`（命令行 / 测试夹具）。
    /// 反过来会打断测试与便携用法：`--out-dir` 是"这次跑在哪里"的硬指定。
    pub out_dir: std::path::PathBuf,
    /// 完成后是否调 Windows Defender 扫一遍（失败只记日志，不影响任务成败）。
    pub scan_after_done: bool,
    /// 重命名模板（空 = 原名）。
    pub rename_template: String,
    /// 同名冲突策略（auto / skip / overwrite）。
    pub on_duplicate: naming::OnDuplicate,
    /// 新任务是否按类型分子目录。命令行 `--category-dirs` 只做「开」
    /// （它没法表达"关掉设置里的勾"），所以与设置是**或**关系。
    pub category_dirs: bool,
}

impl RuntimeTuning {
    /// 从设置投影。**夹住下界 1**：`--connections 0` 是参数错误，
    /// 而 `DragValue` 的下界是 1，正常路径不会出现 0；这里兜住手改 JSON 的情况。
    ///
    /// 落点没设时退回「下载文件夹」（`Settings::effective_out_dir`）；
    /// 真正的启动路径用 [`Self::from_settings_with`] 把 `--out-dir` 传进来。
    pub fn from_settings(s: &settings::Settings) -> Self {
        Self::from_settings_with(s, &s.effective_out_dir(), false)
    }

    /// 同 [`Self::from_settings`]，但落点兜底与命令行的分类开关由调用方给。
    pub fn from_settings_with(
        s: &settings::Settings,
        fallback_out_dir: &std::path::Path,
        cli_category_dirs: bool,
    ) -> Self {
        Self {
            connections: s.connections.max(1),
            max_speed_kib: s.max_speed_kib,
            per_host: s.per_host_connections,
            out_dir: s
                .out_dir
                .clone()
                .unwrap_or_else(|| fallback_out_dir.to_path_buf()),
            category_dirs: s.category_dirs || cli_category_dirs,
            scan_after_done: s.scan_after_done,
            rename_template: s.rename_template.clone(),
            on_duplicate: naming::OnDuplicate::parse(&s.on_duplicate),
        }
    }

    /// `--max-speed` 的值：字节/秒，0 = 不限。
    pub fn max_speed_bytes(&self) -> u64 {
        self.max_speed_kib.saturating_mul(1024)
    }
}

impl Default for RuntimeTuning {
    fn default() -> Self {
        Self::from_settings(&settings::Settings::default())
    }
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
    // 设置要在更新检查之前读：`auto_check_update` 决定这次要不要查。
    // 先读设置、再起线程，顺序不能反 —— 反了就是又一处「定义在设置里、没人调用」。
    let settings_dir = config
        .queue
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let settings = settings::Settings::load(&settings_dir);
    // 本机现状（发行版版本 + 磁盘扩展版本）：既决定「要不要更新」，也决定界面怎么说。
    let local = updater::local_state(&settings_dir, &default_extension_dir());
    // 门禁/集成测试统一离线：`gates.ps1` 会设 `DLM_NO_UPDATE_CHECK=1`，
    // 子进程继承它。用一个环境变量而不是让八九处起 GUI 的用例各自加参数 ——
    // 漏一处就会在门禁里真发一次外网请求，那种测试是假绿：机器断网就变红。
    let offline = std::env::var_os("DLM_NO_UPDATE_CHECK").is_some();
    match (&config.update_url, settings.auto_check_update && !offline) {
        (Some(url), true) => {
            let url = url.clone();
            let delay = config.update_check_delay_secs;
            let local = local.clone();
            let snapshot = Arc::clone(&update_snapshot);
            std::thread::Builder::new()
                .name("dlm-gui-update".into())
                .spawn(move || {
                    // 延迟到冷启动过去再查：不跟首帧抢带宽，也给测试留出「起完就杀」的窗口。
                    if delay > 0 {
                        std::thread::sleep(Duration::from_secs(delay));
                    }
                    let view = check_update(&url, &local);
                    install_update_view(&snapshot, view);
                })
                .context("启动更新检查线程失败")?;
        }
        (Some(_), false) => {
            eprintln!("dlm-gui: 更新检查已在设置里关闭");
        }
        (None, _) => {}
    }
    let (tx, rx): (Sender<Ticket>, _) = mpsc::channel();
    let runner_cfg = config.clone();
    // UI 与 runner 共享同一份快照句柄：runner 写，UI 读。
    // E18：运行时参数从设置投影。以前这里是 `let per_host = 8usize;`，
    // 连接数与限速则压根没往下传 —— 设置面板改了不生效。
    let tuning =
        RuntimeTuning::from_settings_with(&settings, &config.out_dir, config.category_dirs);
    let ui_snapshot = Arc::clone(&snapshot);
    let ui_quota = Arc::clone(&quota_snapshot);
    let ui_update = Arc::clone(&update_snapshot);
    let ui_local = local.clone();
    std::thread::Builder::new()
        .name("dlm-gui-runner".into())
        .spawn(move || runner::run(runner_cfg, store, rx, snapshot, quota_snapshot, tuning))
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
            let mut app = app::App::new(
                ui_snapshot,
                ui_quota,
                ui_update,
                ui_tx,
                app::AppWiring {
                    settings_dir,
                    default_out_dir: config.out_dir.clone(),
                    default_category_dirs: config.category_dirs,
                    update_url: config.update_url.clone(),
                },
                ui_local,
            );
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

fn check_update(url: &str, local: &updater::LocalState) -> UpdateView {
    let fail = |error: String| UpdateView {
        enabled: true,
        current: local.release.clone(),
        checked_at: Some(queue::now_secs()),
        error: Some(error),
        ..UpdateView::disabled()
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
    parse_manifest(&value, local).unwrap_or_else(&fail)
}

/// 解析更新清单。
///
/// 两种格式都认：
///
/// | 版本 | 字段 |
/// |---|---|
/// | 现在（三个版本号） | `release` / `core` / `extension` / `sha256` / `size` / `min_core` |
/// | 老（C3 骨架期） | `version` 或 `latest`，没有摘要 |
///
/// 老清单没有 `sha256`，而**没有摘要的包不许装**：那种情况下 `available` 仍为真
/// （确实有新版），但 `installable` 为假、`blocked` 写明原因。
/// 这样界面能显示「有新版，但只能手动装」，而不是「没有更新」。
fn parse_manifest(
    value: &Value,
    local: &updater::LocalState,
) -> std::result::Result<UpdateView, String> {
    let release = manifest_str(value, "release")
        .or_else(|| manifest_str(value, "version"))
        .or_else(|| manifest_str(value, "latest"))
        .ok_or_else(|| "更新清单缺 release（旧格式为 version/latest）".to_string())?;
    let core_local = env!("CARGO_PKG_VERSION").to_string();
    let mut view = UpdateView {
        enabled: true,
        // 本机发行版版本来自 `installed.json`（装过一次更新之后就不再是编译期常量）。
        current: local.release.clone(),
        available: version_is_newer(&local.release, &release),
        installable: false,
        blocked: None,
        latest: Some(release),
        core_current: core_local.clone(),
        core: manifest_str(value, "core"),
        extension: manifest_str(value, "extension"),
        extension_disk: local.extension.clone(),
        extension_running: None,
        url: manifest_str(value, "url"),
        sha256: manifest_str(value, "sha256").map(|s| s.to_ascii_lowercase()),
        size: value.get("size").and_then(Value::as_u64),
        notes: manifest_str(value, "notes"),
        min_core: manifest_str(value, "min_core"),
        published_at: manifest_str(value, "published_at"),
        checked_at: Some(queue::now_secs()),
        error: None,
    };
    if !view.available {
        return Ok(view);
    }
    // min_core：低于它不能自动更新，只能整包重装（内核没有跨版本迁移能力）。
    if let Some(min_core) = &view.min_core
        && version_is_newer(&core_local, min_core)
    {
        view.blocked = Some(format!(
            "新版本要求桌面端至少 {min_core}（当前 {core_local}），需要手动重装"
        ));
        return Ok(view);
    }
    // 没有摘要的包不许装：装进安装目录的东西不校验等于任意代码执行。
    // 注意这里**不改** `available` —— 「有新版本」是事实，不能因为清单坏了就说没更新。
    if view.sha256.is_none() {
        view.blocked =
            Some("清单缺 sha256，无法校验下载内容；这个版本只能手动下载安装".to_string());
        return Ok(view);
    }
    view.installable = true;
    Ok(view)
}

/// 清单里的一个非空字符串字段。空白串当作没写。
pub(crate) fn manifest_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 把新查到的视图换进槽里，但**保留** `extension_running`。
///
/// 那个字段来自扩展的实时上报（`POST /api/ext-hello`），不是清单里的东西。
/// 整份覆盖会让「浏览器里跑的是哪个版本」在每次检查更新之后凭空丢掉 ——
/// 于是界面永远在提示「请重载扩展」，而用户早就重载过了。
pub(crate) fn install_update_view(
    slot: &Arc<Mutex<UpdateView>>,
    view: UpdateView,
) -> Option<String> {
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let running = guard.extension_running.clone();
    *guard = UpdateView {
        extension_running: running.clone(),
        ..view
    };
    running
}

/// 已安装扩展的目录。与 `packaging/install.ps1` 里的 `$Ext` 同一个地方。
pub fn default_extension_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dlm")
        .join("extension")
}

/// 起 `dlm-updater.exe` 把这一版装上。**调用方随后必须退出自己**。
///
/// ## 为什么先把 updater 拷到临时目录
///
/// 更新会替换安装目录里的 exe。如果 updater 也是从安装目录跑起来的，
/// 它就没法替换自己 —— 于是 `dlm-updater.exe` 永远停在旧版本。
/// 拷一份到临时目录再跑，安装目录里的那份就是可以被覆盖的普通文件。
///
/// ## 为什么把 `--wait-pid` 传成自己的进程号
///
/// 替换要 rename 正在被占用的 exe，Windows 会拒绝。updater 先等我们退出，
/// 是唯一可靠的做法（「先睡两秒再试」那种是赌）。
pub(crate) fn spawn_updater(view: &UpdateView, data_dir: &Path) -> Result<()> {
    let url = view
        .url
        .clone()
        .context("清单里没有下载地址，无法自动更新")?;
    let sha256 = view
        .sha256
        .clone()
        .context("清单里没有 sha256，拒绝自动安装（没有摘要就无法校验）")?;
    let release = view.latest.clone().context("清单里没有发行版版本")?;
    let core = view
        .core
        .clone()
        .unwrap_or_else(|| view.core_current.clone());
    let extension = view
        .extension
        .clone()
        .or_else(|| view.extension_disk.clone())
        .unwrap_or_default();
    let install_dir = std::env::current_exe()
        .context("拿不到自己的可执行文件路径")?
        .parent()
        .map(Path::to_path_buf)
        .context("拿不到安装目录")?;
    let gui_exe = install_dir.join("dlm-gui.exe");
    let updater_src = install_dir.join("dlm-updater.exe");
    if !updater_src.is_file() {
        bail!(
            "安装目录里没有 dlm-updater.exe（{}）。这一版包装得不全，请手动下载安装",
            updater_src.display()
        );
    }
    let staged_dir = std::env::temp_dir().join(format!("dlm-updater-{}", queue::now_secs()));
    fs::create_dir_all(&staged_dir)
        .with_context(|| format!("建临时目录失败：{}", staged_dir.display()))?;
    let staged_updater = staged_dir.join("dlm-updater.exe");
    fs::copy(&updater_src, &staged_updater)
        .with_context(|| format!("把 updater 拷到临时目录失败：{}", staged_updater.display()))?;

    let mut cmd = Command::new(&staged_updater);
    cmd.arg("--url")
        .arg(&url)
        .arg("--sha256")
        .arg(&sha256)
        .arg("--install-dir")
        .arg(&install_dir)
        .arg("--extension-dir")
        .arg(default_extension_dir())
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--release")
        .arg(&release)
        .arg("--core")
        .arg(&core)
        .arg("--extension")
        .arg(&extension)
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .arg("--restart")
        .arg(&gui_exe)
        .arg("--log")
        .arg(data_dir.join("logs").join("update.log"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // `dlm-updater.exe` 是**控制台**子系统（要能手动跑、看输出），而 `dlm-gui.exe`
    // 现在是 windows 子系统、自己就没有控制台。控制台子进程在「父进程没有控制台」时
    // 会**新分配一个**窗口 —— 更新时屏幕上就会闪出一个黑窗口。
    // CREATE_NO_WINDOW 让它不分配控制台；输出本来就走 `--log` 那个文件。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .with_context(|| format!("启动更新器失败：{}", staged_updater.display()))?;
    Ok(())
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
