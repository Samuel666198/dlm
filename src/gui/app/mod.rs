//! 原生 UI 的根：`App` 结构体与 `eframe::App` 实现。
//!
//! ## 一条纪律：UI 线程不碰队列
//!
//! `App` 只做两件事：读快照（`Arc<Mutex<Arc<Vec<TaskView>>>>`）与发 `Ticket`。
//! 队列、子进程、落盘全归 runner 线程。这样 UI 卡住也不会拖慢下载。
//!
//! ## 为什么快照是 `Arc<Mutex<Arc<Vec<_>>>>`
//!
//! runner 每轮产出一个**新的** `Arc<Vec<TaskView>>` 换进去，UI 读时 clone 那个
//! 内层 Arc 就立刻放手 —— 不跨帧持锁。
//!
//! ## eframe 0.37 的 trait 形状
//!
//! 0.37 把入口从 `update(ctx, frame)` 改成了 `ui(&mut Ui, frame)`，
//! 面板要用 `egui::CentralPanel` 之类自己包。这里按新 API 写。

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::gui::Ticket;
use crate::gui::snapshot::{QuotaView, TaskView, UpdateView};

pub mod clipboard_hint;
pub mod dialog;
pub mod drawer;
pub mod fonts;
pub mod list;
/// 速率采样：列表「速度」列与状态栏「总速度」都从这里来。
mod rate;
pub mod settings_ui;
pub mod sidebar;
pub mod status;
pub mod theme;
pub mod toolbar;
pub mod tray;
pub mod update_ui;

/// runner 换快照的句柄。
pub type SnapshotSlot = Arc<Mutex<Vec<TaskView>>>;
/// runner 换配额视图的句柄。
pub type QuotaSlot = Arc<Mutex<Option<QuotaView>>>;
/// runner 换更新状态的句柄。
pub type UpdateSlot = Arc<Mutex<UpdateView>>;

pub struct App {
    pub(super) snapshot: SnapshotSlot,
    pub(super) quota: QuotaSlot,
    pub(super) update: UpdateSlot,
    pub(super) tx: Sender<Ticket>,

    /// 更新清单地址（`--update-url`，缺省是 `gui::DEFAULT_UPDATE_URL`；
    /// `--no-update-check` 时为 `None`）。「立即检查」用它。
    pub(super) update_url: Option<String>,
    /// 手动检查的结果通道：查完由 UI 线程写回 `update` 槽。
    ///
    /// 为什么不用「线程直接写槽 + 置个布尔」：秒粒度的 `checked_at` 分不清
    /// 「这次查完的」和「启动时那次查完的」，界面会一直显示「检查中」。
    pub(super) update_rx: Option<Receiver<UpdateView>>,
    /// 更新面板是否打开。
    pub(super) update_open: bool,
    /// 本机现状（发行版版本 + 磁盘扩展版本）。「立即检查」重查时一起带上 ——
    /// 不然刚装完更新，界面还拿编译期常量当自己的版本。
    pub(super) local: crate::updater::LocalState,
    /// 「立即更新」失败时的提示（更新器起不来之类）。
    pub(super) update_install_error: Option<String>,

    /// 当前选中的任务 id（详情抽屉）。
    pub(super) selected: Option<String>,
    /// 速率采样：每个任务上一轮的 `delivered` 与时刻。
    pub(super) rates: rate::Rates,
    /// 抽屉里那段日志尾巴的缓存：`(任务 id, updated_at, 文本)`。
    ///
    /// 为什么缓存：每帧读一次日志文件会把磁盘读穿，而日志只在任务有进展时才变。
    pub(super) drawer_log: Option<(String, u64, String)>,
    /// 筛选键：`None` = 全部；否则是状态键或分类键。
    pub(super) filter: Option<String>,
    /// 新建任务弹窗是否打开。
    pub(super) adding: bool,
    /// 弹窗里的输入。
    pub(super) add_form: dialog::AddForm,

