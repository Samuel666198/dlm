//! 详情抽屉：点一行任务，右边滑出一块，把「这个任务到底是什么」说清楚。
//!
//! 这块以前只有常量 `theme::DRAWER_W` 和一个被存下来的 `selected`，界面上一片空白 ——
//! 点了任务除了高亮什么也没有。抽屉里每一项都是**已经存在于快照里**的事实，
//! 没有一项是为了好看编出来的：
//!
//! | 显示 | 来源 |
//! |---|---|
//! | 状态 / 进度 / 大小 / 已交付 | `TaskView` |
//! | 速度 / 剩余 | 与列表同一个采样器（`rate::Rates`）|
//! | 重试次数 | `TaskView.attempts` |
//! | 完成后动作 | `TaskView.on_complete` |
//! | 落点 / 日志 / URL | `TaskView.path` / `log` / `url` |
//!
//! 「打开文件 / 打开文件夹」与 `/api/reveal` 同一套动作：**路径只来自队列算出来的落点**，
//! 不接受任何外部输入，否则就成了"打开任意文件"的入口。
//!
//! 为什么要拆出 [`drawer_body`]：面板里那段是纯画法 + 返回"用户点了什么"，
//! 于是可以在测试里真的跑一帧（`egui::Context::run`）而不需要窗口 ——
//! 不然这块 UI 只能靠人眼验，一改就可能画出个 panic。

use eframe::egui;

use super::App;
use crate::actions::{RevealMode, reveal_plan};
use crate::gui::app::list::{eta_text, fmt_bytes, speed_text};
use crate::gui::app::theme;
use crate::gui::snapshot::TaskView;

/// 抽屉里日志尾巴最多显示几行。
const LOG_TAIL_LINES: usize = 40;

/// 用户在抽屉里点了什么。画法（[`drawer_body`]）只负责收集，**不执行**：
/// 执行要 `&mut App`，而画的时候 `App` 已经被面板借走了。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrawerActions {
    pub open_file: bool,
    pub open_folder: bool,
    pub copy_url: bool,
    pub delete: bool,
    pub close: bool,
}

impl App {
    /// 画抽屉。没有选中、或选中的任务已经不在快照里，就什么都不画（面板也不占位）。
    pub(super) fn drawer(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.drawer_task() else {
            return;
        };
        let speed = self.speed_of(&task.id);
        let log_tail = self.drawer_log_tail(&task);

        let actions = egui::Panel::right("drawer")
            .exact_size(theme::DRAWER_W)
            .resizable(false)
            .show(ui, |ui| drawer_body(ui, &task, speed, &log_tail))
            .inner;

        if actions.close {
            self.selected = None;
            self.drawer_log = None;
        }
        if actions.copy_url {
            ui.ctx().copy_text(task.url.clone());
        }
        if actions.open_file {
            self.reveal_task(&task, RevealMode::Open);
        }
        if actions.open_folder {
            self.reveal_task(&task, RevealMode::Select);
        }
        if actions.delete {
            self.send(crate::gui::Ticket::Remove(task.id.clone()));
            self.selected = None;
            self.drawer_log = None;
        }
    }

    /// 当前选中的任务（快照里已经没有就返回 `None`，顺手把选中清掉）。
    fn drawer_task(&mut self) -> Option<TaskView> {
        let id = self.selected.clone()?;
        let found = self.tasks().into_iter().find(|t| t.id == id);
        if found.is_none() {
            self.selected = None;
            self.drawer_log = None;
        }
        found
    }

    /// 抽屉里的日志尾巴。按 `(id, updated_at)` 缓存：任务没进展就不重读文件。
    fn drawer_log_tail(&mut self, task: &TaskView) -> String {
        if let Some((id, stamp, text)) = &self.drawer_log
            && id == &task.id
            && *stamp == task.updated_at
        {
            return text.clone();
        }
        let text = read_log_tail(std::path::Path::new(&task.log), LOG_TAIL_LINES);
        self.drawer_log = Some((task.id.clone(), task.updated_at, text.clone()));
        text
    }

