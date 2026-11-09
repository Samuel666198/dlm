//! 设置面板。
//!
//! 分组照设计稿第八节：下载 / 行为 / 界面。每项都直接读写 `Settings`，
//! 点「保存」才落盘 —— 中途关掉不留半截配置。

use eframe::egui;

use super::App;
use crate::gui::app::theme;

impl App {
    pub(super) fn settings_panel(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut save = false;
        let mut cancel = false;

        egui::Window::new("设置")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(440.0)
                    .show(ui, |ui| {
                        self.settings_body(ui);
                    });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        save = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.add_space(8.0);
                    if let Some(err) = &self.settings_error {
                        ui.label(egui::RichText::new(err).size(11.5).color(theme::BAD));
                    }
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if save {
            if self.draft.on_complete_ok() {
                match self.draft.save(&self.settings_dir) {
                    Ok(()) => {
                        self.settings = self.draft.clone();
                        // 剪贴板开关变化要立刻生效。
                        self.apply_settings_runtime();
                        self.settings_error = None;
                        open = false;
                    }
                    Err(err) => {
                        self.settings_error = Some(format!("保存失败：{err:#}"));
                    }
                }
            } else {
                self.settings_error = Some("完成后动作只认 无 / open / run:<exe>".to_string());
            }
        }
        if cancel {
            self.draft = self.settings.clone();
            self.settings_error = None;
            open = false;
        }

        self.settings_open = open;
    }

    fn settings_body(&mut self, ui: &mut egui::Ui) {
        // ---- 下载 ----
        section(ui, "下载");

        row(ui, "每个任务的连接数", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.connections).range(1..=32));
        });
        row(ui, "同一服务器最大连接数", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.per_host_connections).range(0..=32));
            ui.label(
                egui::RichText::new("0 = 不限")
                    .size(10.5)
                    .color(theme::INK_3),
            );
        });
        row(ui, "全局限速 (KiB/s)", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.max_speed_kib).range(0..=1024 * 100));
            ui.label(
                egui::RichText::new("0 = 不限")
                    .size(10.5)
                    .color(theme::INK_3),
            );
        });

        ui.add_space(10.0);
        section(ui, "行为");

        // 剪贴板：带一句说明，讲清它只看"像下载链接"的文本。
        row(ui, "监视剪贴板", |ui| {
            ui.checkbox(&mut self.draft.watch_clipboard, "");
            ui.label(
                egui::RichText::new("只匹配看起来像下载链接的文本，不上传任何内容")
                    .size(10.5)
                    .color(theme::INK_3),
            );
        });

        row(ui, "完成后动作", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.draft.on_complete)
                    .hint_text("留空 / open / run:程序.exe")
                    .desired_width(220.0),
            );
        });

        ui.add_space(10.0);
        section(ui, "界面");

        row(ui, "关闭窗口时收进托盘", |ui| {
            ui.checkbox(&mut self.draft.close_to_tray, "");
        });
        row(ui, "收进托盘时提示", |ui| {
            ui.checkbox(&mut self.draft.tray_hint, "");
            if self.draft.tray_hint_shown {
                ui.label(
                    egui::RichText::new("已提示过")
                        .size(10.5)
                        .color(theme::INK_3),
                );
            }
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .size(10.5)
            .strong()
            .color(theme::INK_3),
    );
    ui.add_space(4.0);
}

/// 一行设置：左边标签（定宽），右边控件。
fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(160.0, 20.0),
            egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::INK_2)),
        );
        add(ui);
    });
    ui.add_space(2.0);
}
