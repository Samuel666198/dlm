//! 站点抓取（0.6.0 阶段 3 改版）：扫描 → 清单 → 勾选入队。
//!
//! ## 为什么先出清单再下载
//!
//! 站点抓取最容易出的事故是"一按就把整站拖下来"。所以强制两步：先扫描出**清单**
//! （只是文本，不建任何任务），人看过、勾过，才允许入队。
//!
//! ## 这一版抄设计稿的地方
//!
//! 起始页 / 范围 / 模板 / 扫描进度 / 清单 / 跳过折叠 / 底部入队。清单列是
//! **勾选 · 文件名+来源页 · 类型 · 来自 · 大小**；重复项标「已去重」且不许勾选。
//!
//! 两处与设计稿不同，都是因为**做不到就不该画**：
//!
//! - **大小列恒为「—」**：扫描只读 HTML，不发 HEAD，所以这一刻真不知道多大。
//!   挂个假数字比空着更糟；入队之后列表里才有真大小。
//! - **扫描进度是"转圈 + 已用时"而不是百分比**：`grab::scan` 是一次阻塞 BFS，
//!   中途拿不到"还有几页"。硬编一个百分比就是骗人。
//!
//! ## 线程
//!
//! `scan` 要发 HTTP，不能放在 UI 线程。点「扫描」起线程，结果经 `mpsc` 回来，
//! 每帧 `try_recv` 一次。

use std::time::Instant;

use eframe::egui;

use super::App;
use super::sidebar::category_label;
use crate::grab::{self, GrabFile, GrabRules, Limits};
use crate::gui::Ticket;
use crate::gui::app::theme;

/// 「加入队列」能不能点：**必须至少勾了一条**。
///
/// 抽成纯函数是因为这条判据写在设计稿的验收里，而它以前埋在界面代码的分支里 ——
/// 没人能证明"没勾选时按钮一定点不动"，只能靠手点一遍。
/// 一步的标题：小方块里一个阿拉伯数字 + 一句动作。
///
/// 用数字方块而不是 ①②③：那几个字形依赖字体，缺了就变方框（本项目踩过好几次）。
/// 方块自己画，数字用画笔画 —— 与图标同一套做法。
fn step_label(ui: &mut egui::Ui, n: u32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 4.0, theme::accent_soft());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::proportional(11.0),
            theme::accent_ink(),
        );
        ui.label(
            egui::RichText::new(text)
                .size(11.5)
                .strong()
                .color(theme::ink_2()),
        );
    });
}

/// 抓取窗该多宽：跟着主窗走，别把 720 钉死（主窗最小内径是 760，720 几乎顶满）。
/// 抓取屏的内容最大宽度：主区很宽时表单不该拉成一条线。
pub(crate) const GRAB_PANE_MAX_W: f32 = 860.0;

/// 「包含 / 排除」两个输入框的宽度：按这一行剩下的宽度分，并各自留上下限。
pub(crate) fn grab_filter_widths(row_width: f32) -> (f32, f32) {
    let include = (row_width * 0.22).clamp(140.0, 280.0);
    let exclude = (row_width * 0.18).clamp(110.0, 220.0);
    (include, exclude)
}

pub(crate) fn enqueue_enabled(picked: usize) -> bool {
    picked > 0
}

