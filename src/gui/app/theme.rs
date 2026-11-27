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
/// E19：剪贴板提示条高度。只在有命中时占位，所以这个数只影响那一条。
pub const CLIPBOARD_HINT_H: f32 = 32.0;
/// 左侧分类栏宽度（D6：保留）。
pub const SIDEBAR_W: f32 = 184.0;

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
///
/// ## 四层表面（0.7.x 起）
///
/// ```
/// sunken  <  paper  <  surface3  <  surface
/// 凹进去      窗口底     内嵌块       卡片/浮层
/// ```
///
/// 每层只承担**一个**语义，这是"层次感"的来源。0.7.0 只有两档在起作用
/// （`paper`→`surface` 的 ΔE 才 3.61，卡片像贴在底色上），而且 `surface_2`
/// 被迫同时当"次级底"和"hover 底"，职责重叠把层次压平了。
///
/// 门槛按每对的职责定，不搞一刀切 —— 浅色从纸底到纯白只有约 12 ΔE 的跨度，
/// 四层相邻都要求 5 就得吃掉 15，只能靠纯黑纯白（无约束搜索确实跑出过这种废解）：
///
/// | 对 | 门槛 | 理由 |
/// |---|---|---|
/// | `sunken` → `paper` | ≥5 | 凹槽要看得出来，否则输入框和纸底粘在一起 |
/// | `paper` → `surface` | ≥5 | **卡片浮起感的核心** |
/// | `paper` → `surface3` | ≥3 | 内嵌块靠 1px 描边区分，不该抢主卡片 |
/// | `surface3` → `surface` | ≥3 | 同上，故意做淡 |
///
/// 实测（`docs/design/2026-11-17-界面精致度-讨论记录.md` §7）：
/// 浅色 paper→surface **9.06**、深色 **10.50**；四层的暖纸调性 R>G>B 保持。
struct Palette {
    /// 凹进去的底：输入框、进度条轨道。
    sunken: Color32,
    paper: Color32,
    /// 卡片里的内嵌块：模板卡、代码块、清单项。
    surface_3: Color32,
    surface: Color32,
    /// **只做 hover / 选中底**（0.7.x 起不再兼任"次级底"）。
    surface_2: Color32,
    line: Color32,
    line_soft: Color32,
    ink: Color32,
    ink_2: Color32,
    /// 说明文字。浅色下对比度 **4.67**（0.7.0 是 2.62，低于 AA 正文 4.5，不合规）。
    ink_3: Color32,
    /// **仅装饰 / 禁用**：对比度低于 3.0，不许承载信息。
    ink_4: Color32,
    accent: Color32,
    accent_ink: Color32,
    accent_soft: Color32,
    /// **列表行的"正在查看"底**。
    ///
    /// 0.7.0 直接复用 `accent_soft`（那是**标签/胶囊**的底，本来就比较实），
    /// 涂在 52pt 高的整行上就成了一个色块 —— 用户 2026-11-18 报"选中颜色太深"。
    /// 实测：`accent_soft` 对 `surface` 的 ΔE 是 **10.54**，而行选中只需要 5~7
    /// （看得出选中、又不抢正文）。
    row_selected: Color32,
    ok: Color32,
    ok_soft: Color32,
    warn: Color32,
    warn_soft: Color32,
    bad: Color32,
    bad_soft: Color32,
    track: Color32,
}

