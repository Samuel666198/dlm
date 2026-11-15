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
/// 上下文动作条高度（只在选中任务时占位）。
pub const CONTEXT_BAR_H: f32 = 38.0;
/// 状态栏高度。
pub const STATUS_H: f32 = 32.0;
/// E19：剪贴板提示条高度。只在有命中时占位，所以这个数只影响那一条。
pub const CLIPBOARD_HINT_H: f32 = 32.0;
/// 左侧分类栏宽度（D6：保留）。
pub const SIDEBAR_W: f32 = 212.0;

/// 侧栏收起后的宽度：只留一条图标。
///
/// 52 = 16（图标）+ 两侧各 18 的留白。再窄就点不准，再宽就白占地方。
pub const SIDEBAR_MINI_W: f32 = 52.0;
/// 详情抽屉宽度。
/// 右栏详情宽度。
///
/// 浮窗时代是 372（浮窗可以叠在内容上，宽点无妨）；改成右栏后它要**从列表里让出**这么多，
/// 372 会让"文件名"列明显变窄，330 是量与实测之间的折中。
pub const DRAWER_W: f32 = 330.0;
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

// ---------------------------------------------------------------- 取色（明/暗）
//
// 色值不能是裸 `const`：明暗两套要能切换。做法是一个不可变的结构 + 一个全局开关，
// 调用处写 `theme::paper()`。**带下划线的函数名与旧常量一一对应**，替换是机械的，
// 编译器会替我们核对每一处。

use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(false);

/// 当前是不是深色。界面层只需在切换时调 [`set_dark`]。
pub fn set_dark(on: bool) {
    DARK.store(on, Ordering::Relaxed);
}

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

/// 一套配色。字段名 = 旧常量名的小写。
struct Palette {
    paper: Color32,
    surface: Color32,
    surface_2: Color32,
    line: Color32,
    line_soft: Color32,
    ink: Color32,
    ink_2: Color32,
    ink_3: Color32,
    accent: Color32,
    accent_ink: Color32,
    accent_soft: Color32,
    ok: Color32,
    ok_soft: Color32,
    warn: Color32,
    warn_soft: Color32,
    bad: Color32,
    bad_soft: Color32,
    track: Color32,
}

/// 浅色：暖纸底 + 陶土橙（0.6.0 起的样子，不动）。
const LIGHT: Palette = Palette {
    paper: Color32::from_rgb(0xF6, 0xF3, 0xEC),
    surface: Color32::from_rgb(0xFF, 0xFD, 0xF8),
    surface_2: Color32::from_rgb(0xF1, 0xED, 0xE4),
    line: Color32::from_rgb(0xE4, 0xDF, 0xD2),
    line_soft: Color32::from_rgb(0xED, 0xE8, 0xDC),
    ink: Color32::from_rgb(0x2A, 0x27, 0x20),
    ink_2: Color32::from_rgb(0x6C, 0x68, 0x60),
    ink_3: Color32::from_rgb(0x9C, 0x97, 0x8D),
    accent: Color32::from_rgb(0xC0, 0x63, 0x40),
    accent_ink: Color32::from_rgb(0xA3, 0x4F, 0x31),
    accent_soft: Color32::from_rgb(0xF2, 0xE1, 0xD7),
    ok: Color32::from_rgb(0x4E, 0x7A, 0x5C),
    ok_soft: Color32::from_rgb(0xE4, 0xED, 0xE4),
    warn: Color32::from_rgb(0xA9, 0x76, 0x2A),
    warn_soft: Color32::from_rgb(0xF3, 0xEA, 0xD7),
    bad: Color32::from_rgb(0xA2, 0x44, 0x3A),
    bad_soft: Color32::from_rgb(0xF4, 0xE2, 0xDF),
    track: Color32::from_rgb(0xE7, 0xE2, 0xD5),
};

/// 深色：**不是把浅色反相**。
///
/// 三条实测出来的原则：
/// 1. 底色用暖黑（`#191713`）而不是纯黑 —— 纯黑配暖橙会发脏；
/// 2. 强调色提亮到 `#D87E58`：`#C06340` 在深底上几乎没有存在感；
/// 3. 语义色（ok/warn/bad）必须整体提亮 —— 深底上原来的墨绿 `#4E7A5C` 等于隐身。
const DARK_PAL: Palette = Palette {
    paper: Color32::from_rgb(0x19, 0x17, 0x13),
    surface: Color32::from_rgb(0x21, 0x1E, 0x19),
    surface_2: Color32::from_rgb(0x28, 0x24, 0x19),
    line: Color32::from_rgb(0x37, 0x32, 0x2A),
    line_soft: Color32::from_rgb(0x2E, 0x2A, 0x23),
    ink: Color32::from_rgb(0xF2, 0xEE, 0xE6),
    ink_2: Color32::from_rgb(0xB9, 0xB2, 0xA5),
    ink_3: Color32::from_rgb(0x85, 0x7F, 0x72),
    accent: Color32::from_rgb(0xD8, 0x7E, 0x58),
    accent_ink: Color32::from_rgb(0xE8, 0x9A, 0x79),
    accent_soft: Color32::from_rgb(0x3A, 0x2A, 0x22),
    ok: Color32::from_rgb(0x79, 0xA9, 0x8A),
    ok_soft: Color32::from_rgb(0x22, 0x30, 0x1F),
    warn: Color32::from_rgb(0xD2, 0xA0, 0x5A),
    warn_soft: Color32::from_rgb(0x35, 0x2C, 0x1D),
    bad: Color32::from_rgb(0xCF, 0x7A, 0x6E),
    bad_soft: Color32::from_rgb(0x3B, 0x26, 0x22),
    track: Color32::from_rgb(0x33, 0x2E, 0x26),
};

