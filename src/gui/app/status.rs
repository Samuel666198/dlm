//! 状态栏：在跑数、合计速度、**限速（可点直接改）**、配额、版本。
//!
//! 0.6.0 起这一行不只是"读数"，是活动条：限速那颗胶囊点开就是常用档位 + 自定义，
//! 改完立刻作用于下一轮派发（走 `Ticket::ApplyRuntime`，不用重启）。

use eframe::egui;

use super::App;
use crate::gui::app::theme;
use crate::gui::speed_schedule;

/// 限速菜单里的常用档位（KiB/s）。`0` = 不限。
///
/// 顺序从"关掉"到"5 MiB/s"：最常做的是"先不限"和"压到 512"，两头都要一眼够到。
pub(crate) const SPEED_PRESETS: &[u64] = &[0, 128, 512, 1024, 2048, 5120];

/// 限速的人话标签：`不限` / `512 KiB/s`。
///
/// 抽成纯函数：状态栏与设置页显示的是同一个值，两处各写一份格式化迟早对不上
/// （左边写"不限"、右边写"0 KiB/s"这种）。
pub(crate) fn speed_limit_label(kib: u64) -> String {
    if kib == 0 {
        "不限".to_string()
    } else {
        crate::gui::app::list::fmt_speed(kib as f64 * 1024.0)
    }
}

impl App {
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let tasks = self.tasks();
        let running = tasks.iter().filter(|t| t.status == "running").count();
        let waiting = tasks
            .iter()
            .filter(|t| t.status == "queued" && t.hold_until.is_some())
            .count();
        let quota = self.quota_view();

        ui.horizontal_centered(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(format!("{running} 个在跑"))
                    .size(11.5)
                    .color(theme::INK_3),
            );
            if waiting > 0 {
                ui.label(
                    egui::RichText::new(format!("· {waiting} 个等待"))
                        .size(11.5)
                        .color(theme::INK_3),
                );
            }

            // 总速度：所有在跑任务的速率之和。没有在跑、或还没采到两个点时是 0，就不显示 ——
            // 挂一个「0 B/s」比不显示更容易让人以为卡住了。
            let total = self.total_speed();
            if total >= 1.0 {
                ui.label(
                    egui::RichText::new(format!("· {}", crate::gui::app::list::fmt_speed(total)))
                        .size(11.5)
                        .monospace()
                        .color(theme::INK_2),
                );
            }

