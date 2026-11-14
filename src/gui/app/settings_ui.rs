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
use super::settings_spec::{Category, SettingKey, SettingSpec, search, specs_in};
use crate::gui::RuntimeTuning;
use crate::gui::Ticket;
use crate::gui::app::theme;

/// 连接与速度里"0 = 不限"这类尾注。
const ZERO_MEANS_UNLIMITED: &str = "0 = 不限";

impl App {
    pub(super) fn settings_panel(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut save = false;
        let mut cancel = false;

        egui::Window::new("设置")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(760.0)
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
                    .max_height(420.0)
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
                        ui.label(egui::RichText::new(err).size(11.5).color(theme::BAD));
                    } else {
                        ui.label(
                            egui::RichText::new("改动保存后立刻生效（除标注『重启后生效』的项）")
                                .size(10.5)
                                .color(theme::INK_3),
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
                                        theme::ACCENT_INK
                                    } else {
                                        theme::INK_2
                                    },
                                ),
                            )
                            .fill(if selected {
                                theme::ACCENT_SOFT
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
                    .color(theme::INK_3),
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
                        .color(theme::INK_3),
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
                        .color(theme::INK_3),
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
            .fill(theme::SURFACE)
            .stroke(egui::Stroke::new(theme::STROKE, theme::LINE_SOFT))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(300.0);
                        ui.label(egui::RichText::new(spec.title).size(12.5).color(theme::INK));
                        ui.label(
                            egui::RichText::new(spec.hint)
                                .size(10.5)
                                .color(theme::INK_3),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if spec.read_only {
                            ui.label(
                                egui::RichText::new("程序自己记的")
                                    .size(10.5)
                                    .color(theme::INK_3),
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
                    .color(theme::INK_3),
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
                        .color(theme::INK_3),
                );
            }
            SettingKey::MaxSpeedKiB => {
                ui.add(egui::DragValue::new(&mut self.draft.max_speed_kib).range(0..=1024 * 100));
                ui.label(
                    egui::RichText::new(ZERO_MEANS_UNLIMITED)
                        .size(10.5)
                        .color(theme::INK_3),
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
                        .color(theme::INK_3),
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
                                .desired_width(220.0),
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
                                .color(theme::INK_3),
                        );
                    }
                    // 写错当场标红，但不阻止保存：一行代理写错就存不下整份设置更糟。
                    match crate::proxy::parse_proxy(&self.draft.proxy) {
                        Ok(None) => {}
                        Ok(Some(spec)) => {
                            ui.label(
                                egui::RichText::new(format!("生效：{}", spec.redacted()))
                                    .size(10.5)
                                    .color(theme::INK_3),
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
                        .color(theme::INK_2),
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
                        .color(theme::INK_2),
                );
            }
        }
    }

    /// 「Agent 接入」页头：两段可复制的配置 + 一句说明。
    ///
    /// 判据是"CLI 命令现在就能用、MCP 配置没装就置灰"。置灰不是偷懒：
    /// 把一段指向不存在程序的配置交给用户，比不提供更糟。
    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Agent 接入")
                .size(12.5)
                .strong()
                .color(theme::INK),
        );
        ui.label(
            egui::RichText::new("让人或 Agent 都能下：命令行一直可用；MCP 需要另外装 dlm-mcp。")
                .size(10.5)
                .color(theme::INK_3),
        );
        ui.add_space(6.0);

        // CLI：一行命令，复制即用。
        let cli = agent_cli_line();
        ui.horizontal(|ui| {
            ui.add_sized(
                egui::vec2(300.0, 44.0),
                egui::Label::new(
                    egui::RichText::new(&cli)
                        .size(11.0)
                        .monospace()
                        .color(theme::INK_2),
                ),
            );
            if ui.button("复制").clicked() {
                ui.ctx().copy_text(cli.clone());
                self.agent_copied = Some("cli".to_string());
            }
        });
        ui.add_space(6.0);

        // MCP：一段 JSON。没装 dlm-mcp 时置灰。
        let available = mcp_available(&std::env::current_exe().ok(), |p| p.exists());
        let snippet = agent_mcp_snippet();
        ui.horizontal(|ui| {
            if available {
                if ui.button("复制 MCP 配置").clicked() {
                    ui.ctx().copy_text(snippet.clone());
                    self.agent_copied = Some("mcp".to_string());
                }
                ui.label(
                    egui::RichText::new("粘到 MCP 客户端配置里即可")
                        .size(10.5)
                        .color(theme::INK_3),
                );
            } else {
                ui.add_enabled(false, egui::Button::new("复制 MCP 配置"));
                ui.label(
                    egui::RichText::new("没找到 dlm-mcp.exe：先装那一份再复制")
                        .size(10.5)
                        .color(theme::INK_3),
                );
            }
        });
        if let Some(what) = &self.agent_copied {
            ui.label(
                egui::RichText::new(format!("已复制（{what}）"))
                    .size(10.5)
                    .color(theme::OK),
            );
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
                ui.label(egui::RichText::new("窗口").size(11.0).color(theme::INK_3));
                ui.add(egui::DragValue::new(&mut b.start_hour).range(0..=23));
                ui.label("点 到");
                ui.add(egui::DragValue::new(&mut b.end_hour).range(0..=23));
                ui.label("点");
                if b.start_hour > b.end_hour {
                    ui.label(egui::RichText::new("跨午夜").size(10.0).color(theme::INK_3));
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("限速").size(11.0).color(theme::INK_3));
                ui.add(egui::DragValue::new(&mut b.speed_kib).range(0..=1024 * 100));
                ui.label(
                    egui::RichText::new("KiB/s（0 = 不限速）")
                        .size(10.5)
                        .color(theme::INK_3),
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
            .color(theme::INK_3),
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

/// CLI 那一行：照着改就能用。占位符用尖括号，免得被误抄成真路径。
pub(crate) fn agent_cli_line() -> String {
    "dlm --url <下载链接> --out-dir \"<目录>\"".to_string()
}

/// MCP 客户端要的那段配置。路径先给占位符：真正的路径只有本机知道，
/// 而写死一个 `C:\Users\xxx` 只会让每个用户都要手改。
pub(crate) fn agent_mcp_snippet() -> String {
    "{\n  \"mcpServers\": {\n    \"dlm\": {\n      \"command\": \"dlm-mcp.exe\"\n    }\n  }\n}"
        .to_string()
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
    fn the_left_column_lists_all_seven_categories_in_order() {
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
                "高级"
            ]
        );
        assert!(spec_count() > 0);
    }
}
