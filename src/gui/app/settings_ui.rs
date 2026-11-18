//! 设置面板（0.6.0 阶段 2 改版）。
//!
//! ## 形状
//!
//! 左侧七个分类、右侧分组卡片、顶部一个搜索框；每行三列：**标题 + 一句解释 + 控件**。
//! 分类与每行的文字全部来自 [`settings_spec`]，面板只负责"把某一项画成控件"。
//!
//! ## 为什么控件是个穷举 `match`
//!
//! [`SettingKey`] 的每个变体都必须在这里有一条渲染分支；加了设置项却没画控件，
//! **编译**就红。判据要求的"每个设置项都有归属分类、无其它兜底"因此不是靠人盯，
//! 而是靠编译器 + `settings_spec` 里的两条机械用例。
//!
//! ## 保存语义没变
//!
//! 仍是草稿模式：改的是 `self.draft`，点保存才落盘并 `ApplyRuntime`。
//! 中途关掉不留半截配置。
//!
//! ## Agent 接入
//!
//! 那一页有一半是"复制命令"：CLI 一行、MCP 配置一段。**没有 dlm-mcp 二进制时后者置灰**
//! （每个用户按需装，不能假装它在那儿）。

use eframe::egui;

use super::App;
use super::components;
use super::mcp_install;
use super::mcp_install::Host as _;
use super::settings_spec::{Category, SettingKey, SettingSpec, search, specs_in};
use crate::gui::RuntimeTuning;
use crate::gui::Ticket;
use crate::gui::app::theme;

/// 连接与速度里"0 = 不限"这类尾注。
const ZERO_MEANS_UNLIMITED: &str = "0 = 不限";

/// 设置窗的期望尺寸。
const SETTINGS_WINDOW_DESIRED: egui::Vec2 = egui::vec2(760.0, 520.0);
/// 与主窗边缘至少留这么多：不留的话圆角与投影会被客户区裁掉，看起来像"溢出一点点"。
const SETTINGS_WINDOW_MARGIN: f32 = 32.0;
/// 再小就没法用（左分类列 132 + 卡片最少宽度）。
const SETTINGS_WINDOW_MIN: egui::Vec2 = egui::vec2(560.0, 360.0);
/// 标题栏 + 搜索框 + 分隔线 + 底部按钮那一行占掉的高度。
const SETTINGS_CHROME_H: f32 = 100.0;
/// 内容区的最小高度：再矮就只剩一条缝，滚动也失去意义。
const SETTINGS_SCROLL_MIN_H: f32 = 120.0;

/// 一行设置里，左侧「标题 + 解释」占多宽。
///
/// **这条必须跟着行宽走，不能写死 300。** 720pt 的窗里，减掉分类列 132、间距 10、
/// 卡片内边距 20 之后只剩约 560；写死 300 之后，右侧再放一个 220 的输入框加按钮
/// 就是约 590 —— 直接把卡片右缘顶破（用户 2026-11-17 报的代理页溢出）。
///
/// 取 46% 是因为标题最长的那句「关闭窗口时收进托盘」在 12.5pt 下约 250pt 宽；
/// 上下限 220/320 防止窗口特别宽时左列吃掉太多。
pub(crate) fn settings_row_text_width(row_width: f32) -> f32 {
    (row_width * 0.46).clamp(220.0, 320.0)
}

/// 设置窗的期望尺寸。
///
/// **为什么不再写死 `fixed_size(760, 520)`**：`theme::WINDOW_MIN` 是 760×480，
/// 而设置窗曾有 760×520 —— 与主窗**同宽、还高 40pt**。主窗一缩到接近下限，
/// 设置窗就顶满客户区（左右各剩约 7pt，圆角被切），竖向直接被裁。
/// 所以尺寸要由视口算出来：先服从视口，再抬到下限；视口比下限还小时以视口为准，
/// 免得"为了不溢出而溢出"。
pub(crate) fn settings_window_size(viewport: egui::Vec2) -> egui::Vec2 {
    let avail = egui::vec2(
        (viewport.x - SETTINGS_WINDOW_MARGIN).max(0.0),
        (viewport.y - SETTINGS_WINDOW_MARGIN).max(0.0),
    );
    let floor = egui::vec2(
        SETTINGS_WINDOW_MIN.x.min(avail.x),
        SETTINGS_WINDOW_MIN.y.min(avail.y),
    );
    egui::vec2(
        SETTINGS_WINDOW_DESIRED.x.min(avail.x).max(floor.x),
        SETTINGS_WINDOW_DESIRED.y.min(avail.y).max(floor.y),
    )
}

/// 内容区（可滚的那块）有多高。
///
/// 以前是硬编码 `max_height(420)`：窗高一固定，420 恰好合适；一旦窗跟着视口缩，
/// 420 就会把底部按钮挤出窗口。改成由窗高反推 —— 760×520 时仍是 420，观感不变。
pub(crate) fn settings_scroll_height(window: egui::Vec2) -> f32 {
    (window.y - SETTINGS_CHROME_H).max(SETTINGS_SCROLL_MIN_H)
}