impl App {
    /// 「站点抓取」主区屏（**不是弹窗**）。
    ///
    /// 设计稿 v3 里 `#pane-grab` 是主区的一块屏，与列表、新建任务三选一；
    /// 0.7.0 做成了可缩放的 `egui::Window`。用户 2026-11-17 指出这个偏离。
    ///
    /// 改成屏之后「三步」才真正立得住：步骤条属于**这一屏**的标题区，
    /// 而弹窗的标题栏在系统那一层，步骤条只能挤在内容里。
    pub(super) fn grab_pane(&mut self, ui: &mut egui::Ui) {
        self.drain_grab_scan();

        let mut start_scan = false;
        let mut enqueue = false;
        let mut save_template = false;
        let mut apply_template: Option<usize> = None;

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_max_width(GRAB_PANE_MAX_W);
                ui.label(
                    egui::RichText::new("先扫描出清单，勾选后才建任务；扫描本身不下载任何文件")
                        .size(10.5)
                        .color(theme::ink_3()),
                );
                ui.add_space(8.0);

                // ---- 第 1 步：起始页 ----
                step_label(ui, 1, "填起始页，点扫描");
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(theme::surface())
                    .stroke(egui::Stroke::new(theme::STROKE, theme::line()))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("起始页")
                                    .size(11.5)
                                    .color(theme::ink_3()),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut self.grab_url)
                                    .hint_text("https://example.com/downloads/")
                                    .desired_width(ui.available_width() - 110.0),
                            );
                            if self.grab_busy {
                                ui.spinner();
                                ui.label(
                                    egui::RichText::new(self.grab_elapsed_label())
                                        .size(11.0)
                                        .color(theme::ink_3()),
                                );
                            } else if ui.button("扫描").clicked() {
                                start_scan = true;
                            }
                        });

                        ui.add_space(6.0);
                        // 两个输入框的宽度按这一行的实际可用宽度算：写死 150 / 110 时
                        // 窗口一宽就浪费、一窄就把「排除」挤出去。
                        let (include_w, exclude_w) = grab_filter_widths(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("范围").size(11.5).color(theme::ink_3()));
                            ui.label("深度");
                            ui.add(egui::DragValue::new(&mut self.grab_depth).range(1..=3));
                            ui.checkbox(&mut self.grab_same_host, "只抓同站");
                            ui.add_space(6.0);
                            ui.label("包含");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.grab_include)
                                    .hint_text("*.zip, *.mp4（空 = 全收）")
                                    .desired_width(include_w),
                            );
                            ui.label("排除");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.grab_exclude)
                                    .hint_text("*sample*")
                                    .desired_width(exclude_w),
                            );
                        });

                        // ---- 模板（按站点记）----
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("模板").size(11.5).color(theme::ink_3()));
                            let templates: Vec<crate::grab::Template> =
                                grab::templates_for(&self.settings.grab_templates, &self.grab_url)
                                    .into_iter()
                                    .cloned()
                                    .collect();
                            for (i, t) in templates.iter().enumerate() {
                                let label = format!("{}（深度 {}）", t.name, t.depth);
                                if ui.selectable_label(false, label).clicked() {
                                    apply_template = Some(i);
                                }
                            }
                            if templates.is_empty() {
                                ui.label(
                                    egui::RichText::new("这个站还没有模板")
                                        .size(10.5)
                                        .color(theme::ink_3()),
                                );
                            }
                            ui.add_space(6.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.grab_template_name)
                                    .hint_text("模板名")
                                    .desired_width(110.0),
                            );
                            if ui
                                .add_enabled(
                                    !self.grab_template_name.trim().is_empty()
                                        && grab::host_of(&self.grab_url).is_some(),
                                    egui::Button::new("存为模板").small(),
                                )
                                .clicked()
                            {
                                save_template = true;
                            }
                        });
                    });

                if let Some(err) = &self.grab_error {
                    ui.add_space(4.0);
                    ui.colored_label(theme::bad(), err);
                }

                ui.add_space(8.0);

                // ---- 第 2 步：清单 ----
                if let Some(plan) = self.grab_plan.clone() {
                    step_label(ui, 2, "勾选要下的文件");
                    ui.add_space(4.0);
                    let picked = self.grab_checked.iter().filter(|c| **c).count();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "找到 {} 个 · 可勾选 {} · 跳过 {} 条",
                                plan.files.len(),
                                plan.selectable(),
                                plan.skipped.len()
                            ))
                            .size(11.5)
                            .color(theme::ink_2()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("全不选").clicked() {
                                self.grab_checked.iter_mut().for_each(|c| *c = false);
                            }
                            if ui.small_button("全选").clicked() {
                                for (i, f) in plan.files.iter().enumerate() {
                                    if let Some(slot) = self.grab_checked.get_mut(i) {
                                        *slot = !f.duplicate;
                                    }
                                }
                            }
                        });
                    });
                    ui.add_space(2.0);

                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            file_table(ui, &plan.files, &mut self.grab_checked);
                        });

                    // ---- 跳过（折叠）----
                    if !plan.skipped.is_empty() {
                        egui::CollapsingHeader::new(format!("跳过 {} 条", plan.skipped.len()))
                            .id_salt("grab-skipped")
                            .show(ui, |ui| {
                                for (url, why) in &plan.skipped {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(why)
                                                .size(10.5)
                                                .color(theme::ink_3()),
                                        );
                                        ui.label(
                                            egui::RichText::new(url)
                                                .size(10.5)
                                                .monospace()
                                                .color(theme::ink_3()),
                                        );
                                    });
                                }
                            });
                    }

                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    step_label(ui, 3, "加入队列");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("已选 {picked} 个"))
                                .size(11.5)
                                .color(theme::ink_2()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 判据：没勾一个就点不动。
                            if ui
                                .add_enabled(
                                    enqueue_enabled(picked),
                                    egui::Button::new("加入队列").fill(theme::accent()),
                                )
                                .clicked()
                            {
                                enqueue = true;
                            }
                            ui.label(
                                egui::RichText::new("大小要等入队后才显示（扫描不额外发请求）")
                                    .size(10.0)
                                    .color(theme::ink_3()),
                            );
                        });
                    });
                } else if !self.grab_busy {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("还没有清单：填好起始页，点「扫描」")
                            .size(11.5)
                            .color(theme::ink_3()),
                    );
                }
            });
        });

        // 出了闭包再动状态。
        if let Some(i) = apply_template {
            let templates: Vec<crate::grab::Template> =
                grab::templates_for(&self.settings.grab_templates, &self.grab_url)
                    .into_iter()
                    .cloned()
                    .collect();
            if let Some(t) = templates.get(i) {
                let rules = t.rules();
                self.grab_depth = rules.depth;
                self.grab_same_host = rules.same_host;
                self.grab_include = rules.include.join(", ");
                self.grab_exclude = rules.exclude.join(", ");
                self.grab_template_name = t.name.clone();
            }
        }
        if save_template {
            self.store_grab_template();
        }
        if start_scan {
            self.begin_grab_scan();
        }
        if enqueue {
            self.enqueue_grab_picks();
        }

        // Esc 回列表：pane 时代不关窗，只切屏。
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.main_view = super::MainView::List;
        }
    }

    /// 收扫描线程的结果。
    fn drain_grab_scan(&mut self) {
        let Some(rx) = &self.grab_rx else { return };
        match rx.try_recv() {
            Ok(plan) => {
                // 起始页就抓不到时，`scan` 只返回一条 skipped（url == 起始页）——
                // 那要当错误说出来，否则界面显示一张空清单，像在说"这站没有文件"。
                let start = self.grab_url.trim().to_string();
                let start_failed = plan.files.is_empty()
                    && plan.pages.is_empty()
                    && plan.skipped.len() == 1
                    && plan.skipped[0].0 == start;
                self.grab_error = if start_failed {
                    Some(plan.skipped[0].1.clone())
                } else {
                    None
                };
                // 默认只勾可勾选的（已去重的不勾）。
                self.grab_checked = plan.files.iter().map(|f| !f.duplicate).collect();
                self.grab_plan = Some(plan);
                self.grab_busy = false;
                self.grab_rx = None;
                self.grab_started = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.grab_error = Some("扫描线程意外结束".into());
                self.grab_busy = false;
                self.grab_rx = None;
                self.grab_started = None;
            }
        }
    }

    /// 「扫描中 12s」—— 给不了百分比，就给已用时（见模块注释）。
    fn grab_elapsed_label(&self) -> String {
        match self.grab_started {
            Some(t) => format!("扫描中 {}s", t.elapsed().as_secs()),
            None => "扫描中…".to_string(),
        }
    }

    fn begin_grab_scan(&mut self) {
        let url = self.grab_url.trim().to_string();
        if url.is_empty() {
            self.grab_error = Some("先填起始页地址".into());
            return;
        }
        let rules = GrabRules {
            same_host: self.grab_same_host,
            depth: self.grab_depth,
            include: split_patterns(&self.grab_include),
            exclude: split_patterns(&self.grab_exclude),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("dlm-grab-scan".into())
            .spawn(move || {
                // `scan` 不返回 Result：抓不到的东西都进 `plan.skipped`。
                let _ = tx.send(grab::scan(&url, &rules, Limits::default()));
            })
            .ok();
        self.grab_rx = Some(rx);
        self.grab_busy = true;
        self.grab_error = None;
        self.grab_started = Some(Instant::now());
        self.grab_plan = None;
        self.grab_checked.clear();
    }

    /// 把当前表单存成这个站的模板（同名覆盖）。
    fn store_grab_template(&mut self) {
        let rules = GrabRules {
            same_host: self.grab_same_host,
            depth: self.grab_depth,
            include: split_patterns(&self.grab_include),
            exclude: split_patterns(&self.grab_exclude),
        };
        let Some(t) =
            crate::grab::Template::capture(&self.grab_url, self.grab_template_name.trim(), &rules)
        else {
            self.grab_error = Some("起始页要先是 http(s) 地址才能存模板".into());
            return;
        };
        crate::grab::upsert_template(&mut self.settings.grab_templates, t);
        // 模板是"记住这个站"，属于立刻生效的那类：直接落盘，不等设置面板保存。
        if let Err(err) = self.settings.save(&self.settings_dir) {
            self.grab_error = Some(format!("模板保存失败：{err:#}"));
        } else {
            self.grab_error = None;
        }
    }

    /// 勾中的那些进队列。
    fn enqueue_grab_picks(&mut self) {
        let Some(plan) = self.grab_plan.take() else {
            return;
        };
        let mut n = 0usize;
        for (i, file) in plan.files.iter().enumerate() {
            if file.duplicate || !self.grab_checked.get(i).copied().unwrap_or(false) {
                continue;
            }
            self.send(Ticket::Add {
                url: file.url.clone(),
                name: name_of(&file.url),
                referer: None,
                start_at: None,
                on_complete: None,
                reply: crate::gui::discard_reply(),
            });
            n += 1;
        }
        if n > 0 {
            // 清单用掉了就清掉：免得同一条被加两遍之后还留着让人再点一次。
            self.grab_plan = None;
        }
        self.grab_checked.clear();
    }
}

