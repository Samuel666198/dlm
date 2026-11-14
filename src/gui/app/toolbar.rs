//! 工具条：0.6.0 起顶层只留四样东西 —— `＋新建▾` / 搜索 / `⚙` / `⋯`。
//!
//! ## 为什么把动作搬走
//!
//! 以前顶层常年摆着 继续 / 暂停 / 删除 / 全部暂停 / 删除全部 五颗按钮：
//! 没选中任务时它们全是灰的，选中了又和列表里的行抢注意力。现在按设计稿分层：
//!
//! - **任务级动作**（继续 / 暂停 / 删除）→ 上下文动作条，只在有选中时出现（`context_bar.rs`）；
//! - **批量破坏性动作**（全部暂停 / 删除全部）→ 收进 `⋯`，删除全部还要二次确认。
//!
//! ## 菜单为什么用"先记动作、出闭包再执行"
//!
//! egui 的菜单内容是 `FnOnce(&mut Ui)`。在闭包里改 `self` 会和外面正在借的
//! `&mut self`（`self.toolbar(ui)`）打架，编译器不会让它过；而把要用的 id 列表
//! 在闭包外算好、闭包里只写一个 `Option<ToolbarAction>`，就没有任何借用交叉。
//! 上一轮这块做到一半回退，就是踩在这个作用域上。

use eframe::egui;

use super::App;
use super::icons::{Icon, tool_button};
use crate::gui::Ticket;
use crate::gui::app::theme;
use crate::gui::snapshot::TaskView;

/// 搜索框右边那些**宽度固定**的东西一共占多宽：Ctrl K 提示 + 设置 + 更多 两颗工具按钮 + 间距。
///
/// 这个数没有别的用处，就是给搜索框算"还能占多宽"。写成常量而不是散在算式里，
/// 是因为漏一项的后果是「按钮被挤出窗口」—— 不报错、不 panic，只是用不了。
const RIGHT_SIDE_W: f32 = 46.0 /* Ctrl K */ + 56.0 * 2.0 /* 两个工具按钮 */ + 22.0 /* 间距 */;

/// 工具条上能点出来的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolbarAction {
    /// 新建 → 单链接。
    NewSingle,
    /// 新建 → 批量粘贴。
    NewPaste,
    /// 新建 → 站点抓取。
    NewGrab,
    /// ⋯ → 全部暂停。
    PauseAll,
    /// ⋯ → 删除全部（已完成）。**这只是"请求确认"**，见 [`delete_all_step`]。
    DeleteAllDone,
}

/// 点「删除全部」之后该怎么走。
///
/// 单独抽出来是为了让"必须二次确认"这条规矩能被用例钉住：
/// 有已完成任务时返回 [`DeleteAllStep::AskConfirm`]，界面只弹确认框、**一颗都不删**；
/// 没有已完成任务时什么都不做（那颗菜单项本来就是灰的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteAllStep {
    /// 弹确认框，`n` 是要删的条数。
    AskConfirm(usize),
    /// 没事可做。
    Nothing,
}

pub(crate) fn delete_all_step(done_count: usize) -> DeleteAllStep {
    if done_count == 0 {
        DeleteAllStep::Nothing
    } else {
        DeleteAllStep::AskConfirm(done_count)
    }
}

/// 「全部暂停」的作用对象：在跑的和排队的。已暂停/已完成/失败的不重复发暂停。
pub(crate) fn pause_all_targets(tasks: &[TaskView]) -> Vec<String> {
    tasks
        .iter()
        .filter(|t| t.status == "running" || t.status == "queued")
        .map(|t| t.id.clone())
        .collect()
}

/// 「删除全部」的作用对象：只删**已完成**的。
///
/// 之所以不是"删所有"：队列里暂停中的任务还有断点，一刀切掉等于替用户丢进度；
/// 这条规矩从 0.5.0 就在（工具条的「删除全部」只列 done），这里只是把它抽成纯函数。
pub(crate) fn delete_all_targets(tasks: &[TaskView]) -> Vec<String> {
    tasks
        .iter()
        .filter(|t| t.status == "done")
        .map(|t| t.id.clone())
        .collect()
}

impl App {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        // 闭包外先算好：菜单里一个 `self` 都不碰。
        let active_ids = pause_all_targets(&self.tasks());
        let done_ids = delete_all_targets(&self.tasks());
        let mut action: Option<ToolbarAction> = None;