    /// 用户点了"退出"（托盘菜单）。置位后 `close_requested` 不再被拦。
    pub quitting: bool,
    /// 关闭窗口时收进托盘而不是退出（设置项，默认 true）。
    pub close_to_tray: bool,
    /// 窗口当前是否可见。托盘「显示窗口」把它置 true。
    pub window_visible: bool,
    /// 托盘句柄。None = 建不起来（无托盘环境），此时关窗即退出。
    pub tray: Option<tray::Tray>,
    /// 剪贴板监视线程句柄。`None` = 没在跑。
    pub(super) clipboard: Option<crate::gui::clipboard::Watcher>,
    /// 设置面板是否打开。
    pub(super) settings_open: bool,
    /// 已保存的设置。
    pub(super) settings: crate::gui::settings::Settings,
    /// 面板里的草稿。点保存才写回 `settings`。
    pub(super) draft: crate::gui::settings::Settings,
    /// 设置文件所在目录（`queue.json` 同级）。
    pub(super) settings_dir: std::path::PathBuf,
    /// 落点的兜底目录（`--out-dir`，缺省是下载文件夹）。
    ///
    /// 设置里的「默认保存目录」优先；用户没设时才用它 —— 于是测试与便携用法
    /// （`--out-dir` 指到临时目录）不会因为一份空设置就被改到 Downloads。
    pub(super) default_out_dir: std::path::PathBuf,
    /// 命令行 --category-dirs（与设置里的勾是或关系）。
    pub(super) default_category_dirs: bool,
    /// 设置面板里「默认保存目录」的输入框内容（保存时才解析）。
    pub(super) out_dir_text: String,
    /// 保存失败的提示。
    pub(super) settings_error: Option<String>,
    /// 启动时是纯托盘模式（`--autostart`）：倒数几帧后把窗口藏起来。
    ///
    /// 用计数器而不是 bool：首帧窗口可能还没建好，`Visible(false)` 会被丢掉。
    pub start_hidden: u8,

    /// E19：剪贴板发现、但用户还没决定要不要下的链接。
    ///
    /// `Some` 时工具条下方出现一条提示条：「发现下载链接 … [下载] [忽略]」。
    /// **不自动开始下载**（IDM 是自动开始的）：用户复制链接常常只是要发给别人，
    /// 直接开始下载会让人措手不及。
    pub(super) clipboard_hit: Option<crate::gui::clipboard::Hit>,
    /// 用户忽略过的链接：同一条不再提示。
    pub(super) clipboard_dismissed: Option<String>,
}

/// `App` 启动时要的那几样"外部世界"的输入。
///
/// 单独打包而不是继续往 `App::new` 上加参数：构造函数已经 8 个参数了，
/// 再多一个就得 `#[allow(clippy::too_many_arguments)]`，而参数多于 7 个本身就是
/// "这里该有个结构体"的信号。以后加路径类配置往这里放。
pub(crate) struct AppWiring {
    /// 设置文件所在目录（`queue.json` 同级）。
    pub settings_dir: std::path::PathBuf,
    /// 落点兜底（`--out-dir`）。
    pub default_out_dir: std::path::PathBuf,
    /// 命令行给了 `--category-dirs` 就强制开（设置里的勾管不到它）。
    pub default_category_dirs: bool,
    /// 更新清单地址；`None` = `--no-update-check`。
    pub update_url: Option<String>,
}

impl App {
    pub(crate) fn new(
        snapshot: SnapshotSlot,
        quota: QuotaSlot,
        update: UpdateSlot,
        tx: Sender<Ticket>,
        wiring: AppWiring,
        local: crate::updater::LocalState,
    ) -> Self {
        let AppWiring {
            settings_dir,
            default_out_dir,
            default_category_dirs: wiring_category_dirs,
            update_url,
        } = wiring;
        let settings = crate::gui::settings::Settings::load(&settings_dir);
        // 输入框初值：设置里有就写出来，没有就留空（留空 = 用兜底目录）。
        let settings_out_dir_text = settings
            .out_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        // 设置里开了剪贴板监视就在启动时起来（默认关，见 settings.rs）。
        let clipboard = if settings.watch_clipboard {
            crate::gui::clipboard::spawn()
        } else {
            None
        };
        Self {
            snapshot,
            quota,
            update,
            tx,
            update_url,
            update_rx: None,
            update_open: false,
            local,
            update_install_error: None,
            selected: None,
            rates: rate::Rates::default(),
            drawer_log: None,
            filter: None,
            adding: false,
            add_form: dialog::AddForm::default(),
            quitting: false,
            close_to_tray: true,
            window_visible: true,
            tray: tray::Tray::new(),
            clipboard,
            settings_open: false,
            settings: settings.clone(),
            draft: settings,
            settings_dir,
            default_out_dir,
            default_category_dirs: wiring_category_dirs,
            out_dir_text: settings_out_dir_text,
            settings_error: None,
            start_hidden: 0,
            clipboard_hit: None,
            clipboard_dismissed: None,
        }
    }