/// 清单一张表。写成函数是因为它要同时读 `files` 与写 `checked`，
/// 借在 `self` 上会和外面那堆 `self.*` 打架。
fn file_table(ui: &mut egui::Ui, files: &[GrabFile], checked: &mut [bool]) {
    egui::Grid::new("grab-files")
        .num_columns(5)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for title in ["", "文件名", "类型", "来自", "大小"] {
                ui.label(egui::RichText::new(title).size(10.5).color(theme::ink_3()));
            }
            ui.end_row();

            for (i, file) in files.iter().enumerate() {
                if file.duplicate {
                    // 已去重：不可勾，给一枚标记说明"这条是同一份文件的另一个地址"。
                    ui.label(egui::RichText::new("—").size(10.5).color(theme::ink_3()));
                } else {
                    let mut on = checked.get(i).copied().unwrap_or(false);
                    if ui.checkbox(&mut on, "").changed()
                        && let Some(slot) = checked.get_mut(i)
                    {
                        *slot = on;
                    }
                }

                let name = name_of(&file.url);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&name)
                            .size(12.0)
                            .color(if file.duplicate {
                                theme::ink_3()
                            } else {
                                theme::ink()
                            }),
                    );
                    ui.label(
                        egui::RichText::new(&file.url)
                            .size(9.5)
                            .monospace()
                            .color(theme::ink_3()),
                    );
                });

                let kind = category_label(crate::queue::category_for(&name));
                ui.label(egui::RichText::new(kind).size(10.5).color(theme::ink_2()));
                ui.label(
                    egui::RichText::new(&file.from)
                        .size(10.5)
                        .color(theme::ink_3()),
                );
                if file.duplicate {
                    ui.label(
                        egui::RichText::new("已去重")
                            .size(10.5)
                            .color(theme::warn()),
                    );
                } else {
                    // 扫描不查大小（见模块注释），所以这一格恒为 —。
                    ui.label(egui::RichText::new("—").size(10.5).color(theme::ink_3()));
                }
                ui.end_row();
            }
        });
}

