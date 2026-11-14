//! 快捷键：桌面下载器不用鼠标也能干活。
//!
//! ## 一条硬规则：输入框有焦点时，单键快捷键一律不生效
//!
//! 在搜索框里按 `Del` 应该是删一个字，不是删任务；按 `Space` 是打空格。
//! 所以「无修饰键」的快捷键（`Del` / `Space`）只在没有控件持有焦点时才认。
//! 带 `Ctrl` 的组合键不受影响（`Ctrl+F` 本来就是要从任何地方跳到搜索框）。
//!
//! 判定逻辑抽成 [`shortcut_for`] 纯函数：输入是"这一帧按了什么 + 有没有焦点 +
//! 有没有选中任务"，输出是一个动作。这样不用开窗口就能把边界全测完。

use eframe::egui;

use super::App;

/// 一个快捷键动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    /// `Ctrl+N`：新建任务。
    NewTask,
    /// `Ctrl+K`（与旧版 `Ctrl+F`）：聚焦搜索框。
    FocusSearch,
    /// `Del`：删除选中的任务。
    DeleteSelected,
    /// `Space`：暂停 / 继续选中的任务。
    TogglePause,
    /// `Esc`：取消选择（收掉上下文动作条与详情抽屉）。
    ClearSelection,
}

/// 这一帧的按键状态（从 egui 的输入里摘出来，便于测试）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Keys {
    pub ctrl: bool,
    pub n: bool,
    pub f: bool,
    pub k: bool,
    pub delete: bool,
    pub space: bool,
    pub escape: bool,
}

/// 决定这一帧该执行哪个动作。
///
/// `typing`：是否有控件持有焦点（输入框）。为真时 `Del` / `Space` / `Esc` 不算快捷键
/// （打字时的 Esc 是"退出输入法候选"，不是"取消选择"）。
/// `has_selection`：有没有选中任务。没有选中时 `Del` / `Space` / `Esc` 什么都不做 ——
/// 不能让"随手按个空格"变成"删掉列表里的第一个"。
/// `overlay_open`：有弹窗（新建 / 设置 / 粘贴 / 抓取 / 更新 / 确认框）在开着。
/// 这时 Esc 归弹窗自己处理（它要关自己），不能顺手把选中也清掉 —— 用户按一次 Esc
/// 只想关掉眼前的框。
pub(crate) fn shortcut_for(
    keys: Keys,
    typing: bool,
    has_selection: bool,
    overlay_open: bool,
) -> Option<Shortcut> {
    if keys.ctrl && keys.n {
        return Some(Shortcut::NewTask);
    }
    // 0.6.0 起主键位是 Ctrl K（设计稿），Ctrl F 继续认 —— 老用户的手不必改。
    if keys.ctrl && (keys.k || keys.f) {
        return Some(Shortcut::FocusSearch);
    }
    if typing {
        return None;
    }
    if keys.escape && !overlay_open && has_selection {
        return Some(Shortcut::ClearSelection);
    }
    if keys.delete && has_selection {
        return Some(Shortcut::DeleteSelected);
    }
    if keys.space && has_selection {
        return Some(Shortcut::TogglePause);
    }
    None
}