impl App {
    pub(super) fn settings_panel(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut save = false;
        let mut cancel = false;

        // 尺寸由主窗视口算（见 `settings_window_size`）：主窗缩小时跟着缩，不再顶满客户区。
        let window_size = settings_window_size(ctx.content_rect().size());
        let scroll_height = settings_scroll_height(window_size);

        egui::Window::new("设置")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            // **固定尺寸**：原先只固定了宽度，高度跟着内容走 —— 切分类时窗口一高一矮地跳，
            // 眼睛每次都要重新找"保存"在哪。现在尺寸由视口算出，内容在里头滚。
            .fixed_size(window_size)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // 顶部：搜索设置项（跨分类）。
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_search)
                            .hint_text("搜索设置项…")
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .max_height(scroll_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.settings_search.trim().is_empty() {
                            self.settings_category_body(ui);
                        } else {
                            self.settings_search_body(ui);
                        }
                    });

                ui.add_space(10.0);
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
                        ui.label(egui::RichText::new(err).size(11.5).color(theme::bad()));
                    } else {
                        ui.label(
                            egui::RichText::new("改动保存后立刻生效（除标注『重启后生效』的项）")
                                .size(10.5)
                                .color(theme::ink_3()),
                        );
                    }
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if save && self.try_save_settings() {
            open = false;
        }
        if cancel {
            self.reset_settings_draft();
            open = false;
        }

        self.settings_open = open;
    }

    /// 存盘 + 让改动立刻生效。返回是否成功（失败时错误留在 `settings_error`，面板不关）。
    ///
    /// 抽成方法是因为"保存"现在有两个入口（面板按钮、Agent 页复制前的自检），
    /// 两处各写一遍必然会有一处漏掉 `ApplyRuntime`。
    fn try_save_settings(&mut self) -> bool {
        let parsed_dir = crate::gui::settings::parse_out_dir_input(&self.out_dir_text);
        if !self.draft.on_complete_ok() {
            self.settings_error = Some("完成后动作只认 空 / open / run:<exe>".to_string());
            return false;
        }
        let dir = match parsed_dir {
            Ok(dir) => dir,
            Err(msg) => {
                self.settings_error = Some(format!("保存目录：{msg}"));
                return false;
            }
        };
        self.draft.out_dir = dir;
        match self.draft.save(&self.settings_dir) {
            Ok(()) => {
                self.settings = self.draft.clone();
                // 剪贴板开关这类"要动线程"的项。
                self.apply_settings_runtime();
                // E18：连接数 / 限速 / 每 host 上限也立刻生效。
                // 不发这一条的话，用户改完得重启程序 —— 设置界面等于在说谎。
                self.send(Ticket::ApplyRuntime(RuntimeTuning::from_settings_with(
                    &self.settings,
                    &self.default_out_dir,
                    self.default_category_dirs,
                )));
                self.settings_error = None;
                true
            }
            Err(err) => {
                self.settings_error = Some(format!("保存失败：{err:#}"));
                false
            }
        }
    }

    fn reset_settings_draft(&mut self) {
        self.draft = self.settings.clone();
        self.out_dir_text = self
            .draft
            .out_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        self.settings_error = None;
    }

    /// 左侧分类 + 右侧该分类的分组卡片。
    fn settings_category_body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            // 左：分类列。
            ui.vertical(|ui| {
                ui.set_width(132.0);
                for category in Category::ALL {
                    let selected = self.settings_category == *category;
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(category.title()).size(12.0).color(
                                    if selected {
                                        theme::accent_ink()
                                    } else {
                                        theme::ink_2()
                                    },
                                ),
                            )
                            .fill(if selected {
                                theme::accent_soft()
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .min_size(egui::vec2(124.0, 26.0)),
                        )
                        .clicked()
                    {
                        self.settings_category = *category;
                    }
                }
            });

            ui.add_space(10.0);
            // 右：这一类的分组卡片。
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                let category = self.settings_category;
                if category == Category::Agent {
                    self.agent_panel(ui);
                } else if category == Category::Component {
                    // 版本一览就在这一页里头：出问题第一个要点进来的地方，
                    // 而排查第一句要问的就是这四个数。
                    self.component_panel(ui);
                }
                self.render_specs(ui, specs_in(category).collect());
            });
        });
    }

    /// 搜索态：跨分类列出命中的项，每组前标出分类名，点标题能跳过去。
    fn settings_search_body(&mut self, ui: &mut egui::Ui) {
        let hits = search(&self.settings_search);
        if hits.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("没有匹配的设置项")
                    .size(11.5)
                    .color(theme::ink_3()),
            );
            return;
        }
        // 跨分类时按分类分段，保留"这条设置住在哪儿"的信息。
        let mut last: Option<Category> = None;
        for spec in hits {
            if last != Some(spec.category) {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(spec.category.title())
                        .size(10.5)
                        .strong()
                        .color(theme::ink_3()),
                );
                last = Some(spec.category);
            }
            self.render_row(ui, spec);
        }
    }

    /// 把一批设置项按分组画成卡片。
    fn render_specs(&mut self, ui: &mut egui::Ui, specs: Vec<&'static SettingSpec>) {
        let mut group: Option<&'static str> = None;
        for spec in specs {
            if group != Some(spec.group) {
                if group.is_some() {
                    ui.add_space(8.0);
                }
                group = Some(spec.group);
                ui.label(
                    egui::RichText::new(spec.group)
                        .size(11.0)
                        .strong()
                        .color(theme::ink_3()),
                );
                ui.add_space(2.0);
            }
            self.render_row(ui, spec);
        }
    }

    /// 一行设置：标题 + 一句解释（左）｜控件（右）。
    ///
    /// 规则那一项例外：它的编辑器要整行宽（见 [`Self::rules_editor`]）。
    fn render_row(&mut self, ui: &mut egui::Ui, spec: &'static SettingSpec) {
        egui::Frame::new()
            .fill(theme::surface())
            .stroke(egui::Stroke::new(theme::STROKE, theme::line_soft()))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                // **左列宽度由可用宽度算，不写死 300。**
                //
                // 720pt 的窗里，减掉分类列 132、间距 10、卡片内边距 20 之后只剩约 560；
                // 左列占 300 时，右侧再放一个 220 的输入框加「测试」按钮就是约 590 ——
                // 直接把卡片右缘顶破。用户 2026-11-17 报的代理页溢出就是这个。
                let text_w = settings_row_text_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(text_w);
                        ui.label(
                            egui::RichText::new(spec.title)
                                .size(12.5)
                                .color(theme::ink()),
                        );
                        ui.label(
                            egui::RichText::new(spec.hint)
                                .size(10.5)
                                .color(theme::ink_3()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if spec.read_only {
                            ui.label(
                                egui::RichText::new("程序自己记的")
                                    .size(10.5)
                                    .color(theme::ink_3()),
                            );
                        } else {
                            self.setting_control(ui, spec.key);
                        }
                    });
                });
                if spec.key == SettingKey::Rules {
                    ui.add_space(6.0);
                    self.rules_editor(ui);
                }
                if spec.key == SettingKey::NightBatch {
                    ui.add_space(6.0);
                    self.batch_editor(ui);
                }
            });
        ui.add_space(4.0);
    }

    /// 规则编辑器：一行一条，**顺序就是优先级**，所以给「上移 / 下移」而不是编号。
    ///
    /// 每条三个字段：URL 通配、落点目录（留空 = 用默认）、连接数（勾上才覆盖）。
    fn rules_editor(&mut self, ui: &mut egui::Ui) {
        let mut remove: Option<usize> = None;
        let mut move_it: Option<(usize, bool)> = None;

        for (i, rule) in self.draft.rules.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut rule.pattern)
                        .hint_text("*/releases/*")
                        .desired_width(170.0),
                );
                ui.add(
                    egui::TextEdit::singleline(rule_dir_mut(rule))
                        .hint_text("留空 = 默认目录")
                        .desired_width(150.0),
                );

                // 连接数是 `Option<usize>`：勾上才覆盖全局设置。用 `0` 当"没设"会在
                // 界面与语义上都跟"真的要 0 条"混起来（虽然 0 条不合法）。
                let mut override_conn = rule.connections.is_some();
                if ui.checkbox(&mut override_conn, "连接数").changed() {
                    rule.connections = override_conn.then_some(4);
                }
                if let Some(conn) = rule.connections.as_mut() {
                    ui.add(egui::DragValue::new(conn).range(1..=32));
                }

                if ui.small_button("上移").clicked() {
                    move_it = Some((i, true));
                }
                if ui.small_button("下移").clicked() {
                    move_it = Some((i, false));
                }
                if ui.small_button("删除").clicked() {
                    remove = Some(i);
                }

                if let Err(err) = crate::rules::validate_rule(rule) {
                    ui.label(
                        egui::RichText::new(err)
                            .size(10.5)
                            .color(egui::Color32::from_rgb(0xc6, 0x28, 0x28)),
                    );
                }
            });
        }

        ui.horizontal(|ui| {
            if ui.small_button("+ 一条规则").clicked() {
                self.draft.rules.push(crate::rules::Rule {
                    pattern: String::new(),
                    dir: None,
                    connections: None,
                });
            }
            ui.label(
                egui::RichText::new("按顺序先命中先用；留空的字段不改那项")
                    .size(10.5)
                    .color(theme::ink_3()),
            );
        });

        if let Some(i) = remove {
            self.draft.rules.remove(i);
        }
        if let Some((i, up)) = move_it {
            crate::rules::move_rule(&mut self.draft.rules, i, up);
        }
    }

    /// 一个设置项画成什么控件。**穷举 `match`**：加了键不画就编译不过。
    fn setting_control(&mut self, ui: &mut egui::Ui, key: SettingKey) {
        match key {
            SettingKey::OutDir => {
                ui.add(
                    egui::TextEdit::singleline(&mut self.out_dir_text)
                        .hint_text("留空 = 下载文件夹")
                        .desired_width(300.0),
                );
            }
            SettingKey::CategoryDirs => {
                ui.checkbox(&mut self.draft.category_dirs, "");
            }
            SettingKey::RenameTemplate => {
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.rename_template)
                        .hint_text("留空 = 原名")
                        .desired_width(260.0),
                );
            }
            SettingKey::OnDuplicate => {
                egui::ComboBox::from_id_salt("on-dup")
                    .selected_text(match self.draft.on_duplicate.as_str() {
                        "skip" => "跳过",
                        "overwrite" => "覆盖",
                        _ => "自动加编号",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.draft.on_duplicate,
                            "auto".into(),
                            "自动加编号",
                        );
                        ui.selectable_value(&mut self.draft.on_duplicate, "skip".into(), "跳过");
                        ui.selectable_value(
                            &mut self.draft.on_duplicate,
                            "overwrite".into(),
                            "覆盖",
                        );
                    });
            }
            SettingKey::OnComplete => {
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.on_complete)
                        .hint_text("open / run:程序.exe")
                        .desired_width(220.0),
                );
            }
            SettingKey::ScanAfterDone => {
                ui.checkbox(&mut self.draft.scan_after_done, "");
            }
            SettingKey::Connections => {
                ui.add(egui::DragValue::new(&mut self.draft.connections).range(1..=32));
            }
            SettingKey::PerHostConnections => {
                ui.add(egui::DragValue::new(&mut self.draft.per_host_connections).range(0..=32));
                ui.label(
                    egui::RichText::new(ZERO_MEANS_UNLIMITED)
                        .size(10.5)
                        .color(theme::ink_3()),
                );
            }
            SettingKey::MaxSpeedKiB => {
                ui.add(egui::DragValue::new(&mut self.draft.max_speed_kib).range(0..=1024 * 100));
                ui.label(
                    egui::RichText::new(ZERO_MEANS_UNLIMITED)
                        .size(10.5)
                        .color(theme::ink_3()),
                );
            }
            SettingKey::SpeedWindows => {
                // 一行一条，行内可删；"此刻"直接算给用户看（否则得盯着表猜）。
                let mut remove = None;
                ui.vertical(|ui| {
                    for (i, w) in self.draft.speed_windows.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(w)
                                    .hint_text("22:00-06:00=512")
                                    .desired_width(170.0),
                            );
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if ui.small_button("+ 一条").clicked() {
                        self.draft.speed_windows.push(String::new());
                    }
                    let parsed: Vec<_> = self
                        .draft
                        .speed_windows
                        .iter()
                        .filter_map(|w| crate::gui::speed_schedule::SpeedWindow::parse(w).ok())
                        .collect();
                    let eff = crate::gui::speed_schedule::rate_at(
                        crate::gui::speed_schedule::local_minute_of_day(),
                        &parsed,
                        self.draft.max_speed_kib as u32,
                    );
                    ui.label(
                        egui::RichText::new(if eff == 0 {
                            "此刻：不限速".to_string()
                        } else {
                            format!("此刻：{eff} KiB/s")
                        })
                        .size(10.5)
                        .color(theme::ink_3()),
                    );
                });
                if let Some(i) = remove {
                    self.draft.speed_windows.remove(i);
                }
            }
            SettingKey::NightBatch => {
                // 详细三行在 `render_row` 的整行宽区块里（见 `batch_editor`）；
                // 这里只画那一个总开关。
                ui.checkbox(&mut self.draft.night_batch.on, "");
            }
            SettingKey::Proxy => {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.draft.proxy)
                                .hint_text("http://127.0.0.1:7890")
                                // 扣掉「测试」按钮与间距，剩下的都给输入框。
                                // 写死 220 时，窄窗口下这一行会把卡片右缘顶破。
                                .desired_width((ui.available_width() - 72.0).clamp(120.0, 220.0)),
                        );
                        if ui.small_button("测试").clicked() {
                            self.proxy_test = Some(
                                crate::proxy::probe_via(
                                    &self.draft.proxy,
                                    "https://www.gstatic.com/generate_204",
                                    std::time::Duration::from_secs(5),
                                )
                                .unwrap_or_else(|e| e),
                            );
                        }
                    });
                    if let Some(msg) = &self.proxy_test {
                        ui.label(
                            egui::RichText::new(msg.clone())
                                .size(10.5)
                                .color(theme::ink_3()),
                        );
                    }
                    // 写错当场标红，但不阻止保存：一行代理写错就存不下整份设置更糟。
                    match crate::proxy::parse_proxy(&self.draft.proxy) {
                        Ok(None) => {}
                        Ok(Some(spec)) => {
                            ui.label(
                                egui::RichText::new(format!("生效：{}", spec.redacted()))
                                    .size(10.5)
                                    .color(theme::ink_3()),
                            );
                        }
                        Err(err) => {
                            ui.label(
                                egui::RichText::new(err)
                                    .size(10.5)
                                    .color(egui::Color32::from_rgb(0xc6, 0x28, 0x28)),
                            );
                        }
                    }
                });
            }
            SettingKey::AgentConfirm => {
                ui.checkbox(&mut self.draft.agent_confirm, "");
            }
            SettingKey::Autostart => {
                ui.checkbox(&mut self.draft.autostart, "");
            }
            SettingKey::CloseToTray => {
                ui.checkbox(&mut self.draft.close_to_tray, "");
            }
            SettingKey::Theme => {
                let current = match self.draft.theme.as_str() {
                    "light" => "浅色",
                    "dark" => "深色",
                    _ => "跟随系统",
                };
                egui::ComboBox::from_id_salt("theme-combo")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (value, label) in
                            [("system", "跟随系统"), ("light", "浅色"), ("dark", "深色")]
                        {
                            ui.selectable_value(&mut self.draft.theme, value.to_string(), label);
                        }
                    });
                // 立刻生效：主题是"看得见"的开关，等保存会让人以为坏了。
                if self.draft.theme != self.applied_theme {
                    self.applied_theme = self.draft.theme.clone();
                    super::apply_theme_preference(&self.draft.theme);
                    // 原生控件的配色也要跟着换，不然输入框/下拉仍是旧的一套。
                    super::theme::apply(ui.ctx());
                }
            }
            SettingKey::SidebarCollapsed => {
                // 勾上之后**立刻**反映到界面：这类"看得见"的开关如果还要点保存才生效，
                // 用户会以为没生效而反复点。
                if ui.checkbox(&mut self.draft.sidebar_collapsed, "").changed() {
                    self.sidebar_collapsed = self.draft.sidebar_collapsed;
                }
            }
            SettingKey::TrayHint => {
                ui.checkbox(&mut self.draft.tray_hint, "");
            }
            SettingKey::TrayHintShown => {
                // 只读项：`render_row` 已经拦掉了，这里不需要画控件。
            }
            SettingKey::WatchClipboard => {
                ui.checkbox(&mut self.draft.watch_clipboard, "");
            }
            SettingKey::Rules => {
                // 规则编辑器是**整行宽**的（见 `render_row` 里的特判）：一条规则要同时编辑
                // 通配、目录、连接数，塞进右边那 300pt 的控件区会挤成一团。
                ui.label(
                    egui::RichText::new(format!("{} 条", self.draft.rules.len()))
                        .size(11.0)
                        .monospace()
                        .color(theme::ink_2()),
                );
            }
            SettingKey::AutoCheckUpdate => {
                ui.checkbox(&mut self.draft.auto_check_update, "");
            }
            SettingKey::GrabTemplates => {
                // 模板在抓取窗口里「存为模板」时写入，这里只报数（不在设置页编辑：
                // 模板脱离站点域名就没意义，而设置页没有"当前起始页"这个上下文）。
                ui.label(
                    egui::RichText::new(format!("{} 个站点", self.draft.grab_templates.len()))
                        .size(11.0)
                        .monospace()
                        .color(theme::ink_2()),
                );
            }
        }
    }

    /// 「组件」页：桌面端（含内核）/ MCP 桥接 / 浏览器扩展 / 更新通道。
    ///
    /// 这一页回答的是**"我装的到底是哪几个版本、哪个该动一下"** —— 以前这四个数散在
    /// 高级页顶部、浏览器扩展页、Agent 页与状态栏四处，问一遍要翻四个地方。
    fn component_panel(&mut self, ui: &mut egui::Ui) {
        // 装完的回执：后台线程跑完把结果塞回来，这里取。
        self.drain_mcp_install(ui.ctx());

        let rows = self.component_rows();
        let mut action: Option<ComponentAction> = None;

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("组件")
                    .size(12.5)
                    .strong()
                    .color(theme::ink()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("复制版本信息").clicked() {
                    action = Some(ComponentAction::CopyVersions);
                }
            });
        });
        ui.label(
            egui::RichText::new("四个组件各自升级；这一页只看它们的版本与状态")
                .size(10.5)
                .color(theme::ink_3()),
        );
        ui.add_space(6.0);

        for row in &rows {
            component_card(ui, row, &mut action);
            ui.add_space(6.0);
        }

        if let Some(note) = &self.mcp_install_note {
            ui.label(egui::RichText::new(note).size(10.5).color(theme::ink_2()));
        }
        if let Some(what) = &self.agent_copied {
            ui.label(
                egui::RichText::new(format!("已复制（{what}）"))
                    .size(10.5)
                    .color(theme::ok()),
            );
        }

        // 动作统一出闭包再执行：画的时候 `rows` 还借着 `self` 的只读借用。
        match action {
            Some(ComponentAction::InstallMcp) => self.start_mcp_install(ui.ctx()),
            Some(ComponentAction::RecheckMcp) => {
                self.mcp_version_cache = None;
                self.mcp_install_note = Some("重新探测 dlm-mcp…".to_string());
            }
            Some(ComponentAction::OpenUpdatePanel) => self.update_open = true,
            Some(ComponentAction::CheckUpdate) => self.check_update_now(ui.ctx()),
            Some(ComponentAction::OpenReleases) => {
                super::update_ui::open_external(&mcp_install::releases_page_url());
            }
            Some(ComponentAction::CopyExtPath) => {
                ui.ctx()
                    .copy_text(crate::gui::default_extension_dir().display().to_string());
                self.agent_copied = Some("扩展路径".to_string());
            }
            Some(ComponentAction::OpenExtDir) => {
                crate::gui::open_dir(&crate::gui::default_extension_dir());
            }
            Some(ComponentAction::CopyExtPage) => {
                // 浏览器不许程序化打开 `chrome://extensions`，只能复制过去让用户粘。
                ui.ctx().copy_text("chrome://extensions".to_string());
                self.agent_copied = Some("chrome://extensions".to_string());
            }
            Some(ComponentAction::CopyVersions) => {
                ui.ctx().copy_text(components::versions_line(&rows));
                self.agent_copied = Some("版本信息".to_string());
            }
            None => {}
        }
    }

    /// 组件页要的四行状态。事实从快照与本机探测来，判定交给 [`components::build_rows`]。
    ///
    /// `pub(super)`：状态栏悬停摘要也要用同一份判定 —— 两处各算一次，口径迟早会分叉。
    pub(super) fn component_rows(&mut self) -> Vec<components::ComponentRow> {
        let view = self.update_view();
        let exe_dir = current_exe_dir();
        let mcp_present = exe_dir
            .as_ref()
            .map(|dir| dir.join(mcp_install::MCP_BINARY).is_file())
            .unwrap_or(false);

        // 探测版本要起一个子进程：只在第一次进这一页（或点「重新探测」）时做，
        // 每帧跑一次会把界面拖成幻灯片。
        if !mcp_present {
            self.mcp_version_cache = None;
        } else if self.mcp_version_cache.is_none() {
            let version = exe_dir
                .as_ref()
                .map(|dir| dir.join(mcp_install::MCP_BINARY))
                .and_then(|exe| mcp_install::RealHost.probe(&exe));
            self.mcp_version_cache = Some(version);
        }
        let mcp_version = self.mcp_version_cache.clone().flatten();

        // 扩展的三个版本数各有来源，别混：
        //   磁盘上那份 = **本地读目录**（`%LOCALAPPDATA%\dlm\extension` 的 manifest）
        //   本程序内置 = 安装目录里的 `browser/extension`
        //   浏览器中运行 = 扩展自己上报（走快照）
        //
        // 磁盘那份**必须本地读**：清单检查失败（断网、限流）不该让"装了"变成"没检测到"。
        // 实跑时踩到过：更新检查一失败，扩展卡就从「磁盘上有」跳成「未检测到已装入浏览器」。
        let ext_dir = crate::gui::default_extension_dir();
        let disk = disk_extension(
            view.extension_disk.clone(),
            crate::updater::extension_version(&ext_dir),
        );
        let bundled = exe_dir.as_ref().and_then(|dir| {
            crate::updater::extension_version(&dir.join("browser").join("extension"))
        });
        let input = components::ComponentInput {
            app_name: crate::gui::APP_NAME,
            release: crate::gui::RELEASE,
            core_current: &view.core_current,
            extension_disk: disk.as_deref(),
            extension_running: view.extension_running.as_deref(),
            extension_packaged: bundled.as_deref(),
            mcp_version: mcp_version.as_deref(),
            mcp_present,
            channel_url: self.update_url.as_deref(),
            latest: view.latest.as_deref(),
            checked_at: view.checked_at,
            update_error: view.error.as_deref(),
        };
        components::build_rows(&input)
    }

    /// 起后台线程装 dlm-mcp：下载是网络活，不能压在 UI 线程上。
    fn start_mcp_install(&mut self, ctx: &egui::Context) {
        if self.mcp_install_rx.is_some() {
            return;
        }
        let Some(dest) = current_exe_dir() else {
            self.mcp_install_note = Some("拿不到本程序所在目录，无法安装".to_string());
            return;
        };
        let release = crate::gui::RELEASE.to_string();
        let work_root = std::env::temp_dir().join("dlm-mcp-install");
        let suppressed = mcp_install::download_suppressed();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("dlm-mcp-install".into())
            .spawn(move || {
                let out = mcp_install::install_release(
                    &release,
                    &work_root,
                    &dest,
                    suppressed,
                    &mcp_install::RealHost,
                );
                let _ = tx.send(out);
            })
            .ok();
        self.mcp_install_rx = Some(rx);
        self.mcp_install_note = Some("正在下载 dlm-mcp…".to_string());
        ctx.request_repaint();
    }

    /// 收安装回执。成功后清掉版本缓存 —— 下一次画这一页会重新探测。
    fn drain_mcp_install(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.mcp_install_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(out) => {
                self.mcp_install_note = Some(out.message());
                if out.installed() {
                    self.mcp_version_cache = None;
                }
                self.mcp_install_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.mcp_install_rx = None;
                self.mcp_install_note = Some("安装线程异常退出".to_string());
            }
        }
    }

    /// 「Agent 接入」页：两段可复制的配置 + 说明。
    ///
    /// 判据是"CLI 命令现在就能用、MCP 配置没装就置灰"。置灰不是偷懒：
    /// 把一段指向不存在程序的配置交给用户，比不提供更糟。
    ///
    /// ## 2026-11-18 重排（用户报了"右边溢出 + 版式乱"）
    ///
    /// 两个现象，一个根因：**`ui.horizontal` 给子元素的可用宽度是无限的，
    /// 里面的 `label` 因此永不换行**。原来把「去『组件』页安装」按钮和一句
    /// 二十多字的说明塞进同一个 horizontal，必然顶出卡片右缘。
    ///
    /// 所以这一页现在只守两条规则：
    /// 1. **长文案一律独占一行**（纵向上下文里 `label` 会按可用宽度折行），
    ///    横向行里只放短标题 + 按钮 —— 那两样再长也长不过一行。
    /// 2. 用与其它设置页**相同的卡片外观**（`surface` 底 + 细描边 + 内边距），
    ///    不再是一排裸控件飘在页面上。
    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        // 复制动作先记在局部变量里，出了闭包再写回 `self`
        // （闭包里同时借 `self` 和 `ui` 过不了借用检查）。
        let mut copied: Option<&'static str> = None;
        let mut go_component = false;

        // 「已复制」显示在各自按钮上，所以先取一份快照给内容函数用。
        let cli_copied = self.agent_copied.as_deref() == Some("命令行");
        let mcp_copied = self.agent_copied.as_deref() == Some("MCP");

        ui.label(
            egui::RichText::new("Agent 接入")
                .size(12.5)
                .strong()
                .color(theme::ink()),
        );
        ui.label(
            egui::RichText::new(
                "命令行一直可用；MCP 要另外装 dlm-mcp —— 它会自己去找 dlm-gui 写的端点文件，\
                 不必把 token 抄进配置。",
            )
            .size(10.5)
            .color(theme::ink_3()),
        );
        ui.add_space(10.0);

        let available = mcp_available(&std::env::current_exe().ok(), |p| p.exists());
        agent_body(
            ui,
            available,
            cli_copied,
            mcp_copied,
            &mut copied,
            &mut go_component,
        );

        // 出了闭包再动状态。
        if let Some(what) = copied {
            self.agent_copied = Some(what.to_string());
        }
        if go_component {
            // 安装流程只在组件页有一份：两处各写一遍必然有一处落后。
            self.settings_category = Category::Component;
        }
        ui.add_space(8.0);
    }

    /// 夜间批次的三行细节：窗口、限速、跑完关机。
    ///
    /// 总开关关着时**置灰但可见**：让人看得见"这套东西存在、打开后会发生什么"，
    /// 而不是打开开关才突然冒出三行。
    fn batch_editor(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.draft.night_batch;
        let on = b.on;
        ui.add_enabled_ui(on, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("窗口").size(11.0).color(theme::ink_3()));
                ui.add(egui::DragValue::new(&mut b.start_hour).range(0..=23));
                ui.label("点 到");
                ui.add(egui::DragValue::new(&mut b.end_hour).range(0..=23));
                ui.label("点");
                if b.start_hour > b.end_hour {
                    ui.label(
                        egui::RichText::new("跨午夜")
                            .size(10.0)
                            .color(theme::ink_3()),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("限速").size(11.0).color(theme::ink_3()));
                ui.add(egui::DragValue::new(&mut b.speed_kib).range(0..=1024 * 100));
                ui.label(
                    egui::RichText::new("KiB/s（0 = 不限速）")
                        .size(10.5)
                        .color(theme::ink_3()),
                );
            });
            ui.checkbox(&mut b.shutdown_when_done, "队列跑空后关机");
        });
        ui.label(
            egui::RichText::new(if on {
                self.draft.night_batch.summary()
            } else {
                "关着的时候白天也照常派发".to_string()
            })
            .size(10.5)
            .color(theme::ink_3()),
        );
    }
}

