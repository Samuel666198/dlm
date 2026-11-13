//! 快速添加（F3）：把一批链接一次性丢进队列。
//!
//! ## 为什么是"粘贴"而不是"拖拽"
//!
//! 计划里原本写的是拖拽篮。实测 egui 只能收到**文件拖放**（`DroppedFile` 给的是本地
//! 路径/字节），**收不到浏览器里拖出来的 URL** —— 那是浏览器自己的拖放协议
//! （`text/uri-list`），不给窗口。所以这扇窗改成"粘贴多行链接"，把同一件事做完：
//! 从浏览器复制 → 粘进来 → 一次建多个任务。
//!
//! ## 认什么
//!
//! 一行一个 URL，空行与 `#` 开头的注释行忽略；只收 `http(s)`。不在客户端做更多校验 ——
//! 真正的校验在 `queue::add`，这里多判一遍只会让两处规则慢慢分叉。

use eframe::egui;

use super::App;
use crate::gui::Ticket;

/// 从多行文本里摘出要下载的 URL。
///
/// 顺带的活：去掉首尾空白、跳过空行与 `#` 注释、去重（同一个链接粘两次只建一个任务）。
pub(crate) fn parse_urls(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !(line.starts_with("http://") || line.starts_with("https://")) {
            continue;
        }
        if seen.insert(line.to_string()) {
            out.push(line.to_string());
        }
    }
    out
}

impl App {
    pub(super) fn basket(&mut self, ui: &mut egui::Ui) {
        if !self.basket_open {
            return;
        }
        let mut open = true;
        let mut submit = false;
        let mut close = false;
        egui::Window::new("快速添加")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_width(460.0)
            .default_height(240.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("一行一个链接，空行和 # 开头的行会忽略")
                        .size(10.5)
                        .color(super::theme::INK_3),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::multiline(&mut self.basket_text)
                        .hint_text("https://example.com/a.zip")
                        .desired_width(f32::INFINITY)
                        .desired_rows(6),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let urls = parse_urls(&self.basket_text);
                    let label = if urls.is_empty() {
                        "添加".to_string()
                    } else {
                        format!("添加 {} 个", urls.len())
                    };
                    if ui
                        .add_enabled(!urls.is_empty(), egui::Button::new(label))
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button("清空").clicked() {
                        self.basket_text.clear();
                    }
                    if ui.button("关闭").clicked() {
                        close = true;
                    }
                });
            });
        if submit {
            for url in parse_urls(&self.basket_text) {
                self.send(Ticket::Add {
                    url: url.clone(),
                    // 名字交给队列从 URL 推导 —— 这里不猜，猜错了 E1/E2 还得再改一次名。
                    name: String::new(),
                    referer: None,
                    start_at: None,
                    on_complete: None,
                    reply: crate::gui::discard_reply(),
                });
            }
            self.basket_text.clear();
            self.basket_open = false;
        }
        if !open || close {
            self.basket_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_urls;

    #[test]
    fn blank_lines_comments_and_duplicates_are_dropped() {
        let text = "\n  https://a.example/1.zip  \n# 注释\n\nhttps://a.example/1.zip\nhttp://b.example/2.bin\nftp://c/x\n";
        assert_eq!(
            parse_urls(text),
            vec![
                "https://a.example/1.zip".to_string(),
                "http://b.example/2.bin".to_string()
            ],
            "空行/注释/重复/非 http(s) 都要滤掉，且保持原顺序"
        );
    }

    #[test]
    fn nothing_usable_yields_no_tasks() {
        assert!(parse_urls("").is_empty());
        assert!(parse_urls("# 只有注释\n\n").is_empty());
        assert!(parse_urls("magnet:?xt=urn:btih:x").is_empty());
    }
}
