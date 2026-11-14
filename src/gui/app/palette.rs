//! 命令面板（0.6.0 · 特色功能 3）：`Ctrl K` 打开，敲英文命令或中文解释都能搜到。
//!
//! ## 为什么和搜索共用 `Ctrl K`
//!
//! 设计稿里说"面板就是升级过的搜索"：以前 `Ctrl K` 只是把焦点扔进搜索框，现在它打开
//! 面板，面板第一项就是「搜索任务」—— 想搜任务的人多按一个回车就回到原来的行为，
//! 想找命令的人在这里找到全部。**一个键，一条心路**。
//!
//! `Ctrl F` 仍然直接聚焦搜索框（老习惯）。
//!
//! ## 为什么命令表是数据
//!
//! 界面只负责"画一张表 + 高亮第几条"，命令本身（英文名、中文名、一句解释、执行什么）
//! 全在 [`COMMANDS`] 里。加一条命令改一处，而且 `filtered` 是纯函数，能测。

use eframe::egui;

use super::App;

/// 面板里的一条命令。
pub(crate) struct Command {
    /// 稳定 id，执行时按它分发。
    pub id: &'static str,
    /// 英文命令名（也参与搜索：有人习惯敲 `settings`）。
    pub en: &'static str,
    /// 中文名（列表里显示这个）。
    pub zh: &'static str,
    /// 一句解释：这条命令到底干什么。
    pub hint: &'static str,
}

/// 全部命令。**顺序就是面板里的顺序**：常用的在前。
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        id: "search",
        en: "search-tasks",
        zh: "搜索任务",
        hint: "回到搜索框，按文件名 / 站点过滤列表",
    },
    Command {
        id: "new",
        en: "new-task",
        zh: "新建任务",
        hint: "粘贴一条链接，选个落点就开下",
    },
    Command {
        id: "paste",
        en: "paste-links",
        zh: "批量粘贴链接",
        hint: "一次贴一堆链接，逐条建任务",
    },
    Command {
        id: "grab",
        en: "grab-site",
        zh: "站点抓取",
        hint: "扫一个站点出清单，勾选后再下",
    },
    Command {
        id: "pause-all",
        en: "pause-all",
        zh: "全部暂停",
        hint: "正在跑的与排队的都停下，已下载的分片保留",
    },
    Command {
        id: "delete-done",
        en: "delete-completed",
        zh: "删除全部已完成",
        hint: "只清列表记录，磁盘上的文件不动",
    },
    Command {
        id: "settings",
        en: "settings",
        zh: "设置",
        hint: "连接数、限速、代理、规则、夜间批次",
    },
    Command {
        id: "open-out-dir",
        en: "open-out-dir",
        zh: "打开保存目录",
        hint: "在资源管理器里打开默认落点",
    },
    Command {
        id: "check-update",
        en: "check-update",
        zh: "检查更新",
        hint: "查一次清单，有新版会提示",
    },
];

/// 这条命令跟查询词对不对得上（英文名 / 中文名 / 解释任一处包含即可）。
pub(crate) fn matches(cmd: &Command, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    cmd.en.to_lowercase().contains(&q)
        || cmd.zh.to_lowercase().contains(&q)
        || cmd.hint.to_lowercase().contains(&q)
        || cmd.id.to_lowercase().contains(&q)
}

/// 命中查询词的命令（保持表里的顺序）。
pub(crate) fn filtered(query: &str) -> Vec<&'static Command> {
    COMMANDS.iter().filter(|c| matches(c, query)).collect()
}

impl App {
    /// 画命令面板。返回被执行的命令 id（没执行就是 `None`）。
    pub(super) fn palette(&mut self, ctx: &egui::Context) -> Option<&'static str> {
        if !self.palette_open {
            return None;
        }
        let hits = filtered(&self.palette_query);
        // 选中项跟着筛选结果收缩，避免"上下键走到列表外面"。
        if self.palette_sel >= hits.len() {
            self.palette_sel = hits.len().saturating_sub(1);
        }

        let mut chosen: Option<&'static str> = None;
        let mut close = false;

