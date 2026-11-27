//! 任务列表：表头 + 行 + 进度条。虚拟滚动由 `ScrollArea::show_rows` 负责。

use eframe::egui;

use super::App;
use crate::actions::RevealMode;
use crate::gui::app::theme;
use crate::gui::snapshot::TaskView;

use super::batch;

/// 列宽（point）。与 demo 对齐：文件名弹性，其余定宽。
const COL_SIZE: f32 = 92.0;
const COL_PROGRESS: f32 = 150.0;
const COL_SPEED: f32 = 96.0;
const COL_ETA: f32 = 84.0;
const COL_STATUS: f32 = 88.0;
const GAP: f32 = 12.0;
const PAD: f32 = 16.0;

/// 进度条至少这么宽才有信息量（再窄就只剩一条线）。
const COL_PROGRESS_MIN: f32 = 64.0;
/// 勾选框中心距行左边的距离。列排布从它右边开始，所以文件名列要跟着让。
const NAME_MIN: f32 = 150.0;

/// 批量悬浮岛的宽度与高度。
///
/// 宽 420：五个元素（计数 + 三颗按钮 + 取消选择）在 11.5pt 下刚好排开，再多就是空。
/// 高 40：比一行按钮高 8pt，看起来才像"一块浮起来的东西"而不是压扁的横条。
const ISLAND_W: f32 = 420.0;
const ISLAND_H: f32 = 40.0;

/// 勾选框那一列的宽度（含左右留白）。
const CB_COL: f32 = 34.0;
/// 勾选框中心距行左边的距离。
const CB_X: f32 = 20.0;

/// 右键菜单里的动作。
///
/// 0.7.0 那版这里是六个 `let _ = ui.button("…")` —— 一个都没接线，点下去什么都不发生，
/// 而且 `tests.rs` 里搜不到任何与"右键"有关的用例，所以它从引入起就是死的。
///
/// 现在动作走枚举 + 穷举 `match`：往菜单里加一项却忘了处理，**编译**就红。
/// 这条比"记得写用例"更结实，因为漏接线这件事本来就发生在写用例之前。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowMenuAction {
    Resume,
    Pause,
    OpenFile,
    OpenFolder,
    CopyLink,
    Remove,
}

/// 右键菜单的一项。
pub(crate) struct RowMenuItem {
    pub action: RowMenuAction,
    pub label: &'static str,
    /// 右侧的快捷键提示（设计决定 10 要求带）。
    pub shortcut: Option<&'static str>,
    pub enabled: bool,
    /// 这一项前面是否来一条分隔线（分组）。
    pub separator_before: bool,
}

/// 某一行的右键菜单。
///
/// **可用性由任务状态决定**，不交给调用方判断：「继续」对正在跑的没意义；
/// 「打开文件」对还没下完的只会在 runner 那边换来一句"任务还没下完"。
pub(crate) fn row_menu_items(task: &TaskView) -> Vec<RowMenuItem> {
    let can_resume = matches!(task.status.as_str(), "paused" | "failed");
    let can_pause = matches!(task.status.as_str(), "running" | "queued");
    let done = task.status == "done";
    vec![
        RowMenuItem {
            action: RowMenuAction::Resume,
            label: "继续",
            shortcut: Some("Space"),
            enabled: can_resume,
            separator_before: false,
        },
        RowMenuItem {
            action: RowMenuAction::Pause,
            label: "暂停",
            shortcut: Some("Space"),
            enabled: can_pause,
            separator_before: false,
        },
        RowMenuItem {
            action: RowMenuAction::OpenFile,
            label: "打开文件",
            shortcut: None,
            enabled: done,
            separator_before: true,
        },
        RowMenuItem {
            action: RowMenuAction::OpenFolder,
            label: "打开文件夹",
            shortcut: None,
            enabled: done,
            separator_before: false,
        },
        RowMenuItem {
            action: RowMenuAction::CopyLink,
            label: "复制链接",
            shortcut: None,
            enabled: true,
            separator_before: false,
        },
        RowMenuItem {
            action: RowMenuAction::Remove,
            label: "删除",
            shortcut: Some("Del"),
            enabled: true,
            separator_before: true,
        },
    ]
}

/// 画右键菜单，返回被点中的那一项。
///
/// 返回值必须被处理。**注意 `#[must_use]` 挡不住 `let _ =`**（实测：
/// `let _ = f();` 不会触发 `unused_must_use`）—— 0.7.0 那六个死按钮正是这么写出来的。
/// 真正拦住"加了菜单项却没接线"的是调用处的**穷举 `match`**：
/// 往 [`RowMenuAction`] 里加一个变体而不处理，编译直接报 non-exhaustive。
/// 这个属性只是顺手拦住"忘了接收返回值"那种写法。
#[must_use]
pub(crate) fn row_menu_ui(ui: &mut egui::Ui, task: &TaskView) -> Option<RowMenuAction> {
    let mut picked = None;
    for item in row_menu_items(task) {
        if item.separator_before {
            ui.separator();
        }
        let button = egui::Button::new(item.label);
        let button = match item.shortcut {
            Some(key) => {
                button.shortcut_text(egui::RichText::new(key).size(10.0).color(theme::ink_3()))
            }
            None => button,
        };
        if ui.add_enabled(item.enabled, button).clicked() {
            picked = Some(item.action);
            ui.close();
        }
    }
    picked
}