/// 浅色：暖纸底 + 陶土橙。
const LIGHT: Palette = Palette {
    sunken: Color32::from_rgb(0xE1, 0xD8, 0xC5),
    paper: Color32::from_rgb(0xED, 0xE6, 0xD8),
    surface_3: Color32::from_rgb(0xF8, 0xF4, 0xEC),
    surface: Color32::from_rgb(0xFF, 0xFD, 0xF7),
    surface_2: Color32::from_rgb(0xE7, 0xDF, 0xCD),
    line: Color32::from_rgb(0xDC, 0xD4, 0xC4),
    line_soft: Color32::from_rgb(0xE8, 0xE2, 0xD4),
    ink: Color32::from_rgb(0x2A, 0x27, 0x20),
    ink_2: Color32::from_rgb(0x57, 0x52, 0x46),
    ink_3: Color32::from_rgb(0x6B, 0x65, 0x55),
    ink_4: Color32::from_rgb(0xA6, 0x9F, 0x91),
    accent: Color32::from_rgb(0xC0, 0x63, 0x40),
    accent_ink: Color32::from_rgb(0xA3, 0x4F, 0x31),
    accent_soft: Color32::from_rgb(0xF2, 0xE1, 0xD7),
    row_selected: Color32::from_rgb(0xF9, 0xEE, 0xE6),
    ok: Color32::from_rgb(0x4E, 0x7A, 0x5C),
    ok_soft: Color32::from_rgb(0xE4, 0xED, 0xE4),
    warn: Color32::from_rgb(0xA9, 0x76, 0x2A),
    warn_soft: Color32::from_rgb(0xF3, 0xEA, 0xD7),
    bad: Color32::from_rgb(0xA2, 0x44, 0x3A),
    bad_soft: Color32::from_rgb(0xF4, 0xE2, 0xDF),
    track: Color32::from_rgb(0xDD, 0xD5, 0xC3),
};