        ui.horizontal_centered(|ui| {
            ui.add_space(10.0);

            // ＋ 新建 ▾：主按钮，三个入口收在一个下拉里。
            let new_btn = new_task_button(ui);
            egui::Popup::menu(&new_btn).show(|ui| {
                ui.set_min_width(168.0);
                if ui.button("单链接…").clicked() {
                    action = Some(ToolbarAction::NewSingle);
                    ui.close();
                }
                if ui.button("批量粘贴…").clicked() {
                    action = Some(ToolbarAction::NewPaste);
                    ui.close();
                }
                if ui.button("站点抓取…").clicked() {
                    action = Some(ToolbarAction::NewGrab);
                    ui.close();
                }
            });

            ui.add_space(10.0);

            // 搜索占满中间：**必须先把右侧固定宽度的东西扣掉**再问 `available_width`，
            // 否则最后两颗按钮会被挤出窗口右缘 —— 第一版就是这么丢的 `⋯`
            // （截图实测：1684pt 宽的窗口里只剩「设置」，更多按钮整个落在窗口外）。
            let search_w = (ui.available_width() - RIGHT_SIDE_W).max(120.0);
            let search = ui.add_sized(
                [search_w, 28.0],
                egui::TextEdit::singleline(&mut self.search).hint_text("搜索名字、链接或站点"),
            );
            // Ctrl K 只是置个标记，真正抢焦点要等控件画出来这一帧。
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            ui.add_space(4.0);
            // 快捷键提示：点它也能聚焦，省得记不住键位还要去翻文档。
            let kbd = ui.add(
                egui::Label::new(
                    egui::RichText::new("Ctrl K")
                        .size(10.5)
                        .monospace()
                        .color(theme::INK_3),
                )
                .sense(egui::Sense::click()),
            );
            if kbd.on_hover_text("聚焦搜索框").clicked() {
                self.focus_search = true;
            }

            ui.add_space(6.0);
            if tool_button(ui, Icon::Gear, "设置", true).clicked() {
                self.settings_open = true;
                self.draft = self.settings.clone();
                self.settings_error = None;
            }

            // ⋯：批量与破坏性动作。主线上不放这些 —— 它们不该占着顶层。
            let more_btn = tool_button(ui, Icon::Dots, "更多", true);
            egui::Popup::menu(&more_btn).show(|ui| {
                ui.set_min_width(180.0);
                let label = if active_ids.is_empty() {
                    "全部暂停".to_string()
                } else {
                    format!("全部暂停（{}）", active_ids.len())
                };
                if ui
                    .add_enabled(!active_ids.is_empty(), egui::Button::new(label))
                    .clicked()
                {
                    action = Some(ToolbarAction::PauseAll);
                    ui.close();
                }
                let label = if done_ids.is_empty() {
                    "删除全部已完成".to_string()
                } else {
                    format!("删除全部已完成（{}）", done_ids.len())
                };
                if ui
                    .add_enabled(!done_ids.is_empty(), egui::Button::new(label))
                    .clicked()
                {
                    action = Some(ToolbarAction::DeleteAllDone);
                    ui.close();
                }
            });
        });

        // 出闭包再动状态：这里可以自由借 `self`。
        match action {
            Some(ToolbarAction::NewSingle) => {
                self.adding = true;
                self.add_form.focused = false;
            }
            Some(ToolbarAction::NewPaste) => self.basket_open = true,
            Some(ToolbarAction::NewGrab) => self.grab_open = true,
            Some(ToolbarAction::PauseAll) => {
                for id in active_ids {
                    self.send(Ticket::Pause(id));
                }
            }
            // **不在这里删任何东西**：先要确认，见 `delete_all_step` 与确认框。
            Some(ToolbarAction::DeleteAllDone) => {
                if let DeleteAllStep::AskConfirm(n) = delete_all_step(done_ids.len()) {
                    self.confirm_delete_all = Some(n);
                }
            }
            None => {}
        }
    }
}

/// 「＋ 新建 ▾」主按钮：强调色实底 + 画出来的加号与下拉箭头。
///
/// 为什么不用 `ui.menu_button("＋ 新建 ▾")`：`＋` 与 `▾` 这两个字符在
/// Microsoft YaHei 里没有字形，实测画出来是**方框**（与早年的 ▶ ❚❚ 🗑 同一个坑）。
/// 所以按钮自己画，菜单用 [`egui::Popup::menu`] 挂在它的 response 上 ——
/// 点击切换开合这件事 `Popup::menu` 已经处理好了。
fn new_task_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(104.0, 28.0), egui::Sense::click());
    let ink = egui::Color32::from_rgb(0xFF, 0xF9, 0xF4);
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        theme::RADIUS - 2.0,
        if response.hovered() {
            theme::ACCENT_INK
        } else {
            theme::ACCENT
        },
    );
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        egui::vec2(11.0, 11.0),
    );
    super::icons::draw_icon(painter, Icon::Plus, icon_rect, ink);
    painter.text(
        egui::pos2(rect.left() + 27.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "新建",
        egui::FontId::proportional(12.5),
        ink,
    );
    let chevron = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 13.0, rect.center().y),
        egui::vec2(9.0, 9.0),
    );
    super::icons::draw_icon(painter, Icon::ChevronDown, chevron, ink);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::snapshot::TaskView;

    fn view(id: &str, status: &str) -> TaskView {
        TaskView {
            id: id.to_string(),
            url: "https://example.com/a.bin".into(),
            name: format!("{id}.bin"),
            path: String::new(),
            referer: None,
            status: status.to_string(),
            pid: None,
            exit_code: None,
            log: String::new(),
            progress: 0.0,
            total_len: None,
            delivered: None,
            created_at: 0,
            updated_at: 0,
            start_at: None,
            hold_until: None,
            on_complete: None,
            attempts: 0,
            category: "other",
        }
    }

    /// 「全部暂停」只碰在跑与排队的：已完成/失败/已暂停的不该收到暂停票。
    #[test]
    fn pause_all_covers_exactly_the_live_tasks() {
        let tasks = vec![
            view("a", "running"),
            view("b", "queued"),
            view("c", "paused"),
            view("d", "done"),
            view("e", "failed"),
        ];
        assert_eq!(pause_all_targets(&tasks), vec!["a", "b"]);
    }

    /// 「删除全部」只删已完成：暂停中的还有断点，替用户删掉等于丢进度。
    #[test]
    fn delete_all_never_touches_unfinished_work() {
        let tasks = vec![
            view("a", "running"),
            view("b", "paused"),
            view("c", "done"),
            view("d", "done"),
        ];
        assert_eq!(delete_all_targets(&tasks), vec!["c", "d"]);
    }

    /// 破坏性动作必须先要确认：这一步的返回值就决定了"弹框"还是"什么都不做"。
    #[test]
    fn deleting_everything_always_asks_first() {
        assert_eq!(delete_all_step(0), DeleteAllStep::Nothing, "没东西可删");
        assert_eq!(delete_all_step(3), DeleteAllStep::AskConfirm(3));
    }
}
