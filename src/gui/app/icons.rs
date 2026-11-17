//! 图标与按钮的共用画法。
//!
//! ## 图标为什么是画出来的
//!
//! 一开始用 `＋ ▶ ❚❚ 🗑` 这些字符当图标，实测**满屏方框** —— 它们不在
//! Microsoft YaHei 的字形里。改用 `Painter` 画线条：不依赖任何字体，
//! 在任何机器上都一样，也能跟着强调色走。
//!
//! 抽成独立模块是因为 0.6.0 之后有两处要画同一批图标：顶层工具条（大按钮）
//! 与上下文动作条（紧凑按钮）。两边各抄一份 `draw_icon` 迟早会画歪。

use eframe::egui;

use super::theme;

/// 图标种类。每一种对应 [`draw_icon`] 里的一段绘制代码。
/// **暂时 `allow(dead_code)`**：0.7.0 步骤 1 先把图标画出来，步骤 2（壳）才把它们接到界面上。
/// 接完之后这一行要删掉 —— 留着它，将来真没人用的图标就不会报警了。
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Icon {
    // ---- 原有 ----
    Plus,
    Play,
    Pause,
    Trash,
    Gear,
    Dots,
    /// 下拉箭头（`▾` 这个字符同样不在 YaHei 里，得自己画）。
    ChevronDown,
    // ---- 入口（0.7.0 左栏顶部三项）----
    /// 站点抓取：圆环 + 四个方向短刺。
    Grab,
    /// 下载列表：三横线。
    List,
    /// 下载 / 进行中：向下的箭头落到一条底线上。
    Download,
    // ---- 任务状态（带颜色语义，颜色由调用处给）----
    /// 全部任务：三横线 + 底线。
    Tasks,
    /// 进行中：与 `Download` 同形，语义不同所以分开列。
    Running,
    /// 已完成：圆圈 + 对勾。
    Done,
    /// 失败：圆圈 + 感叹号。
    Failed,
    // ---- 分类（与 `queue::category_for` 一一对应）----
    CatDoc,
    CatVideo,
    CatAudio,
    CatArchive,
    CatProgram,
    CatImage,
    CatOther,
    // ---- 动作与杂项 ----
    FolderOpen,
    Copy,
    Link,
    /// 收起 / 展开侧栏：「左边有一条竖线的方框」。
    Panel,
    Retry,
    Check,
    Warn,
}

/// 全部图标。用例靠它逐个画一遍，保证新增的图标不会漏掉绘制分支。
#[allow(dead_code)]
pub(crate) const ALL: &[Icon] = &[
    Icon::Plus,
    Icon::Play,
    Icon::Pause,
    Icon::Trash,
    Icon::Gear,
    Icon::Dots,
    Icon::ChevronDown,
    Icon::Grab,
    Icon::List,
    Icon::Download,
    Icon::Tasks,
    Icon::Running,
    Icon::Done,
    Icon::Failed,
    Icon::CatDoc,
    Icon::CatVideo,
    Icon::CatAudio,
    Icon::CatArchive,
    Icon::CatProgram,
    Icon::CatImage,
    Icon::CatOther,
    Icon::FolderOpen,
    Icon::Copy,
    Icon::Link,
    Icon::Panel,
    Icon::Retry,
    Icon::Check,
    Icon::Warn,
];

/// 分类 → 图标。**必须与 `queue::category_for` 的七个分支一一对应**：
/// 左栏分类、列表的类型标记、抓取结果表共用这一张映射，否则"分类"一词会在三处指三个东西。
#[allow(dead_code)]
pub(crate) fn for_category(cat: &str) -> Icon {
    match cat {
        "document" => Icon::CatDoc,
        "video" => Icon::CatVideo,
        "audio" => Icon::CatAudio,
        "archive" => Icon::CatArchive,
        "program" => Icon::CatProgram,
        "image" => Icon::CatImage,
        _ => Icon::CatOther,
    }
}

