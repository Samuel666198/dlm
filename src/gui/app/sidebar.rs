//! 左侧分类栏：任务状态筛选 + 扩展名分类（D6 保留这一栏）。

use eframe::egui;

use super::App;
use crate::gui::app::theme;

/// 状态筛选项：(内部键, 显示名)。
const STATES: &[(&str, &str)] = &[
    ("all", "全部任务"),
    ("active", "进行中"),
    ("done", "已完成"),
    ("failed", "失败"),
];

/// 扩展名分类：(`category_for` 的返回值, 显示名)。
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
        let tasks = self.tasks();
        ui.add_space(12.0);

        section_label(ui, "任务");
        for (key, label) in STATES {
            let count = tasks
                .iter()
                .filter(|t| matches_state(&t.status, key))
                .count();
            if nav_item(ui, label, count, self.filter.as_deref() == Some(*key)) {
                self.filter = if *key == "all" {
                    None
                } else {
                    Some((*key).to_string())
                };
            }
        }

        ui.add_space(6.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, theme::LINE);
        ui.add_space(6.0);

        section_label(ui, "分类");
        for (key, label) in CATEGORIES {
            let count = tasks.iter().filter(|t| t.category == *key).count();
            // 分类与状态筛选共用一个 `filter` 字段，这里用前缀区分。
            let active = self.filter.as_deref() == Some(*key);
            if nav_item(ui, label, count, active) {
                self.filter = Some((*key).to_string());
            }
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

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
    ui.painter().text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text.to_uppercase(),
        egui::FontId::proportional(10.5),
        theme::INK_3,
    );
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

/// 一行导航项。返回 `true` 表示被点了。
fn nav_item(ui: &mut egui::Ui, label: &str, count: usize, active: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }

    if active {
        ui.painter().rect_filled(rect, 7.0, theme::ACCENT_SOFT);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 7.0, eframe::egui::Color32::from_rgb(0xE9, 0xE4, 0xD8));
    }

    let ink = if active {
        theme::ACCENT_INK
    } else {
        theme::INK_2
    };
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
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
            theme::ACCENT_INK
        } else {
            theme::INK_3
        },
    );

    response.clicked()
}