/// 批量条上按下的东西。收集起来出闭包再执行 —— 闭包里动 `self` 会与 `&self.batch` 打架。
enum BatchAction {
    Resume,
    Pause,
    Remove,
    Clear,
}

/// 一行的列矩形。
///
/// **行与表头共用这一份排布**：两边各算一次，迟早会错位 —— 0.7.0 把详情改成右栏之后，
/// 列表可用宽度少了 330pt，行与表头的算法只要有一处没跟上，进度条就会压到文件名上。
///
/// 放不下时**先从右边丢掉「剩余」「速度」两列**：这两列多数时候显示 `—`，
/// 而文件名与进度每行都要看。
struct Columns {
    name: egui::Rect,
    size: egui::Rect,
    progress: egui::Rect,
    speed: Option<egui::Rect>,
    eta: Option<egui::Rect>,
    status: egui::Rect,
}

/// 按可用宽度排出这一行的各列位置。
fn columns(rect: egui::Rect) -> Columns {
    let inner = rect.width() - PAD - CB_COL;
    // 满编需要的宽度：五列 + 五个间隔 + 文件名最小宽。
    let full = COL_SIZE + COL_PROGRESS + COL_SPEED + COL_ETA + COL_STATUS + GAP * 5.0 + NAME_MIN;
    // 只留「大小 + 进度(最小) + 状态」时需要的宽度。
    let lean = COL_SIZE + COL_PROGRESS_MIN + COL_STATUS + GAP * 3.0 + NAME_MIN;
    let show_eta = inner >= full;
    let show_speed = inner >= full - COL_ETA - GAP;

    let mut right = rect.right() - PAD;
    let mut col = |w: f32| -> egui::Rect {
        let r = egui::Rect::from_min_max(
            egui::pos2(right - w, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        right -= w + GAP;
        r
    };

    let status = col(COL_STATUS);
    let eta = if show_eta { Some(col(COL_ETA)) } else { None };
    let speed = if show_speed {
        Some(col(COL_SPEED))
    } else {
        None
    };
    // 进度条吃掉能给的宽度，但不超设计值、不低于最小值。
    let progress_w = if show_speed {
        COL_PROGRESS
    } else {
        (inner - lean + COL_PROGRESS_MIN).clamp(COL_PROGRESS_MIN, COL_PROGRESS)
    };
    let progress = col(progress_w);
    let size = col(COL_SIZE);
    let name = egui::Rect::from_min_max(
        egui::pos2(rect.left() + CB_COL, rect.top()),
        egui::pos2(right, rect.bottom()),
    );
    Columns {
        name,
        size,
        progress,
        speed,
        eta,
        status,
    }
}

impl App {
    pub(super) fn list(&mut self, ui: &mut egui::Ui) {
        let all = self.tasks();
        let rows: Vec<TaskView> = all.iter().filter(|t| self.visible(t)).cloned().collect();

        // 删掉的任务可能还留在批量里（快照晚一帧），先清一遍再画。
        let alive: Vec<String> = all.iter().map(|t| t.id.clone()).collect();
        batch::prune(&mut self.batch, &alive);

        let visible_ids: Vec<String> = rows.iter().map(|t| t.id.clone()).collect();
        if self.thead(ui, &visible_ids) {
            batch::toggle_all(&mut self.batch, &visible_ids);
        }

        // 外部下载（浏览器直连 / HLS）作为**只读行**接在任务后面：
        // 它们本来就不是队列任务，所以不计入任何计数，也不参与筛选。
        let outsider = self.externals.clone();

        if rows.is_empty() && outsider.is_empty() {
            self.empty_state(ui);
            return;
        }

        let total_rows = rows.len() + outsider.len();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, theme::ROW_H, total_rows, |ui, range| {
                for i in range {
                    if i < rows.len() {
                        self.row(ui, &rows[i]);
                    } else {
                        Self::external_row(ui, &outsider[i - rows.len()]);
                    }
                }
            });

        // 悬浮岛**最后画**：它要盖在列表上面，而且不参与布局（列表位置不动）。
        self.batch_island(ui, &visible_ids);
    }

    /// 一行**外部下载**（浏览器直连 / HLS）：与任务行同高同列，但没有勾选框、
    /// 没有右键菜单 —— 它不由队列控制，暂停/续传都无从谈起。
    ///
    /// 状态列里放一枚「外部」胶囊：这是用户要的标签，也是"为什么这条点不动"的解释。
    fn external_row(ui: &mut egui::Ui, item: &crate::queue::External) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::ROW_H),
            egui::Sense::hover(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }
        let cols = columns(rect);
        let painter = ui.painter();

        // 名字（左）—— 与任务行同一起点，列才对得齐。
        painter.text(
            cols.name.left_center(),
            egui::Align2::LEFT_CENTER,
            &item.name,
            egui::FontId::proportional(12.5),
            theme::ink(),
        );

        // 大小（右对齐）
        painter.text(
            cols.size.right_center(),
            egui::Align2::RIGHT_CENTER,
            fmt_bytes(item.total),
            egui::FontId::proportional(11.0),
            theme::ink_2(),
        );

        // 进度：整体一条（外部行不画分段 —— 分块细节留给详情）
        let track_h = 4.0;
        let track = egui::Rect::from_min_size(
            egui::pos2(cols.progress.left(), rect.center().y - track_h / 2.0),
            egui::vec2((cols.progress.width() - 34.0 - 8.0).max(20.0), track_h),
        );
        painter.rect_filled(track, 99.0, theme::track());
        let pct = (item.percent() / 100.0).clamp(0.0, 1.0);
        if pct > 0.0 {
            painter.rect_filled(
                egui::Rect::from_min_size(track.min, egui::vec2(track.width() * pct, track_h)),
                99.0,
                theme::accent_soft(),
            );
        }

        // 百分比：与任务行同一位置、同字号，这样两行的进度列读起来一致。
        painter.text(
            egui::pos2(cols.progress.right(), rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("{:.0}%", item.percent()),
            egui::FontId::monospace(11.0),
            theme::ink_2(),
        );

        // 「外部」胶囊 —— 与状态列里那些胶囊同一套画法（圆角到底 + 居中文案）。
        let galley = painter.layout_no_wrap(
            "外部".to_string(),
            egui::FontId::proportional(11.0),
            theme::ink_3(),
        );
        let pill = egui::Rect::from_min_size(
            egui::pos2(cols.status.left(), rect.center().y - 19.0 / 2.0),
            egui::vec2((galley.size().x + 20.0).min(cols.status.width()), 19.0),
        );
        painter.rect_filled(pill, 99.0, theme::surface_3());
        painter.text(
            pill.center(),
            egui::Align2::CENTER_CENTER,
            "外部",
            egui::FontId::proportional(11.0),
            theme::ink_3(),
        );

        // 悬停说明：这条为什么在这儿、为什么点不动。
        response.on_hover_text(format!(
            "{}\n外部进程在下载（浏览器直连 / HLS），不在队列里 —— 桌面端只读，不能暂停或续传。",
            item.path.display()
        ));
    }

    /// 一行任务。
    ///
    /// 单击 = 在右栏查看它；Ctrl+单击 = 顺便把它勾进批量；勾复选框 = 只动批量。
    /// 三者的分工写在 `batch` 模块的文档里，这里只是照做。
    fn row(&mut self, ui: &mut egui::Ui, task: &TaskView) {
        let selected = self.selected.as_deref() == Some(task.id.as_str());
        let checked = self.batch.contains(&task.id);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::ROW_H),
            egui::Sense::click(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }

        let painter = ui.painter();
        if selected {
            // 用专用的 `row_selected`，不再借 `accent_soft`（那是标签/胶囊的底）。
            // 整行 52pt 涂一个"标签色"就是色块，太重 —— 见 theme.rs 的字段注释。
            painter.rect_filled(rect, 0.0, theme::row_selected());
            // 左侧 2.5pt 强调条。
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(0.0, 8.0),
                    egui::vec2(2.5, rect.height() - 16.0),
                ),
                2.0,
                theme::accent(),
            );
        } else if checked {
            // 勾中但没在看：淡蓝底。与"正在查看"的橙条区分开，两种状态可以同时存在。
            // 勾中：走 `surface_3`（内嵌层），与"悬停"区分开 —— 两种状态可以同时存在。
            painter.rect_filled(rect, 0.0, theme::surface_3());
        } else if response.hovered() {
            // 悬停才用 `surface_2`。这一处是它从今往后的**唯一**语义。
            painter.rect_filled(rect, 0.0, theme::surface_2());
        }
        // 行分隔线。
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::vec2(rect.width(), 1.0),
            ),
            0.0,
            theme::line_soft(),
        );

        self.row_w = rect.width();
        let cols = columns(rect);
        let name_rect = cols.name;
        let size_rect = cols.size;
        let progress_rect = cols.progress;
        let status_rect = cols.status;

        self.draw_name(ui, name_rect, task);
        text_right(
            ui,
            size_rect,
            &fmt_bytes_opt(task.total_len),
            theme::ink_2(),
            12.0,
        );
        draw_progress(ui, progress_rect, task);
        let speed = self.speed_of(&task.id);
        if let Some(speed_rect) = cols.speed {
            text_right(
                ui,
                speed_rect,
                &speed_text(task, speed),
                theme::ink_2(),
                12.0,
            );
        }
        if let Some(eta_rect) = cols.eta {
            text_right(ui, eta_rect, &eta_text(task, speed), theme::ink_3(), 12.0);
        }
        draw_status(ui, status_rect, task);

        // 告诉无障碍树"这一行是个可点的项"。**画出来的控件默认不在树里** ——
        // 不写这一句，读屏软件读不到列表，UI 自动化也点不到任何一行
        // （0.7.0 出证据时卡在这：headless 禁物理点击，而树里没有行）。
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!(
                    "{} · {} · {}",
                    task.name,
                    task.status,
                    fmt_bytes_opt(task.total_len)
                ),
            )
        });
        // ---- 左侧勾选框 ----
        //
        // **用真的 `Checkbox` 而不是自绘**：自绘版上报 Role::CheckBox 之后，
        // UI 自动化发来的是 `LegacyIAccessible.DoDefaultAction`，egui 不认这个动作 ——
        // 结果是"树里有、点不动"。真控件自己处理动作与键盘，代价是长得略朴素，
        // 但一个能用的控件比一张漂亮的图重要。
        let cb_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + CB_X, rect.center().y),
            egui::vec2(16.0, 16.0),
        );
        let mut is_checked = checked;
        let cb_resp = ui
            .put(cb_rect, egui::Checkbox::without_text(&mut is_checked))
            .on_hover_text(format!("勾选 {}", task.name));

        if cb_resp.changed() {
            // 勾复选框**只动批量**：右栏不跳，避免"我想多选几条"把详情也带着乱翻。
            batch::toggle(&mut self.batch, &task.id);
        } else if response.clicked() {
            self.selected = Some(task.id.clone());
            // Ctrl+单击：查看它的同时顺手勾上，键盘用户不必去够小方框。
            if ui.input(|i| i.modifiers.command) {
                batch::toggle(&mut self.batch, &task.id);
            }
        }

        // ---- 右键菜单 ----
        //
        // 右键先把这一行设为"正在查看"：否则菜单里的动作会作用在右栏那个看不见的任务上。
        if response.secondary_clicked() {
            self.selected = Some(task.id.clone());
        }
        let mut menu_action: Option<RowMenuAction> = None;
        let mut menu_opened = false;
        response.context_menu(|ui| {
            menu_opened = true;
            ui.set_min_width(180.0);
            // `row_menu_ui` 带 `#[must_use]`：这里若把它丢掉，门禁（clippy -D warnings）
            // 就会以 `unused_must_use` 失败 —— 0.7.0 的六个死按钮就是这么来的。
            let picked = row_menu_ui(ui, task);
            if picked.is_some() {
                menu_action = picked;
            }
        });
        if menu_opened {
            self.context_menu_for = Some(task.id.clone());
        }
        match menu_action {
            Some(RowMenuAction::Resume) => self.send(crate::gui::Ticket::Resume(task.id.clone())),
            Some(RowMenuAction::Pause) => self.send(crate::gui::Ticket::Pause(task.id.clone())),
            Some(RowMenuAction::OpenFile) => self.reveal_task(task, RevealMode::Open),
            Some(RowMenuAction::OpenFolder) => self.reveal_task(task, RevealMode::Select),
            Some(RowMenuAction::CopyLink) => ui.ctx().copy_text(task.url.clone()),
            Some(RowMenuAction::Remove) => {
                self.send(crate::gui::Ticket::Remove(task.id.clone()));
                if self.selected.as_deref() == Some(task.id.as_str()) {
                    self.selected = None;
                    self.drawer_log = None;
                }
            }
            None => {}
        }
    }

    fn draw_name(&self, ui: &mut egui::Ui, rect: egui::Rect, task: &TaskView) {
        let painter = ui.painter();
        // 主名：去掉分类前缀。
        let name = task.name.rsplit('/').next().unwrap_or(&task.name);
        painter.text(
            rect.left_top() + egui::vec2(0.0, rect.height() / 2.0 - 8.0),
            egui::Align2::LEFT_CENTER,
            elide(name, 46),
            egui::FontId::proportional(12.5),
            theme::ink(),
        );
        // 副标题：host 或落点目录。
        painter.text(
            rect.left_top() + egui::vec2(0.0, rect.height() / 2.0 + 8.0),
            egui::Align2::LEFT_CENTER,
            elide(&host_of(&task.url), 56),
            egui::FontId::proportional(11.0),
            theme::ink_3(),
        );
    }

    /// 表头。返回 `true` 表示点了全选框。
    fn thead(&mut self, ui: &mut egui::Ui, visible: &[String]) -> bool {
        let all_on = batch::all_selected(&self.batch, visible);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::THEAD_H),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        // 表头是内容的"页眉条"，用 surface_3 而不是与列表同色 —— 否则表头与行连成一片。
        painter.rect_filled(rect, 0.0, theme::surface_3());
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::vec2(rect.width(), 1.0),
            ),
            0.0,
            theme::line(),
        );

        let cols = columns(rect);
        let name_rect = cols.name;
        let size_rect = cols.size;
        let progress_rect = cols.progress;
        let status_rect = cols.status;
        self.thead_w = rect.width();

        let font = egui::FontId::proportional(10.5);
        let ink = theme::ink_3();
        painter.text(
            name_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "文件名",
            font.clone(),
            ink,
        );
        painter.text(
            size_rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            "大小",
            font.clone(),
            ink,
        );
        painter.text(
            progress_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "进度",
            font.clone(),
            ink,
        );
        if let Some(speed_rect) = cols.speed {
            painter.text(
                speed_rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                "速度",
                font.clone(),
                ink,
            );
        }
        if let Some(eta_rect) = cols.eta {
            painter.text(
                eta_rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                "剩余",
                font.clone(),
                ink,
            );
        }
        painter.text(
            status_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "状态",
            font,
            ink,
        );

        // 全选框：与行里那个同位置、同尺寸，视觉上连成一条竖线。
        //
        // 这里也必须用真 `Checkbox`（理由同行内那个）：自绘的 CheckBox 角色收到
        // `LegacyIAccessible.DoDefaultAction` 时 egui 不响应，等于点不动。
        let cb_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + CB_X, rect.center().y),
            egui::vec2(16.0, 16.0),
        );
        let mut is_all_on = all_on;
        let hit = ui
            .put(cb_rect, egui::Checkbox::without_text(&mut is_all_on))
            .on_hover_text("全选 / 取消全选（只作用于当前可见项）");
        hit.changed()
    }

    /// 批量动作条：**底部悬浮岛**，只在勾了东西时出现。
    ///
    /// ## 为什么从"列表上方一条"改成"底部悬浮岛"
    ///
    /// 用户 2026-11-17："选择的时候这个工具栏变为底部一个悬浮岛，不然突然出现很突兀。"
    ///
    /// 原来它插在表头之上：勾第一条的瞬间，**整张列表被往下推 38pt**，视线跟着跳。
    /// 悬浮岛浮在列表之上，不参与布局，所以列表位置纹丝不动 —— 这才是"不突兀"的关键，
    /// 比加动画更根本（动画只能让跳动变慢，不能让它不跳）。
    fn batch_island(&mut self, ui: &mut egui::Ui, visible: &[String]) {
        let targets = batch::targets(&self.batch, visible);
        if targets.is_empty() {
            return;
        }
        let mut act: Option<BatchAction> = None;

        // 岛的位置：列表区域底部居中，距底 18pt。
        let area = ui.max_rect();
        let island_w = (ISLAND_W).min(area.width() - 32.0);
        let island_h = ISLAND_H;
        let island_rect = egui::Rect::from_center_size(
            egui::pos2(area.center().x, area.bottom() - 18.0 - island_h / 2.0),
            egui::vec2(island_w, island_h),
        );

        // 浮层：先画底（含 1px 亮边——egui 没有高斯模糊阴影，靠描边分层）
        let painter = ui.painter();
        painter.rect_filled(island_rect, theme::RADIUS, theme::surface_2());
        painter.rect_stroke(
            island_rect,
            theme::RADIUS,
            egui::Stroke::new(theme::STROKE, theme::line()),
            egui::StrokeKind::Inside,
        );

        let mut island_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(island_rect.shrink2(egui::vec2(14.0, 6.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut island_ui;
        ui.label(
            egui::RichText::new(format!("已选 {} 项", targets.len()))
                .size(11.5)
                .color(theme::ink_2()),
        );
        ui.add_space(8.0);
        if ui.button("继续").clicked() {
            act = Some(BatchAction::Resume);
        }
        if ui.button("暂停").clicked() {
            act = Some(BatchAction::Pause);
        }
        if ui.button("删除").clicked() {
            act = Some(BatchAction::Remove);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("取消选择").clicked() {
                act = Some(BatchAction::Clear);
            }
        });

        match act {
            Some(BatchAction::Resume) => {
                for id in &targets {
                    self.send(crate::gui::Ticket::Resume(id.clone()));
                }
            }
            Some(BatchAction::Pause) => {
                for id in &targets {
                    self.send(crate::gui::Ticket::Pause(id.clone()));
                }
            }
            Some(BatchAction::Remove) => {
                // 一条以上先问：批量删除一次丢掉的是 N 条断点进度，误触成本与单条不对等。
                if batch::needs_batch_confirm(targets.len()) {
                    self.confirm_batch_delete = Some(targets.len());
                } else {
                    self.remove_targets(&targets);
                }
            }
            Some(BatchAction::Clear) => self.batch.clear(),
            None => {}
        }
    }

    /// 真去删这批任务。**批量条与确认框共用这一份**：
    /// 两处各写一遍迟早有一处忘了清 `selected` / `drawer_log`。
    /// `pub(super)`：确认框在 `dialog` 模块里。
    pub(super) fn remove_targets(&mut self, ids: &[String]) {
        for id in ids {
            self.send(crate::gui::Ticket::Remove(id.clone()));
        }
        if let Some(sel) = self.selected.clone()
            && ids.contains(&sel)
        {
            self.selected = None;
            self.drawer_log = None;
        }
        // 删掉的东西不能继续留在批量里：留着的话下一次批量动作会对着空气发。
        // 用 `remove` 而不是 `toggle` —— toggle 对不在集合里的 id 会**加进去**。
        for id in ids {
            self.batch.remove(id);
        }
    }

    fn empty_state(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(120.0);
            ui.label(egui::RichText::new("↓").size(44.0).color(theme::ink_3()));
            ui.add_space(9.0);
            ui.label(
                egui::RichText::new("队列是空的")
                    .size(13.0)
                    .strong()
                    .color(theme::ink_2()),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "点左上角「新建任务」，或在浏览器里右键链接交给 dlm。\n\
                     扩展会在页面上留一个悬浮球，随时能投递。",
                )
                .size(12.0)
                .color(theme::ink_3()),
            );
        });
    }

    /// 筛选：状态键与分类键共用一个字段，先按状态再按分类判断；搜索是**与**关系。
    pub(super) fn visible(&self, task: &TaskView) -> bool {
        if !matches_search(task, &self.search) {
            return false;
        }
        match self.filter.as_deref() {
            None => true,
            Some(key) => match key {
                "active" => {
                    task.status == "running" || task.status == "queued" || task.status == "paused"
                }
                "done" => task.status == "done",
                "failed" => task.status == "failed",
                cat => task.category == cat,
            },
        }
    }
}

