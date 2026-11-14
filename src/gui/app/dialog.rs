//! 新建任务弹窗。Esc 关闭。

use eframe::egui;

use super::App;
use crate::gui::Ticket;
use crate::gui::app::theme;

/// 弹窗里的输入状态。关掉弹窗不清空，方便用户改完再提交。
#[derive(Default)]
pub struct AddForm {
    pub url: String,
    pub name: String,
    pub referer: String,
    pub error: Option<String>,
    /// 打开弹窗时只把焦点给 URL 框一次，不要在每帧都抢焦点。
    pub focused: bool,
}

impl App {
    pub(super) fn add_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;

        egui::Window::new("添加下载任务")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("支持公开直链；带 Cookie 的下载仍走浏览器入口。")
                        .size(11.5)
                        .color(theme::INK_3),
                );
                ui.add_space(10.0);

                ui.label(
                    egui::RichText::new("下载 URL")
                        .size(11.5)
                        .color(theme::INK_2),
                );
                let url = ui.add(
                    egui::TextEdit::singleline(&mut self.add_form.url)
                        .hint_text("https://example.com/file.bin")
                        .desired_width(f32::INFINITY),
                );
                // 打开时把焦点给 URL 框，只给一次。
                if !self.add_form.focused {
                    url.request_focus();
                    self.add_form.focused = true;
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new("文件名").size(11.5).color(theme::INK_2));
                ui.add(
                    egui::TextEdit::singleline(&mut self.add_form.name)
                        .hint_text("自动从 URL 推断")
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Referer（可选）")
                        .size(11.5)
                        .color(theme::INK_2),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.add_form.referer)
                        .hint_text("https://example.com/")
                        .desired_width(f32::INFINITY),
                );

                if let Some(err) = &self.add_form.error {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(err).size(11.5).color(theme::BAD));
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let can_submit = !self.add_form.url.trim().is_empty();
                    if ui
                        .add_enabled(can_submit, egui::Button::new("添加"))
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });

        // Esc 关弹窗。
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if submit {
            let url = self.add_form.url.trim().to_string();
            let raw_name = self.add_form.name.trim().to_string();
            let referer = self.add_form.referer.trim().to_string();
            let name = if raw_name.is_empty() {
                infer_name(&url)
            } else {
                raw_name
            };
            // UI 线程不等回执（等会死锁）。任务是否成功由快照里的状态体现。
            self.send(Ticket::Add {
                url,
                name,
                referer: if referer.is_empty() {
                    None
                } else {
                    Some(referer)
                },
                start_at: None,
                on_complete: None,
                reply: crate::gui::discard_reply(),
            });
            self.add_form = AddForm::default();
            open = false;
        }

        if cancel {
            open = false;
        }
        self.adding = open;
    }
}

/// 从 URL 末段推断文件名，推断不出就给个兜底名。
fn infer_name(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    if last.is_empty() {
        "download.bin".to_string()
    } else {
        last.to_string()
    }
}

/// 「删除全部已完成」的二次确认框。
///
/// 为什么不直接删：这是**一条命令删掉一批文件记录**的破坏性动作，而工具条上的
/// `⋯` 就在设置按钮旁边 —— 手滑一下就能点到。确认框里点「删除」才真的发票。
///
/// 确认时**重算一遍**要删的 id：等着确认的这几秒里可能又有任务下完了、
/// 也可能有任务被别的路径删掉了。拿打开那一刻的快照去删会删错东西。
impl App {
    pub(super) fn confirm_delete_all_dialog(&mut self, ctx: &egui::Context) {
        let Some(count) = self.confirm_delete_all else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;

        egui::Window::new("删除全部已完成的任务？")
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{count} 个已完成的任务会从列表里移除。"))
                        .size(12.5),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("已经下载到磁盘的文件不会被删除，只是不再列在队列里。")
                        .size(11.5)
                        .color(theme::INK_3),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("删除").color(theme::BAD),
                        ))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if confirm {
            let ids = crate::gui::app::toolbar::delete_all_targets(&self.tasks());
            for id in ids {
                self.send(Ticket::Remove(id));
            }
            self.selected = None;
            self.drawer_log = None;
            self.confirm_delete_all = None;
        }
        if cancel {
            self.confirm_delete_all = None;
        }
    }
}
