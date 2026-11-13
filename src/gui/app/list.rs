//! 任务列表：表头 + 行 + 进度条。虚拟滚动由 `ScrollArea::show_rows` 负责。

use eframe::egui;

use super::App;
use crate::gui::app::theme;
use crate::gui::snapshot::TaskView;

/// 列宽（point）。与 demo 对齐：文件名弹性，其余定宽。
const COL_SIZE: f32 = 92.0;
const COL_PROGRESS: f32 = 150.0;
const COL_SPEED: f32 = 96.0;
const COL_ETA: f32 = 84.0;
const COL_STATUS: f32 = 88.0;
const GAP: f32 = 12.0;
const PAD: f32 = 16.0;

impl App {
    pub(super) fn list(&mut self, ui: &mut egui::Ui) {
        let all = self.tasks();
        let rows: Vec<TaskView> = all.iter().filter(|t| self.visible(t)).cloned().collect();

        self.thead(ui);

        if rows.is_empty() {
            self.empty_state(ui);
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, theme::ROW_H, rows.len(), |ui, range| {
                for i in range {
                    self.row(ui, &rows[i]);
                }
            });
    }

    /// 一行任务。点击选中并展开详情。
    fn row(&mut self, ui: &mut egui::Ui, task: &TaskView) {
        let selected = self.selected.as_deref() == Some(task.id.as_str());
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::ROW_H),
            egui::Sense::click(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }

        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 0.0, eframe::egui::Color32::from_rgb(0xFB, 0xF6, 0xF0));
            // 左侧 2.5pt 强调条。
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(0.0, 8.0),
                    egui::vec2(2.5, rect.height() - 16.0),
                ),
                2.0,
                theme::ACCENT,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, eframe::egui::Color32::from_rgb(0xFB, 0xF8, 0xF2));
        }
        // 行分隔线。
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::vec2(rect.width(), 1.0),
            ),
            0.0,
            theme::LINE_SOFT,
        );

        // 从右往左排定宽列，剩下的给文件名。
        let mut right = rect.right() - PAD;
        let mut col = |w: f32| -> egui::Rect {
            let r = egui::Rect::from_min_max(
                egui::pos2(right - w, rect.top()),
                egui::pos2(right, rect.bottom()),
            );
            right -= w + GAP;
            r
        };

        let status_rect = col(COL_STATUS);
        let eta_rect = col(COL_ETA);
        let speed_rect = col(COL_SPEED);
        let progress_rect = col(COL_PROGRESS);
        let size_rect = col(COL_SIZE);
        let name_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + PAD, rect.top()),
            egui::pos2(right, rect.bottom()),
        );

        self.draw_name(ui, name_rect, task);
        text_right(
            ui,
            size_rect,
            &fmt_bytes_opt(task.total_len),
            theme::INK_2,
            12.0,
        );
        draw_progress(ui, progress_rect, task);
        let speed = self.speed_of(&task.id);
        text_right(ui, speed_rect, &speed_text(task, speed), theme::INK_2, 12.0);
        text_right(ui, eta_rect, &eta_text(task, speed), theme::INK_3, 12.0);
        draw_status(ui, status_rect, task);

        if response.clicked() {
            self.selected = Some(task.id.clone());
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
            theme::INK,
        );
        // 副标题：host 或落点目录。
        painter.text(
            rect.left_top() + egui::vec2(0.0, rect.height() / 2.0 + 8.0),
            egui::Align2::LEFT_CENTER,
            elide(&host_of(&task.url), 56),
            egui::FontId::proportional(11.0),
            theme::INK_3,
        );
    }

    fn thead(&self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::THEAD_H),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, theme::SURFACE);
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::vec2(rect.width(), 1.0),
            ),
            0.0,
            theme::LINE,
        );

        let mut right = rect.right() - PAD;
        let mut col = |w: f32| -> egui::Rect {
            let r = egui::Rect::from_min_max(
                egui::pos2(right - w, rect.top()),
                egui::pos2(right, rect.bottom()),
            );
            right -= w + GAP;
            r
        };
        let status_rect = col(COL_STATUS);
        let eta_rect = col(COL_ETA);
        let speed_rect = col(COL_SPEED);
        let progress_rect = col(COL_PROGRESS);
        let size_rect = col(COL_SIZE);
        let name_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + PAD, rect.top()),
            egui::pos2(right, rect.bottom()),
        );

        let font = egui::FontId::proportional(10.5);
        let ink = theme::INK_3;
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
        painter.text(
            speed_rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            "速度",
            font.clone(),
            ink,
        );
        painter.text(
            eta_rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            "剩余",
            font.clone(),
            ink,
        );
        painter.text(
            status_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "状态",
            font,
            ink,
        );
    }

    fn empty_state(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(120.0);
            ui.label(egui::RichText::new("↓").size(44.0).color(theme::INK_3));
            ui.add_space(9.0);
            ui.label(
                egui::RichText::new("队列是空的")
                    .size(13.0)
                    .strong()
                    .color(theme::INK_2),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "点左上角「新建任务」，或在浏览器里右键链接交给 dlm。\n\
                     扩展会在页面上留一个悬浮球，随时能投递。",
                )
                .size(12.0)
                .color(theme::INK_3),
            );
        });
    }

    /// 筛选：状态键与分类键共用一个字段，先按状态再按分类判断。
    fn visible(&self, task: &TaskView) -> bool {
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
    painter.rect_filled(track_rect, 99.0, theme::TRACK);

    let pct = (task.progress / 100.0).clamp(0.0, 1.0) as f32;
    if pct > 0.0 {
        let fill = egui::Rect::from_min_size(
            track_rect.min,
            egui::vec2(track_rect.width() * pct, track_h),
        );
        let color = if task.status == "done" {
            theme::OK
        } else {
            theme::ACCENT
        };
        painter.rect_filled(fill, 99.0, color);
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
        theme::INK_2,
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