/// 搜索匹配：名字和 URL 都参与，大小写不敏感；空串（或全空白）全部通过。
pub(crate) fn matches_search(task: &TaskView, needle: &str) -> bool {
    let n = needle.trim();
    if n.is_empty() {
        return true;
    }
    let n = n.to_lowercase();
    task.name.to_lowercase().contains(&n) || task.url.to_lowercase().contains(&n)
}

// ---------------------------------------------------------------- 绘制辅助

fn text_right(ui: &mut egui::Ui, rect: egui::Rect, text: &str, color: egui::Color32, size: f32) {
    ui.painter().text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        text,
        egui::FontId::monospace(size),
        color,
    );
}

fn draw_progress(ui: &mut egui::Ui, rect: egui::Rect, task: &TaskView) {
    let painter = ui.painter();
    let track_h = 4.0;
    let pct_w = 34.0;
    let track_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - track_h / 2.0),
        egui::vec2(rect.width() - pct_w - 8.0, track_h),
    );
    painter.rect_filled(track_rect, 99.0, theme::track());

    // 列表只画**整体进度**：连续下完的就是一条实心条。
    //
    // 只有真的存在"还没下的段"时才画成分段 —— 否则一百多个相邻段会铺成一串碎块，
    // 看着像进度条坏了。分块的细节留给右侧详情（用户 2026-11-27 的决定）。
    let show_segments = !task.segments.is_empty()
        && task
            .total_len
            .is_some_and(|t| super::segment_bar::has_holes(t, &task.segments));
    if !show_segments {
        let pct = (task.progress / 100.0).clamp(0.0, 1.0) as f32;
        if pct > 0.0 {
            let fill = egui::Rect::from_min_size(
                track_rect.min,
                egui::vec2(track_rect.width() * pct, track_h),
            );
            let color = if task.status == "done" {
                theme::ok()
            } else {
                theme::accent()
            };
            painter.rect_filled(fill, 99.0, color);
        }
    } else {
        super::segment_bar::draw(ui, track_rect, task);
    }

    let label = if task.progress > 0.0 {
        format!("{:.0}%", task.progress)
    } else {
        "—".to_string()
    };
    painter.text(
        egui::pos2(rect.right(), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        label,
        egui::FontId::monospace(11.0),
        theme::ink_2(),
    );
}

