//! 详情抽屉：点一行任务，右边滑出一块，把「这个任务到底是什么」说清楚。
//!
//! 这块以前只有常量 `theme::DRAWER_W` 和一个被存下来的 `selected`，界面上一片空白 ——
//! 点了任务除了高亮什么也没有。抽屉里每一项都是**已经存在于快照里**的事实，
//! 没有一项是为了好看编出来的：
//!
//! | 显示 | 来源 |
//! |---|---|
//! | 状态 / 进度 / 大小 / 已交付 | `TaskView` |
//! | 速度 / 剩余 | 与列表同一个采样器（`rate::Rates`）|
//! | 重试次数 | `TaskView.attempts` |
//! | 完成后动作 | `TaskView.on_complete` |
//! | 落点 / 日志 / URL | `TaskView.path` / `log` / `url` |
//!
//! 「打开文件 / 打开文件夹」与 `/api/reveal` 同一套动作：**路径只来自队列算出来的落点**，
//! 不接受任何外部输入，否则就成了"打开任意文件"的入口。
//!
//! 为什么要拆出 [`drawer_body`]：面板里那段是纯画法 + 返回"用户点了什么"，
//! 于是可以在测试里真的跑一帧（`egui::Context::run`）而不需要窗口 ——
//! 不然这块 UI 只能靠人眼验，一改就可能画出个 panic。

use eframe::egui;

use super::App;
use crate::actions::RevealMode;
use crate::gui::app::list::{avg_speed_text, elapsed_text, eta_text, fmt_bytes, speed_text};
use crate::gui::app::theme;
use crate::gui::snapshot::TaskView;

// ---------------------------------------------------------------- 动画

/// 右栏滑入时长。0.16s 与 `theme` 里的全局 `animation_time` 一致 ——
/// 面板动画比控件慢的话，整个界面会显得拖泥带水。
pub(crate) const ANIM_SECS: f32 = 0.16;

/// 滑入动画的 Id。关闭时要拿同一个 Id 把它拨回 0，所以抽成常量
/// （两处各写一遍字符串，改一处漏一处就会变成"两次不同的动画"）。
pub(crate) const DRAWER_ANIM_ID: &str = "drawer-open";

/// 右栏不显示时，把滑入动画拨回 0。
///
/// 不这么做的话，`animate_bool` 的状态会在右栏关闭期间冻结；下次打开时
/// elapsed 已远超动画时长，egui 直接补齐到 1.0 —— 表现就是"只有第一次打开有滑入"。
/// 实测见 `the_drawer_slide_animation_progresses_and_restarts_after_a_gap`。
pub(crate) fn reset_drawer_anim(ctx: &egui::Context) {
    let _ = ctx.animate_bool_with_time(egui::Id::unique(DRAWER_ANIM_ID), false, ANIM_SECS);
}

/// 滑入过程中至少留这么宽：宽度取 0 时 `Panel::right` 的边界计算容易抖。
const MIN_SLIDE_W: f32 = 40.0;

/// 抽屉里日志尾巴最多显示几行。
const LOG_TAIL_LINES: usize = 40;

/// 用户在抽屉里点了什么。画法（[`drawer_body`]）只负责收集，**不执行**：
/// 执行要 `&mut App`，而画的时候 `App` 已经被面板借走了。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrawerActions {
    pub open_file: bool,
    pub open_folder: bool,
    pub copy_url: bool,
    pub delete: bool,
    pub close: bool,
}

