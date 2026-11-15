//! 左侧栏：三个入口 + 任务状态筛选 + 扩展名分类。
//!
//! ## 0.7.0 的改动（相对 0.6.0）
//!
//! 1. **可以收起**成一条 52pt 的图标条，状态记在 `Settings.sidebar_collapsed`；
//! 2. **三个入口搬到栏顶**（新建任务 / 站点抓取 / 下载列表）—— 原先藏在工具条的
//!    「＋新建 ▾」下拉里，藏起来的功能等于没做；
//! 3. 每一项都配图标，任务状态的图标**带颜色语义**（进行中=强调、已完成=绿、失败=红）：
//!    一列黑字要逐行读才知道哪行是失败，带色可以扫过去。
//!
//! ## 收起态的三个坑（写在这里，免得下次改版重踩）
//!
//! - **分组标题必须换成一条分隔线**：52pt 放不下 10.5pt 的组名，硬塞就是两行乱码；
//! - **每项要 `on_hover_text`**：只剩图标时，悬停是唯一能确认"这是哪个"的手段；
//! - **图标要居中**：收起时若还按左边距排，图标会偏到一侧，看着像没对齐的故障。

use eframe::egui;

use super::App;
use super::icons::{self, Icon};
use crate::gui::app::theme;

/// 状态筛选项：(内部键, 显示名, 图标)。
const STATES: &[(&str, &str, Icon)] = &[
    ("all", "全部任务", Icon::Tasks),
    ("active", "进行中", Icon::Running),
    ("done", "已完成", Icon::Done),
    ("failed", "失败", Icon::Failed),
];

/// 扩展名分类：(`category_for` 的返回值, 显示名)。图标由 `icons::for_category` 派生。
const CATEGORIES: &[(&str, &str)] = &[
    ("document", "文档"),
    ("video", "视频"),
    ("audio", "音频"),
    ("archive", "压缩包"),
    ("program", "程序"),
    ("image", "图片"),
    ("other", "其它"),
];

impl App {
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let collapsed = self.sidebar_collapsed;
        let tasks = self.tasks();
        ui.add_space(6.0);

        // ---- 栏头：收起按钮（左对齐）+ 三个入口 ----
        if entry_row(ui, !collapsed, Icon::Panel, "", collapsed, "展开侧栏") {
            self.sidebar_collapsed = !collapsed;
            self.persist_sidebar_collapsed();
        }
        if entry_row(ui, !collapsed, Icon::Plus, "新建任务", collapsed, "") {
            self.adding = true;
            self.add_form.focused = false;
            self.filter = None;
        }
        if entry_row(ui, !collapsed, Icon::Grab, "站点抓取", collapsed, "") {
            self.grab_open = true;
            self.filter = None;
        }
        if entry_row(ui, !collapsed, Icon::List, "下载列表", collapsed, "") {
            // 「下载列表」= 清掉所有筛选，回到完整清单。
            self.filter = None;
        }

        ui.add_space(4.0);
        divider(ui, collapsed);
        section_label(ui, "任务", collapsed);

        for (key, label, icon) in STATES {
            let count = tasks
                .iter()
                .filter(|t| matches_state(&t.status, key))
                .count();
            let active = self.filter.as_deref() == Some(*key);
            if nav_item(ui, collapsed, *icon, label, count, active) {
                self.filter = if *key == "all" {
                    None
                } else {
                    Some((*key).to_string())
                };
            }
        }

        divider(ui, collapsed);
        section_label(ui, "分类", collapsed);

        for (key, label) in CATEGORIES {
            let count = tasks.iter().filter(|t| t.category == *key).count();
            // 分类与状态筛选共用一个 `filter` 字段，这里用前缀区分。
            let active = self.filter.as_deref() == Some(*key);
            if nav_item(
                ui,
                collapsed,
                icons::for_category(key),
                label,
                count,
                active,
            ) {
                self.filter = Some((*key).to_string());
            }
        }
    }

    /// 栏头那个"收起/展开"是纯 UI 状态，改完立刻落盘（不值得走设置面板的保存流程）。
    fn persist_sidebar_collapsed(&mut self) {
        self.settings.sidebar_collapsed = self.sidebar_collapsed;
        if let Err(err) = self.settings.save(&self.settings_dir) {
            eprintln!("dlm-gui: 保存侧栏状态失败：{err}");
        }
    }
}

