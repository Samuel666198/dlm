//! 工具条：新建任务、继续/暂停、全部暂停、删除、删除全部、设置。
//!
//! ## 图标为什么是画出来的
//!
//! 一开始用 `＋ ▶ ❚❚ 🗑` 这些字符当图标，实测**满屏方框** —— 它们不在
//! Microsoft YaHei 的字形里。改用 `Painter` 画线条：不依赖任何字体，
//! 在任何机器上都一样，也能跟着强调色走。

use eframe::egui;

use super::App;
use crate::gui::Ticket;
use crate::gui::app::theme;

/// 工具条上的图标种类。每一种对应一段绘制代码。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Plus,
    Play,
    Pause,
    PauseAll,
    Trash,
    TrashAll,
    Gear,
}

impl App {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.add_space(10.0);

            if tool_button(ui, Icon::Plus, "新建任务", true, true).clicked() {
                self.adding = true;
                self.add_form.focused = false;
            }

            // 继续 / 暂停按选中任务的状态启用，不常年灰着。
            let selected = self.selected_task();
            let can_resume = selected
                .as_ref()
                .is_some_and(|t| t.status == "paused" || t.status == "failed");
            let can_pause = selected
                .as_ref()
                .is_some_and(|t| t.status == "running" || t.status == "queued");

            // 0.6.0：没有选中时这两颗按钮不出现（以前是常年灰着占位置）。
            if task_actions_visible(self.selected.is_some()) {
                if tool_button(ui, Icon::Play, "继续", can_resume, false).clicked()
                    && let Some(id) = self.selected.clone()
                {
                    self.send(Ticket::Resume(id));
                }
                if tool_button(ui, Icon::Pause, "暂停", can_pause, false).clicked()
                    && let Some(id) = self.selected.clone()
                {
                    self.send(Ticket::Pause(id));
                }
            }

            ui.add_space(6.0);
            separator(ui);
            ui.add_space(6.0);

            let active_ids: Vec<String> = self
                .tasks()
                .iter()
                .filter(|t| t.status == "running" || t.status == "queued")
                .map(|t| t.id.clone())
                .collect();
            if tool_button(
                ui,
                Icon::PauseAll,
                "全部暂停",
                !active_ids.is_empty(),
                false,
            )
            .clicked()
            {
                for id in active_ids {
                    self.send(Ticket::Pause(id));
                }
            }

            let has_selected = self.selected.is_some();
            // 删除也只在选中时出现：破坏性动作不该在没有对象时占着位置。
            if task_actions_visible(has_selected)
                && tool_button(ui, Icon::Trash, "删除", true, false).clicked()
                && let Some(id) = self.selected.clone()
            {
                self.send(Ticket::Remove(id));
                self.selected = None;
            }

            let done_ids: Vec<String> = self
                .tasks()
                .iter()
                .filter(|t| t.status == "done")
                .map(|t| t.id.clone())
                .collect();
            if tool_button(ui, Icon::TrashAll, "删除全部", !done_ids.is_empty(), false).clicked()
            {
                for id in done_ids {
                    self.send(Ticket::Remove(id));
                }
                self.selected = None;
            }

            ui.add_space(6.0);
            separator(ui);
            ui.add_space(6.0);

            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("搜索名字或链接")
                    .desired_width(180.0),
            );
            // Ctrl+F 只是置个标记，真正抢焦点要等控件画出来这一帧。
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            ui.add_space(6.0);
            if tool_button(ui, Icon::Plus, "快速添加", false, false).clicked() {
                self.basket_open = true;
            }
            ui.add_space(6.0);
            if ui.button("站点抓取").clicked() {
                self.grab_open = true;
            }
            ui.add_space(6.0);
            if tool_button(ui, Icon::Gear, "设置", true, false).clicked() {
                self.settings_open = true;
                self.draft = self.settings.clone();
                self.settings_error = None;
            }
        });
    }

    fn selected_task(&self) -> Option<crate::gui::TaskView> {
        let id = self.selected.as_ref()?;
        self.tasks().iter().find(|t| &t.id == id).cloned()
    }
}

