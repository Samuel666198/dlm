//! E19：剪贴板命中提示条。
//!
//! ## 为什么是「提示」而不是「自动开始下载」
//!
//! IDM 那个勾选框写的是「监视剪贴板中的下载链接地址**并自动开始下载**」。
//! dlm 这里**只提示**，理由：
//!
//! 1. 用户复制一个链接，绝大多数时候是要**发给别人**或粘到别处，不是要下载。
//!    自动开始下载会让磁盘里凭空多出文件，而且是用户没要求的那一个。
//! 2. 剪贴板里可能出现不该下载的东西（同事发来的内部链接、付费资源直链）。
//!    让用户点一下确认，代价是一秒，收益是「程序不会替我做决定」。
//!
//! 所以语义是「发现了，要不要下」，不是「已经在下」。
//!
//! ## 同一条只提示一次
//!
//! `clipboard_dismissed` 记下用户忽略过的那条。否则用户点了「忽略」，
//! 而剪贴板内容没变（序号没变就不会再命中，但用户可能又复制了一次同样的链接），
//! 提示条会反复冒出来。

use eframe::egui;

use super::App;
use crate::gui::Ticket;
use crate::gui::app::theme;

impl App {
    /// 把监视线程交来的命中收进 UI 状态。每帧调一次，非阻塞。
    pub(super) fn drain_clipboard(&mut self, ctx: &egui::Context) {
        let Some(watcher) = &self.clipboard else {
            // 监视没开（或这个平台没有实现）⇒ 顺手清掉残留提示，
            // 否则关掉开关后那条提示会一直挂着。
            self.clipboard_hit = None;
            return;
        };
        let mut latest = None;
        while let Ok(hit) = watcher.hits.try_recv() {
            latest = Some(hit);
        }
        let Some(hit) = latest else {
            return;
        };
        // 用户明确忽略过的那一条不再提示。
        if self.clipboard_dismissed.as_deref() == Some(hit.url.as_str()) {
            return;
        }
        self.clipboard_hit = Some(hit);
        // 提示条出现要立刻重绘，不能等下一拍（用户可能正盯着窗口）。
        ctx.request_repaint();
    }

    /// 工具条下面那条提示：「发现下载链接 … [下载] [忽略]」。
    pub(super) fn clipboard_hint_bar(&mut self, ui: &mut egui::Ui) {
        let Some(hit) = self.clipboard_hit.clone() else {
            return;
        };
        let mut download = false;
        let mut dismiss = false;

        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(egui::RichText::new("剪贴板").size(10.5).color(theme::INK_3));
            // 只显示末段，整条 URL 太长会把按钮挤出去；完整地址在 tooltip 里。
            let short = short_name(&hit.url);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short)
                        .size(11.5)
                        .color(theme::INK_2)
                        .monospace(),
                )
                .truncate(),
            )
            .on_hover_text(&hit.url);

            // 按钮靠右：先占位再画，免得 URL 长短影响按钮位置。
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if ui.button("忽略").clicked() {
                    dismiss = true;
                }
                if ui.button("下载").clicked() {
                    download = true;
                }
            });
        });

        if download {
            let name = short_name(&hit.url);
            self.send(Ticket::Add {
                url: hit.url.clone(),
                name,
                referer: None,
                start_at: None,
                on_complete: None,
                reply: crate::gui::discard_reply(),
            });
            self.clipboard_hit = None;
        }
        if dismiss {
            // 记下"用户不要这一条"，否则同一链接再复制一次还会弹。
            self.clipboard_dismissed = Some(hit.url);
            self.clipboard_hit = None;
        }
    }
}

/// URL 的末段，用作默认文件名与提示条上的短标签。
///
/// 与 `dialog::infer_name` 同一口径，但这里多兜一层：末段为空（`https://x/`）时
/// 退到 host，免得提示条上出现一个空白。
fn short_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    if !last.is_empty() && last.contains('.') {
        return last.to_string();
    }
    // 没有像文件名的末段：用 host 当标签，至少让人认得出是哪来的。
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("download.bin")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_prefers_the_last_segment_when_it_looks_like_a_file() {
        assert_eq!(short_name("https://x.test/a/b/file.zip"), "file.zip");
        assert_eq!(short_name("https://x.test/file.zip?t=1"), "file.zip");
    }

    /// 末段不是文件名（目录页、根路径）⇒ 退到 host。
    /// 这条不是为了好看：提示条上出现空串会让人不知道下的是什么。
    #[test]
    fn short_name_falls_back_to_the_host_when_there_is_no_file_segment() {
        assert_eq!(short_name("https://x.test/"), "x.test");
        assert_eq!(short_name("https://x.test"), "x.test");
        assert_eq!(short_name("https://cdn.x.test/dir/"), "cdn.x.test");
    }
}
