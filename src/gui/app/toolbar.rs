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
const RIGHT_SIDE_W: f32 = 56.0 * 2.0 /* 两个工具按钮 */ + 22.0 /* 间距 */;

/// 搜索框的最大宽度。
///
/// 占满整条会让人以为它是万能输入框 —— 它只搜任务名与链接。两轮试过：340 显得局促，
/// 560 才是"够用但不压人"的位置。`Ctrl K` 提示内嵌在框内右端，不另占宽度。
const SEARCH_MAX_W: f32 = 560.0;

/// 工具条上能点出来的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolbarAction {
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
            ui.add_space(12.0);

            // 品牌位取代了 0.6.0 的「＋新建 ▾」大按钮：入口已经在左栏平铺，
            // 顶栏再放一个橙色大按钮既是重复，又把视线从内容上抢走。
            brand_block(ui);
            ui.add_space(26.0);

            // 搜索框：**先扣掉右侧固定宽度的东西**再问 `available_width`，否则最后两颗
            // 按钮会被挤出窗口右缘（0.6.0 第一版就是这么丢的 `⋯`，1684pt 窗口里只剩「设置」）。
            // 宽度取 560 与可用宽度中较小者：占满整条会让人以为它是万能输入框，它只搜任务。
            let search_w = (ui.available_width() - RIGHT_SIDE_W).clamp(160.0, SEARCH_MAX_W);
            self.search_field(ui, search_w);

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

/// 品牌位：**只有文字，没有图标**。
///
/// ## 为什么删掉那个方块图标
///
/// 窗口顶部本来就有两个 `Download Manager`：上面是 Windows **原生标题栏**的窗口标题，
/// 下面是这里画的"方块标 + 全称"。`docs/superpowers/specs/2026-11-10-native-shell-errata.md`
/// 的 E10 已经裁定走原生装饰（理由是"下载器不是设计作品，系统一致性更重要"），
/// 所以自绘品牌位是那条裁定的**遗留矛盾**。
///
/// 用户 2026-11-17 的处置：**删图标、保留文字、文字加粗**。
/// 这样系统标题栏里的那个是"系统告诉你的"，这里是"设计上的品牌位"，
/// 两份不再长得一模一样，也就不刺眼了。图标挪去别处（窗口图标 + 托盘图标都还在用同一份像素）。
fn brand_block(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(BRAND_W, 28.0), egui::Sense::hover());
    // **真加粗**：egui 的 `RichText::strong()` 只改颜色不改字形，
    // 所以这里直接指定名字为 `dlm-bold` 的字族（见 `fonts::BOLD_FAMILY`）。
    // 找不到粗体字体时退回 Proportional，至少文字还在、只是不粗。
    let family = if super::fonts::bold_available() {
        egui::FontFamily::Name(super::fonts::BOLD_FAMILY.into())
    } else {
        egui::FontFamily::Proportional
    };
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        crate::gui::APP_NAME,
        egui::FontId::new(14.5, family),
        theme::ink(),
    );
}

/// 品牌位占的宽度。
///
/// 原来是 180（塞了个 24pt 方块标 + 文字）。删掉图标后按文字实测宽度给，
/// 多留 8pt 是因为中文回退字体下 `APP_NAME` 比 Ubuntu 排出来略宽。
const BRAND_W: f32 = 150.0;

impl App {
    /// 搜索框：自己画底，输入框无边框塞进去，`Ctrl K` 提示**内嵌**在右端。
    ///
    /// 为什么不用 `add_sized(TextEdit)` 加一个外置快捷键标签：外置的那个像第二个按钮，
    /// 内嵌才像"输入框的一部分"。代价是得手画底与留白。
    fn search_field(&mut self, ui: &mut egui::Ui, width: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::hover());
        let painter = ui.painter().clone();
        // **`sunken` 而不是 `surface_2`**：输入框是"凹进去"的，`surface_2` 是 hover 色。
        // 一个色值兼两个语义，正是 0.7.0 层次压平的原因之一。
        painter.rect_filled(rect, theme::RADIUS - 2.0, theme::sunken());

        let chip_w = 46.0;
        let inner = egui::Rect::from_min_max(
            rect.min + egui::vec2(9.0, 2.0),
            egui::pos2(rect.right() - chip_w - 8.0, rect.bottom() - 2.0),
        );
        let resp = ui
            .new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            )
            .add_sized(
                [inner.width(), 22.0],
                egui::TextEdit::singleline(&mut self.search)
                    .frame(egui::Frame::NONE)
                    .hint_text("搜索任务、链接或站点"),
            );
        // Ctrl K 只是置个标记，真正抢焦点要等控件画出来这一帧。
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }

        let chip = egui::Rect::from_center_size(
            egui::pos2(rect.right() - chip_w / 2.0 - 7.0, rect.center().y),
            egui::vec2(chip_w, 18.0),
        );
        painter.rect_filled(chip, 4.0, theme::surface());
        painter.text(
            chip.center(),
            egui::Align2::CENTER_CENTER,
            "Ctrl K",
            egui::FontId::monospace(9.5),
            theme::ink_3(),
        );
        // 点提示也能聚焦：记不住键位的人有个可点的地方。
        if ui
            .interact(chip, ui.scope_id().with("kbd-hint"), egui::Sense::click())
            .on_hover_text("聚焦搜索框")
            .clicked()
        {
            self.focus_search = true;
        }
    }
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
