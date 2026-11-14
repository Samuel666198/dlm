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

pub mod basket;
pub mod clipboard_hint;
pub mod context_bar;
pub mod dialog;
pub mod drawer;
pub mod fonts;
pub mod grab_ui;
pub mod icons;
pub mod list;
pub mod palette;
/// 速率采样：列表「速度」列与状态栏「总速度」都从这里来。
mod rate;
pub mod settings_spec;
pub mod settings_ui;
pub mod shortcuts;
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
    /// 搜索框内容（名字 / URL，大小写不敏感）。
    pub(super) search: String,
    /// 快速添加窗口是否打开（F3）。
    pub(super) basket_open: bool,
    /// 快速添加窗口里的多行文本。
    pub(super) basket_text: String,
    /// 站点抓取窗口（0.4.0 · A）
    pub(super) grab_open: bool,
    pub(super) grab_url: String,
    pub(super) grab_depth: u8,
    pub(super) grab_same_host: bool,
    pub(super) grab_include: String,
    pub(super) grab_exclude: String,
    pub(super) grab_busy: bool,
    pub(super) grab_plan: Option<crate::grab::GrabPlan>,
    pub(super) grab_checked: Vec<bool>,
    pub(super) grab_error: Option<String>,
    pub(super) grab_rx: Option<std::sync::mpsc::Receiver<crate::grab::GrabPlan>>,
    /// 这一轮扫描开始的时刻。「扫描中 12s」靠它 —— 给不出百分比，就给已用时。
    pub(super) grab_started: Option<std::time::Instant>,
    /// 「存为模板」时用的名字。
    pub(super) grab_template_name: String,
    /// 代理「测试」的结果（只在设置页显示）。
    pub(super) proxy_test: Option<String>,
    /// 下一帧是否把焦点给搜索框（Ctrl+F）。
    pub(super) focus_search: bool,
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
    /// 上次改托盘 tooltip 的 Unix 秒（用于节流）。
    pub tray_hint_at: u64,
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
    /// 设置面板里「搜索设置项」的内容（空 = 按分类浏览）。
    pub(super) settings_search: String,
    /// 设置面板当前选中的分类（左侧那一列）。
    pub(super) settings_category: settings_spec::Category,
    /// Agent 接入页刚复制了什么（`cli` / `mcp`），只用来显示一行"已复制"。
    pub(super) agent_copied: Option<String>,
    /// 命令面板（`Ctrl K`）是否打开。
    pub(super) palette_open: bool,
    pub(super) palette_query: String,
    /// 当前高亮第几条（在筛选结果里的下标）。
    pub(super) palette_sel: usize,
    /// 面板刚打开：下一帧把焦点给输入框。
    pub(super) palette_take_focus: bool,
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

    /// 「删除全部已完成」还等着二次确认时，这里是**那一刻**要删的条数。
    ///
    /// `Some` 就是"确认框开着"。真正的 id 列表在点确认时重算（见 `confirm_delete_all_dialog`）：
    /// 等着确认的这几秒里任务可能又完成或被别处删掉，拿旧快照删会删错。
    pub(super) confirm_delete_all: Option<usize>,
    /// 限速菜单里"自定义"那一格的当前值（KiB/s）。与应用过的限速分开存，
    /// 否则拖一下数字就等于改设置。
    pub(super) speed_custom_kib: u64,
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
        // 限速菜单「自定义」的初值 = 当前生效的限速（`settings` 随后会被移进结构体）。
        let speed_custom_kib = settings.max_speed_kib;
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
            search: String::new(),
            basket_open: false,
            basket_text: String::new(),
            grab_open: false,
            grab_url: String::new(),
            grab_depth: 1,
            grab_same_host: true,
            grab_include: String::new(),
            grab_exclude: String::new(),
            grab_busy: false,
            grab_plan: None,
            grab_checked: Vec::new(),
            grab_error: None,
            grab_rx: None,
            grab_started: None,
            grab_template_name: String::new(),
            proxy_test: None,
            focus_search: false,
            adding: false,
            add_form: dialog::AddForm::default(),
            quitting: false,
            close_to_tray: true,
            window_visible: true,
            tray: tray::Tray::new(),
            tray_hint_at: 0,
            clipboard,
            settings_open: false,
            settings: settings.clone(),
            draft: settings,
            settings_dir,
            default_out_dir,
            default_category_dirs: wiring_category_dirs,
            out_dir_text: settings_out_dir_text,
            settings_error: None,
            settings_search: String::new(),
            settings_category: settings_spec::Category::Download,
            agent_copied: None,
            palette_open: false,
            palette_query: String::new(),
            palette_sel: 0,
            palette_take_focus: false,
            start_hidden: 0,
            clipboard_hit: None,
            clipboard_dismissed: None,
            confirm_delete_all: None,
            speed_custom_kib,
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

    /// 当前选中的任务（按 id 从快照里取）。
    pub(super) fn selected_task(&self) -> Option<TaskView> {
        let id = self.selected.as_ref()?;
        self.tasks().iter().find(|t| &t.id == id).cloned()
    }

    /// 有没有全屏/居中层的框开着。
    ///
    /// Esc 的归属靠它决定：有框时 Esc 关框，没框时才取消选择（见 `shortcuts::shortcut_for`）。
    pub(super) fn overlay_open(&self) -> bool {
        self.adding
            || self.settings_open
            || self.basket_open
            || self.grab_open
            || self.update_open
            || self.palette_open
            || self.confirm_delete_all.is_some()
    }

    /// 执行一条命令面板里的命令（id 来自 `palette::COMMANDS`）。
    ///
    /// 分发集中在这里，而不是让每条命令自带闭包：命令表的文字是数据，动作是代码，
    /// 混在一起会让"加一条命令"变成"加一段闭包"，而闭包没法在测试里对账。
    pub(super) fn run_command(&mut self, id: &str, ctx: &egui::Context) {
        match id {
            "search" => self.focus_search = true,
            "new" => {
                self.adding = true;
                self.add_form.focused = false;
            }
            "paste" => self.basket_open = true,
            "grab" => self.grab_open = true,
            "pause-all" => {
                let ids = toolbar::pause_all_targets(&self.tasks());
                for id in ids {
                    self.send(crate::gui::Ticket::Pause(id));
                }
            }
            "delete-done" => {
                // 与 `⋯` → 删除全部同一套：先问，确认时才重算要删的 id。
                self.confirm_delete_all = Some(toolbar::delete_all_targets(&self.tasks()).len());
            }
            "settings" => {
                self.settings_open = true;
                self.draft = self.settings.clone();
                self.settings_error = None;
            }
            "open-out-dir" => {
                // 与「打开文件」走同一个闸门：门禁与自动化里弹资源管理器会把测试机的
                // 桌面弄脏（而且没人关）。
                let dir = self.settings.effective_out_dir();
                let plan = crate::actions::reveal_plan(crate::actions::RevealMode::Open, &dir);
                if crate::actions::shell_suppressed() {
                    eprintln!(
                        "dlm-gui: 打开目录被抑制（{}）：{}",
                        crate::actions::NO_SHELL_ENV,
                        dir.display()
                    );
                } else if let Err(err) = crate::actions::execute(&plan) {
                    self.settings_error = Some(format!("打开目录失败：{err:#}"));
                }
            }
            "check-update" => {
                self.update_open = true;
                self.check_update_now(ctx);
            }
            other => eprintln!("dlm-gui: 未知命令 id {other}"),
        }
    }

    /// 改限速（KiB/s，0 = 不限）：存盘 + 立刻作用于下一轮派发。
    ///
    /// 三件事必须一起做，少一件都是"界面在说谎"：
    /// 1. 写进 `settings` 并落盘 —— 否则重启就回到旧值；
    /// 2. 同步 `draft` —— 否则下次打开设置面板会显示旧值，用户以为没生效；
    /// 3. 发 `ApplyRuntime` —— 否则要重启才生效（E18 那次的老毛病）。
    pub(super) fn set_speed_limit(&mut self, kib: u64) {
        self.settings.max_speed_kib = kib;
        self.draft.max_speed_kib = kib;
        if let Err(err) = self.settings.save(&self.settings_dir) {
            self.settings_error = Some(format!("限速保存失败：{err:#}"));
        }
        self.send(Ticket::ApplyRuntime(
            crate::gui::RuntimeTuning::from_settings_with(
                &self.settings,
                &self.default_out_dir,
                self.default_category_dirs,
            ),
        ));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ui_frame(ui);
    }
}