/// 规则那一行的目录输入框：`Option<String>` 里取出一个可编辑的 `&mut String`。
///
/// 清空之后留下的是 `Some("")`，`rules::dir_path_for` 把空白当"没写"（回默认目录），
/// 所以这里不需要在失焦时把 `Some("")` 收拾回 `None`。
fn rule_dir_mut(rule: &mut crate::rules::Rule) -> &mut String {
    rule.dir.get_or_insert_with(String::new)
}

/// 组件卡上按下的动作。收集起来出闭包再执行 —— 画卡的时候还借着 `rows` 的只读借用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentAction {
    InstallMcp,
    RecheckMcp,
    OpenUpdatePanel,
    CheckUpdate,
    OpenReleases,
    CopyExtPath,
    OpenExtDir,
    CopyExtPage,
    CopyVersions,
}

/// 「磁盘上的扩展版本」取谁：**本地读到的优先**，清单里那份只是补充。
///
/// 本地那份才是"磁盘上到底有没有"的事实来源；清单那份是网络回来的，断网就没了。
pub(crate) fn disk_extension(from_check: Option<String>, local: Option<String>) -> Option<String> {
    local.or(from_check)
}

/// 本程序所在目录：`dlm-mcp.exe` 就装在这儿。
fn current_exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.to_path_buf())
}