impl App {
    /// 画详情**悬浮窗**。没有选中、或选中的任务已经不在快照里，就什么都不画。
    ///
    /// ## 为什么是悬浮窗而不是右侧面板
    ///
    /// 第一版做成了 `Panel::right`，实测在 1120pt 宽的窗口里被挤成 ~180pt：
    /// 面板拿到的宽度不够，里面的行就横向溢出、文字飘到面板外面去（截图见交接文档）。
    /// 悬浮窗不吃主布局的宽度，用户可以拖动、缩放、关掉，窄窗口下也不会把列表挤扁 ——
    /// 而且它天然"浮在内容之上"，和"点一行看详情"的心智一致。
    pub(super) fn drawer(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.drawer_task() else {
            return;
        };
        let diag = DrawerDiag {
            speed: self.speed_of(&task.id),
            history: self.rates.history(&task.id).to_vec(),
            long: self.rates.long_history(&task.id).to_vec(),
            stalled_secs: self
                .rates
                .stalled(&task.id, std::time::Instant::now())
                .map(|d| d.as_secs()),
        };
        let log_tail = self.drawer_log_tail(&task);

        // **右栏而不是浮窗**（0.7.0）。浮窗会挡住列表，窄窗口下还会被窗口边缘裁掉 ——
        // 用户截图里那半个被切掉的详情就是它。改成固定宽度的右栏后，主区让位，
        // 列表列宽随之收窄，但没有任何东西被盖住。
        // **右栏滑入**：宽度从 0 动画到 DRAWER_W。
        //
        // 用户 2026-11-17："可以做一些过渡动画，减少一些突兀感。"330pt 的面板凭空出现
        // 是这份界面里最突兀的一处 —— 点一行，半个屏幕突然少掉 330pt。
        //
        // 用 `animate_bool_with_time` 而不是自己写插值：它会在动画期间自动请求重绘，
        // 这正是这类代码最容易写错的地方（不请求重绘就卡在半路）。
        //
        // **`drawer()` 只在选中任务时才被调用**，所以右栏关着的那几帧根本不会求值，
        // `AnimationManager` 里那个按 Id 存的值就停在原地。实测（见用例
        // `the_drawer_slide_animation_progresses_and_restarts_after_a_gap`）：
        // 断一段时间再求值，elapsed 远超动画时长 ⇒ **直接补齐到 1.0**，
        // 于是"第二次打开右栏"没有滑入，只有第一次有。
        //
        // 修法：右栏不显示时显式把它往 0 动画一次（`open_and_reset` 在 `App::ui`
        // 里每帧调），这样下次显示从 0 重新爬。
        let t = ui
            .ctx()
            .animate_bool_with_time(egui::Id::unique(DRAWER_ANIM_ID), true, ANIM_SECS);
        let width = (theme::DRAWER_W * t).max(MIN_SLIDE_W);
        let actions = egui::Panel::right("drawer")
            .exact_size(width)
            .resizable(false)
            // **动画期间不画内容**：330pt 的正文塞进 40pt 会横溢、文字飘到面板外
            // （0.7.0 记过这个坑）。宽度到位之后再画。
            .show(ui, |ui| {
                // 动画早期的帧不画内容（宽度本身在变，画了会横溢），
                // 返回 `None` = 这一帧没有任何动作被触发。
                if t >= 0.98 {
                    Some(drawer_body(ui, &task, &diag, &log_tail))
                } else {
                    None
                }
            })
            .inner;

        let Some(actions) = actions else {
            // 滑入动画还没结束：这一帧不该处理任何动作。
            return;
        };
        if actions.close {
            self.selected = None;
            self.drawer_log = None;
            return;
        }
        if actions.copy_url {
            ui.ctx().copy_text(task.url.clone());
        }
        if actions.open_file {
            self.reveal_task(&task, RevealMode::Open);
        }
        if actions.open_folder {
            self.reveal_task(&task, RevealMode::Select);
        }
        if actions.delete {
            self.send(crate::gui::Ticket::Remove(task.id.clone()));
            self.selected = None;
            self.drawer_log = None;
        }
    }

    /// 当前选中的任务（快照里已经没有就返回 `None`，顺手把选中清掉）。
    fn drawer_task(&mut self) -> Option<TaskView> {
        let id = self.selected.clone()?;
        let found = self.tasks().into_iter().find(|t| t.id == id);
        if found.is_none() {
            self.selected = None;
            self.drawer_log = None;
        }
        found
    }

    /// 抽屉里的日志尾巴。按 `(id, updated_at)` 缓存：任务没进展就不重读文件。
    fn drawer_log_tail(&mut self, task: &TaskView) -> String {
        if let Some((id, stamp, text)) = &self.drawer_log
            && id == &task.id
            && *stamp == task.updated_at
        {
            return text.clone();
        }
        let text = read_log_tail(std::path::Path::new(&task.log), LOG_TAIL_LINES);
        self.drawer_log = Some((task.id.clone(), task.updated_at, text.clone()));
        text
    }