        egui::Window::new("命令面板")
            .open(&mut close)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
            .show(ctx, |ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .hint_text("敲命令…（英文名或中文都行）")
                        .desired_width(f32::INFINITY),
                );
                // 面板一打开就把焦点给输入框：这是"开始敲"的界面，点一下才能用太别扭。
                if !resp.has_focus() && self.palette_take_focus {
                    resp.request_focus();
                    self.palette_take_focus = false;
                }
                ui.add_space(4.0);

                if hits.is_empty() {
                    ui.label(
                        egui::RichText::new("没有匹配的命令")
                            .size(11.0)
                            .color(super::theme::INK_3),
                    );
                }
                for (i, cmd) in hits.iter().enumerate() {
                    let selected = i == self.palette_sel;
                    let resp = ui
                        .push_id(cmd.id, |ui| {
                            ui.selectable_label(
                                selected,
                                egui::RichText::new(format!("{}   {}", cmd.zh, cmd.en)).size(12.0),
                            )
                        })
                        .inner;
                    // 解释放右边，不挤在标题里。
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new(cmd.hint)
                                .size(10.0)
                                .color(super::theme::INK_3),
                        );
                    });
                    if resp.clicked() {
                        self.palette_sel = i;
                        chosen = Some(cmd.id);
                    }
                }

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("↑↓ 选择 · 回车执行 · Esc 关闭")
                        .size(10.0)
                        .color(super::theme::INK_3),
                );
            });

        // 键盘：上下移动、回车执行、Esc 关闭。
        let (mut up, mut down, mut enter) = (false, false, false);
        ctx.input(|i| {
            up = i.key_pressed(egui::Key::ArrowUp);
            down = i.key_pressed(egui::Key::ArrowDown);
            enter = i.key_pressed(egui::Key::Enter);
            if i.key_pressed(egui::Key::Escape) {
                close = true;
            }
        });
        if down && !hits.is_empty() {
            self.palette_sel = (self.palette_sel + 1) % hits.len();
        }
        if up && !hits.is_empty() {
            self.palette_sel = (self.palette_sel + hits.len() - 1) % hits.len();
        }
        if enter
            && chosen.is_none()
            && let Some(cmd) = hits.get(self.palette_sel)
        {
            chosen = Some(cmd.id);
        }

        if chosen.is_some() || close {
            self.palette_open = false;
            self.palette_query.clear();
            self.palette_sel = 0;
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(query: &str) -> Vec<&'static str> {
        filtered(query).into_iter().map(|c| c.id).collect()
    }

    #[test]
    fn an_empty_query_lists_every_command_in_order() {
        assert_eq!(ids("").len(), COMMANDS.len());
        assert_eq!(ids("")[0], "search", "常用的排最前");
    }

    #[test]
    fn both_the_english_name_and_the_chinese_name_find_a_command() {
        assert_eq!(ids("settings"), vec!["settings"]);
        assert_eq!(ids("设置"), vec!["settings"]);
        assert_eq!(ids("抓取"), vec!["grab"]);
        assert_eq!(ids("grab"), vec!["grab"]);
    }

    /// 解释也参与搜索：用户记得"那个清列表的命令"却记不住名字。
    #[test]
    fn the_hint_is_searchable_too() {
        assert_eq!(
            ids("磁盘上的文件"),
            vec!["delete-done"],
            "解释里的半句话也要能定位到具体那条"
        );
        assert!(
            ids("资源管理器").contains(&"open-out-dir"),
            "解释里的词要能搜到"
        );
        assert!(ids("分片").contains(&"pause-all"));
    }

    #[test]
    fn a_nonsense_query_matches_nothing() {
        assert!(filtered("绝对没有这条命令").is_empty());
    }

    /// 每条命令的三样文字都得有：没有解释的条目正是判据里点名的"半成品"。
    #[test]
    fn every_command_is_filled_in() {
        for c in COMMANDS {
            assert!(!c.id.is_empty());
            assert!(!c.en.is_empty(), "{} 缺英文名", c.id);
            assert!(!c.zh.is_empty(), "{} 缺中文名", c.id);
            assert!(!c.hint.is_empty(), "{} 缺一句解释", c.id);
        }
        // id 不能重（执行时按 id 分发）。
        let mut seen = std::collections::BTreeSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.id), "命令 id 重复：{}", c.id);
        }
    }
}