/// 这张卡上该有哪些按钮。`right_to_left` 布局下**第一个画在最右**，所以主按钮排前面。
fn component_actions(row: &components::ComponentRow) -> Vec<(&'static str, ComponentAction)> {
    match row.id {
        components::ComponentId::Desktop => vec![("更新…", ComponentAction::OpenUpdatePanel)],
        components::ComponentId::Mcp => match &row.state {
            components::ComponentState::Missing => vec![
                ("下载并安装", ComponentAction::InstallMcp),
                ("打开发布页", ComponentAction::OpenReleases),
            ],
            components::ComponentState::Unknown(_) => vec![
                ("重新探测", ComponentAction::RecheckMcp),
                ("打开发布页", ComponentAction::OpenReleases),
            ],
            _ => vec![("重新探测", ComponentAction::RecheckMcp)],
        },
        components::ComponentId::Extension => vec![
            ("复制 chrome://extensions", ComponentAction::CopyExtPage),
            ("打开目录", ComponentAction::OpenExtDir),
            ("复制路径", ComponentAction::CopyExtPath),
        ],
        components::ComponentId::Channel => vec![
            ("立即检查", ComponentAction::CheckUpdate),
            ("打开发布页", ComponentAction::OpenReleases),
        ],
    }
}

/// 一张组件卡：左「标题 + 状态 + 版本」，右动作按钮，底下一行说明。
fn component_card(
    ui: &mut egui::Ui,
    row: &components::ComponentRow,
    action: &mut Option<ComponentAction>,
) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(theme::STROKE, theme::line_soft()))
        .inner_margin(egui::Margin::symmetric(11, 9))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(232.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(row.id.title())
                                .size(12.5)
                                .color(theme::ink()),
                        );
                        ui.label(
                            egui::RichText::new(row.state.short())
                                .size(10.5)
                                .color(row.state.color()),
                        );
                    });
                    ui.label(
                        egui::RichText::new(&row.version)
                            .size(11.5)
                            .monospace()
                            .color(theme::ink_2()),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    for (label, act) in component_actions(row) {
                        if ui.button(label).clicked() {
                            *action = Some(act);
                        }
                    }
                });
            });
            // 需处理的那一句必须显眼：它是"该干什么"的唯一出处。
            if let components::ComponentState::NeedsAction(what) = &row.state {
                ui.label(egui::RichText::new(what).size(10.5).color(theme::warn()));
            }
            ui.label(
                egui::RichText::new(&row.hint)
                    .size(10.5)
                    .color(theme::ink_3()),
            );
        });
}

