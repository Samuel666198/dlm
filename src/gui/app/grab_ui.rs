//! 站点抓取（0.4.0 · A）：扫描 → 清单 → 勾选入队。
//!
//! ## 为什么先出清单再下载
//!
//! 站点抓取最容易出的事故是"一按就把整站拖下来"。所以这里强制两步：
//! 先扫描出**清单**（只是文本，不建任何任务），人看过、勾过，才允许入队。
//!
//! ## 线程
//!
//! `scan` 是阻塞的（要发 HTTP），不能放在 UI 线程。点「扫描」时起一个线程，
//! 结果通过 `mpsc` 回来；每帧 `try_recv` 一次，收到就填清单。

use eframe::egui;

use super::App;
use crate::grab::{self, GrabRules, Limits};
use crate::gui::Ticket;

impl App {
    pub(super) fn grab_window(&mut self, ui: &mut egui::Ui) {
        if !self.grab_open {
            return;
        }
        // 后台线程的扫描结果
        if let Some(rx) = &self.grab_rx {
            match rx.try_recv() {
                Ok(Ok(plan)) => {
                    self.grab_checked = vec![true; plan.files.len()];
                    self.grab_plan = Some(plan);
                    self.grab_busy = false;
                    self.grab_rx = None;
                    self.grab_error = None;
                }
                Ok(Err(reason)) => {
                    self.grab_error = Some(reason);
                    self.grab_busy = false;
                    self.grab_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.grab_error = Some("扫描线程意外结束".into());
                    self.grab_busy = false;
                    self.grab_rx = None;
                }
            }
        }

        let mut open = true;
        let mut start = false;
        let mut enqueue = false;
        egui::Window::new("站点抓取")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_width(560.0)
            .default_height(420.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("先扫描出清单，勾选后才建任务；扫描本身不下载任何文件")
                        .size(10.5)
                        .color(super::theme::INK_3),
                );
                ui.horizontal(|ui| {
                    ui.label("起始页");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.grab_url)
                            .hint_text("https://example.com/downloads/")
                            .desired_width(360.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("深度");
                    ui.add(egui::DragValue::new(&mut self.grab_depth).range(1..=3));
                    ui.checkbox(&mut self.grab_same_host, "只抓同站");
                    if self.grab_busy {
                        ui.spinner();
                        ui.label("扫描中…");
                    } else if ui.button("扫描").clicked() {
                        start = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("包含");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.grab_include)
                            .hint_text("*.zip, *.mp4（空 = 全收）")
                            .desired_width(200.0),
                    );
                    ui.label("排除");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.grab_exclude)
                            .hint_text("*sample*")
                            .desired_width(160.0),
                    );
                });
                if let Some(err) = &self.grab_error {
                    ui.colored_label(egui::Color32::from_rgb(0xc6, 0x28, 0x28), err);
                }
                ui.separator();

                if let Some(plan) = &self.grab_plan {
                    let picked = self.grab_checked.iter().filter(|v| **v).count();
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "找到 {} 个文件，已勾选 {} 个；另有 {} 条被规则跳过",
                            plan.files.len(),
                            picked,
                            plan.skipped.len()
                        ));
                        if ui.button("全选").clicked() {
                            self.grab_checked.iter_mut().for_each(|v| *v = true);
                        }
                        if ui.button("全不选").clicked() {
                            self.grab_checked.iter_mut().for_each(|v| *v = false);
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for (i, url) in plan.files.iter().enumerate() {
                                let mut on = self.grab_checked.get(i).copied().unwrap_or(false);
                                if ui.checkbox(&mut on, url).changed()
                                    && let Some(slot) = self.grab_checked.get_mut(i)
                                {
                                    *slot = on;
                                }
                            }
                            if !plan.skipped.is_empty() {
                                ui.separator();
                                ui.label(
                                    egui::RichText::new("被跳过（只读）")
                                        .size(10.5)
                                        .color(super::theme::INK_3),
                                );
                                for (url, why) in &plan.skipped {
                                    ui.label(
                                        egui::RichText::new(format!("{url} — {why}"))
                                            .size(10.0)
                                            .color(super::theme::INK_3),
                                    );
                                }
                            }
                        });
                    if ui.button(format!("加入队列（{picked}）")).clicked() && picked > 0 {
                        enqueue = true;
                    }
                }
            });

        if start {
            self.start_grab_scan();
        }
        if enqueue {
            self.enqueue_grabbed();
        }
        if !open {
            self.grab_open = false;
        }
    }

    fn start_grab_scan(&mut self) {
        let url = self.grab_url.trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.grab_error = Some("请填 http(s) 开头的地址".into());
            return;
        }
        let rules = GrabRules {
            same_host: self.grab_same_host,
            depth: self.grab_depth,
            include: split_patterns(&self.grab_include),
            exclude: split_patterns(&self.grab_exclude),
        };
        self.grab_busy = true;
        self.grab_error = None;
        self.grab_plan = None;
        self.grab_checked.clear();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let plan = grab::scan(&url, &rules, Limits::default());
            let _ = tx.send(Ok(plan));
        });
        self.grab_rx = Some(rx);
    }

    fn enqueue_grabbed(&mut self) {
        let Some(plan) = self.grab_plan.take() else {
            return;
        };
        for (i, url) in plan.files.iter().enumerate() {
            if !self.grab_checked.get(i).copied().unwrap_or(false) {
                continue;
            }
            self.send(Ticket::Add {
                url: url.clone(),
                name: String::new(),
                referer: None,
                start_at: None,
                on_complete: None,
                reply: crate::gui::discard_reply(),
            });
        }
        self.grab_checked.clear();
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
    use super::split_patterns;

    #[test]
    fn patterns_split_on_comma_semicolon_and_chinese_comma() {
        assert_eq!(
            split_patterns("*.zip, *.mp4;*.rar"),
            vec!["*.zip", "*.mp4", "*.rar"]
        );
        assert_eq!(split_patterns("  "), Vec::<String>::new());
    }
}