/// 深色：**不是把浅色反相**。
///
/// 三条实测出来的原则：
/// 1. 底色用暖黑（`#1C1915`）而不是纯黑 —— 纯黑配暖橙会发脏；
/// 2. 强调色提亮到 `#D87E58`：`#C06340` 在深底上几乎没有存在感；
/// 3. 语义色（ok/warn/bad）必须整体提亮 —— 深底上原来的墨绿 `#4E7A5C` 等于隐身。
///
/// 深色下定稿的表面阶梯：`paper #1C1915` → `surface #332D24`，ΔE **10.50**
/// （0.7.0 只有 3.76，深色反而比浅色更平）。`sunken` 压到 `#0D0B09`，
/// 深色里"凹槽"不压到接近纯黑就看不出来。
const DARK_PAL: Palette = Palette {
    sunken: Color32::from_rgb(0x0D, 0x0B, 0x09),
    paper: Color32::from_rgb(0x1C, 0x19, 0x15),
    surface_3: Color32::from_rgb(0x28, 0x23, 0x19),
    surface: Color32::from_rgb(0x33, 0x2D, 0x24),
    surface_2: Color32::from_rgb(0x3E, 0x37, 0x2C),
    line: Color32::from_rgb(0x44, 0x3D, 0x31),
    line_soft: Color32::from_rgb(0x32, 0x2D, 0x25),
    ink: Color32::from_rgb(0xF4, 0xF0, 0xE8),
    ink_2: Color32::from_rgb(0xC6, 0xBF, 0xB2),
    ink_3: Color32::from_rgb(0xA7, 0x9F, 0x91),
    ink_4: Color32::from_rgb(0x5F, 0x59, 0x4F),
    accent: Color32::from_rgb(0xD8, 0x7E, 0x58),
    accent_ink: Color32::from_rgb(0xE8, 0x9A, 0x79),
    accent_soft: Color32::from_rgb(0x3A, 0x2A, 0x22),
    row_selected: Color32::from_rgb(0x40, 0x37, 0x2C),
    ok: Color32::from_rgb(0x79, 0xA9, 0x8A),
    ok_soft: Color32::from_rgb(0x22, 0x30, 0x1F),
    warn: Color32::from_rgb(0xD2, 0xA0, 0x5A),
    warn_soft: Color32::from_rgb(0x35, 0x2C, 0x1D),
    bad: Color32::from_rgb(0xCF, 0x7A, 0x6E),
    bad_soft: Color32::from_rgb(0x3B, 0x26, 0x22),
    track: Color32::from_rgb(0x2A, 0x25, 0x1C),
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
/// 卡片里的内嵌块（模板卡、代码块、清单项）。
///
/// 0.7.0 没有这一层，需要"再低一层"的地方只能借 `surface_2` —— 而那是 hover 色，
/// 一个色值两个语义，层次就平了。
pub fn surface_3() -> Color32 {
    pal().surface_3
}
/// 凹进去的底：输入框、进度条轨道。
pub fn sunken() -> Color32 {
    pal().sunken
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
/// **仅装饰 / 禁用**：对比度低于 3.0，不许承载信息。
///
/// 立这一档是为了把"淡"和"看不清"分开：0.7.0 只有一个 `ink_3`，
/// 既当"说明文字"（该读得清）又当"装饰"（该退下去），结果浅色下对比度只有 2.62，
/// 两边都没做好。
pub fn ink_4() -> Color32 {
    pal().ink_4
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
/// 列表行的"正在查看"底。与 `accent_soft` 分开：后者是标签/胶囊的底，更实。
pub fn row_selected() -> Color32 {
    pal().row_selected
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
    // 输入框这类"凹进去"的底走 `sunken`（0.7.0 借用的是 surface_2，和 hover 撞了）。
    visuals.extreme_bg_color = sunken();
    visuals.faint_bg_color = surface_3();
    visuals.override_text_color = Some(ink());

    visuals.widgets.noninteractive.bg_fill = surface();
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(STROKE, line());

    visuals.widgets.inactive.bg_fill = surface();
    visuals.widgets.inactive.bg_stroke = Stroke::new(STROKE, line());
    visuals.widgets.inactive.fg_stroke = Stroke::new(STROKE, ink_2());

    // hover 才用 surface_2 —— 它现在**只**承担这一个语义。
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

    // ------------------------------------------------------------------
    // Style：**手绘控件与 egui 原生控件必须用同一套度量**
    //
    // 0.7.0 完全没接管这一块，于是 `item_spacing` 还是默认的 (8, 3)、
    // `button_padding` 是 (8, 2)、`window_margin` 是 6 —— 原生按钮/输入框/下拉
    // 用默认密度，而手绘的行高是 52、圆角是 9。两套度量摆在一起，
    // 看起来像"没做完"，这正是"粗糙"最实在的来源之一。
    // ------------------------------------------------------------------
    // egui 0.37 起 Style 是**按主题分开存**的（dark_style / light_style），
    // `Context::style()` / `set_style()` 已经没了。这里用 `style_mut_of` 只改
    // 当前这一份 —— 两套主题各调一次 `apply`，所以另一份到时候也会被调到。
    let theme = if is_dark() {
        eframe::egui::Theme::Dark
    } else {
        eframe::egui::Theme::Light
    };
    ctx.style_mut_of(theme, |style| {
        // 间距阶梯：4 的倍数。默认 (8, 3) 的 3 是个没有来历的数。
        style.spacing.item_spacing = eframe::egui::vec2(8.0, 4.0);
        style.spacing.button_padding = eframe::egui::vec2(10.0, 5.0);
        style.spacing.window_margin = eframe::egui::Margin::same(10);
        style.spacing.menu_margin = eframe::egui::Margin::same(6);
        // 交互目标最小尺寸：默认 (40, 20) 在 150% 缩放下太矮，指针不好点。
        style.spacing.interact_size = eframe::egui::vec2(40.0, 26.0);

        // 圆角三档，贯彻同一个 RADIUS：
        //   控件（按钮/输入框）= RADIUS - 3
        //   容器（卡片/菜单/浮层）= RADIUS
        //   胶囊（进度条/状态点）= 大值由调用处给
        style.visuals.widgets.noninteractive.corner_radius =
            eframe::egui::CornerRadius::same(RADIUS_U8);
        style.visuals.widgets.inactive.corner_radius =
            eframe::egui::CornerRadius::same(RADIUS_U8.saturating_sub(3));
        style.visuals.widgets.hovered.corner_radius =
            eframe::egui::CornerRadius::same(RADIUS_U8.saturating_sub(3));
        style.visuals.widgets.active.corner_radius =
            eframe::egui::CornerRadius::same(RADIUS_U8.saturating_sub(3));
        style.visuals.window_corner_radius = eframe::egui::CornerRadius::same(RADIUS_U8);
        style.visuals.menu_corner_radius = eframe::egui::CornerRadius::same(RADIUS_U8);

        // 动画时长：0.2s 是 egui 默认，保留；这里显式写出来是为了让它成为**我们的决定**，
        // 而不是"碰巧用了默认值"。逐项动画的取舍见 `docs/design/2026-11-17-*` §6。
        style.animation_time = 0.16;
    });
}

/// `RADIUS` 的整数版：egui 的 `CornerRadius` 收 `u8`。
const RADIUS_U8: u8 = RADIUS as u8;

#[cfg(test)]
mod tests {
    use super::*;

    /// 相对亮度（WCAG）。
    fn lum(c: Color32) -> f64 {
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    /// CIELAB ΔE76（感知色差）。判据：<2 看不出、2-5 微妙、>5 明显分层。
    fn delta_e(a: Color32, b: Color32) -> f64 {
        fn lab(c: Color32) -> (f64, f64, f64) {
            let f = |v: u8| {
                let v = v as f64 / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            let (r, g, bl) = (f(c.r()), f(c.g()), f(c.b()));
            let x = (0.4124 * r + 0.3576 * g + 0.1805 * bl) / 0.95047;
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * bl;
            let z = (0.0193 * r + 0.1192 * g + 0.9505 * bl) / 1.08883;
            let gg = |t: f64| {
                if t > 0.008856 {
                    t.powf(1.0 / 3.0)
                } else {
                    7.787 * t + 16.0 / 116.0
                }
            };
            let (fx, fy, fz) = (gg(x), gg(y), gg(z));
            (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
        }
        let (l1, a1, b1) = lab(a);
        let (l2, a2, b2) = lab(b);
        ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
    }

    /// 对比度。1.0 = 完全一样。
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// **层次用例 1：卡片必须从窗口底上浮起来。**
    ///
    /// 这是用户报「颜色太接近，整体没层次」的直接判据。
    /// 0.7.0 实测 paper→surface 对比度只有 1.090（ΔE 3.61），肉眼几乎分不开；
    /// 现在要求 ≥ 1.18（ΔE 约 9）。用固定数值断言，不靠肉眼看截图 —— 观感会骗人。
    #[test]
    fn the_content_panel_separates_from_the_window_background() {
        for (name, p) in [("浅色", &LIGHT), ("深色", &DARK_PAL)] {
            let c = contrast(p.paper, p.surface);
            assert!(
                c >= 1.18,
                "{name}：窗口底与卡片太接近（对比度 {c:.3}，要求 >= 1.18）\
                 —— 这正是「没有层次」的样子"
            );
        }
    }

    /// **层次用例 2：四层表面必须逐层变亮**，且相邻两层看得出差别。
    ///
    /// 单调这条防的是「调了个色，结果把顺序调乱了」—— 比如内嵌块比卡片还亮，
    /// 看起来像浮在卡片外面。
    #[test]
    fn the_surface_ladder_is_monotonic_and_readable() {
        for (name, p) in [("浅色", &LIGHT), ("深色", &DARK_PAL)] {
            let ls = [
                ("sunken", p.sunken),
                ("paper", p.paper),
                ("surface_3", p.surface_3),
                ("surface", p.surface),
            ];
            for w in ls.windows(2) {
                let (a, b) = (w[0], w[1]);
                assert!(
                    lum(b.1) > lum(a.1),
                    "{name}：{} 应当比 {} 更亮（层次顺序乱了）",
                    b.0,
                    a.0
                );
                // 相邻层必须有可测量的差别（>=1.04，约 dE 3）：完全相等就是"没做这层"。
                let c = contrast(a.1, b.1);
                assert!(
                    c >= 1.04,
                    "{name}：{} 与 {} 之间太接近（{c:.3} < 1.04）—— 这一层等于没做",
                    a.0,
                    b.0
                );
            }
        }
    }

    /// **层次用例 3：说明文字必须读得清。**
    ///
    /// 0.7.0 的 `ink_3` 在浅色下对比度只有 **2.62**，低于 WCAG AA 正文的 4.5，
    /// 也低于大字号/UI 元素的 3.0 —— 而它被大量用在副标题、灰字说明、计数上。
    /// 根因是"色值不够用"：一个 `ink_3` 既要"淡"又要"读得清"。
    #[test]
    fn explanatory_text_meets_the_readable_threshold() {
        for (name, p) in [("浅色", &LIGHT), ("深色", &DARK_PAL)] {
            let c = contrast(p.ink_3, p.paper);
            assert!(
                c >= 4.5,
                "{name}：ink_3 对比度 {c:.2} 低于 AA 正文 4.5 —— 说明文字读不清"
            );
            assert!(
                contrast(p.ink, p.paper) > contrast(p.ink_2, p.paper),
                "{name}：ink 应比 ink_2 更突出"
            );
            assert!(
                contrast(p.ink_2, p.paper) > contrast(p.ink_3, p.paper),
                "{name}：ink_2 应比 ink_3 更突出"
            );
        }
    }

    /// **层次用例 4：`ink_4` 只做装饰，不许被当成正文用。**
    ///
    /// 它的存在价值就是"明确地不可读"—— 如果它也能到 4.5，就该合并回 ink_3，
    /// 多一个 token 只是增加选择困难。
    #[test]
    fn ink_4_is_decorative_only() {
        for (name, p) in [("浅色", &LIGHT), ("深色", &DARK_PAL)] {
            let c = contrast(p.ink_4, p.paper);
            assert!(
                c < 3.0,
                "{name}：ink_4 对比度 {c:.2} 已经够读 —— 那它不该只当装饰"
            );
        }
    }

    /// 语义色在各自的浅底上要看得见（这就是深色单独重调过的原因）。
    #[test]
    fn semantic_colors_stay_visible_on_their_own_soft_backgrounds() {
        for (name, p) in [("浅色", &LIGHT), ("深色", &DARK_PAL)] {
            for (label, fg, bg) in [
                ("ok", p.ok, p.ok_soft),
                ("warn", p.warn, p.warn_soft),
                ("bad", p.bad, p.bad_soft),
                ("accent", p.accent_ink, p.accent_soft),
            ] {
                let c = contrast(fg, bg);
                assert!(c >= 3.0, "{name}：{label} 在它的浅底上对比度只有 {c:.2}");
            }
        }
    }
    /// **行选中色不能太重**（用户 2026-11-18 报"选中颜色太深"）。
    ///
    /// 0.7.0 直接借 `accent_soft` 涂整行 —— 那是标签/胶囊的底，ΔE 到 10.5，
    /// 52pt 高的整行就成了色块。行选中只要"看得出选中、不抢正文"，5~8 合适。
    #[test]
    fn the_row_selection_tint_is_visible_but_not_a_color_block() {
        for (name, p) in [("浅色", &LIGHT), ("深色", &DARK_PAL)] {
            let d = delta_e(p.row_selected, p.surface);
            assert!(
                (5.0..=8.5).contains(&d),
                "{name}：行选中底与卡片差 ΔE {d:.2}，要求 5~8.5 \
                 （比这更淡看不出选中，更重就成了色块）"
            );
            // 必须比 accent_soft 淡：否则说明又退回"借标签色"那条路了。
            assert!(
                delta_e(p.row_selected, p.surface) < delta_e(p.accent_soft, p.surface),
                "{name}：行选中比标签底还重，说明又借了 accent_soft"
            );
        }
    }
    /// 侧栏宽度按动画进度插值：0 = 展开（212）、1 = 收起（52）。
    ///
    /// 抽成函数是为了能测方向 —— 把两端写反的话界面不会报错，
    /// 只会表现为"点收起它反而变宽"。这种错靠肉眼看动画很难抓。
    #[test]
    fn the_sidebar_width_interpolates_in_the_right_direction() {
        let at = |t: f32| SIDEBAR_W - (SIDEBAR_W - SIDEBAR_MINI_W) * t;
        assert_eq!(at(0.0), SIDEBAR_W, "t=0 应当是展开态");
        assert_eq!(at(1.0), SIDEBAR_MINI_W, "t=1 应当是收起态");
        // 中间必须是单调递减的（动画过程里宽度只会一路变窄）。
        let mut prev = at(0.0);
        for step in 1..=10 {
            let now = at(step as f32 / 10.0);
            assert!(now < prev, "t={step}/10 时宽度没有继续变窄");
            prev = now;
        }
        // 收起态必须真的比展开态窄，且窄得明显（不是差 1pt）。
        // 用 `const` 断言而不是 `assert!`：两个常量相减是编译期就能算出的值，
        // clippy 会把它判成"this assertion has a constant value"。
        const _: () = assert!(SIDEBAR_W - SIDEBAR_MINI_W >= 100.0);
    }
}