impl App {
    /// 一帧的全部界面。
    ///
    /// 从 [`eframe::App::ui`] 里抽出来只为一个理由：`eframe::Frame` 在测试里造不出来，
    /// 而"整帧画一遍不 panic"是这一层唯一能自动化的硬检查 —— 工具条的自绘按钮、
    /// 上下文动作条、状态栏的限速菜单都只在这条路上被走到。
    pub(super) fn ui_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // 速率采样要在画之前做：这一帧画出来的数字与它同一时刻。
        self.sample_rates();
        self.handle_shortcuts(&ctx);
        self.basket(ui);
        self.grab_window(ui);

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

        // 关窗策略（2026-11-13 改）：
        // - 有任务在跑/排队 → 收进托盘。误点 X 不该把正在下的活停掉。
        // - 全空闲 → 真的退出。用户点 X 通常就是"我要关掉它"。
        // - 托盘没建起来 → 绝不隐藏，否则窗口藏了却没有任何入口找回来。
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && self.hide_instead_of_quit()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.window_visible = false;
            self.hint_hidden_to_tray();
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

        // 0.6.0：上下文动作条。**只有选中任务时才占一行** —— 没选中时这里
        // 连高度都不占，列表就多一行可见空间（这正是"动作收进上下文"的好处）。
        if context_bar::task_actions_visible(self.selected.is_some()) {
            egui::Panel::top("context-bar")
                .exact_size(theme::CONTEXT_BAR_H)
                .show(ui, |ui| self.context_bar(ui));
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
        if self.confirm_delete_all.is_some() {
            self.confirm_delete_all_dialog(&ctx);
        }
        // 命令面板盖在最上面：它是"临时打断"，别的框开着时也能叫出来。
        if let Some(id) = self.palette(&ctx) {
            self.run_command(id, &ctx);
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

impl App {
    /// 收进托盘后的提示：一分钟内最多改一次 tooltip，避免反复点 X 反复刷。
    fn hint_hidden_to_tray(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if tray::should_notify(now, self.tray_hint_at) {
            if let Some(tray) = &self.tray {
                tray.hint_downloading();
            }
            self.tray_hint_at = now;
        }
    }
    /// 点 X 时该藏进托盘还是真的退出。
    ///
    /// 抽成纯判定是为了能直接测：这三种组合以前混在 UI 分支里，
    /// 出问题时只能靠人手点窗口复现（"关了却关不掉"就是这么来的）。
    pub(crate) fn hide_instead_of_quit(&self) -> bool {
        hide_on_close(
            self.close_to_tray,
            self.tray.is_some(),
            self.tasks()
                .iter()
                .any(|t| t.status == "running" || t.status == "queued"),
        )
    }
}

/// 纯判定：`close_to_tray` 开关 + 托盘是否可用 + 是否还有活。
pub(crate) fn hide_on_close(close_to_tray: bool, tray_available: bool, busy: bool) -> bool {
    close_to_tray && tray_available && busy
}
