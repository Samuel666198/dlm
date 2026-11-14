//! 上下文动作条：**只在选中任务时出现**的那一行（0.6.0 阶段 1）。
//!
//! 设计稿把它放在工具条下面，浅蓝底，内容是「已选 · 名字」+ 继续 / 暂停 / 删除 + 右端快捷键提示。
//!
//! ## 为什么要有这一条
//!
//! 顶层工具条以前常年摆着 继续 / 暂停 / 删除：没选中时是灰的，选中时又和列表里的行
//! 抢视线。搬到这里之后，界面在"没有对象"的状态下不再出现任何任务级动作 ——
//! 那正是 [`task_actions_visible`] 这条纯判定守着的事（它以前住在 `toolbar.rs`）。

use eframe::egui;

use super::App;
use super::icons::{Icon, action_button};
use crate::gui::Ticket;
use crate::gui::app::theme;

/// 这一条该不该出现：只由"有没有选中"决定。
///
/// 抽成函数（而不是写成 `if selected.is_some()`）是为了让这条规矩能被用例钉住：
/// 0.6.0 之前它藏在工具条的分支里，谁也没法证明"没选中就一定不出现"。
pub(crate) fn task_actions_visible(has_selection: bool) -> bool {
    has_selection
}

impl App {
    pub(super) fn context_bar(&mut self, ui: &mut egui::Ui) {
        // 没选中时这个函数根本不该被调用；真被调用了也不画东西。
        if !task_actions_visible(self.selected.is_some()) {
            return;
        }
        let Some(task) = self.selected_task() else {
            return;
        };

        // 底：浅蓝（设计稿 #f0f7ff）—— 与工具条、列表都区分开，一眼看出"这是上下文"。
        let rect = ui.max_rect();
        ui.painter()
            .rect_filled(rect, 0.0, egui::Color32::from_rgb(0xF0, 0xF7, 0xFF));

        ui.horizontal_centered(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("已选 · ")
                    .size(11.5)
                    .color(theme::ACCENT_INK),
            );
            // 名字可能很长：截一半也不许把按钮挤出去。
            let name = elide(&task.name, 28);
            ui.label(
                egui::RichText::new(name)
                    .size(11.5)
                    .strong()
                    .color(theme::INK),
            );

            ui.add_space(12.0);

            let can_resume = task.status == "paused" || task.status == "failed";
            let can_pause = task.status == "running" || task.status == "queued";

            if action_button(ui, Icon::Play, "继续", can_resume, false).clicked() {
                self.send(Ticket::Resume(task.id.clone()));
            }
            if action_button(ui, Icon::Pause, "暂停", can_pause, false).clicked() {
                self.send(Ticket::Pause(task.id.clone()));
            }
            if action_button(ui, Icon::Trash, "删除", true, true).clicked() {
                self.send(Ticket::Remove(task.id.clone()));
                self.selected = None;
                self.drawer_log = None;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new("Esc 取消选择")
                        .size(11.0)
                        .color(theme::INK_3),
                );
            });
        });
    }
}

/// 长名字截断成 `head…`。
///
/// 按 `char` 数而不是字节数：中文名按字节截会切出半个字，界面直接显示乱码。
/// 抽成函数是为了能测"截断后仍然是人能读的字符串"。
pub(crate) fn elide(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let mut out: String = name.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{elide, task_actions_visible};

    #[test]
    fn context_actions_appear_only_with_a_selection() {
        assert!(
            !task_actions_visible(false),
            "没选中时不该出现任务动作：那是会话级的噪音"
        );
        assert!(task_actions_visible(true));
    }

    #[test]
    fn long_names_are_cut_on_character_boundaries() {
        assert_eq!(elide("a.bin", 8), "a.bin", "短名字原样");
        assert_eq!(elide("一二三四五", 3), "一二三…");
        // 按字符截：中文字符串不会被切成乱码（字节截断会）。
        assert_eq!(elide("素材包-03.7z", 4), "素材包-…");
    }
}