    /// 「打开文件 / 打开文件夹」。与 `/api/reveal` 同一套动作，也同一套抑制开关：
    /// 门禁里 `DLM_NO_SHELL_OPEN` 置上时不真的拉起资源管理器。
    /// `pub(super)`：列表的右键菜单也要用同一套 —— 两处各写一遍必然有一处忘了抑制开关。
    pub(super) fn reveal_task(&self, task: &TaskView, mode: RevealMode) {
        if crate::actions::shell_suppressed() {
            return;
        }
        // 用 `reveal_plan_for`：文件可能还没落盘（排队中），那时 `explorer /select,`
        // 会退到某个默认位置 —— 就是用户说的"打开所在文件夹不精准"。
        let plan = crate::actions::reveal_plan_for(mode, std::path::Path::new(&task.path));
        if let Err(err) = crate::actions::execute(&plan) {
            eprintln!("dlm-gui: 打开落点失败：{err:#}");
        }
    }
}

/// 抽屉里要显示的诊断数据（速度 / 曲线 / 卡了多久）。
///
/// 打成一个结构是因为它们**必须来自同一个采样源**（`rate` 模块）：曲线、速度、卡顿
/// 各算一次的话，界面上会出现"速度 2 MiB/s 且卡住 20s"这种自相矛盾的组合。
pub(crate) struct DrawerDiag {
    pub speed: Option<f64>,
    /// 最近若干个速率点（老的在左）。短窗口，供"刚才有多快"。
    pub history: Vec<f64>,
    /// **长时序列**：每个秒桶一个值，覆盖整个下载。曲线用它画，
    /// 否则长任务的曲线永远只有最近十几秒（用户：尺度要覆盖整个下载）。
    pub long: Vec<f64>,
    /// 已经卡了多少秒（没过阈值为 `None`）。
    pub stalled_secs: Option<u64>,
}

/// 画那条速度曲线：一条折线 + 一圈淡底。
///
/// 点少于两个时 `spark_points` 给空表 —— 那时画一条水平底线（"在跑但还没测出速度"），
/// 而不是留一块空白让人以为界面坏了。
fn painter_spark(ui: &egui::Ui, rect: egui::Rect, history: &[f64], stalled: bool) {
    let painter = ui.painter();
    // 曲线图的底是"凹槽"，用 sunken；surface_2 留给 hover。
    painter.rect_filled(rect, 4.0, theme::sunken());
    let color = if stalled {
        theme::warn()
    } else {
        theme::accent()
    };
    let pts = super::diagnose::spark_points(history, rect.shrink(3.0));
    if pts.len() < 2 {
        painter.line_segment(
            [
                egui::pos2(rect.left() + 3.0, rect.center().y),
                egui::pos2(rect.right() - 3.0, rect.center().y),
            ],
            egui::Stroke::new(1.0, theme::line()),
        );
        return;
    }
    painter.add(egui::Shape::line(pts, egui::Stroke::new(1.4, color)));
}

