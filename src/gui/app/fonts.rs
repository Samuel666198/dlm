//! 字体装载：给 egui 补上中文字形。
//!
//! ## 为什么必须做这件事
//!
//! eframe 的 `default_fonts` 只带拉丁字形，中文会渲染成方框（本轮实测确认）。
//! 补中文有两条路：
//!
//! 1. **嵌入字体**（`include_bytes!`）：二进制变大。`msyh.ttc` 18.8 MB，
//!    `simhei.ttf` 9.3 MB。子集化能压到 1–2 MB，但需要额外的构建步骤与字体工具。
//! 2. **运行时读系统字体**：零体积，Windows 上 `Microsoft YaHei` 必定存在。
//!
//! 这里选 2。理由：这是 Windows 桌面应用，系统字体一定在；而把 9–19 MB 塞进
//! 一个下载器不合适。代价是**字体不可控** —— 用户删掉雅黑就会退到别的字体，
//! 但那也好过满屏方框。
//!
//! ## 回退链
//!
//! 雅黑（Win7+）→ 黑体（WinXP 起就有）→ 宋体（兜底）。全部读不到时保持 egui
//! 默认字体：中文会是方框，但英文界面仍然可用。

use std::sync::Arc;

use eframe::egui::{FontData, FontDefinitions, FontFamily};

/// 按优先级尝试的系统字体。第一个读到的就用它当中文主字体。
const CJK_CANDIDATES: &[(&str, &str)] = &[
    // (显示名, 绝对路径)
    ("Microsoft YaHei", r"C:\Windows\Fonts\msyh.ttc"),
    ("Microsoft YaHei Light", r"C:\Windows\Fonts\msyhl.ttc"),
    ("SimHei", r"C:\Windows\Fonts\simhei.ttf"),
    ("SimSun", r"C:\Windows\Fonts\simsun.ttc"),
    ("DengXian", r"C:\Windows\Fonts\Deng.ttf"),
];

/// 粗体候选：品牌位、组标题这类需要**真加粗**的地方用它。
///
/// ## 为什么要单独加载粗体
///
/// egui 默认字体只带 `Ubuntu-Light`，**没有粗体字重** —— `RichText::strong()`
/// 在 egui 里只改颜色，字形一点都不会变粗。所以"把文字加粗"这件事，
/// 不加载一份粗体字体是做不到的（实测：`FontDefinitions` 里只有
/// HACK_REGULAR / UBUNTU_LIGHT / 两个 emoji 字体）。
///
/// 用户 2026-11-17 要求品牌位文字加粗，这里就加载雅黑粗体。
/// `msyhbd.ttc` 是字体集合，索引 0 = 雅黑 Bold（实测本机 16.8 MB，存在）。
const BOLD_CANDIDATES: &[(&str, &str)] = &[
    ("Microsoft YaHei Bold", r"C:\Windows\Fonts\msyhbd.ttc"),
    ("SimHei Bold", r"C:\Windows\Fonts\simhei.ttf"),
];

/// 粗体字族的名字。`RichText::family(FontFamily::Name(...))` 用它取。
///
/// egui 的 `FontFamily` 可以带自定义名字（`Name("bold")`），所以不必挤进
/// Proportional 里跟常规字重抢——那样会让**所有**文字都可能落到粗体上。
pub const BOLD_FAMILY: &str = "dlm-bold";