            ui.add_space(10.0);
            self.speed_limit_pill(ui);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!("v{}", crate::gui::RELEASE))
                        .size(11.5)
                        .monospace()
                        .color(theme::INK_3),
                );

                if let Some(q) = quota {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}/{}",
                            crate::gui::app::list::fmt_bytes(q.used),
                            crate::gui::app::list::fmt_bytes(q.limit)
                        ))
                        .size(11.0)
                        .monospace()
                        .color(theme::INK_2),
                    );
                    ui.add_space(4.0);
                    // 配额条：一条 78pt 的细轨。
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(78.0, 4.0), egui::Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, 99.0, theme::TRACK);
                    let frac = if q.limit == 0 {
                        0.0
                    } else {
                        (q.used as f64 / q.limit as f64).clamp(0.0, 1.0) as f32
                    };
                    if frac > 0.0 {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(rect.width() * frac, rect.height()),
                            ),
                            99.0,
                            theme::ACCENT,
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("配额").size(11.5).color(theme::INK_3));
                }

                // 有新版 / 检查失败时，这里多一枚可点的标记。没更新的机器看不到任何东西。
                ui.add_space(12.0);
                self.update_badge(ui);
            });
        });
    }

    /// 「限速 512 KiB/s ▾」：点开是常用档位 + 自定义。
    ///
    /// 显示的是**此刻生效**的值（限速窗口命中时是窗口值），不是设置里那个静态值 ——
    /// 不然用户看见"不限"却在跑 512，会以为限速坏了。
    ///
    /// 那颗下拉箭头是**画**出来的，不是 `▾` 字符：截图实测 `▾`（U+25BE）在这台机器的
    /// Microsoft YaHei 里没有字形，显示成一个空心方框 —— 与早年的 `＋ ▶ 🗑` 是同一个坑。
    fn speed_limit_pill(&mut self, ui: &mut egui::Ui) {
        let now = self.speed_limit_now_kib();
        let pill = pill_button(ui, &format!("限速 {}", speed_limit_label(now)));

        // 菜单闭包里只碰这两个本地量：`self` 留在外面。
        let mut picked: Option<u64> = None;
        let mut custom = self.speed_custom_kib;
        egui::Popup::menu(&pill).show(|ui| {
            ui.set_min_width(180.0);
            for &preset in SPEED_PRESETS {
                let text = if preset == now {
                    format!("✓ {}", speed_limit_label(preset))
                } else {
                    // 用全角空格占位而不是三个半角空格：中文行高下两者对不齐。
                    format!("　{}", speed_limit_label(preset))
                };
                if ui.button(text).clicked() {
                    picked = Some(preset);
                    ui.close();
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut custom)
                        .range(0..=1024 * 100)
                        .suffix(" KiB/s"),
                );
                if ui.button("应用").clicked() {
                    picked = Some(custom);
                    ui.close();
                }
            });
        });

        if let Some(kib) = picked {
            self.speed_custom_kib = kib;
            self.set_speed_limit(kib);
        }
    }

    /// 此刻生效的限速（KiB/s，0 = 不限）。
    ///
    /// 只有真配了限速窗口才去解析 —— 常见情况下 `speed_windows` 是空的，
    /// 于是每帧这里只是一次空判断，不产生分配。
    fn speed_limit_now_kib(&self) -> u64 {
        if self.settings.speed_windows.is_empty() {
            return self.settings.max_speed_kib;
        }
        let windows: Vec<speed_schedule::SpeedWindow> = self
            .settings
            .speed_windows
            .iter()
            .filter_map(|w| speed_schedule::SpeedWindow::parse(w).ok())
            .collect();
        speed_schedule::rate_at(
            speed_schedule::local_minute_of_day(),
            &windows,
            self.settings.max_speed_kib as u32,
        ) as u64
    }
}

/// 状态栏里那种「胶囊按钮」：圆角淡底 + 文字 + **画出来的**下拉箭头。
///
/// 为什么不用 `egui::Button` 加一个 `▾`：见 [`App::speed_limit_pill`] 的注释 ——
/// 那个字符在这台机器的字体里是空心方框（截图实测）。
fn pill_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, theme::INK_2);
    // 左内边距 8 + 文字 + 间距 6 + 箭头 9 + 右内边距 8。
    let width = galley.size().x + 31.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        99.0,
        if response.hovered() {
            theme::ACCENT_SOFT
        } else {
            theme::SURFACE_2
        },
    );
    let galley_y = rect.center().y - galley.size().y / 2.0;
    painter.galley(
        egui::pos2(rect.left() + 8.0, galley_y),
        galley,
        theme::INK_2,
    );
    let chevron = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        egui::vec2(9.0, 9.0),
    );
    super::icons::draw_icon(
        painter,
        super::icons::Icon::ChevronDown,
        chevron,
        theme::INK_3,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::{SPEED_PRESETS, speed_limit_label};

    #[test]
    fn the_speed_limit_label_says_unlimited_in_words() {
        assert_eq!(speed_limit_label(0), "不限", "0 是『不限』，不是 0 B/s");
        // 512 KiB/s = 524288 B/s，走同一个 fmt_speed（三位数以上不带小数）。
        assert_eq!(speed_limit_label(512), "512 KiB/s");
    }

    #[test]
    fn the_speed_limit_label_reports_binary_units() {
        // 1 MiB/s 在设置里是 1024 KiB/s；显示走同一个 fmt_speed，避免两处口径不一致。
        assert_eq!(speed_limit_label(1024), "1.0 MiB/s");
        assert_eq!(speed_limit_label(2048), "2.0 MiB/s");
    }

    #[test]
    fn the_preset_list_starts_with_unlimited_and_is_sorted() {
        assert_eq!(
            SPEED_PRESETS[0], 0,
            "第一项必须是『不限』：最常做的是先放开"
        );
        assert!(
            SPEED_PRESETS.windows(2).all(|w| w[0] < w[1]),
            "档位要按从小到大排，肉眼扫的时候才有方向感"
        );
    }
}