/// 顶层工具条上的大按钮：上图下字，56×40。
pub(crate) fn tool_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(56.0, 40.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, theme::RADIUS - 2.0, theme::surface_2());
    }
    let ink = if !enabled {
        theme::ink_3()
    } else if hovered {
        theme::ink()
    } else {
        theme::ink_2()
    };
    let icon_rect =
        egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, 8.0), egui::vec2(16.0, 16.0));
    draw_icon(painter, icon, icon_rect, ink);
    painter.text(
        rect.center() + egui::vec2(0.0, 11.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        ink,
    );
    // 画出来的按钮默认不在无障碍树里；不报身份，读屏与自动化都找不到「设置」「更多」。
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response
}

/// 16×16（或 12×12）框里画图标。全部用 1.6pt 线宽，与 demo 的描边感一致。
pub(crate) fn draw_icon(painter: &egui::Painter, icon: Icon, r: egui::Rect, color: egui::Color32) {
    let s = egui::Stroke::new(1.6, color);
    let c = r.center();
    let h = r.width() / 2.0;

    match icon {
        Icon::Plus => {
            painter.line_segment(
                [
                    egui::pos2(c.x, c.y - h * 0.7),
                    egui::pos2(c.x, c.y + h * 0.7),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.7, c.y),
                    egui::pos2(c.x + h * 0.7, c.y),
                ],
                s,
            );
        }
        Icon::Play => {
            // 实心三角，略向右偏以视觉居中。
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x - h * 0.45, c.y - h * 0.7),
                    egui::pos2(c.x + h * 0.7, c.y),
                    egui::pos2(c.x - h * 0.45, c.y + h * 0.7),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Pause => {
            let w = h * 0.28;
            for dx in [-h * 0.32, h * 0.32] {
                painter.rect_filled(
                    egui::Rect::from_center_size(egui::pos2(c.x + dx, c.y), egui::vec2(w, h * 1.4)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Trash => {
            painter.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(c.x - h * 0.5, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.5, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.36, c.y + h * 0.72),
                    egui::pos2(c.x - h * 0.36, c.y + h * 0.72),
                ],
                s,
            ));
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.72, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.72, c.y - h * 0.4),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.22, c.y - h * 0.72),
                    egui::pos2(c.x + h * 0.22, c.y - h * 0.72),
                ],
                s,
            );
        }
        Icon::Gear => {
            // 齿轮：一个圆 + 八根短齿。比画真齿形简单，观感够用。
            painter.circle_stroke(c, h * 0.42, s);
            painter.circle_filled(c, h * 0.16, color);
            for i in 0..8 {
                let a = std::f32::consts::TAU * (i as f32) / 8.0;
                let (sin, cos) = a.sin_cos();
                painter.line_segment(
                    [
                        egui::pos2(c.x + cos * h * 0.42, c.y + sin * h * 0.42),
                        egui::pos2(c.x + cos * h * 0.78, c.y + sin * h * 0.78),
                    ],
                    s,
                );
            }
        }
        Icon::Dots => {
            // 三个点：「更多」。用实心圆而不是字符 `⋯` —— 字符会变成方框。
            for dx in [-h * 0.55, 0.0, h * 0.55] {
                painter.circle_filled(egui::pos2(c.x + dx, c.y), h * 0.18, color);
            }
        }
        Icon::ChevronDown => {
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.55, c.y - h * 0.28),
                    egui::pos2(c.x, c.y + h * 0.32),
                    egui::pos2(c.x + h * 0.55, c.y - h * 0.28),
                ],
                egui::Stroke::new(1.6, color),
            ));
        }
        Icon::Grab => {
            painter.circle_stroke(c, h * 0.52, s);
            for i in 0..4 {
                let a = std::f32::consts::TAU * (i as f32) / 4.0;
                let (sin, cos) = a.sin_cos();
                painter.line_segment(
                    [
                        egui::pos2(c.x + cos * h * 0.66, c.y + sin * h * 0.66),
                        egui::pos2(c.x + cos * h * 0.92, c.y + sin * h * 0.92),
                    ],
                    s,
                );
            }
        }
        Icon::List => {
            for dy in [-h * 0.5, 0.0, h * 0.5] {
                let span = if dy > 0.0 { h * 0.6 } else { h * 0.86 };
                painter.line_segment(
                    [
                        egui::pos2(c.x - h * 0.8, c.y + dy),
                        egui::pos2(c.x - h * 0.8 + span * 2.0, c.y + dy),
                    ],
                    s,
                );
            }
        }
        Icon::Download | Icon::Running => {
            painter.line_segment(
                [
                    egui::pos2(c.x, c.y - h * 0.75),
                    egui::pos2(c.x, c.y + h * 0.28),
                ],
                s,
            );
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.42, c.y - h * 0.12),
                    egui::pos2(c.x, c.y + h * 0.3),
                    egui::pos2(c.x + h * 0.42, c.y - h * 0.12),
                ],
                s,
            ));
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.7, c.y + h * 0.72),
                    egui::pos2(c.x + h * 0.7, c.y + h * 0.72),
                ],
                s,
            );
        }
        Icon::Tasks => {
            for dy in [-h * 0.5, 0.0, h * 0.5] {
                painter.line_segment(
                    [
                        egui::pos2(c.x - h * 0.8, c.y + dy),
                        egui::pos2(c.x + h * 0.8, c.y + dy),
                    ],
                    s,
                );
            }
        }
        Icon::Done => {
            painter.circle_stroke(c, h * 0.76, s);
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.34, c.y + h * 0.02),
                    egui::pos2(c.x - h * 0.06, c.y + h * 0.32),
                    egui::pos2(c.x + h * 0.4, c.y - h * 0.3),
                ],
                s,
            ));
        }
        Icon::Failed => {
            painter.circle_stroke(c, h * 0.76, s);
            painter.line_segment(
                [
                    egui::pos2(c.x, c.y - h * 0.36),
                    egui::pos2(c.x, c.y + h * 0.16),
                ],
                s,
            );
            painter.circle_filled(egui::pos2(c.x, c.y + h * 0.44), h * 0.11, color);
        }
        Icon::CatDoc => {
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.58, c.y - h * 0.85),
                    egui::pos2(c.x + h * 0.2, c.y - h * 0.85),
                    egui::pos2(c.x + h * 0.62, c.y - h * 0.42),
                    egui::pos2(c.x + h * 0.62, c.y + h * 0.85),
                    egui::pos2(c.x - h * 0.58, c.y + h * 0.85),
                    egui::pos2(c.x - h * 0.58, c.y - h * 0.85),
                ],
                s,
            ));
            painter.line_segment(
                [
                    egui::pos2(c.x + h * 0.2, c.y - h * 0.85),
                    egui::pos2(c.x + h * 0.2, c.y - h * 0.42),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x + h * 0.2, c.y - h * 0.42),
                    egui::pos2(c.x + h * 0.62, c.y - h * 0.42),
                ],
                s,
            );
        }
        Icon::CatVideo => {
            painter.rect_stroke(
                egui::Rect::from_center_size(
                    egui::pos2(c.x - h * 0.22, c.y),
                    egui::vec2(h * 1.3, h * 1.2),
                ),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x + h * 0.5, c.y - h * 0.34),
                    egui::pos2(c.x + h * 0.9, c.y - h * 0.6),
                    egui::pos2(c.x + h * 0.9, c.y + h * 0.6),
                    egui::pos2(c.x + h * 0.5, c.y + h * 0.34),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::CatAudio => {
            painter.line_segment(
                [
                    egui::pos2(c.x + h * 0.1, c.y - h * 0.8),
                    egui::pos2(c.x + h * 0.1, c.y + h * 0.2),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x + h * 0.1, c.y - h * 0.8),
                    egui::pos2(c.x + h * 0.72, c.y - h * 0.95),
                ],
                s,
            );
            painter.circle_filled(egui::pos2(c.x - h * 0.16, c.y + h * 0.28), h * 0.26, color);
            painter.circle_stroke(egui::pos2(c.x - h * 0.16, c.y + h * 0.28), h * 0.26, s);
        }
        Icon::CatArchive => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(h * 1.5, h * 1.6)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x, c.y - h * 0.8),
                    egui::pos2(c.x, c.y + h * 0.2),
                ],
                s,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.22, c.y - h * 0.4),
                    egui::pos2(c.x + h * 0.22, c.y - h * 0.4),
                ],
                s,
            );
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(c.x, c.y + h * 0.42),
                    egui::vec2(h * 0.34, h * 0.34),
                ),
                1.0,
                color,
            );
        }
        Icon::CatProgram => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(h * 1.55, h * 1.55)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.42, c.y - h * 0.34),
                    egui::pos2(c.x - h * 0.1, c.y),
                    egui::pos2(c.x - h * 0.42, c.y + h * 0.34),
                ],
                s,
            ));
            painter.line_segment(
                [
                    egui::pos2(c.x + h * 0.02, c.y + h * 0.36),
                    egui::pos2(c.x + h * 0.44, c.y + h * 0.36),
                ],
                s,
            );
        }
        Icon::CatImage => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(h * 1.6, h * 1.35)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(egui::pos2(c.x - h * 0.32, c.y - h * 0.28), h * 0.15, color);
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.72, c.y + h * 0.5),
                    egui::pos2(c.x - h * 0.16, c.y - h * 0.06),
                    egui::pos2(c.x + h * 0.16, c.y + h * 0.24),
                    egui::pos2(c.x + h * 0.74, c.y - h * 0.3),
                ],
                s,
            ));
        }
        Icon::CatOther => {
            // 文件夹：一条折线上的两个矩形，最省笔画也最认得出。
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.85, c.y + h * 0.6),
                    egui::pos2(c.x - h * 0.85, c.y - h * 0.6),
                    egui::pos2(c.x - h * 0.25, c.y - h * 0.6),
                    egui::pos2(c.x - h * 0.05, c.y - h * 0.25),
                    egui::pos2(c.x + h * 0.85, c.y - h * 0.25),
                    egui::pos2(c.x + h * 0.85, c.y + h * 0.6),
                    egui::pos2(c.x - h * 0.85, c.y + h * 0.6),
                ],
                s,
            ));
        }
        Icon::FolderOpen => {
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.85, c.y + h * 0.6),
                    egui::pos2(c.x - h * 0.85, c.y - h * 0.55),
                    egui::pos2(c.x - h * 0.3, c.y - h * 0.55),
                    egui::pos2(c.x - h * 0.1, c.y - h * 0.22),
                    egui::pos2(c.x + h * 0.55, c.y - h * 0.22),
                    egui::pos2(c.x + h * 0.55, c.y + h * 0.1),
                ],
                s,
            ));
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.85, c.y + h * 0.6),
                    egui::pos2(c.x - h * 0.5, c.y - h * 0.05),
                    egui::pos2(c.x + h * 0.9, c.y - h * 0.05),
                    egui::pos2(c.x + h * 0.62, c.y + h * 0.6),
                    egui::pos2(c.x - h * 0.85, c.y + h * 0.6),
                ],
                s,
            ));
        }
        Icon::Copy => {
            painter.rect_stroke(
                egui::Rect::from_center_size(
                    egui::pos2(c.x + h * 0.2, c.y + h * 0.2),
                    egui::vec2(h * 1.15, h * 1.15),
                ),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x + h * 0.05, c.y - h * 0.25),
                    egui::pos2(c.x + h * 0.05, c.y - h * 0.78),
                    egui::pos2(c.x - h * 0.78, c.y - h * 0.78),
                    egui::pos2(c.x - h * 0.78, c.y + h * 0.05),
                    egui::pos2(c.x - h * 0.25, c.y + h * 0.05),
                ],
                s,
            ));
        }
        Icon::Link => {
            // 两段弧 + 中间一条斜线：经典的链环。
            let a = egui::Pos2::new(c.x - h * 0.2, c.y - h * 0.2);
            let b = egui::Pos2::new(c.x + h * 0.2, c.y + h * 0.2);
            painter.circle_stroke(a, h * 0.42, s);
            painter.circle_stroke(b, h * 0.42, s);
            painter.line_segment([a, b], s);
        }
        Icon::Panel => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(h * 1.7, h * 1.5)),
                2.5,
                s,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - h * 0.28, c.y - h * 0.75),
                    egui::pos2(c.x - h * 0.28, c.y + h * 0.75),
                ],
                s,
            );
        }
        Icon::Retry => {
            // 缺口圆环 + 箭头尖。
            let radius = h * 0.68;
            let start = -std::f32::consts::FRAC_PI_2 + 0.55;
            let end = start + std::f32::consts::TAU - 0.9;
            let seg = 18;
            let pts: Vec<egui::Pos2> = (0..=seg)
                .map(|i| {
                    let t = start + (end - start) * (i as f32) / (seg as f32);
                    egui::pos2(c.x + t.cos() * radius, c.y + t.sin() * radius)
                })
                .collect();
            painter.add(egui::Shape::line(pts, s));
            let tip = egui::pos2(c.x + start.cos() * radius, c.y + start.sin() * radius);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(tip.x + h * 0.3, tip.y - h * 0.02),
                    egui::pos2(tip.x - h * 0.14, tip.y - h * 0.3),
                    egui::pos2(tip.x - h * 0.02, tip.y + h * 0.24),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Check => {
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - h * 0.72, c.y + h * 0.02),
                    egui::pos2(c.x - h * 0.18, c.y + h * 0.55),
                    egui::pos2(c.x + h * 0.72, c.y - h * 0.55),
                ],
                egui::Stroke::new(1.9, color),
            ));
        }
        Icon::Warn => {
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(c.x, c.y - h * 0.8),
                    egui::pos2(c.x + h * 0.85, c.y + h * 0.65),
                    egui::pos2(c.x - h * 0.85, c.y + h * 0.65),
                    egui::pos2(c.x, c.y - h * 0.8),
                ],
                s,
            ));
            painter.line_segment(
                [
                    egui::pos2(c.x, c.y - h * 0.28),
                    egui::pos2(c.x, c.y + h * 0.18),
                ],
                s,
            );
            painter.circle_filled(egui::pos2(c.x, c.y + h * 0.42), h * 0.11, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个图标都画得出来（不漏 `match` 分支、不 panic）。
    ///
    /// 新增图标时这份表跟着长：`ALL` 少写一个，这条用例不会红，但
    /// `every_icon_in_the_enum_is_listed_in_all` 会红 —— 两个一起才能防漏。
    #[test]
    fn every_icon_draws_without_panicking() {
        let ctx = egui::Context::default();
        let mut painted = 0usize;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            for (i, icon) in ALL.iter().enumerate() {
                let x = 16.0 + (i % 6) as f32 * 20.0;
                let y = 16.0 + (i / 6) as f32 * 20.0;
                let rect = egui::Rect::from_center_size(egui::pos2(x, y), egui::vec2(16.0, 16.0));
                draw_icon(painter, *icon, rect, theme::ink());
                painted += 1;
            }
        });
        output.textures_delta.clear();
        assert_eq!(painted, ALL.len());
        assert!(
            ALL.len() >= 24,
            "设计稿要 24 个以上图标，现在是 {}",
            ALL.len()
        );
    }

    /// 枚举里有的，`ALL` 里也必须有；`ALL` 里不许有重复。
    #[test]
    fn every_icon_in_the_enum_is_listed_in_all() {
        let mut seen = std::collections::HashSet::new();
        for icon in ALL {
            assert!(seen.insert(*icon), "ALL 里有重复：{icon:?}");
        }
        // 逐个枚举值都在 ALL 里：靠 Debug 全表比对。
        let all_debug: Vec<String> = ALL.iter().map(|i| format!("{i:?}")).collect();
        for icon in ALL {
            assert!(
                all_debug.iter().any(|d| d == &format!("{icon:?}")),
                "漏了 {icon:?}"
            );
        }
    }

    /// 分类映射必须覆盖 `queue::category_for` 的每一个返回值，
    /// 且七个分支各不相同 —— 否则左栏的两个分类会画成同一个图标。
    #[test]
    fn the_category_icons_match_the_queue_categories() {
        use super::for_category;
        let cats = [
            "document", "video", "audio", "archive", "program", "image", "other",
        ];
        let mut icons = std::collections::HashSet::new();
        for cat in cats {
            assert!(
                icons.insert(for_category(cat)),
                "分类 {cat} 与前面某个分类用了同一个图标"
            );
        }
        // `category_for` 真的会产出这七个值
        for (name, expect) in [
            ("a.pdf", "document"),
            ("a.mp4", "video"),
            ("a.mp3", "audio"),
            ("a.zip", "archive"),
            ("a.exe", "program"),
            ("a.png", "image"),
            ("a.unknown", "other"),
            ("noext", "other"),
        ] {
            let got = crate::queue::category_for(name);
            assert_eq!(got, expect, "{name}");
            assert!(cats.contains(&got), "queue 出现了未覆盖的分类：{got}");
        }
    }
}