// ---------------------------------------------------------------------------
// Agent 页的排版积木
//
// 这一页 2026-11-18 重排过一次，原因见 `App::agent_panel` 的文档：
// **`ui.horizontal` 里的 `label` 不换行**，长文案并排就必然溢出。
// 下面三个函数就是"安全用法"的唯一出口 —— 想让长文案出现在这一页，必须走它们。
// ---------------------------------------------------------------------------

/// Agent 页的**卡片内容**（不含页头标题）。
///
/// 抽成自由函数是为了能被用例直接量：把它放进一个已知宽度的 `Ui` 里排一遍，
/// 看 `ui.min_rect()` 有没有超过给定宽度，就能钉住"长文案不溢出"这件事。
/// 挂在 `App` 上的方法拿不到这种测量（要构造整个 App 与一帧 UI）。
fn agent_body(
    ui: &mut egui::Ui,
    mcp_installed: bool,
    cli_shows_copied: bool,
    mcp_shows_copied: bool,
    copied: &mut Option<&'static str>,
    go_component: &mut bool,
) {
    // ---------------- 命令行 ----------------
    let cli = agent_cli_line();
    agent_card(ui, |ui| {
        agent_head(ui, "命令行", |ui| {
            let label = if cli_shows_copied {
                "已复制"
            } else {
                "复制命令"
            };
            if ui.button(label).clicked() {
                ui.ctx().copy_text(cli.clone());
                *copied = Some("命令行");
            }
        });
        agent_hint(
            ui,
            "现在就能用，不需要装别的东西。把尖括号里的两处换成你自己的。",
        );
        agent_code_block(ui, &cli);
    });

    ui.add_space(8.0);

    // ---------------- MCP ----------------
    let snippet = agent_mcp_snippet();
    agent_card(ui, |ui| {
        if mcp_installed {
            agent_head(ui, "MCP 服务器", |ui| {
                let label = if mcp_shows_copied {
                    "已复制"
                } else {
                    "复制 MCP 配置"
                };
                if ui.button(label).clicked() {
                    ui.ctx().copy_text(snippet.clone());
                    *copied = Some("MCP");
                }
            });
            agent_hint(
                ui,
                "粘到 MCP 客户端的配置文件里即可。路径已经填成本机的绝对路径 —— \
                 客户端要能直接找到这个 exe，写裸文件名它起不动，界面只会报\
                 「Host 尚未注册该 stdio Server 的工具」。",
            );
            agent_code_block(ui, &snippet);
        } else {
            agent_head(ui, "MCP 服务器", |ui| {
                // 置灰而不是藏起来：让人看见「这个能力存在、缺什么」。
                ui.add_enabled(false, egui::Button::new("复制 MCP 配置"));
            });
            agent_hint(
                ui,
                "还没装 dlm-mcp.exe（它要和 dlm-gui 放在同一个目录）。\
                 装好之后，这里的配置就能直接复制了。",
            );
            // 按钮与说明**分两行**：并排就一定溢出（见 `agent_hint` 的文档）。
            if ui.button("去「组件」页安装").clicked() {
                *go_component = true;
            }
            ui.add_space(4.0);
            agent_hint(ui, "在那一页点「下载并安装」。");
        }
    });
}