/// 一个工具条按钮：上图下字，40pt 高、56pt 宽。
fn tool_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    primary: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(56.0, 40.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );

    let hovered = enabled && response.hovered();
    let painter = ui.painter();

    // 主按钮：强调色实底。其余悬停时给淡底。
    if primary && enabled {
        painter.rect_filled(rect, theme::RADIUS - 2.0, theme::ACCENT);
    } else if hovered {
        painter.rect_filled(rect, theme::RADIUS - 2.0, theme::SURFACE_2);
    }

    let ink = if !enabled {
        theme::INK_3
    } else if primary {
        egui::Color32::from_rgb(0xFF, 0xF9, 0xF4)
    } else if hovered {
        theme::INK
    } else {
        theme::INK_2
    };

    // 图标：16×16 的框，居中偏上。
    let icon_rect =
        egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, 8.0), egui::vec2(16.0, 16.0));
    draw_icon(painter, icon, icon_rect, ink);

    painter.text(
        rect.center() + egui::vec2(0.0, 11.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        ink,
    );

    response
}

/// 16×16 框里画图标。全部用 1.6pt 线宽，与 demo 的描边感一致。
fn draw_icon(painter: &egui::Painter, icon: Icon, r: egui::Rect, color: egui::Color32) {
    let s = egui::Stroke::new(1.6, color);
    let c = r.center();
    let h = r.width() / 2.0;

    match icon {
        Icon::Plus => {
            painter.line_segment(
                [
                    egui::pos2(c.x, c.y - h * 0.7),
                    egui::pos2(c.x, c.y + h * 0.7),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.7, c.y),
                    egui::pos2(c.x + h * 0.7, c.y),
                ],
                s,
            );
        }
        Icon::Play => {
            // 实心三角，略向右偏以视觉居中。
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x - h * 0.45, c.y - h * 0.7),
                    egui::pos2(c.x + h * 0.7, c.y),
                    egui::pos2(c.x - h * 0.45, c.y + h * 0.7),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Pause => {
            let w = h * 0.28;
            for dx in [-h * 0.32, h * 0.32] {
                painter.rect_filled(
                    egui::Rect::from_center_size(egui::pos2(c.x + dx, c.y), egui::vec2(w, h * 1.4)),
                    1.0,
                    color,
                );
            }
        }
        Icon::PauseAll => {
            // 三条细竖线：全部暂停。
            let w = h * 0.2;
            for dx in [-h * 0.55, 0.0, h * 0.55] {
                painter.rect_filled(
                    egui::Rect::from_center_size(egui::pos2(c.x + dx, c.y), egui::vec2(w, h * 1.4)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Trash => {
            // 桶身：上宽下窄的梯形。
            painter.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(c.x - h * 0.5, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.5, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.36, c.y + h * 0.72),
                    egui::pos2(c.x - h * 0.36, c.y + h * 0.72),
                ],
                s,
            ));
            // 桶盖。
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.72, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.72, c.y - h * 0.4),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.22, c.y - h * 0.72),
                    egui::pos2(c.x + h * 0.22, c.y - h * 0.72),
                ],
                s,
            );
        }
        Icon::TrashAll => {
            // 桶 + 一条斜杠：删除全部。
            painter.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(c.x - h * 0.5, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.5, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.36, c.y + h * 0.72),
                    egui::pos2(c.x - h * 0.36, c.y + h * 0.72),
                ],
                s,
            ));
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.72, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.72, c.y - h * 0.4),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.8, c.y + h * 0.8),
                    egui::pos2(c.x + h * 0.8, c.y - h * 0.8),
                ],
                egui::Stroke::new(1.6, theme::BAD),
            );
        }
        Icon::Gear => {
            // 齿轮：一个圆 + 八根短齿。比画真齿形简单，观感够用。
            painter.circle_stroke(c, h * 0.42, s);
            painter.circle_filled(c, h * 0.16, color);
            for i in 0..8 {
                let a = std::f32::consts::TAU * (i as f32) / 8.0;
                let (sin, cos) = a.sin_cos();
                painter.line_segment(
                    [
                        egui::pos2(c.x + cos * h * 0.42, c.y + sin * h * 0.42),
                        egui::pos2(c.x + cos * h * 0.78, c.y + sin * h * 0.78),
                    ],
                    s,
                );
            }
        }
    }
}

fn separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 26.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::LINE);
}
/// 任务级动作（继续 / 暂停 / 删除）该不该出现在工具栏上。
///
/// 只由"有没有选中"决定：没有选中时它们本来就点不动，留着只是噪音。
/// 抽成函数是为了让这条规矩能被测试钉住，而不是埋在界面代码里。
pub(crate) fn task_actions_visible(has_selection: bool) -> bool {
    has_selection
}

#[cfg(test)]
mod tests {
    use super::task_actions_visible;

    #[test]
    fn task_actions_appear_only_with_a_selection() {
        assert!(!task_actions_visible(false), "没选中时不该出现任务动作");
        assert!(task_actions_visible(true));
    }
}
