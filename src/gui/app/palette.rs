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
    /// 分组（新建 / 视图 / 任务操作 / 系统）。**同组的必须连在一起** —— 面板按组打小标题，
    /// 顺序一乱就会同一个组名出现两次。
    pub group: &'static str,
}

/// 四个分组，顺序即面板里的顺序。
/// 命令面板的高度上限：**按视口算**，不写死。
///
/// 15 条命令每条还带一行说明，加起来 700+ 点；在 733 高的窗口里必然冲出底边被裁，
/// 后面几条（打开保存目录、检查更新）就是这么"消失"的（2026-11-27 用户截图）。
/// 减去 90（顶部锚点）+ 24（底部呼吸），再兜一个下限，
/// 免得窗口被拉得极小时面板塌成一条缝。
pub(crate) fn palette_max_height(viewport_height: f32) -> f32 {
    (viewport_height - 90.0 - 24.0).max(180.0)
}

pub(crate) const GROUPS: [&str; 4] = ["新建", "视图", "任务操作", "系统"];

/// 全部命令。**顺序就是面板里的顺序**：常用的在前。
pub(crate) const COMMANDS: &[Command] = &[
    // ---- 新建 ----
    Command {
        id: "new",
        en: "new-task",
        zh: "新建任务",
        hint: "粘贴一条链接，选个落点就开下",
        group: "新建",
    },
    Command {
        id: "paste",
        en: "paste-links",
        zh: "批量粘贴链接",
        hint: "一次贴一堆链接，逐条建任务",
        group: "新建",
    },
    Command {
        id: "grab",
        en: "grab-site",
        zh: "站点抓取",
        hint: "扫一个站点出清单，勾选后再下",
        group: "新建",
    },
    // ---- 视图 ----
    Command {
        id: "search",
        en: "search-tasks",
        zh: "搜索任务",
        hint: "回到搜索框，按文件名 / 站点过滤列表",
        group: "视图",
    },
    Command {
        id: "toggle-sidebar",
        en: "toggle-sidebar",
        zh: "收起 / 展开侧栏",
        hint: "只留一条图标，把宽度让给列表",
        group: "视图",
    },
    // ---- 任务操作 ----
    Command {
        id: "pause-all",
        en: "pause-all",
        zh: "全部暂停",
        hint: "正在跑的与排队的都停下，已下载的分片保留",
        group: "任务操作",
    },
    Command {
        id: "delete-done",
        en: "delete-completed",
        zh: "删除全部已完成",
        hint: "只清列表记录，磁盘上的文件不动",
        group: "任务操作",
    },
    // ---- 系统 ----
    Command {
        id: "settings",
        en: "settings",
        zh: "设置",
        hint: "连接数、限速、代理、规则、夜间批次",
        group: "系统",
    },
    Command {
        id: "open-out-dir",
        en: "open-out-dir",
        zh: "打开保存目录",
        hint: "在资源管理器里打开默认落点",
        group: "系统",
    },
    Command {
        id: "check-update",
        en: "check-update",
        zh: "检查更新",
        hint: "查一次清单，有新版会提示",
        group: "系统",
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

        // **不要传 `.open(&mut close)`**：egui 把这个 bool 的初值当成"窗口是否打开"，
        // 初值 `false` 会让它每帧直接跳过绘制 —— 快捷键明明触发了，面板却不出现
        // （0.6.0 发布版就是这个症状）。面板本来没有标题栏，也就没有X按钮需要这个参数。
        // 高度上限**按视口算**，不写死：15 条命令每条还带一行说明，
        // 加起来 700+ 点，在 733 高的窗口里必然冲出底边被裁 —— 后面几条
        // （打开保存目录、检查更新）就是这么"消失"的。
        // 减去 90（顶部锚点）+ 24（给底部留点呼吸），再兜一个最小值，
        // 免得窗口被拉得极小时面板塌成一条缝。
        // egui 0.37 里 `Context` 已经没有 `screen_rect()` 了，用 `content_rect()`
        // （= 可用内容区；`viewport_rect()` 含安全边距，会让上限偏大）。
        let max_height = palette_max_height(ctx.content_rect().height());
        let shown = egui::Window::new("命令面板")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .default_width(460.0)
            .max_height(max_height)
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
                            .color(super::theme::ink_3()),
                    );
                }
                // 列表滚动：命令总数是固定的十几条，做分页要多出翻页和页码两个交互，
                // 收益不抵成本（VS Code / Spotlight 这类面板都是滚动）。
                // 高度 = 窗口上限 - 输入框 - 底部提示，留出呼吸。
                let list_height = (max_height - 104.0).max(80.0);
                let mut last_group: Option<&str> = None;
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, cmd) in hits.iter().enumerate() {
                            // 组小标题：只在换组时出现一次。搜索态下它就是"这条命令住在哪"。
                            if last_group != Some(cmd.group) {
                                // 组名只能是那四个之一（调试版里当场炸，别等它悄悄画出个野组名）。
                                debug_assert!(
                                    GROUPS.contains(&cmd.group),
                                    "命令 {} 的组名不在 GROUPS 里",
                                    cmd.id
                                );
                                if last_group.is_some() {
                                    ui.add_space(2.0);
                                }
                                ui.label(
                                    egui::RichText::new(cmd.group)
                                        .size(9.5)
                                        .strong()
                                        .color(super::theme::ink_3()),
                                );
                                last_group = Some(cmd.group);
                            }
                            let selected = i == self.palette_sel;
                            let resp = ui
                                .push_id(cmd.id, |ui| {
                                    ui.selectable_label(
                                        selected,
                                        egui::RichText::new(format!("{}   {}", cmd.zh, cmd.en))
                                            .size(12.0),
                                    )
                                })
                                .inner;
                            // 解释放右边，不挤在标题里。
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(
                                    egui::RichText::new(cmd.hint)
                                        .size(10.0)
                                        .color(super::theme::ink_3()),
                                );
                            });
                            // 键盘挪动之后必须把选中项滚进视野，否则"看不见自己在选第几条"。
                            if selected && self.palette_scroll_to_sel {
                                resp.scroll_to_me(Some(egui::Align::Center));
                            }
                            if resp.clicked() {
                                self.palette_sel = i;
                                chosen = Some(cmd.id);
                            }
                        }
                    });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("↑↓ 选择 · 回车执行 · Esc 关闭")
                        .size(10.0)
                        .color(super::theme::ink_3()),
                );
            });
        // 记下这一帧面板实际占的上下边：用例靠它断言"在矮窗口里不会冲出视口"。
        // （这是唯一能自动验证"没被裁掉"的途径 —— 光看代码只能说"我加了 max_height"。）
        if let Some(inner) = shown {
            self.palette_last_rect = Some((inner.response.rect.min.y, inner.response.rect.max.y));
        }

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
            self.palette_scroll_to_sel = true;
        }
        if up && !hits.is_empty() {
            self.palette_sel = (self.palette_sel + hits.len() - 1) % hits.len();
            self.palette_scroll_to_sel = true;
        }
        // 这一帧的绘制已经用过了，清掉 —— 免得不按键也每帧都往中间拽。
        self.palette_scroll_to_sel = false;
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
        // 0.7.0 起按组分块，第一项从「搜索任务」变成「新建任务」：
        // 「新建」是这条面板最常被打开的理由，而搜索有 Ctrl F 这个专用键。
        assert_eq!(ids("")[0], "new", "新建排最前");
        assert!(ids("").contains(&"search"), "搜索仍在表里，只是不在第一位");
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

    /// **同组必须连在一起**，而且组名只能是那四个之一。
    ///
    /// 面板按组打小标题：一组接一组地画，换组才画标题。命令表里只要有一条排错位置，
    /// 界面上就会出现两次「系统」这种重复小标题 —— 而且不会报错，只是看着乱。
    #[test]
    fn commands_are_clustered_by_group() {
        let mut seen: Vec<&str> = Vec::new();
        let mut last: Option<&str> = None;
        for c in COMMANDS {
            assert!(
                GROUPS.contains(&c.group),
                "未知分组：{}（{}）",
                c.group,
                c.id
            );
            if last != Some(c.group) {
                assert!(
                    !seen.contains(&c.group),
                    "分组「{}」出现了两次：{} 之后又接回来，面板里会重复小标题",
                    c.group,
                    c.id
                );
                seen.push(c.group);
                last = Some(c.group);
            }
        }
        // 四个分组都有命令：空组等于面板里缺一整类。
        for g in GROUPS {
            assert!(COMMANDS.iter().any(|c| c.group == g), "分组「{g}」是空的");
        }
    }
}
