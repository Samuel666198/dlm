//! 原生外壳的视觉常量。
//!
//! 全部按 **point** 写，不是 px（勘误 E12）。egui 的 `pixels_per_point` 在 150%
//! 缩放下会把 point 换成像素，写死 px 会直接错位。
//!
//! 配色取自 `docs/mockups/2026-11-09-native-ui.html`：暖纸底 + 陶土橙单强调色。
//! 只用纯色、1px 边框、圆角 —— 这些 egui 做得到；毛玻璃/渐变/复杂投影一律不用。

use eframe::egui::Color32;

// ---------------------------------------------------------------- 尺寸（point）

/// 标题栏高度。自绘窗口用（见勘误 E10：默认走原生装饰，这块先留着）。
pub const TITLEBAR_H: f32 = 34.0;
/// 工具条高度。
pub const TOOLBAR_H: f32 = 50.0;
/// 状态栏高度。
pub const STATUS_H: f32 = 32.0;
/// 左侧分类栏宽度（D6：保留）。
pub const SIDEBAR_W: f32 = 212.0;
/// 详情抽屉宽度。
pub const DRAWER_W: f32 = 372.0;
/// 任务行高度。
pub const ROW_H: f32 = 52.0;
/// 表头高度。
pub const THEAD_H: f32 = 34.0;
/// 统一圆角。
pub const RADIUS: f32 = 9.0;
/// 描边宽度。
pub const STROKE: f32 = 1.0;

/// 窗口初始尺寸（point）。
pub const WINDOW_DEFAULT: [f32; 2] = [1120.0, 700.0];
/// 窗口最小尺寸（point）：再窄列表就放不下列了。
pub const WINDOW_MIN: [f32; 2] = [760.0, 480.0];

// ---------------------------------------------------------------- 颜色

/// 窗口底色：暖纸。
pub const PAPER: Color32 = Color32::from_rgb(0xF6, 0xF3, 0xEC);
/// 列表、抽屉底色。
pub const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFD, 0xF8);
/// 侧栏、状态栏、标题栏底色。
pub const SURFACE_2: Color32 = Color32::from_rgb(0xF1, 0xED, 0xE4);
/// 主描边。
pub const LINE: Color32 = Color32::from_rgb(0xE4, 0xDF, 0xD2);
/// 弱描边（行分隔）。
pub const LINE_SOFT: Color32 = Color32::from_rgb(0xED, 0xE8, 0xDC);
/// 正文。
pub const INK: Color32 = Color32::from_rgb(0x2A, 0x27, 0x20);
/// 次要文字。
pub const INK_2: Color32 = Color32::from_rgb(0x6C, 0x68, 0x60);
/// 弱化文字：占位、表头。
pub const INK_3: Color32 = Color32::from_rgb(0x9C, 0x97, 0x8D);
/// 强调色：陶土橙。只用于主按钮、进度、选中态。
pub const ACCENT: Color32 = Color32::from_rgb(0xC0, 0x63, 0x40);
/// 强调色按下态。
pub const ACCENT_INK: Color32 = Color32::from_rgb(0xA3, 0x4F, 0x31);
/// 强调色淡底。
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xF2, 0xE1, 0xD7);
/// 成功：已完成。
pub const OK: Color32 = Color32::from_rgb(0x4E, 0x7A, 0x5C);
pub const OK_SOFT: Color32 = Color32::from_rgb(0xE4, 0xED, 0xE4);
/// 警告：等待、暂停。
pub const WARN: Color32 = Color32::from_rgb(0xA9, 0x76, 0x2A);
pub const WARN_SOFT: Color32 = Color32::from_rgb(0xF3, 0xEA, 0xD7);
/// 失败。
pub const BAD: Color32 = Color32::from_rgb(0xA2, 0x44, 0x3A);
pub const BAD_SOFT: Color32 = Color32::from_rgb(0xF4, 0xE2, 0xDF);
/// 进度槽底。
pub const TRACK: Color32 = Color32::from_rgb(0xE7, 0xE2, 0xD5);

// ---------------------------------------------------------------- 字体

/// 正文与控件用的无衬线。Windows 上 `Segoe UI Variable` 从 Win11 才有，
/// 退到 `Segoe UI`；中文走 `Microsoft YaHei UI`。
pub const SANS: &str = "Segoe UI";
/// 数字列用的等宽。下载器天天盯数字，列不齐最难受。
pub const MONO: &str = "Consolas";

// ---------------------------------------------------------------- 状态色

/// 状态字符串 → (文字, 前景色, 底色)。
///
/// `TaskView.status` 是 `String`（HTTP 那边要序列化它），所以这里按字符串匹配，
/// 不用 `TaskStatus`。UI 与托盘 tooltip 共用这一份。
pub fn status_style(status: &str) -> (&'static str, Color32, Color32) {
    match status {
        "running" => ("下载中", ACCENT_INK, ACCENT_SOFT),
        "queued" => ("排队中", WARN, WARN_SOFT),
        "paused" => ("已暂停", WARN, WARN_SOFT),
        "done" => ("已完成", OK, OK_SOFT),
        "failed" => ("失败", BAD, BAD_SOFT),
        _ => ("未知", INK_3, SURFACE_2),
    }
}

/// 把 egui 的视觉风格调成 demo 那套：去掉默认的蓝色高亮、圆角收小、
/// 描边统一成 1pt 暖灰。
pub fn apply(ctx: &eframe::egui::Context) {
    use eframe::egui::{Stroke, Visuals};

    let mut visuals = Visuals::light();
    visuals.panel_fill = PAPER;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = SURFACE_2;
    visuals.faint_bg_color = SURFACE_2;
    visuals.override_text_color = Some(INK);

    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(STROKE, LINE);

    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(STROKE, LINE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(STROKE, INK_2);

    visuals.widgets.hovered.bg_fill = SURFACE_2;
    visuals.widgets.hovered.bg_stroke = Stroke::new(STROKE, LINE);
    visuals.widgets.hovered.fg_stroke = Stroke::new(STROKE, INK);

    visuals.widgets.active.bg_fill = ACCENT_SOFT;
    visuals.widgets.active.bg_stroke = Stroke::new(STROKE, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(STROKE, ACCENT_INK);

    visuals.selection.bg_fill = ACCENT_SOFT;
    visuals.selection.stroke = Stroke::new(STROKE, ACCENT_INK);

    ctx.set_visuals(visuals);
}
