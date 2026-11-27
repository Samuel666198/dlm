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
    /// 「新建任务」主区屏（**不是弹窗**）。
    ///
    /// 设计稿 v3 里 `#pane-new` 是主区的一块屏，与列表、抓取三选一；0.7.0 做成了
    /// `egui::Window` 弹窗。用户 2026-11-17："新建任务和站点抓取不是讨论的单独界面
    /// 形式，和 html 画的不太一样。"
    ///
    /// 改为屏之后有两个直接好处：
    /// 1. 左栏「新建任务」能显示"我就是当前那一屏"（`entry_row_active`）；
    /// 2. 表单能占满主区，链接框不再被 420pt 的弹窗宽度限制。
    pub(super) fn new_task_pane(&mut self, ui: &mut egui::Ui) {
        let mut submit = false;
        let mut cancel = false;

        // 内容整体左对齐、限宽 720：主区很宽时表单不该拉成一条线。
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_max_width(720.0);
                ui.label(
                    egui::RichText::new("新建任务")
                        .size(14.0)
                        .strong()
                        .color(theme::ink()),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("支持公开直链；带 Cookie 的下载仍走浏览器入口。")
                        .size(11.5)
                        .color(theme::ink_3()),
                );
                ui.add_space(12.0);

                ui.label(
                    egui::RichText::new("下载 URL")
                        .size(11.5)
                        .color(theme::ink_2()),
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
                ui.label(
                    egui::RichText::new("文件名")
                        .size(11.5)
                        .color(theme::ink_2()),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.add_form.name)
                        .hint_text("自动从 URL 推断")
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Referer（可选）")
                        .size(11.5)
                        .color(theme::ink_2()),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.add_form.referer)
                        .hint_text("https://example.com/")
                        .desired_width(f32::INFINITY),
                );

                if let Some(err) = &self.add_form.error {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(err).size(11.5).color(theme::bad()));
                }

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let can_submit = !self.add_form.url.trim().is_empty();
                    if ui
                        .add_enabled(can_submit, egui::Button::new("加入队列"))
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Esc 返回列表")
                            .size(10.5)
                            .color(theme::ink_3()),
                    );
                });
            });
        });

        // Esc 回列表：pane 时代它不关窗，只是切回列表屏。
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
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
                cookie: None,
                reply: crate::gui::discard_reply(),
            });
            self.add_form = AddForm::default();
            self.main_view = super::MainView::List;
        }

        if cancel {
            self.main_view = super::MainView::List;
        }
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
                        .color(theme::ink_3()),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("删除").color(theme::bad()),
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

/// 「删除选中的 N 个任务」的二次确认框。
///
/// 为什么单条不问、批量要问：单条删除的对象就在眼前（行上的复选框、上下文条上都写着名字），
/// 多问一句只是拖慢；批量是一次点掉一批，丢掉的是 N 条断点进度。
///
/// 确认时**重算一遍**要删的 id（与「删除全部已完成」同一套理由）：等着确认的这几秒里，
/// 队列可能又变了，或者用户又勾掉了一条。拿打开那一刻的集合去删会删错东西。
impl App {
    pub(super) fn confirm_batch_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(count) = self.confirm_batch_delete else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;

        egui::Window::new("删除选中的任务？")
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("{count} 个任务会从列表里移除。")).size(12.5));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("已经下载到磁盘的文件不会被删除；未下完的会丢掉断点进度。")
                        .size(11.5)
                        .color(theme::ink_3()),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("删除").color(theme::bad()),
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
            // 重算：只删"现在还勾着、且还看得见"的那些。
            let visible: Vec<String> = self
                .tasks()
                .iter()
                .filter(|t| self.visible(t))
                .map(|t| t.id.clone())
                .collect();
            let ids = crate::gui::app::batch::targets(&self.batch, &visible);
            self.remove_targets(&ids);
            self.confirm_batch_delete = None;
        }
        if cancel {
            self.confirm_batch_delete = None;
        }
    }
}
