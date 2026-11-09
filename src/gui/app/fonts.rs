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

    ctx.set_fonts(fonts);
    true
}

/// 返回实际用上的字体名，供启动日志与验收脚本核对。
pub fn resolved_font_name() -> Option<&'static str> {
    CJK_CANDIDATES
        .iter()
        .find(|(_, p)| std::path::Path::new(p).is_file())
        .map(|(name, _)| *name)
}