fn draw_status(ui: &mut egui::Ui, rect: egui::Rect, task: &TaskView) {
    let (label, fg, bg) = theme::status_style(&task.status);
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(11.0), fg);
    let w = galley.size().x + 20.0;
    let h = 19.0;
    let pill = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - h / 2.0),
        egui::vec2(w.min(rect.width()), h),
    );
    painter.rect_filled(pill, 99.0, bg);
    painter.text(
        pill.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        fg,
    );
}

// ---------------------------------------------------------------- 格式化

fn fmt_bytes_opt(n: Option<u64>) -> String {
    match n {
        Some(v) => fmt_bytes(v),
        None => "?".to_string(),
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// 已经花了多久（墙钟时间，**含排队与暂停**）。
///
/// 进行中 = 现在 - 创建时刻；已完成/失败 = 更新时刻 - 创建时刻。
/// 之所以不叫"下载时长"：队列里拿不到"纯下载"的时间，内核也没有回报它 ——
/// 标签写「用时」是在说实话，写成"下载时长"就是在猜。
pub fn elapsed_text(task: &TaskView, now: u64) -> String {
    let end = if task.status == "running" || task.status == "queued" {
        now
    } else {
        task.updated_at
    };
    let secs = end.saturating_sub(task.created_at);
    if secs == 0 {
        return "—".to_string();
    }
    fmt_eta(secs)
}

/// **平均速度**：已下字节 / 用时。与「瞬时速度」是两件事，界面上必须分开写。
///
/// 瞬时速度回答"现在多快"，平均速度回答"整体快不快" ——
/// 抢带宽、被限速、中途暂停过，两者能差一倍。
pub fn avg_speed_text(task: &TaskView, now: u64) -> String {
    let Some(delivered) = task.delivered.filter(|d| *d > 0) else {
        return "—".to_string();
    };
    let end = if task.status == "running" || task.status == "queued" {
        now
    } else {
        task.updated_at
    };
    let secs = end.saturating_sub(task.created_at);
    if secs == 0 {
        return "—".to_string();
    }
    fmt_speed(delivered as f64 / secs as f64)
}

/// 速度：**只看采样器给的速率**，采样器没给出数就显示「—」。
///
/// 三条判据（对应 `rate::Rates` 的测试）：
/// - 没在跑（queued / paused / done / failed）⇒「—」；暂停显示 0 B/s 会像"在跑但没数据"
/// - 第一次采样（只有一个点）⇒「—」：一个点算不出斜率，不许编
/// - 有速率就按真实数值格式化，包括 0（那是"跑着但这一瞬没进展"，是真话）
pub fn speed_text(task: &TaskView, speed: Option<f64>) -> String {
    if task.status != "running" {
        return "—".to_string();
    }
    match speed {
        Some(v) => fmt_speed(v),
        None => "—".to_string(),
    }
}

/// 剩余时间：用 `(总量 - 已交付) / 速率` 算，速率未知或总量未知时老实显示「—」。
pub fn eta_text(task: &TaskView, speed: Option<f64>) -> String {
    match task.status.as_str() {
        "done" => return "—".to_string(),
        "paused" => return "已暂停".to_string(),
        "failed" => return "—".to_string(),
        _ if task.hold_until.is_some() => return "等待".to_string(),
        _ => {}
    }
    let (Some(total), Some(done)) = (task.total_len, task.delivered) else {
        return "—".to_string();
    };
    let Some(rate) = speed.filter(|v| *v > 1.0) else {
        return "—".to_string();
    };
    if total <= done {
        return "—".to_string();
    }
    fmt_eta(((total - done) as f64 / rate).ceil() as u64)
}

/// 字节/秒 → 人读的字符串（`1.2 MiB/s` / `837 KiB/s` / `12 B/s`）。
pub fn fmt_speed(bytes_per_s: f64) -> String {
    if !bytes_per_s.is_finite() || bytes_per_s < 0.0 {
        return "—".to_string();
    }
    format!("{}/s", fmt_bytes(bytes_per_s.round() as u64))
}

/// 秒数 → 人读的剩余时间。一小时以上带小时，一分钟以上带分。
pub fn fmt_eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} 秒"),
        60..=3599 => {
            let (m, s) = (secs / 60, secs % 60);
            if s == 0 {
                format!("{m} 分")
            } else {
                format!("{m} 分 {s} 秒")
            }
        }
        _ => {
            let (h, m) = (secs / 3600, (secs % 3600) / 60);
            if m == 0 {
                format!("{h} 小时")
            } else {
                format!("{h} 小时 {m} 分")
            }
        }
    }
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}