/// 一块设置卡：与 `render_row` 同一套外观（`surface` 底 + 细描边 + 内边距）。
fn agent_card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(theme::STROKE, theme::line_soft()))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, body);
}

/// 卡片标题行：左边短标题，右边一个按钮。
///
/// **两边都必须是短的。** 这是 `horizontal` 里唯一安全的用法 —— 横向布局给子元素
/// 的可用宽度是无限的，里面任何需要折行的文本都会直接顶出卡片右缘。
/// 长的解释一律改用 [`agent_hint`] 放到下一行（纵向上下文才会折行）。
fn agent_head(ui: &mut egui::Ui, title: &str, button: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(12.5).color(theme::ink()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            button(ui);
        });
    });
}

/// 卡片里的一句解释。**独占一行**，因此会按可用宽度自动折行。
fn agent_hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(10.5).color(theme::ink_3()));
}

/// 等宽配置块：独占一整行，按可用宽度折行。
///
/// 用 `.wrap()` 显式声明，而不是指望默认样式：万一外层样式被改过，
/// 这里也不会退化成"一行到底、顶出卡片"。
fn agent_code_block(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::sunken())
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(10.5)
                        .monospace()
                        .color(theme::ink_2()),
                )
                .wrap(),
            );
        });
}

/// CLI 那一行：照着改就能用。占位符用尖括号，免得被误抄成真路径。
pub(crate) fn agent_cli_line() -> String {
    "dlm --url <下载链接> --out-dir \"<目录>\"".to_string()
}

/// MCP 客户端配置里那个 `command` 该写什么。
///
/// ## 必须是绝对路径（2026-11-18 用户实测：Host 注册不到工具）
///
/// 原来这里写的是裸名字 `dlm-mcp.exe`。但 MCP 客户端是用
/// `Command::new(<command>)` 去起子进程的 —— 裸名字只在它**恰好在 PATH 上**时
/// 才解析得到。`dlm-mcp.exe` 装在程序目录里，不在 PATH，于是：
///
/// - 客户端起不动这个进程
/// - 界面报「Host 尚未注册该 stdio Server 的工具，请检查启动日志后重试」
///
/// 实测对照（同一台机器、同一个 exe）：
/// - `dlm-mcp.exe` → 系统找不到指定的文件
/// - `E:\Tool\dlm\bin\dlm-mcp.exe` → `initialize` / `tools/list` 正常应答
///
/// 所以这里取「与 dlm-gui 同目录」的绝对路径 —— 那正是组件页安装 MCP 的落点。
/// 拿不到当前 exe 路径时才退回裸名字（那种环境下也没有更好的选择）。
fn mcp_command() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(mcp_install::MCP_BINARY)))
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| mcp_install::MCP_BINARY.to_string())
}

/// MCP 客户端要的那段配置。
///
/// 走 `serde_json` 而不是手拼字符串：**Windows 路径里的反斜杠必须转义成 `\\`**，
/// 手拼极容易漏（漏了就是一粘就坏的 JSON，而用户不会怀疑是我们生成的）。
pub(crate) fn agent_mcp_snippet() -> String {
    mcp_snippet_for(&mcp_command())
}

/// 由给定的 `command` 生成配置文本。抽出来是为了能对**任意路径**（含反斜杠、
/// 含空格）做用例，不必依赖跑用例时真实的 exe 路径。
pub(crate) fn mcp_snippet_for(command: &str) -> String {
    let value = serde_json::json!({
        "mcpServers": {
            "dlm": { "command": command }
        }
    });
    // `to_string_pretty` 对 `Value` 不会失败；真失败了也不该让设置页崩。
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| String::new())
}

/// 本机有没有 `dlm-mcp.exe`（与 GUI 同目录）。
///
/// `exists` 作为参数传进来是为了能测：真去问文件系统的话，用例就得往
/// 安装目录里塞文件，那是测试不该做的事。
pub(crate) fn mcp_available(
    current_exe: &Option<std::path::PathBuf>,
    exists: impl Fn(&std::path::Path) -> bool,
) -> bool {
    let Some(exe) = current_exe else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    exists(&dir.join("dlm-mcp.exe"))
}