/// 状态键 → 匹配。`active` 覆盖在跑、排队、暂停三种"还没结束"的状态。
fn matches_state(status: &str, key: &str) -> bool {
    match key {
        "all" => true,
        "active" => matches!(status, "running" | "queued" | "paused"),
        "done" => status == "done",
        "failed" => status == "failed",
        _ => true,
    }
}

/// 组标题。收起时退化成一条短分隔线。
fn section_label(ui: &mut egui::Ui, text: &str, collapsed: bool) {
    if collapsed {
        return;
    }
    ui.add_space(2.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
    ui.painter().text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text.to_uppercase(),
        egui::FontId::proportional(10.5),
        theme::ink_3(),
    );
}

/// 一条短分隔线。收起态靠它把「入口 / 任务 / 分类」三段分开。
fn divider(ui: &mut egui::Ui, collapsed: bool) {
    ui.add_space(if collapsed { 6.0 } else { 2.0 });
    let width = if collapsed {
        24.0
    } else {
        ui.available_width() - 16.0
    };
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    let line = egui::Rect::from_center_size(rect.center(), egui::vec2(width, 1.0));
    ui.painter().rect_filled(line, 0.0, theme::line());
    ui.add_space(if collapsed { 6.0 } else { 2.0 });
}

/// 扩展名分类的显示名（`queue::category_for` 的返回值 → 中文）。
///
/// 侧栏与抓取清单都用它：同一个键在两处显示成两个名字，用户会以为是两回事。
pub(crate) fn category_label(key: &str) -> &'static str {
    CATEGORIES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, label)| *label)
        .unwrap_or("其它")
}

/// 栏顶的入口行：左图右字，收起时只有图标。
///
/// `icon` 为 `None` 时画一行等高空白 —— 用来在栏头与入口之间留出视觉分组。
/// 返回 `true` 表示被点了。
fn entry_row(
    ui: &mut egui::Ui,
    expanded: bool,
    icon: Icon,
    label: &str,
    collapsed: bool,
    hover: &str,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }

    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect.shrink2(egui::vec2(4.0, 1.0)), 7.0, theme::surface_2());
    }
    let ink = if hovered {
        theme::ink()
    } else {
        theme::ink_2()
    };
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            if collapsed {
                rect.center().x
            } else {
                rect.left() + 20.0
            },
            rect.center().y,
        ),
        egui::vec2(16.0, 16.0),
    );
    icons::draw_icon(ui.painter(), icon, icon_rect, ink);
    if expanded && !label.is_empty() {
        ui.painter().text(
            rect.left_center() + egui::vec2(34.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            ink,
        );
    }
    let tip = if hover.is_empty() { label } else { hover };
    if !tip.is_empty() {
        response.clone().on_hover_text(tip);
    }
    // 同样是"画出来的控件默认不在无障碍树里"：不报身份，读屏与自动化都找不到它。
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            if label.is_empty() { tip } else { label },
        )
    });
    response.clicked()
}

/// 一行导航项。返回 `true` 表示被点了。
fn nav_item(
    ui: &mut egui::Ui,
    collapsed: bool,
    icon: Icon,
    label: &str,
    count: usize,
    active: bool,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }

    if active {
        ui.painter().rect_filled(rect, 7.0, theme::accent_soft());
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 7.0, theme::surface_2());
    }

    let ink = if active {
        theme::accent_ink()
    } else {
        theme::ink_2()
    };

    {
        // 状态图标带颜色语义：一列黑字要逐行读才知道哪行是失败，带色可以扫过去。
        let icon_color = if active {
            theme::accent_ink()
        } else {
            match icon {
                Icon::Running => theme::accent(),
                Icon::Done => theme::ok(),
                Icon::Failed => theme::bad(),
                _ => theme::ink_3(),
            }
        };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(
                if collapsed {
                    rect.center().x
                } else {
                    rect.left() + 18.0
                },
                rect.center().y,
            ),
            egui::vec2(16.0, 16.0),
        );
        icons::draw_icon(ui.painter(), icon, icon_rect, icon_color);
    }

    if !collapsed {
        ui.painter().text(
            rect.left_center() + egui::vec2(34.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            ink,
        );
        ui.painter().text(
            rect.right_center() - egui::vec2(10.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::monospace(11.0),
            if active {
                theme::accent_ink()
            } else {
                theme::ink_3()
            },
        );
    } else {
        response
            .clone()
            .on_hover_text(format!("{label}（{count}）"));
    }

    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{label}（{count}）"))
    });
    response.clicked()
}
