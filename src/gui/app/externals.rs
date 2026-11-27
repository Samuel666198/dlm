//! 「外部下载」条带：不属于队列、但仍然在这台机器上下载的活儿。
//!
//! ## 为什么需要它
//!
//! 2026-11-27 用户问「插件请求的按理桌面端该有记录吧」。答案是：**只有 GUI 自己派发的任务
//! 才进 `queue.json`**。浏览器在 GUI 没开时会走宿主直连（现在会先尝试拉起 GUI，拉不起来才
//! 直连），HLS 也永远走直连 —— 这些下载在队列里没有记录，用户只能看扩展的悬浮球。
//!
//! 但它们的**断点文件是共用的**：`<落点>/<名>.dlm.json`。所以桌面端可以被动发现它们：
//! 扫落点目录就够了，不用改宿主、也不用让 CLI 学会写队列（那是 `FLAG_ADD_ONLY` 的历史欠账，
//! 两个进程写同一份队列的并发风险不值得）。
//!
//! ## 边界
//!
//! - **只读**：不给暂停/续传。那是队列任务的语义，外部进程的命令行不认我们的暂停信号。
//! - 只看得到**进行中/失败**的：成功收尾会删断点文件（`--keep-state` 除外），所以下完就没影了。
//! - 队列里已有的同名任务不算外部（那可能就是 GUI 自己在下）。

use eframe::egui;

use crate::queue::External;

use super::{list, theme};

/// 最多平铺几条；再多就折叠成一句，别把列表区挤没了。
const MAX_ROWS: usize = 3;

/// 排掉队列里已经有的同名任务 —— 那可能就是 GUI 自己在下，断点文件当然也在。
/// 不排掉的话，同一个文件会同时出现在「任务列表」和「外部下载」两处。
pub(crate) fn without_known(
    items: Vec<External>,
    known: &std::collections::HashSet<String>,
) -> Vec<External> {
    items
        .into_iter()
        .filter(|e| !known.contains(&e.name))
        .collect()
}

/// 画那条条带。`items` 为空时**不占任何高度**（调用方也据此决定要不要开面板）。
pub(super) fn strip(ui: &mut egui::Ui, items: &[External]) {
    if items.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("外部下载 {}", items.len()))
                .size(10.0)
                .strong()
                .color(theme::ink_2()),
        );
        ui.label(
            egui::RichText::new("不属于队列，只读（浏览器直连 / HLS）")
                .size(10.0)
                .color(theme::ink_3()),
        );
    });

    for item in items.iter().take(MAX_ROWS) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(egui::RichText::new(&item.name).size(11.0));
            ui.label(
                egui::RichText::new(format!(
                    "{} · {:.0}%",
                    list::fmt_bytes(item.delivered),
                    item.percent()
                ))
                .size(10.0)
                .color(theme::ink_3()),
            );
        });
        bar(ui, item);
    }

    if items.len() > MAX_ROWS {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(format!("…还有 {} 条", items.len() - MAX_ROWS))
                    .size(10.0)
                    .color(theme::ink_3()),
            );
        });
    }
    ui.add_space(4.0);
}

/// 细进度条：已覆盖的区间画实色，未覆盖留底色。
///
/// 用的是与列表、抽屉**同一套** `segment_bar::spans`（闭区间相邻也合并），
/// 所以三处看到的"块"是一致的。
fn bar(ui: &mut egui::Ui, item: &External) {
    let width = (ui.available_width() - 24.0).max(40.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 6.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::track());
    for (from, to) in super::segment_bar::spans(item.total, &item.segments, rect.width()) {
        let piece = egui::Rect::from_min_max(
            egui::pos2(rect.left() + from, rect.top()),
            egui::pos2(rect.left() + to, rect.bottom()),
        );
        painter.rect_filled(piece, 1.0, theme::accent());
    }
}