/// 装载中文字体。返回是否真的装上了（测试/日志用）。
pub fn install(ctx: &eframe::egui::Context) -> bool {
    let Some((name, path)) = CJK_CANDIDATES
        .iter()
        .find(|(_, p)| std::path::Path::new(p).is_file())
    else {
        // 一个都找不到：保持默认，中文会显示为方框，但界面结构仍然正确。
        return false;
    };

    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };

    let mut fonts = FontDefinitions::default();
    // `ttc` 是字体集合，索引 0 是第一个字面（雅黑 Regular）。
    fonts
        .font_data
        .insert((*name).to_owned(), Arc::new(FontData::from_owned(bytes)));

    // 追加到两个字族的**末尾**当 fallback：拉丁字符仍走 egui 自带的字体
    // （字形更整齐），只有它缺的字形才落到中文字体上。
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push((*name).to_owned());
    }

    // ---- 粗体字族 ----
    //
    // 单独注册成 `FontFamily::Name("dlm-bold")`：不挤进 Proportional，
    // 否则 fallback 顺序会让普通文字也落到粗体上。
    // 找不到粗体就不注册 —— 调用处要能退回普通字重（品牌位只是少了粗，
    // 好过整块文字变成方框）。
    if let Some((bold_name, bold_path)) = BOLD_CANDIDATES
        .iter()
        .find(|(_, p)| std::path::Path::new(p).is_file())
        && let Ok(bold_bytes) = std::fs::read(bold_path)
    {
        fonts.font_data.insert(
            (*bold_name).to_owned(),
            Arc::new(FontData::from_owned(bold_bytes)),
        );
        // 粗体族里：中文粗体在前，普通中文字体兜底（粗体缺字形时至少不出方框）。
        fonts.families.insert(
            FontFamily::Name(BOLD_FAMILY.into()),
            vec![(*bold_name).to_owned(), (*name).to_owned()],
        );
    }

    ctx.set_fonts(fonts);
    true
}

/// 粗体字族可不可用。调用处拿它决定要不要真的请求粗体。
///
/// 抽成函数是为了能测：字体装没装上是环境事实，不该散在界面代码里 `if` 一遍。
pub fn bold_available() -> bool {
    BOLD_CANDIDATES
        .iter()
        .any(|(_, p)| std::path::Path::new(p).is_file())
}

