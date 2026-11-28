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

/// 一次批量最多展开多少条。
///
/// 上限不是为了省内存，而是为了**别一次把队列塞爆**：`[1-100000]` 这种手滑
/// 会生成十万个任务，界面和调度都受不了。超过就整行丢弃并计数（见返回值）。
pub const MAX_EXPAND: usize = 500;

/// `[001-100]` 这样的区间写法：IDM 的经典批量形态。
///
/// 语义：
/// - `[1-5]` ⇒ 1..=5；`[001-100]` ⇒ **保留前导零**（001、002…100）
/// - 起止可以倒着写（`[5-1]` 当 `[1-5]`）
/// - 一行里可以有**多个**区间，取笛卡尔积（`a[1-2]b[1-2]` ⇒ 4 条）
///
/// 认不出的一律原样返回 —— 宁可少展开，也不要把用户的 URL 改坏。
pub(crate) fn expand_ranges(line: &str) -> Vec<String> {
    // 先找出全部区间（byte 位置，方便原地替换）
    let mut spans: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(close_rel) = line[i..].find(']') else {
            break;
        };
        let close = i + close_rel;
        let inner = &line[i + 1..close];
        match range_values(inner) {
            // `Some(空)` 表示"是区间但超了上限" ⇒ 整行丢弃（交给调用方计数）。
            // 不能当成"认不出"混过去：那样 `[1-2000]` 会被原样当成一个 URL 收下。
            Some(values) if values.is_empty() => return Vec::new(),
            Some(values) => {
                spans.push((i, close + 1, values));
                i = close + 1;
            }
            None => i += 1,
        }
    }
    if spans.is_empty() {
        return vec![line.to_string()];
    }

    // 笛卡尔积：从右往左折叠，替换一处就复制一份
    let mut current: Vec<String> = vec![line.to_string()];
    for (start, end, values) in spans.iter().rev() {
        let mut next = Vec::new();
        for base in &current {
            for value in values {
                next.push(format!("{}{}{}", &base[..*start], value, &base[*end..]));
            }
            if next.len() > MAX_EXPAND {
                return Vec::new(); // 超上限：整行丢弃，由调用方计数
            }
        }
        current = next;
    }
    current
}

/// `1-5` / `001-100` ⇒ 展开后的字符串；不是区间就 `None`。
fn range_values(inner: &str) -> Option<Vec<String>> {
    let (from, to) = inner.split_once('-')?;
    let from = from.trim();
    let to = to.trim();
    let a: u64 = from.parse().ok()?;
    let b: u64 = to.parse().ok()?;
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if hi - lo >= MAX_EXPAND as u64 {
        return Some(Vec::new()); // 让调用方按"超限"处理
    }
    // 前导零：按起点写了几位就补几位（`001` ⇒ 3 位）
    let width = if from.len() > 1 && from.starts_with('0') {
        from.len()
    } else {
        0
    };
    Some((lo..=hi).map(|n| format!("{n:0width$}")).collect())
}

/// 从多行文本里摘出要下载的 URL。
///
/// 顺带的活：去掉首尾空白、跳过空行与 `#` 注释、展开 `[001-100]` 区间、
/// 去重（同一个链接粘两次只建一个任务）。
///
/// 返回 `(urls, 因超上限被丢弃的行数)` —— 后者要显示出来，
/// 否则用户会以为"我粘了 100 行怎么只有 30 条"。
pub(crate) fn parse_urls(text: &str) -> (Vec<String>, usize) {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !(line.starts_with("http://") || line.starts_with("https://")) {
            continue;
        }
        let expanded = expand_ranges(line);
        if expanded.is_empty() {
            dropped += 1;
            continue;
        }
        for url in expanded {
            if seen.insert(url.clone()) {
                out.push(url);
            }
        }
    }
    (out, dropped)
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
                        .color(super::theme::ink_3()),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::multiline(&mut self.basket_text)
                        .hint_text("https://example.com/a.zip")
                        .desired_width(f32::INFINITY)
                        .desired_rows(6),
                );
                ui.add_space(8.0);
                // 超上限被跳过的行数要说出来，否则用户会以为"我粘了 100 行怎么只有 30 条"。
                {
                    let (_, dropped) = parse_urls(&self.basket_text);
                    if dropped > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "有 {dropped} 行超过单行 {MAX_EXPAND} 条的上限，已跳过"
                            ))
                            .size(super::super::theme::FONT_LABEL)
                            .color(super::super::theme::warn()),
                        );
                    }
                }
                ui.horizontal(|ui| {
                    let (urls, _dropped) = parse_urls(&self.basket_text);
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
            let (urls, _) = parse_urls(&self.basket_text);
            for url in urls {
                self.send(Ticket::Add {
                    url: url.clone(),
                    // 名字交给队列从 URL 推导 —— 这里不猜，猜错了 E1/E2 还得再改一次名。
                    name: String::new(),
                    referer: None,
                    start_at: None,
                    on_complete: None,
                    // 批量/抓取入口不带凭证：这里只有 URL，没有浏览器的 cookie。
                    video: false,
                    cookie: None,
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
            parse_urls(text).0,
            vec![
                "https://a.example/1.zip".to_string(),
                "http://b.example/2.bin".to_string()
            ],
            "空行/注释/重复/非 http(s) 都要滤掉，且保持原顺序"
        );
    }

    #[test]
    fn nothing_usable_yields_no_tasks() {
        assert!(parse_urls("").0.is_empty());
        assert!(parse_urls("# 只有注释\n\n").0.is_empty());
        assert!(parse_urls("magnet:?xt=urn:btih:x").0.is_empty());
    }

    /// `[001-100]` 这种写法是 IDM 的经典批量形态，要能展开。
    #[test]
    fn bracket_ranges_expand_and_keep_leading_zeros() {
        use super::expand_ranges;
        assert_eq!(
            expand_ranges("https://x/f[1-3].zip"),
            vec!["https://x/f1.zip", "https://x/f2.zip", "https://x/f3.zip"]
        );
        // 前导零按起点写了几位就补几位
        let padded = expand_ranges("https://x/f[008-010].zip");
        assert_eq!(padded.len(), 3);
        assert_eq!(padded[0], "https://x/f008.zip");
        assert_eq!(padded[2], "https://x/f010.zip");
        // 倒着写当正着
        assert_eq!(
            expand_ranges("a[3-1]"),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()]
        );
        // 一行多处区间取笛卡尔积
        assert_eq!(expand_ranges("a[1-2]b[1-2]").len(), 4);
        // 认不出的原样返回，绝不改坏用户的 URL
        assert_eq!(expand_ranges("a[b-c].zip"), vec!["a[b-c].zip".to_string()]);
        assert_eq!(expand_ranges("a[1].zip"), vec!["a[1].zip".to_string()]);
        assert_eq!(expand_ranges("no-range"), vec!["no-range".to_string()]);
    }

    /// 超上限要**整行丢弃并计数**，否则 `[1-100000]` 会把队列塞爆。
    #[test]
    fn an_oversized_range_is_dropped_and_counted() {
        let text = "https://x/f[1-2000].zip\nhttps://y/ok.zip";
        let (urls, dropped) = parse_urls(text);
        assert_eq!(dropped, 1, "超限那一行要被计数");
        assert_eq!(urls, vec!["https://y/ok.zip".to_string()]);
    }
}