/// 抽屉的画法。返回用户点了什么，由调用方执行。
pub(crate) fn drawer_body(
    ui: &mut egui::Ui,
    task: &TaskView,
    diag: &DrawerDiag,
    log_tail: &str,
) -> DrawerActions {
    let mut actions = DrawerActions::default();

    ui.painter()
        .rect_filled(ui.max_rect(), 0.0, theme::surface());
    ui.add_space(theme::SPACE_3);

    // ---- 标题 + 关闭 ----
    ui.horizontal(|ui| {
        ui.add_space(theme::SPACE_4);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(elide(&task.name, 34))
                    .size(14.0)
                    .strong()
                    .color(theme::ink()),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(host_of(&task.url))
                    .size(theme::FONT_SECONDARY)
                    .color(theme::ink_3()),
            );
        });
        // **只有一个关闭入口**。0.6.0 是浮窗：标题栏自带一个 ×，这里又画一个，
        // 并排两个 × 都关同一个东西。改成右栏之后没有标题栏了，就由这里提供唯一那个。
        // 用 `×`（U+00D7）而不是 `✕`（U+2715）：后者在 Microsoft YaHei 里没有字形。
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.add_space(theme::SPACE_2);
            if ui
                .add(egui::Button::new(egui::RichText::new("×").size(15.0)).frame(false))
                .on_hover_text("关闭详情")
                .clicked()
            {
                actions.close = true;
            }
        });
    });
    ui.add_space(theme::SPACE_2);
    // 靠留白分层，不画线（只保留下面那条主分隔）
    ui.add_space(theme::SPACE_2);

    // ---- 进度 ----
    ui.horizontal(|ui| {
        ui.add_space(theme::SPACE_4);
        ui.label(
            egui::RichText::new(format!("{:.0}%", task.progress))
                .size(20.0)
                .monospace()
                .color(theme::accent()),
        );
        ui.add_space(theme::SPACE_2);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(status_label(&task.status))
                    .size(theme::FONT_BODY)
                    .color(theme::ink()),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}",
                    fmt_bytes(task.delivered.unwrap_or(0)),
                    task.total_len.map_or("?".to_string(), fmt_bytes)
                ))
                .size(theme::FONT_SECONDARY)
                .monospace()
                .color(theme::ink_3()),
            );
        });
    });

    // ---- 平均速度 / 剩余 / 用时 ----
    //
    // 这里显示**平均速度**（已下字节 / 用时），不是瞬时速度：
    // 瞬时值放到下面的曲线上，两处各回答一个问题（"现在多快" vs "整体快不快"）。
    // 抢带宽 / 被限速 / 中途暂停过时两者能差一倍，混成一个数就是在骗人。
    let now = crate::queue::now_secs();
    ui.add_space(theme::SPACE_2);
    ui.horizontal(|ui| {
        ui.add_space(theme::SPACE_4);
        kv(ui, "平均", &avg_speed_text(task, now));
        ui.add_space(theme::SPACE_4);
        kv(ui, "剩余", &eta_text(task, diag.speed));
        ui.add_space(theme::SPACE_4);
        kv(ui, "用时", &elapsed_text(task, now));
    });

    if task.attempts > 0 {
        ui.add_space(theme::SPACE_2);
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            kv(ui, "重试", &format!("第 {} 次", task.attempts));
        });
    }
    if let Some(action) = &task.on_complete {
        ui.add_space(theme::SPACE_2);
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            kv(ui, "完成后", action);
        });
    }

    // ---- 分段（覆盖集）----
    //
    // 与上面那条进度是**同一份数据的两种看法**：进度答"到哪了"，这格答"怎么走到这的"
    // —— 中间有没有空洞、是不是几段并行在推。数据来自断点文件，
    // 最多每 5 秒刷新一次（`STATE_SAVE_MAX_INTERVAL_SECS`），所以标题里把精度写明，
    // 别让人以为这是实时连接图。
    //
    // 没有覆盖集时整格不出现：画一条空轨道只会让人以为"进度丢了"。
    if !task.segments.is_empty() {
        ui.add_space(theme::SPACE_2);
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            ui.label(
                egui::RichText::new("分段")
                    .size(theme::FONT_LABEL)
                    .color(theme::ink_3()),
            );
            ui.add_space(theme::SPACE_2);
            // 文字必须说"视觉上几块"：断点里存的是调度器的原始行，几百段很常见，
            // 而相邻的那些在下面的条上是**连着的一整块**。只报原始行数就会出现
            // "文字说 353 段、图上像一整条"的自相矛盾（2026-11-27 用户问过这事）。
            let raw = task.segments.len();
            let blocks = crate::queue::merged_count(&task.segments);
            let caption = if blocks == raw {
                format!("断点里 {raw} 个区间 · 每 5 秒刷新")
            } else {
                format!("断点里 {raw} 段，合并后 {blocks} 块 · 每 5 秒刷新")
            };
            ui.label(
                egui::RichText::new(caption)
                    .size(10.0)
                    .color(theme::ink_3()),
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width() - 16.0, 12.0),
                egui::Sense::hover(),
            );
            let track = rect.shrink2(egui::vec2(0.0, 3.0));
            // 缺口地图：默认画满，只标"还没下"的区间（用户：分段「糊成一团」）。
            super::segment_bar::draw_gaps(ui, track, task);
        });
    }

    // ---- 诊断：曲线 + 两行注脚（0.6.0 阶段 4-4）----
    //
    // 只在"在跑"时画：暂停/完成的任务没有曲线也没有卡顿可言，
    // 留一块空白比画一条平线更诚实。
    if task.status == "running" {
        let notes = super::diagnose::notes(task.attempts, diag.stalled_secs);
        ui.add_space(theme::SPACE_2);
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            ui.label(
                egui::RichText::new("速度曲线（整个下载）")
                    .size(theme::FONT_LABEL)
                    .color(theme::ink_3()),
            );
            ui.label(
                egui::RichText::new(speed_text(task, diag.speed))
                    .size(theme::FONT_LABEL)
                    .monospace()
                    .color(theme::ink_2()),
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width() - 16.0, 34.0),
                egui::Sense::hover(),
            );
            // 用长时序列：X 轴从任务开始一直到现在。
            let series = if diag.long.is_empty() {
                &diag.history
            } else {
                &diag.long
            };
            painter_spark(ui, rect, series, diag.stalled_secs.is_some());
        });
        for note in notes {
            ui.horizontal(|ui| {
                ui.add_space(theme::SPACE_4);
                ui.label(egui::RichText::new(note).size(theme::FONT_LABEL).color(
                    if diag.stalled_secs.is_some() {
                        theme::warn()
                    } else {
                        theme::ink_3()
                    },
                ));
            });
        }
    }

    ui.add_space(theme::SPACE_3);
    // 同上
    ui.add_space(theme::SPACE_2);

    // ---- 落点 / URL ----
    for (label, value) in [("落点", task.path.as_str()), ("URL", task.url.as_str())] {
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            ui.label(
                egui::RichText::new(label)
                    .size(theme::FONT_LABEL)
                    .color(theme::ink_3()),
            );
        });
        // B4：不再截断成 `C:\…\x`，改成**横向滚动**看全文（用户要的）。
        // 路径与 URL 都可能很长，省略号会把人最想确认的那一截吃掉。
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_4);
            egui::ScrollArea::horizontal()
                .id_salt(("drawer-scroll", label))
                .max_width(ui.available_width() - 16.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(value)
                                .size(theme::FONT_SECONDARY)
                                .monospace()
                                .color(theme::ink_2()),
                        )
                        .sense(egui::Sense::click())
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        });
        ui.add_space(theme::SPACE_2);
    }

    // ---- 动作 ----
    ui.horizontal_wrapped(|ui| {
        ui.add_space(theme::SPACE_4);
        if ui.button("打开文件").clicked() {
            actions.open_file = true;
        }
        if ui.button("打开文件夹").clicked() {
            actions.open_folder = true;
        }
        if ui.button("复制链接").clicked() {
            actions.copy_url = true;
        }
    });
    ui.add_space(theme::SPACE_2);
    ui.horizontal(|ui| {
        ui.add_space(theme::SPACE_4);
        // 这里没有「重新下载」：队列按名字去重，同名再 Add 一次会被拒。
        // 真正的重下要么先删后加（两步之间有竞态），要么等队列支持"覆盖同名"；
        // 放一个点了就报错的按钮，比不放更糟。
        if ui.button("删除任务").clicked() {
            actions.delete = true;
        }
    });

    // ---- 日志尾巴 ----
    ui.add_space(theme::SPACE_3);
    ui.separator();
    ui.add_space(theme::SPACE_2);
    ui.horizontal(|ui| {
        ui.add_space(theme::SPACE_4);
        ui.label(
            egui::RichText::new("日志")
                .size(theme::FONT_LABEL)
                .color(theme::ink_3()),
        );
    });
    ui.add_space(theme::SPACE_1);
    // **不要包在 `ui.horizontal(...)` 里**：horizontal 的横轴是无限宽，label 于是永远不换行，
    // 一条长报错会把整个窗口撑变形（0.6.0 用户截图里那条 os error 10061 就是这么溢出的）。
    // 用 margin 做缩进，让 label 按抽屉宽度换行；外面再套竖向滚动，长日志在框内滚。
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("drawer-log")
                .max_height(200.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let text = if log_tail.is_empty() {
                        "（尚无日志）"
                    } else {
                        log_tail
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .size(theme::FONT_LABEL)
                                .monospace()
                                .color(theme::ink_2()),
                        )
                        .wrap(),
                    );
                });
        });

    actions
}

/// 读日志文件的最后 `lines` 行。读不到就返回空串（抽屉里显示"还没有日志"）。
pub(crate) fn read_log_tail(path: &std::path::Path, lines: usize) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = raw.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(key)
                .size(theme::FONT_LABEL)
                .color(theme::ink_3()),
        );
        ui.label(
            egui::RichText::new(value)
                .size(11.5)
                .monospace()
                .color(theme::ink()),
        );
    });
}

fn status_label(status: &str) -> &'static str {
    match status {
        "running" => "下载中",
        "queued" => "排队中",
        "paused" => "已暂停",
        "done" => "已完成",
        "failed" => "失败",
        _ => "未知",
    }
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}
