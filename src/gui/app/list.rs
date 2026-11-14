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

/// 进度条至少这么宽才有信息量（再窄就只剩一条线）。
const COL_PROGRESS_MIN: f32 = 64.0;
/// 文件名列的最小可读宽度。
const NAME_MIN: f32 = 150.0;

/// 一行的列矩形。
///
/// **行与表头共用这一份排布**：两边各算一次，迟早会错位 —— 0.7.0 把详情改成右栏之后，
/// 列表可用宽度少了 330pt，行与表头的算法只要有一处没跟上，进度条就会压到文件名上。
///
/// 放不下时**先从右边丢掉「剩余」「速度」两列**：这两列多数时候显示 `—`，
/// 而文件名与进度每行都要看。
struct Columns {
    name: egui::Rect,
    size: egui::Rect,
    progress: egui::Rect,
    speed: Option<egui::Rect>,
    eta: Option<egui::Rect>,
    status: egui::Rect,
}

/// 按可用宽度排出这一行的各列位置。
fn columns(rect: egui::Rect) -> Columns {
    let inner = rect.width() - PAD * 2.0;
    // 满编需要的宽度：五列 + 五个间隔 + 文件名最小宽。
    let full = COL_SIZE + COL_PROGRESS + COL_SPEED + COL_ETA + COL_STATUS + GAP * 5.0 + NAME_MIN;
    // 只留「大小 + 进度(最小) + 状态」时需要的宽度。
    let lean = COL_SIZE + COL_PROGRESS_MIN + COL_STATUS + GAP * 3.0 + NAME_MIN;
    let show_eta = inner >= full;
    let show_speed = inner >= full - COL_ETA - GAP;

    let mut right = rect.right() - PAD;
    let mut col = |w: f32| -> egui::Rect {
        let r = egui::Rect::from_min_max(
            egui::pos2(right - w, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        right -= w + GAP;
        r
    };

    let status = col(COL_STATUS);
    let eta = if show_eta { Some(col(COL_ETA)) } else { None };
    let speed = if show_speed {
        Some(col(COL_SPEED))
    } else {
        None
    };
    // 进度条吃掉能给的宽度，但不超设计值、不低于最小值。
    let progress_w = if show_speed {
        COL_PROGRESS
    } else {
        (inner - lean + COL_PROGRESS_MIN).clamp(COL_PROGRESS_MIN, COL_PROGRESS)
    };
    let progress = col(progress_w);
    let size = col(COL_SIZE);
    let name = egui::Rect::from_min_max(
        egui::pos2(rect.left() + PAD, rect.top()),
        egui::pos2(right, rect.bottom()),
    );
    Columns {
        name,
        size,
        progress,
        speed,
        eta,
        status,
    }
}

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

        let cols = columns(rect);
        let name_rect = cols.name;
        let size_rect = cols.size;
        let progress_rect = cols.progress;
        let status_rect = cols.status;

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
        if let Some(speed_rect) = cols.speed {
            text_right(ui, speed_rect, &speed_text(task, speed), theme::INK_2, 12.0);
        }
        if let Some(eta_rect) = cols.eta {
            text_right(ui, eta_rect, &eta_text(task, speed), theme::INK_3, 12.0);
        }
        draw_status(ui, status_rect, task);

        // 告诉无障碍树"这一行是个可点的项"。**画出来的控件默认不在树里** ——
        // 不写这一句，读屏软件读不到列表，UI 自动化也点不到任何一行
        // （0.7.0 出证据时卡在这：headless 禁物理点击，而树里没有行）。
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!(
                    "{} · {} · {}",
                    task.name,
                    task.status,
                    fmt_bytes_opt(task.total_len)
                ),
            )
        });
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

        let cols = columns(rect);
        let name_rect = cols.name;
        let size_rect = cols.size;
        let progress_rect = cols.progress;
        let status_rect = cols.status;

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
        if let Some(speed_rect) = cols.speed {
            painter.text(
                speed_rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                "速度",
                font.clone(),
                ink,
            );
        }
        if let Some(eta_rect) = cols.eta {
            painter.text(
                eta_rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                "剩余",
                font.clone(),
                ink,
            );
        }
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

    /// 筛选：状态键与分类键共用一个字段，先按状态再按分类判断；搜索是**与**关系。
    fn visible(&self, task: &TaskView) -> bool {
        if !matches_search(task, &self.search) {
            return false;
        }
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

/// 搜索匹配：名字和 URL 都参与，大小写不敏感；空串（或全空白）全部通过。
pub(crate) fn matches_search(task: &TaskView, needle: &str) -> bool {
    let n = needle.trim();
    if n.is_empty() {
        return true;
    }
    let n = n.to_lowercase();
    task.name.to_lowercase().contains(&n) || task.url.to_lowercase().contains(&n)
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
#[cfg(test)]
mod tests {
    use super::columns;
    use eframe::egui;

    /// **列不许重叠**（0.7.0 详情改成右栏之后踩到的就是这个）。
    ///
    /// 宽窗口下五列齐备、彼此不压；窄窗口下先丢掉「剩余」「速度」，
    /// 文件名仍不少于 `NAME_MIN` —— 而不是让进度条骑到文件名上。
    #[test]
    fn columns_never_overlap_and_drop_the_least_useful_when_narrow() {
        let probe = |w: f32| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, 44.0));
            columns(rect)
        };

        // 宽：五列齐备，且从左到右严格递增、互不重叠。
        let wide = probe(1100.0);
        assert!(
            wide.speed.is_some() && wide.eta.is_some(),
            "1100 宽时应当五列齐备"
        );
        // 列是**从右往左**排出来的，所以按从右到左的顺序检查：每列的左边界
        // 必须不小于前一个（更靠右的）列的右边界。
        let right_to_left = [
            Some(wide.status),
            wide.eta,
            wide.speed,
            Some(wide.progress),
            Some(wide.size),
            Some(wide.name),
        ];
        let mut prev_left = f32::INFINITY;
        for r in right_to_left.into_iter().flatten() {
            assert!(
                r.right() <= prev_left + 0.01,
                "列重叠了：右边界 {} > 前一列左边界 {prev_left}",
                r.right()
            );
            prev_left = r.left();
        }

        // 窄（右栏打开、窗口不大）：丢掉「剩余」，其余各列仍不重叠。
        let narrow = probe(560.0);
        assert!(narrow.eta.is_none(), "560 宽时「剩余」应当先被丢掉");
        assert!(
            narrow.name.width() >= 150.0 - 0.5,
            "文件名列被压到 {} 宽，低于可读下限",
            narrow.name.width()
        );
        assert!(
            narrow.name.right() <= narrow.size.left() + 0.01,
            "文件名与大小列重叠"
        );
        assert!(
            narrow.size.right() <= narrow.progress.left() + 0.01,
            "大小压到进度上"
        );
    }
}