impl App {
    /// 处理快捷键。放在帧首：这一帧的按键这一帧就生效。
    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let keys = ctx.input(|i| Keys {
            ctrl: i.modifiers.command,
            n: i.key_pressed(egui::Key::N),
            f: i.key_pressed(egui::Key::F),
            k: i.key_pressed(egui::Key::K),
            delete: i.key_pressed(egui::Key::Delete),
            space: i.key_pressed(egui::Key::Space),
            escape: i.key_pressed(egui::Key::Escape),
        });
        let has_selection = self.selected.is_some();
        match shortcut_for(keys, typing, has_selection, self.overlay_open()) {
            Some(Shortcut::NewTask) => {
                self.adding = true;
                self.add_form.focused = false;
            }
            Some(Shortcut::FocusSearch) => self.focus_search = true,
            Some(Shortcut::ClearSelection) => {
                self.selected = None;
                self.drawer_log = None;
            }
            Some(Shortcut::DeleteSelected) => {
                if let Some(id) = self.selected.clone() {
                    self.send(crate::gui::Ticket::Remove(id));
                    self.selected = None;
                    self.drawer_log = None;
                }
            }
            Some(Shortcut::TogglePause) => {
                if let Some(task) = self
                    .selected
                    .clone()
                    .and_then(|id| self.tasks().into_iter().find(|t| t.id == id))
                {
                    let ticket = if task.status == "running" {
                        crate::gui::Ticket::Pause(task.id)
                    } else {
                        crate::gui::Ticket::Resume(task.id)
                    };
                    self.send(ticket);
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只给要测的那一两个键置位，其余留空 —— 一条用例只钉一个行为。
    fn keys(ctrl: bool, n: bool, f: bool, delete: bool, space: bool) -> Keys {
        Keys {
            ctrl,
            n,
            f,
            delete,
            space,
            ..Keys::default()
        }
    }

    #[test]
    fn ctrl_n_opens_a_new_task_from_anywhere() {
        assert_eq!(
            shortcut_for(keys(true, true, false, false, false), false, false, false),
            Some(Shortcut::NewTask)
        );
        // 在搜索框里也要能按
        assert_eq!(
            shortcut_for(keys(true, true, false, false, false), true, true, false),
            Some(Shortcut::NewTask)
        );
    }

    #[test]
    fn ctrl_k_and_the_old_ctrl_f_both_focus_search() {
        let ctrl_k = Keys {
            ctrl: true,
            k: true,
            ..Keys::default()
        };
        assert_eq!(
            shortcut_for(ctrl_k, true, false, false),
            Some(Shortcut::FocusSearch),
            "Ctrl K 是设计稿的主键位，且输入框里有焦点也要认"
        );
        assert_eq!(
            shortcut_for(keys(true, false, true, false, false), true, false, false),
            Some(Shortcut::FocusSearch),
            "Ctrl F 继续认：老用户的手不该被迫改"
        );
    }

    #[test]
    fn delete_and_space_do_nothing_while_an_input_has_focus() {
        assert_eq!(
            shortcut_for(keys(false, false, false, true, false), true, true, false),
            None,
            "在搜索框里按 Del 是删字，不是删任务"
        );
        assert_eq!(
            shortcut_for(keys(false, false, false, false, true), true, true, false),
            None,
            "在输入框里按空格是打空格"
        );
    }

    #[test]
    fn delete_and_space_need_a_selection() {
        assert_eq!(
            shortcut_for(keys(false, false, false, true, false), false, false, false),
            None,
            "没选中就按 Del，不能删任何东西"
        );
        assert_eq!(
            shortcut_for(keys(false, false, false, false, true), false, false, false),
            None
        );
        assert_eq!(
            shortcut_for(keys(false, false, false, true, false), false, true, false),
            Some(Shortcut::DeleteSelected)
        );
        assert_eq!(
            shortcut_for(keys(false, false, false, false, true), false, true, false),
            Some(Shortcut::TogglePause)
        );
    }

    /// Esc 取消选择有三个前提：有选中、不在打字、没有弹窗先接着。
    #[test]
    fn escape_clears_the_selection_only_when_nothing_else_owns_it() {
        let esc = Keys {
            escape: true,
            ..Keys::default()
        };
        assert_eq!(
            shortcut_for(esc, false, true, false),
            Some(Shortcut::ClearSelection)
        );
        assert_eq!(
            shortcut_for(esc, false, false, false),
            None,
            "没选中就没什么可取消的"
        );
        assert_eq!(
            shortcut_for(esc, true, true, false),
            None,
            "在输入框里按 Esc 是 IME/输入的事，不该顺手清掉选中"
        );
        assert_eq!(
            shortcut_for(esc, false, true, true),
            None,
            "弹窗开着时 Esc 归弹窗：一次 Esc 只关眼前那一个框"
        );
    }
}