/// 按字符数截断，超出补省略号。中文按 1 个字符算。
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}
#[cfg(test)]
mod tests {
    use super::{RowMenuAction, columns, row_menu_items};
    use crate::gui::snapshot::TaskView;
    use eframe::egui;

    /// **列不许重叠**（0.7.0 详情改成右栏之后踩到的就是这个）。
    ///
    /// 宽窗口下五列齐备、彼此不压；窄窗口下先丢掉「剩余」「速度」，
    /// 文件名仍不少于 `NAME_MIN` —— 而不是让进度条骑到文件名上。
    #[test]
    fn columns_never_overlap_and_drop_the_least_useful_when_narrow() {
        let probe = |w: f32| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, 44.0));
            columns(rect)
        };

        // 宽：五列齐备，且从左到右严格递增、互不重叠。
        let wide = probe(1100.0);
        assert!(
            wide.speed.is_some() && wide.eta.is_some(),
            "1100 宽时应当五列齐备"
        );
        // 列是**从右往左**排出来的，所以按从右到左的顺序检查：每列的左边界
        // 必须不小于前一个（更靠右的）列的右边界。
        let right_to_left = [
            Some(wide.status),
            wide.eta,
            wide.speed,
            Some(wide.progress),
            Some(wide.size),
            Some(wide.name),
        ];
        let mut prev_left = f32::INFINITY;
        for r in right_to_left.into_iter().flatten() {
            assert!(
                r.right() <= prev_left + 0.01,
                "列重叠了：右边界 {} > 前一列左边界 {prev_left}",
                r.right()
            );
            prev_left = r.left();
        }

        // 窄（右栏打开、窗口不大）：丢掉「剩余」，其余各列仍不重叠。
        let narrow = probe(560.0);
        assert!(narrow.eta.is_none(), "560 宽时「剩余」应当先被丢掉");
        assert!(
            narrow.name.width() >= 150.0 - 0.5,
            "文件名列被压到 {} 宽，低于可读下限",
            narrow.name.width()
        );
        assert!(
            narrow.name.right() <= narrow.size.left() + 0.01,
            "文件名与大小列重叠"
        );
        assert!(
            narrow.size.right() <= narrow.progress.left() + 0.01,
            "大小压到进度上"
        );
    }

    /// 右键菜单：六项都在，且每一项都有动作。
    ///
    /// 这条用例的直接来历：0.7.0 那版六个按钮全是 `let _ = ui.button(...)`，
    /// 点了什么都不会发生，而仓库里没有任何用例能发现它。
    #[test]
    fn the_row_menu_offers_six_live_actions() {
        let task = task_with("done");
        let items = row_menu_items(&task);
        assert_eq!(
            items.len(),
            6,
            "继续/暂停/打开文件/打开文件夹/复制链接/删除"
        );

        let mut seen = std::collections::BTreeSet::new();
        for item in &items {
            assert!(!item.label.trim().is_empty(), "菜单项必须有文字");
            // 动作不许重复：两项指同一个动作，说明有一项是摆设。
            assert!(
                seen.insert(format!("{:?}", item.action)),
                "动作重复：{:?}",
                item.action
            );
        }
        for action in [
            RowMenuAction::Resume,
            RowMenuAction::Pause,
            RowMenuAction::OpenFile,
            RowMenuAction::OpenFolder,
            RowMenuAction::CopyLink,
            RowMenuAction::Remove,
        ] {
            assert!(
                items.iter().any(|i| i.action == action),
                "菜单里缺 {action:?}"
            );
        }
        // 破坏性那一项必须有快捷键提示，且和 Del 一致。
        let remove = items
            .iter()
            .find(|i| i.action == RowMenuAction::Remove)
            .unwrap();
        assert_eq!(remove.shortcut, Some("Del"));
        assert!(remove.separator_before, "删除要单独一组，别贴着复制链接");
    }

    /// 可用性跟着任务状态走：不能让人点一个只会换来报错的项。
    #[test]
    fn the_row_menu_only_enables_what_makes_sense_for_the_status() {
        let enabled = |status: &str, action: RowMenuAction| -> bool {
            row_menu_items(&task_with(status))
                .into_iter()
                .find(|i| i.action == action)
                .map(|i| i.enabled)
                .unwrap_or(false)
        };

        // 失败的任务：可以继续，不能暂停，也没文件可打开。
        assert!(enabled("failed", RowMenuAction::Resume));
        assert!(!enabled("failed", RowMenuAction::Pause));
        assert!(!enabled("failed", RowMenuAction::OpenFile));

        // 正在跑：能暂停不能继续。
        assert!(enabled("running", RowMenuAction::Pause));
        assert!(!enabled("running", RowMenuAction::Resume));

        // 下完了：打开文件/文件夹才有意义。
        assert!(enabled("done", RowMenuAction::OpenFile));
        assert!(enabled("done", RowMenuAction::OpenFolder));
        assert!(!enabled("queued", RowMenuAction::OpenFile));

        // 复制链接与删除：任何状态都能用。
        for status in ["running", "queued", "paused", "failed", "done"] {
            assert!(enabled(status, RowMenuAction::CopyLink), "{status}");
            assert!(enabled(status, RowMenuAction::Remove), "{status}");
        }
    }

    fn task_with(status: &str) -> TaskView {
        TaskView {
            id: format!("t-{status}"),
            url: "https://example.com/a.bin".to_string(),
            name: "a.bin".to_string(),
            path: "C:\\dl\\a.bin".to_string(),
            referer: None,
            status: status.to_string(),
            pid: None,
            exit_code: None,
            log: "C:\\dl\\a.log".to_string(),
            progress: 0.0,
            total_len: Some(1),
            delivered: Some(0),
            created_at: 0,
            updated_at: 0,
            start_at: None,
            hold_until: None,
            on_complete: None,
            attempts: 0,
            segments: Vec::new(),
            category: "other",
        }
    }
}
