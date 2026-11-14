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
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Plus,
    Play,
    Pause,
    Trash,
    Gear,
    Dots,
    /// 下拉箭头（`▾` 这个字符同样不在 YaHei 里，得自己画）。
    ChevronDown,
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
        painter.rect_filled(rect, theme::RADIUS - 2.0, theme::SURFACE_2);
    }
    let ink = if !enabled {
        theme::INK_3
    } else if hovered {
        theme::INK
    } else {
        theme::INK_2
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
    response
}

/// 上下文动作条上的紧凑按钮：左图右字，78×26。
///
/// 比 [`tool_button`] 矮一半：动作条整行只有 40pt，再塞 40pt 的大按钮会顶满，
/// 「已选 N 项」那行字就没地方站了。
pub(crate) fn action_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    danger: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(78.0, 26.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let painter = ui.painter();
    // 描边 + 悬停淡底：动作条是浅蓝色底，按钮要能被看出来是钮。
    painter.rect(
        rect,
        theme::RADIUS - 3.0,
        if hovered {
            theme::SURFACE
        } else {
            egui::Color32::TRANSPARENT
        },
        egui::Stroke::new(
            theme::STROKE,
            if danger && enabled {
                egui::Color32::from_rgb(0xF3, 0xCD, 0xC7)
            } else {
                theme::LINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    let ink = if !enabled {
        theme::INK_3
    } else if danger {
        theme::BAD
    } else {
        theme::ACCENT_INK
    };
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        egui::vec2(12.0, 12.0),
    );
    draw_icon(painter, icon, icon_rect, ink);
    painter.text(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        ink,
    );
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
    }
}