    /// 读一份快照。
    ///
    /// 返回 clone 而不是持有锁：`Vec<TaskView>` 一次几十条，clone 很便宜，
    /// 而持锁跨帧会让 runner 每 500ms 的一轮更新被 UI 拖住。
    pub(super) fn tasks(&self) -> Vec<TaskView> {
        match self.snapshot.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub(super) fn quota_view(&self) -> Option<QuotaView> {
        match self.quota.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 每帧采一次样：把这一帧的 `delivered` 交给采样器。
    ///
    /// 放在帧首、只读一次快照：列表与状态栏都从 `rates` 取，不各自再读一遍。
    pub(super) fn sample_rates(&mut self) {
        let now = std::time::Instant::now();
        let tasks = self.tasks();
        for t in &tasks {
            self.rates
                .observe(&t.id, t.delivered.unwrap_or(0), t.status == "running", now);
        }
        self.rates.retain(tasks.iter().map(|t| t.id.clone()));
    }

    /// 某个任务当前速率（字节/秒）。没在跑、或还没采到两个点时为 `None`。
    pub(super) fn speed_of(&self, id: &str) -> Option<f64> {
        self.rates.speed(id)
    }

    /// 在跑任务的总速率（字节/秒）。
    pub(super) fn total_speed(&self) -> f64 {
        self.rates.total()
    }

    /// 处理托盘动作。
    ///
    /// 「显示窗口」发 `Visible(true)` + `Focus`；「退出」先置 `quitting` 再发 Close，
    /// 否则会被自己的 `CancelClose` 吞掉（勘误 E6）。
    fn drain_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        while let Some(action) = tray.poll() {
            match action {
                tray::TrayAction::Show => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                }
                tray::TrayAction::NewTask => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.adding = true;
                    self.add_form.focused = false;
                    ctx.request_repaint();
                }
                tray::TrayAction::PauseAll => {
                    let ids: Vec<String> = self
                        .tasks()
                        .iter()
                        .filter(|t| t.status == "running" || t.status == "queued")
                        .map(|t| t.id.clone())
                        .collect();
                    for id in ids {
                        self.send(Ticket::Pause(id));
                    }
                }
                tray::TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// 设置里那些"立刻生效"的项。
    ///
    /// 目前只有剪贴板监视：开了就起线程，关了就让线程退出（勘误旁注：
    /// 不能靠"启动时不启动"，热切换要能停）。
    pub(super) fn apply_settings_runtime(&mut self) {
        let want = self.settings.watch_clipboard;
        let running = self.clipboard.is_some();
        match (want, running) {
            (true, false) => {
                self.clipboard = crate::gui::clipboard::spawn();
            }
            (false, true) => {
                // 丢句柄 = 置停止标志，线程下一轮自己退。
                self.clipboard = None;
            }
            _ => {}
        }
        self.close_to_tray = self.settings.close_to_tray;
    }

    /// 发一个 ticket。队列已退出时静默丢弃 —— UI 不该因为 runner 挂了而崩。
    pub(super) fn send(&self, ticket: Ticket) {
        let _ = self.tx.send(ticket);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // 速率采样要在画之前做：这一帧画出来的数字与它同一时刻。
        self.sample_rates();

        // `--autostart`：等窗口稳定后藏起来。
        //
        // 实测：`ViewportBuilder::with_visible(false)` 与首帧的 `Visible(false)`
        // 都会被随后的窗口初始化覆盖，所以用帧计数器等到第 20 帧再发。
        if self.start_hidden > 0 {
            self.start_hidden -= 1;
            if self.start_hidden == 0 {
                self.window_visible = false;
                eprintln!("dlm-gui: 隐藏窗口（纯托盘模式）");
                hide_window();
            }
            ctx.request_repaint();
        }

        // 托盘动作（勘误 E6/E3）：在主线程 poll，不另起消息泵。
        self.drain_tray(&ctx);

        // 关窗收进托盘（勘误 E6）：先看用户是不是真想退出。
        if ctx.input(|i| i.viewport().close_requested()) && self.close_to_tray && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.window_visible = false;
            return;
        }

        // 帧率策略：有任务在跑就保持流畅（runner 500ms 换一次快照，60fps 能及时反映），
        // 空闲时降到 500ms 一次 —— 但**必须**显式请求，否则 egui 会一直睡到有输入事件，
        // 快照变了也不重绘（本轮实测：任务已 done，界面还停在"下载中"）。
        let busy = self
            .tasks()
            .iter()
            .any(|t| t.status == "running" || t.status == "queued");
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::Panel::top("toolbar")
            .exact_size(theme::TOOLBAR_H)
            .show(ui, |ui| self.toolbar(ui));

        // E19：剪贴板提示条，贴在工具条下面。只有真的有命中时才占高度。
        self.drain_clipboard(&ctx);
        if self.clipboard_hit.is_some() {
            egui::Panel::top("clipboard-hint")
                .exact_size(theme::CLIPBOARD_HINT_H)
                .show(ui, |ui| self.clipboard_hint_bar(ui));
        }

        // 更新：把手动检查的结果收进槽里（非阻塞）。
        self.drain_update(&ctx);

        egui::Panel::bottom("status")
            .exact_size(theme::STATUS_H)
            .show(ui, |ui| self.status_bar(ui));

        egui::Panel::left("sidebar")
            .exact_size(theme::SIDEBAR_W)
            .resizable(false)
            .show(ui, |ui| self.sidebar(ui));

        // 详情抽屉：右边，只有选中任务时才占位（没选中就完全不出现在布局里）。
        if self.selected.is_some() {
            self.drawer(ui);
        }

        egui::CentralPanel::default().show(ui, |ui| self.list(ui));

        if self.adding {
            self.add_dialog(&ctx);
        }
        if self.settings_open {
            self.settings_panel(&ctx);
        }
        if self.update_open {
            self.update_panel(&ctx);
        }
    }
}

/// 把窗口藏起来（`--autostart` 纯托盘用）。
///
/// 为什么不用 `ViewportCommand::Visible(false)`：eframe 0.37 在这台机器上
/// 发了命令、`IsWindowVisible` 仍是 true（实测）。直接调 Win32 的 `ShowWindow`
/// 确定有效，代价是这段代码是 Windows-only。
///
/// `GetActiveWindow` 在无焦点时会返回 null，所以退到 `EnumWindows` 找本进程
/// 的可见顶层窗口 —— 那是 eframe 的主窗口。
#[cfg(windows)]
fn hide_window() {
    use std::ffi::c_void;
    // SAFETY: 只调 user32 的查询/设置函数，句柄由本进程自己枚举得到。
    unsafe {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetActiveWindow() -> *mut c_void;
            fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
            fn IsWindowVisible(hwnd: *mut c_void) -> i32;
            fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
            fn EnumWindows(
                cb: unsafe extern "system" fn(*mut c_void, isize) -> i32,
                lparam: isize,
            ) -> i32;
        }

        const SW_HIDE: i32 = 0;

        // 先试活动窗口；不是我们的就枚举。
        let active = GetActiveWindow();
        if !active.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(active, &mut pid);
            if pid == std::process::id() {
                ShowWindow(active, SW_HIDE);
                return;
            }
        }

        unsafe extern "system" fn cb(hwnd: *mut c_void, lparam: isize) -> i32 {
            let me = lparam as u32;
            let mut pid = 0u32;
            // SAFETY: 句柄由系统在回调里给出，两个函数都只读/设置窗口状态。
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == me && IsWindowVisible(hwnd) != 0 {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
            1 // 继续枚举
        }

        EnumWindows(cb, std::process::id() as isize);
    }
}

#[cfg(not(windows))]
fn hide_window() {}