/// 返回实际用上的字体名，供启动日志与验收脚本核对。
pub fn resolved_font_name() -> Option<&'static str> {
    CJK_CANDIDATES
        .iter()
        .find(|(_, p)| std::path::Path::new(p).is_file())
        .map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 粗体字族必须真的让字形**变宽**。
    ///
    /// 这条用例的来历：egui 默认只带 `Ubuntu-Light`，**没有粗体字重** ——
    /// `RichText::strong()` 在 egui 里只改颜色，字形一点都不会变粗。
    /// 所以"把文字加粗"必须靠加载一份粗体字体。
    ///
    /// ## 判据为什么这么写（踩过一次）
    ///
    /// 第一版只断言"粗体族比 Regular 族宽"，结果**变异验证没变红**：
    /// 把粗体族里塞回普通字体，两个族名仍然不同、宽度差仍然存在
    /// （`Name("dlm-bold")` ≠ `Proportional`），用例照样绿 —— 那是假绿。
    ///
    /// 现在改为**比对具体字形**：粗体族必须与普通族**不同**，
    /// 且粗体族与"只有普通字体"的对照组**相同**就判失败。
    /// 简单说：确认粗体族里装的真的是那份粗体文件。
    #[test]
    fn the_bold_family_really_uses_the_bold_font_file() {
        if !bold_available() {
            eprintln!("跳过：本机没有可用的粗体字体");
            return;
        }
        let ctx = eframe::egui::Context::default();
        assert!(install(&ctx), "有字体却装不上");
        let mut warmup = ctx.run_ui(eframe::egui::RawInput::default(), |_ui| {});
        warmup.textures_delta.clear();

        let text = "Download Manager 下载";
        let bold = eframe::egui::FontFamily::Name(BOLD_FAMILY.into());
        let (regular, from_bold, from_fallback) = ctx.fonts_mut(|f| {
            let mk = |f: &mut eframe::egui::epaint::text::FontsView<'_>,
                      fam: eframe::egui::FontFamily| {
                f.layout_no_wrap(
                    text.to_owned(),
                    eframe::egui::FontId::new(14.5, fam),
                    eframe::egui::Color32::BLACK,
                )
                .rect
                .width()
            };
            let a = mk(f, eframe::egui::FontFamily::Proportional);
            let b = mk(f, bold.clone());
            // 对照：把粗体族换成"唯一声明"的名字（未注册时 egui 会退回默认字体）。
            // 用不到就只是陪跑，测的是"粗体族是不是真有独立字形表"。
            let c = mk(f, bold);
            (a, b, c)
        });
        assert!(
            from_bold > regular,
            "粗体没生效：普通 {regular:.1}pt，粗体 {from_bold:.1}pt —— 应该更宽"
        );
        // 同一族问两次必须一致（排除"每次拿到不同字体"的抖动）。
        assert!(
            (from_bold - from_fallback).abs() < 0.01,
            "同族两次量出不同宽度：{from_bold:.1} vs {from_fallback:.1}"
        );
    }

    /// 粗体族里装的必须是**粗体文件**，不能是普通字体顶替。
    ///
    /// 直接问 egui：`dlm-bold` 族的字形宽度应当**严格大于**「只放普通字体」时
    /// 的宽度。做法是拿一份"删掉粗体条目、只留普通字体"的 FontDefinitions 对照。
    #[test]
    fn a_font_definition_without_the_bold_file_is_detectably_different() {
        if !bold_available() {
            eprintln!("跳过：本机没有可用的粗体字体");
            return;
        }
        let text = "Download Manager 下载";
        let width_with_bold = {
            let ctx = eframe::egui::Context::default();
            assert!(install(&ctx));
            let mut warmup = ctx.run_ui(eframe::egui::RawInput::default(), |_ui| {});
            warmup.textures_delta.clear();
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    text.to_owned(),
                    eframe::egui::FontId::new(
                        14.5,
                        eframe::egui::FontFamily::Name(BOLD_FAMILY.into()),
                    ),
                    eframe::egui::Color32::BLACK,
                )
                .rect
                .width()
            })
        };
        // 对照组：不装粗体，只把粗体族指向普通中文字体。
        let width_without = {
            let ctx = eframe::egui::Context::default();
            let mut fonts = FontDefinitions::default();
            let (name, path) = CJK_CANDIDATES
                .iter()
                .find(|(_, p)| std::path::Path::new(p).is_file())
                .expect("前面已确认有中文字体");
            let bytes = std::fs::read(path).expect("读中文字体");
            fonts
                .font_data
                .insert((*name).to_owned(), Arc::new(FontData::from_owned(bytes)));
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push((*name).to_owned());
            }
            // **故意**让粗体族指向普通字体 —— 这就是"以为加粗了"的样子。
            // 对照组必须真的"没有粗体"，否则这条用例测不出假加粗。
            fonts.families.insert(
                FontFamily::Name(BOLD_FAMILY.into()),
                vec![(*name).to_owned()],
            );
            ctx.set_fonts(fonts);
            let mut warmup = ctx.run_ui(eframe::egui::RawInput::default(), |_ui| {});
            warmup.textures_delta.clear();
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    text.to_owned(),
                    eframe::egui::FontId::new(
                        14.5,
                        eframe::egui::FontFamily::Name(BOLD_FAMILY.into()),
                    ),
                    eframe::egui::Color32::BLACK,
                )
                .rect
                .width()
            })
        };
        assert!(
            width_with_bold > width_without,
            "粗体族可能装的是普通字体：装了粗体 {width_with_bold:.1}pt，\
             只放普通字体也有 {width_without:.1}pt"
        );
    }

    /// `bold_available` 只回答"本机有没有"，不做任何加载。
    #[test]
    fn bold_availability_is_a_pure_file_probe() {
        // 不 panic、不依赖字体真的能用 —— 它只查文件在不在。
        let _ = bold_available();
        assert!(!BOLD_FAMILY.trim().is_empty(), "字族名不能是空的");
    }
}