fn pal() -> &'static Palette {
    if is_dark() { &DARK_PAL } else { &LIGHT }
}

pub fn paper() -> Color32 {
    pal().paper
}
pub fn surface() -> Color32 {
    pal().surface
}
pub fn surface_2() -> Color32 {
    pal().surface_2
}
pub fn line() -> Color32 {
    pal().line
}
pub fn line_soft() -> Color32 {
    pal().line_soft
}
pub fn ink() -> Color32 {
    pal().ink
}
pub fn ink_2() -> Color32 {
    pal().ink_2
}
pub fn ink_3() -> Color32 {
    pal().ink_3
}
pub fn accent() -> Color32 {
    pal().accent
}
pub fn accent_ink() -> Color32 {
    pal().accent_ink
}
pub fn accent_soft() -> Color32 {
    pal().accent_soft
}
pub fn ok() -> Color32 {
    pal().ok
}
pub fn ok_soft() -> Color32 {
    pal().ok_soft
}
pub fn warn() -> Color32 {
    pal().warn
}
pub fn warn_soft() -> Color32 {
    pal().warn_soft
}
pub fn bad() -> Color32 {
    pal().bad
}
pub fn bad_soft() -> Color32 {
    pal().bad_soft
}
pub fn track() -> Color32 {
    pal().track
}
/// 状态字符串 → (文字, 前景色, 底色)。
///
/// `TaskView.status` 是 `String`（HTTP 那边要序列化它），所以这里按字符串匹配，
/// 不用 `TaskStatus`。UI 与托盘 tooltip 共用这一份。
pub fn status_style(status: &str) -> (&'static str, Color32, Color32) {
    match status {
        "running" => ("下载中", accent_ink(), accent_soft()),
        "queued" => ("排队中", warn(), warn_soft()),
        "paused" => ("已暂停", warn(), warn_soft()),
        "done" => ("已完成", ok(), ok_soft()),
        "failed" => ("失败", bad(), bad_soft()),
        _ => ("未知", ink_3(), surface_2()),
    }
}

/// 把 egui 的视觉风格调成 demo 那套：去掉默认的蓝色高亮、圆角收小、
/// 描边统一成 1pt 暖灰。
///
/// **明暗两套都要调**：只改 `Visuals::light()` 的底色而文字仍按深色配，
/// 深色模式下原生控件（输入框、下拉箭头）会看不见。
pub fn apply(ctx: &eframe::egui::Context) {
    use eframe::egui::{Stroke, Visuals};

    let mut visuals = if is_dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = paper();
    visuals.window_fill = surface();
    visuals.extreme_bg_color = surface_2();
    visuals.faint_bg_color = surface_2();
    visuals.override_text_color = Some(ink());

    visuals.widgets.noninteractive.bg_fill = surface();
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(STROKE, line());

    visuals.widgets.inactive.bg_fill = surface();
    visuals.widgets.inactive.bg_stroke = Stroke::new(STROKE, line());
    visuals.widgets.inactive.fg_stroke = Stroke::new(STROKE, ink_2());

    visuals.widgets.hovered.bg_fill = surface_2();
    visuals.widgets.hovered.bg_stroke = Stroke::new(STROKE, line());
    visuals.widgets.hovered.fg_stroke = Stroke::new(STROKE, ink());

    visuals.widgets.active.bg_fill = accent_soft();
    visuals.widgets.active.bg_stroke = Stroke::new(STROKE, accent());
    visuals.widgets.active.fg_stroke = Stroke::new(STROKE, accent_ink());

    visuals.selection.bg_fill = accent_soft();
    visuals.selection.stroke = Stroke::new(STROKE, accent_ink());

    // **两件都要做**：`set_visuals` 只改当前这一份；`set_theme` 才定下"这套配色的身份"。
    // 只做前者的话，eframe 一旦按系统外观重新应用主题（Windows 浅色模式），
    // 我们设的深色就会被顶掉 —— 实测表现是"一半控件深、一半浅"。
    ctx.set_theme(if is_dark() {
        eframe::egui::Theme::Dark
    } else {
        eframe::egui::Theme::Light
    });
    ctx.set_visuals(visuals);
}