/// URL 末段当文件名；末段是空的就给个兜底名。
///
/// 走 `grab::path_of` 而不是自己按 `/` 切：自己切会把 `https://a.example/` 的
/// **host** 当成文件名（切出来 `a.example`），因为 scheme 后面那一段也在切分里。
pub(crate) fn name_of(url: &str) -> String {
    let path = crate::grab::path_of(url).unwrap_or_default();
    // 以 `/` 结尾 ⇒ 这一层是**目录**，没有文件名可用（`/dir/` 不该叫 `dir`）。
    let last = if path.ends_with('/') {
        ""
    } else {
        path.rsplit('/').next().unwrap_or("")
    };
    if last.is_empty() {
        "download.bin".to_string()
    } else {
        last.to_string()
    }
}

/// `*.zip, *.mp4` → `["*.zip","*.mp4"]`；空串给空表。
pub(crate) fn split_patterns(text: &str) -> Vec<String> {
    text.split(['，', ',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{enqueue_enabled, grab_filter_widths, name_of, split_patterns};

    /// 「包含 / 排除」两个框按行宽分，且各自有上下限。
    #[test]
    fn the_filter_fields_share_the_row_width() {
        let (include, exclude) = grab_filter_widths(600.0);
        assert_eq!((include, exclude), (140.0, 110.0), "窄行取下限");
        let (include, exclude) = grab_filter_widths(1200.0);
        assert!(include > 200.0 && exclude > 160.0, "宽行要长起来");
        assert!(include <= 280.0 && exclude <= 220.0, "但也别无限长");
        // 排除永远比包含短一档：它填的是收窄条件，通常更短。
        let (i, e) = grab_filter_widths(1000.0);
        assert!(e < i);
        // 极窄也不许出 0 或负数。
        let (i, e) = grab_filter_widths(10.0);
        assert!(i > 0.0 && e > 0.0);
    }

    #[test]
    fn patterns_split_on_comma_semicolon_and_chinese_comma() {
        assert_eq!(
            split_patterns("*.zip, *.mp4;*.rar"),
            vec!["*.zip", "*.mp4", "*.rar"]
        );
        assert_eq!(split_patterns("  "), Vec::<String>::new());
    }

    /// 设计稿的验收条件之一：**没勾选时入队按钮点不动**。
    #[test]
    fn enqueue_needs_at_least_one_pick() {
        assert!(!enqueue_enabled(0));
        assert!(enqueue_enabled(1));
    }

    #[test]
    fn names_come_from_the_last_path_segment() {
        assert_eq!(
            name_of("https://a.example/x/pack-01.zip?v=2"),
            "pack-01.zip"
        );
        assert_eq!(name_of("https://a.example/"), "download.bin");
        assert_eq!(name_of("https://a.example/dir/"), "download.bin");
    }
}