    /// 「打开文件 / 打开文件夹」。与 `/api/reveal` 同一套动作，也同一套抑制开关：
    /// 门禁里 `DLM_NO_SHELL_OPEN` 置上时不真的拉起资源管理器。
    fn reveal_task(&self, task: &TaskView, mode: RevealMode) {
        if crate::actions::shell_suppressed() {
            return;
        }
        let plan = reveal_plan(mode, std::path::Path::new(&task.path));
        if let Err(err) = crate::actions::execute(&plan) {
            eprintln!("dlm-gui: 打开落点失败：{err:#}");
        }
    }
}

/// 抽屉的画法。返回用户点了什么，由调用方执行。
pub(crate) fn drawer_body(
    ui: &mut egui::Ui,
    task: &TaskView,
    speed: Option<f64>,
    log_tail: &str,
) -> DrawerActions {
    let mut actions = DrawerActions::default();

    ui.painter().rect_filled(ui.max_rect(), 0.0, theme::SURFACE);
    ui.add_space(12.0);

    // ---- 标题 + 关闭 ----
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(elide(&task.name, 34))
                    .size(14.0)
                    .strong()
                    .color(theme::INK),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(host_of(&task.url))
                    .size(11.0)
                    .color(theme::INK_3),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.add_space(12.0);
            if ui.small_button("✕").clicked() {
                actions.close = true;
            }
        });
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);

    // ---- 进度 ----
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(format!("{:.0}%", task.progress))
                .size(20.0)
                .monospace()
                .color(theme::ACCENT),
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(status_label(&task.status))
                    .size(12.0)
                    .color(theme::INK),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}",
                    fmt_bytes(task.delivered.unwrap_or(0)),
                    task.total_len.map_or("?".to_string(), fmt_bytes)
                ))
                .size(11.0)
                .monospace()
                .color(theme::INK_3),
            );
        });
    });

    // ---- 速度 / 剩余 ----
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        kv(ui, "速度", &speed_text(task, speed));
        ui.add_space(18.0);
        kv(ui, "剩余", &eta_text(task, speed));
    });

    if task.attempts > 0 {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            kv(ui, "重试", &format!("第 {} 次", task.attempts));
        });
    }
    if let Some(action) = &task.on_complete {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            kv(ui, "完成后", action);
        });
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    // ---- 落点 / URL ----
    for (label, value) in [("落点", task.path.as_str()), ("URL", task.url.as_str())] {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(egui::RichText::new(label).size(10.5).color(theme::INK_3));
        });
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(elide(value, 46))
                        .size(11.0)
                        .monospace()
                        .color(theme::INK_2),
                )
                .sense(egui::Sense::click()),
            );
        });
        ui.add_space(8.0);
    }

    // ---- 动作 ----
    ui.horizontal_wrapped(|ui| {
        ui.add_space(16.0);
        if ui.button("打开文件").clicked() {
            actions.open_file = true;
        }
        if ui.button("打开文件夹").clicked() {
            actions.open_folder = true;
        }
        if ui.button("复制链接").clicked() {
            actions.copy_url = true;
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        // 这里没有「重新下载」：队列按名字去重，同名再 Add 一次会被拒。
        // 真正的重下要么先删后加（两步之间有竞态），要么等队列支持"覆盖同名"；
        // 放一个点了就报错的按钮，比不放更糟。
        if ui.button("删除任务").clicked() {
            actions.delete = true;
        }
    });

    // ---- 日志尾巴 ----
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(egui::RichText::new("日志").size(10.5).color(theme::INK_3));
    });
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new(if log_tail.is_empty() {
                        "（还没有日志：任务还没开始跑）"
                    } else {
                        log_tail
                    })
                    .size(10.5)
                    .monospace()
                    .color(theme::INK_2),
                );
            });
        });

    actions
}

/// 读日志文件的最后 `lines` 行。读不到就返回空串（抽屉里显示"还没有日志"）。
pub(crate) fn read_log_tail(path: &std::path::Path, lines: usize) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = raw.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(key).size(10.5).color(theme::INK_3));
        ui.label(
            egui::RichText::new(value)
                .size(11.5)
                .monospace()
                .color(theme::INK),
        );
    });
}

fn status_label(status: &str) -> &'static str {
    match status {
        "running" => "下载中",
        "queued" => "排队中",
        "paused" => "已暂停",
        "done" => "已完成",
        "failed" => "失败",
        _ => "未知",
    }
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}
