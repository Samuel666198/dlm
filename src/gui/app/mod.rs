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

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::gui::Ticket;
use crate::gui::snapshot::{QuotaView, TaskView, UpdateView};

pub mod dialog;
pub mod fonts;
pub mod list;
pub mod sidebar;
pub mod status;
pub mod theme;
pub mod toolbar;

/// runner 换快照的句柄。
pub type SnapshotSlot = Arc<Mutex<Vec<TaskView>>>;
/// runner 换配额视图的句柄。
pub type QuotaSlot = Arc<Mutex<Option<QuotaView>>>;
/// runner 换更新状态的句柄。
pub type UpdateSlot = Arc<Mutex<UpdateView>>;

pub struct App {
    pub(super) snapshot: SnapshotSlot,
    pub(super) quota: QuotaSlot,
    #[allow(dead_code)] // 更新检查在步骤 5 接进 UI；句柄先留着。
    pub(super) update: UpdateSlot,
    pub(super) tx: Sender<Ticket>,

    /// 当前选中的任务 id（详情抽屉）。
    pub(super) selected: Option<String>,
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
    /// 窗口当前是否可见。托盘"显示窗口"把它置 true。
    pub window_visible: bool,
}

impl App {
    pub(crate) fn new(
        snapshot: SnapshotSlot,
        quota: QuotaSlot,
        update: UpdateSlot,
        tx: Sender<Ticket>,
    ) -> Self {
        Self {
            snapshot,
            quota,
            update,
            tx,
            selected: None,
            filter: None,
            adding: false,
            add_form: dialog::AddForm::default(),
            quitting: false,
            close_to_tray: true,
            window_visible: true,
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

    /// 发一个 ticket。队列已退出时静默丢弃 —— UI 不该因为 runner 挂了而崩。
    pub(super) fn send(&self, ticket: Ticket) {
        let _ = self.tx.send(ticket);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

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

        egui::Panel::bottom("status")
            .exact_size(theme::STATUS_H)
            .show(ui, |ui| self.status_bar(ui));

        egui::Panel::left("sidebar")
            .exact_size(theme::SIDEBAR_W)
            .resizable(false)
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default().show(ui, |ui| self.list(ui));

        if self.adding {
            self.add_dialog(&ctx);
        }
    }
}
