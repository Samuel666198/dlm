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

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
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
            });
        });
    }
}