/// 设置面板一行都没画时，至少得让人看得出"有哪些分类"—— 这里只是给用例用的自查。
#[cfg(test)]
pub(crate) fn spec_count() -> usize {
    super::settings_spec::SPECS.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_mcp_copy_button_needs_an_actual_binary_next_to_the_gui() {
        let exe = Some(PathBuf::from("C:\\app\\dlm-gui.exe"));
        assert!(
            !mcp_available(&exe, |_| false),
            "没装 dlm-mcp 时不能让人复制一段指向空气的配置"
        );
        assert!(mcp_available(&exe, |p| p == Path::new("C:\\app\\dlm-mcp.exe")));
        assert!(!mcp_available(&None, |_| true), "拿不到自身路径就当没装");
    }

    #[test]
    fn the_agent_snippets_name_the_commands_they_want() {
        assert!(
            agent_cli_line().contains("dlm --url"),
            "CLI 那行得是用户能直接粘的命令"
        );
        let mcp = agent_mcp_snippet();
        assert!(mcp.contains("mcpServers"), "{mcp}");
        assert!(mcp.contains("dlm-mcp.exe"), "{mcp}");
        // 是合法 JSON：粘到客户端配置里不能一粘就坏。
        assert!(
            serde_json::from_str::<serde_json::Value>(&mcp).is_ok(),
            "MCP 配置必须是合法 JSON"
        );
    }

    /// 每个分类都要能落到界面上：分类列表就是 `Category::ALL`，这里确认顺序稳定。
    #[test]
    fn the_left_column_lists_all_categories_in_order() {
        let titles: Vec<&str> = Category::ALL.iter().map(|c| c.title()).collect();
        assert_eq!(
            titles,
            vec![
                "下载",
                "连接与速度",
                "限速计划",
                "代理",
                "Agent 接入",
                "界面",
                "组件",
                "高级"
            ]
        );
        assert!(spec_count() > 0);
        // 「组件」是状态面板：有页面、没有设置项。它必须是**有名有姓的空**，
        // 而不是"漏了一条设置"。
        assert!(
            specs_in(Category::Component).next().is_none(),
            "组件页不该混进设置项：那是状态面板"
        );
    }

    /// 磁盘上的扩展版本必须本地优先：清单检查失败不该把"装了"变成"没检测到"。
    #[test]
    fn the_disk_extension_version_survives_a_failed_manifest_check() {
        // 断网：清单那份没有，但本地读到了 —— 必须留下本地的。
        assert_eq!(
            disk_extension(None, Some("0.2.16".into())),
            Some("0.2.16".to_string())
        );
        // 两边都有、且不一致：本地才是磁盘的事实。
        assert_eq!(
            disk_extension(Some("0.2.15".into()), Some("0.2.16".into())),
            Some("0.2.16".to_string())
        );
        // 本地读不到才退到清单那份。
        assert_eq!(
            disk_extension(Some("0.2.15".into()), None),
            Some("0.2.15".to_string())
        );
        assert_eq!(disk_extension(None, None), None);
    }

    /// 每张卡都得有按钮，且「没装 MCP」时的主按钮必须是安装本身。
    #[test]
    fn every_component_card_offers_an_action_and_mcp_missing_offers_install() {
        use crate::gui::app::components::{ComponentId, ComponentState};

        let missing = crate::gui::app::components::ComponentRow {
            id: ComponentId::Mcp,
            version: "未安装".to_string(),
            state: ComponentState::Missing,
            hint: String::new(),
        };
        let actions = component_actions(&missing);
        assert_eq!(
            actions.first().map(|(label, _)| *label),
            Some("下载并安装"),
            "没装时主按钮就是安装（right_to_left 下第一个画在最右）"
        );
        assert!(
            actions
                .iter()
                .any(|(_, a)| *a == ComponentAction::InstallMcp)
        );
        assert!(
            actions
                .iter()
                .any(|(_, a)| *a == ComponentAction::OpenReleases),
            "还要留一条手动下载的路：直链可能 404"
        );

        // 其余三张卡也不能是"有状态没动作"的死卡。
        for id in [
            ComponentId::Desktop,
            ComponentId::Extension,
            ComponentId::Channel,
        ] {
            let row = crate::gui::app::components::ComponentRow {
                id,
                version: "–".to_string(),
                state: ComponentState::Ok,
                hint: String::new(),
            };
            assert!(
                !component_actions(&row).is_empty(),
                "{} 卡上一个按钮都没有",
                id.title()
            );
        }
    }

    /// 设置窗跟着主窗收敛，不许顶满客户区。
    ///
    /// 原来的写法是 `fixed_size(760, 520)`，而 `theme::WINDOW_MIN` 是 760×480 ——
    /// 同宽、还高 40pt，所以主窗接近下限时设置窗右侧与底部都被裁。
    #[test]
    fn the_settings_window_shrinks_with_the_main_window_instead_of_overflowing() {
        // 主窗按默认尺寸（1120×700）：设置窗保持设计尺寸。
        assert_eq!(
            settings_window_size(egui::vec2(1120.0, 700.0)),
            SETTINGS_WINDOW_DESIRED
        );

        // 主窗缩到最小内径（760×480）：设置窗必须跟着收，且留出边距。
        let small = settings_window_size(egui::vec2(760.0, 480.0));
        assert_eq!(small, egui::vec2(728.0, 448.0));
        assert!(
            small.x <= 760.0 - SETTINGS_WINDOW_MARGIN && small.y <= 480.0 - SETTINGS_WINDOW_MARGIN,
            "设置窗不许超出视口：{small:?}"
        );

        // 视口比下限还小：以视口为准，绝不"为了不溢出而溢出"。
        let tiny = settings_window_size(egui::vec2(400.0, 300.0));
        assert!(
            tiny.x <= 400.0 - SETTINGS_WINDOW_MARGIN && tiny.y <= 300.0 - SETTINGS_WINDOW_MARGIN,
            "{tiny:?}"
        );

        // 尺寸随视口单调不增：视口变小，窗不能反而变大。
        let mut last = settings_window_size(egui::vec2(1600.0, 1000.0));
        for w in [1400.0, 1200.0, 1000.0, 900.0, 800.0, 760.0] {
            let now = settings_window_size(egui::vec2(w, 700.0));
            assert!(now.x <= last.x, "视口缩小时设置窗宽度不该变大");
            last = now;
        }
    }

    /// 内容区高度由窗高反推，设计尺寸下仍是 420（观感不变）。
    #[test]
    fn the_settings_content_area_follows_the_window_height() {
        assert_eq!(
            settings_scroll_height(SETTINGS_WINDOW_DESIRED),
            420.0,
            "760×520 时必须还是原来那个 420，否则等于顺手改了观感"
        );
        assert!(settings_scroll_height(egui::vec2(760.0, 448.0)) < 420.0);
        assert_eq!(settings_scroll_height(egui::vec2(560.0, 360.0)), 260.0);
        // 再矮也留一条能滚的缝，不能算出负数。
        assert_eq!(
            settings_scroll_height(egui::vec2(560.0, 150.0)),
            SETTINGS_SCROLL_MIN_H
        );
    }
    /// 设置行左列不能写死宽度：它必须跟着行宽走，否则右侧控件会把卡片顶破。
    ///
    /// 用户 2026-11-17 报的「代理界面右上角还有一点点溢出」根因就在这里：
    /// 左列 300 + 输入框 220 + 按钮 ≈ 590 > 实际可用约 560。
    #[test]
    fn the_settings_row_text_column_adapts_instead_of_using_a_fixed_300() {
        // 720pt 窗、分类列 132、间距 10、卡片内边距 20 → 约 558
        let narrow = settings_row_text_width(558.0);
        assert!(
            narrow < 300.0,
            "可用 558 时左列还占 {narrow}，右侧控件没地方站"
        );
        // 右侧至少要有 240pt 才放得下一个输入框加按钮。
        assert!(
            558.0 - narrow >= 240.0,
            "右侧只剩 {:.0}pt，代理那行的输入框会被挤出去",
            558.0 - narrow
        );

        // 宽窗口下也不能无限长。
        assert_eq!(settings_row_text_width(2000.0), 320.0);
        // 极窄也不塌成 0。
        assert_eq!(settings_row_text_width(200.0), 220.0);
    }

    /// 把一段布局放进**已知宽度**的 `Ui` 里排一遍，返回它实际占的宽度。
    ///
    /// `min_rect()` 是这一帧所有已排布内容的包围盒 —— 只要里面有东西不肯换行，
    /// 这个宽度就会超过给它的宽度。
    fn laid_out_width(width: f32, build: impl FnOnce(&mut egui::Ui)) -> f32 {
        let ctx = egui::Context::default();
        let mut got = 0.0;
        // 字体要等第一帧才装好，`run_ui` 就是那一帧。
        let mut warm = ctx.run_ui(egui::RawInput::default(), |_| {});
        warm.textures_delta.clear();
        // `build` 是 FnOnce，而 `run_ui` 的回调会被当成 FnMut 借用 —— 用 Option 取一次。
        let mut once = Some(build);
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1200.0, 3000.0),
                )),
                ..Default::default()
            },
            |ui| {
                ui.allocate_ui(egui::vec2(width, 2000.0), |ui| {
                    if let Some(f) = once.take() {
                        f(ui);
                    }
                    got = ui.min_rect().width();
                });
            },
        );
        out.textures_delta.clear();
        got
    }

    /// **Agent 接入页的长文案必须换行，不许顶出右缘**（用户 2026-11-18 截图）。
    ///
    /// 病根只有一条：`ui.horizontal` 给子元素的可用宽度是无限的，里面的 `label`
    /// 于是永不换行。旧代码把「去『组件』页安装」按钮和一句二十多字的说明塞进
    /// 同一个 horizontal，必然溢出。
    ///
    /// 所以这条按**多种宽度**量：设置窗随主窗缩放，窄窗才是最容易暴露的地方。
    /// 变异验证：把任一长句改回 horizontal 并排，对应宽度立刻变红。
    #[test]
    fn the_agent_page_wraps_long_text_instead_of_overflowing() {
        // 从很窄到常规，覆盖设置窗被主窗压小的情形。
        for width in [220.0f32, 280.0, 360.0, 440.0, 560.0] {
            for mcp_installed in [false, true] {
                let got = laid_out_width(width, |ui| {
                    let mut copied = None;
                    let mut go = false;
                    agent_body(ui, mcp_installed, false, false, &mut copied, &mut go);
                });
                assert!(
                    got <= width + 0.5,
                    "宽度 {width}（mcp_installed={mcp_installed}）时内容排到了 {got:.1}pt —— 长文案没有换行"
                );
            }
        }

        // 反面保险：万一哪天这两个长句被改短到根本不需要换行，这条用例就测不出东西了。
        // 先确认它们在这个宽度下**确实**需要多行，否则上面那组断言是空的。
        let longest = "还没装 dlm-mcp.exe（它要和 dlm-gui 放在同一个目录）。\
                       装好之后，这里的配置就能直接复制了。";
        let one_line = laid_out_width(f32::INFINITY, |ui| agent_hint(ui, longest));
        let narrow = laid_out_width(240.0, |ui| agent_hint(ui, longest));
        assert!(
            one_line > 240.0,
            "这段说明只有 {one_line:.1}pt 宽，在 240pt 里一行就放得下 —— \
             那「会不会溢出」这件事已经测不到了，换一句更长的"
        );
        assert!(
            narrow <= 240.5,
            "窄容器里说明排到了 {narrow:.1}pt，没有换行"
        );
    }

    /// **MCP 配置里必须是绝对路径，不能是裸文件名**（2026-11-18 用户实测）。
    ///
    /// 客户端用 `Command::new(<command>)` 起进程，裸名字只在 PATH 上才解析得到；
    /// `dlm-mcp.exe` 装在程序目录里、不在 PATH ⇒ 起不动 ⇒ 界面报
    /// 「Host 尚未注册该 stdio Server 的工具」。
    ///
    /// 实测对照（同机同 exe）：裸名字「系统找不到指定的文件」；
    /// 完整路径 `initialize` / `tools/list` 正常应答。
    #[test]
    fn the_mcp_snippet_carries_an_absolute_path_the_host_can_spawn() {
        // Windows 路径里的反斜杠必须转义成 \\ —— 手拼最容易漏这一处。
        let cmd = r"E:\Tool\dlm\bin\dlm-mcp.exe";
        let snippet = mcp_snippet_for(cmd);

        assert!(
            snippet.contains(r"E:\\Tool\\dlm\\bin\\dlm-mcp.exe"),
            "配置里的路径必须转义反斜杠：{snippet}"
        );
        // 粘到客户端里不能一粘就坏：必须是合法 JSON，且解析回来等于原路径。
        let parsed: serde_json::Value = serde_json::from_str(&snippet)
            .unwrap_or_else(|e| panic!("MCP 配置不是合法 JSON（{e}）：{snippet}"));
        assert_eq!(
            parsed["mcpServers"]["dlm"]["command"].as_str(),
            Some(cmd),
            "command 解析回来必须与原路径一致"
        );

        // 反面保险：绝不能退化成裸名字 —— 那正是这条用例要挡的形态。
        assert_ne!(
            parsed["mcpServers"]["dlm"]["command"].as_str(),
            Some("dlm-mcp.exe"),
            "裸文件名在 PATH 上找不到，Host 注册不到工具"
        );

        // 带空格的安装路径也要能用（例如装在 Program Files）。
        let spaced = r"C:\Program Files\dlm\bin\dlm-mcp.exe";
        let parsed: serde_json::Value = serde_json::from_str(&mcp_snippet_for(spaced)).unwrap();
        assert_eq!(
            parsed["mcpServers"]["dlm"]["command"].as_str(),
            Some(spaced)
        );

        // **走真实接线**：上面测的是"给定路径怎么拼字符串"，测不到"路径是怎么取的"。
        // 变异验证：把 `mcp_command()` 改回返回裸名字，只有这一段会红 ——
        // 少了它就是又一次假绿（上一轮动画的假绿就是这么来的）。
        let real: serde_json::Value = serde_json::from_str(&agent_mcp_snippet())
            .unwrap_or_else(|e| panic!("设置页生成的配置不是合法 JSON：{e}"));
        let cmd = real["mcpServers"]["dlm"]["command"]
            .as_str()
            .expect("command 必须是字符串");
        assert!(cmd.ends_with("dlm-mcp.exe"), "{cmd}");
        assert!(
            std::path::Path::new(cmd).is_absolute(),
            "设置页复制出去的 MCP 配置里必须是绝对路径，实际是 {cmd:?} —— 客户端起不动裸名字"
        );
    }
}
