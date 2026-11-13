//! 状态栏：总速度、在跑数量、配额、版本。

use eframe::egui;

use super::App;
use crate::gui::app::theme;

impl App {
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let tasks = self.tasks();
        let running = tasks.iter().filter(|t| t.status == "running").count();
        let waiting = tasks
            .iter()
            .filter(|t| t.status == "queued" && t.hold_until.is_some())
            .count();
        let quota = self.quota_view();

        ui.horizontal_centered(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(format!("{running} 个在跑"))
                    .size(11.5)
                    .color(theme::INK_3),
            );
            if waiting > 0 {
                ui.label(
                    egui::RichText::new(format!("· {waiting} 个等待"))
                        .size(11.5)
                        .color(theme::INK_3),
                );
            }

            // 总速度：所有在跑任务的速率之和。没有在跑、或还没采到两个点时是 0，就不显示 ——
            // 挂一个「0 B/s」比不显示更容易让人以为卡住了。
            let total = self.total_speed();
            if total >= 1.0 {
                ui.label(
                    egui::RichText::new(format!("· {}", crate::gui::app::list::fmt_speed(total)))
                        .size(11.5)
                        .monospace()
                        .color(theme::INK_2),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!("v{}", crate::gui::RELEASE))
                        .size(11.5)
                        .monospace()
                        .color(theme::INK_3),
                );

                if let Some(q) = quota {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}/{}",
                            crate::gui::app::list::fmt_bytes(q.used),
                            crate::gui::app::list::fmt_bytes(q.limit)
                        ))
                        .size(11.0)
                        .monospace()
                        .color(theme::INK_2),
                    );
                    ui.add_space(4.0);
                    // 配额条：一条 78pt 的细轨。
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(78.0, 4.0), egui::Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, 99.0, theme::TRACK);
                    let frac = if q.limit == 0 {
                        0.0
                    } else {
                        (q.used as f64 / q.limit as f64).clamp(0.0, 1.0) as f32
                    };
                    if frac > 0.0 {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(rect.width() * frac, rect.height()),
                            ),
                            99.0,
                            theme::ACCENT,
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("配额").size(11.5).color(theme::INK_3));
                }

                // 有新版 / 检查失败时，这里多一枚可点的标记。没更新的机器看不到任何东西。
                ui.add_space(12.0);
                self.update_badge(ui);
            });
        });
    }
}
