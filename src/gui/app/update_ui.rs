//! 更新提示界面。
//!
//! ## 为什么以前「零显示」
//!
//! C3 阶段只做了骨架：`check_update` 会拉清单、`/api/update` 会返回结果，但原生界面
//! 一个字都没画（`App.update` 挂着 `#[allow(dead_code)]`）。用户永远看不到「有新版」。
//! 这一层就是把那条数据接到屏幕上。
//!
//! ## 三块
//!
//! | 位置 | 显示什么 |
//! |---|---|
//! | 状态栏右侧 | 有新版时一枚可点的标记；检查失败时一枚灰标记 |
//! | 更新面板 | 发行版 / 桌面端 / 扩展三个版本号 + 发布说明 + 不能自动装的原因 |
//! | 面板按钮 | 立即检查 / 打开发布页 |
//!
//! **不显示的东西**：「自动下载并安装」——那是 `dlm-updater.exe` 的活，正在做。
//!
//! ## 一条纪律：面板只读快照
//!
//! 面板不自己发网络请求以外的任何动作，检查结果走通道回 UI 线程 ——
//! 与列表、配额一样，UI 线程不碰队列、不碰子进程。

use eframe::egui;

use super::App;
use crate::gui::app::theme;
use crate::gui::snapshot::UpdateView;

impl App {
    /// 读一份更新视图。
    pub(super) fn update_view(&self) -> UpdateView {
        match self.update.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 把后台检查交来的结果收进槽里。每帧调一次，非阻塞。
    pub(super) fn drain_update(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.update_rx else { return };
        let received = match rx.try_recv() {
            Ok(view) => Some(view),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            // 线程没了（panic 或进程收尾）：把通道丢掉，界面别一直挂着「检查中」。
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.update_rx = None;
                ctx.request_repaint();
                return;
            }
        };
        let Some(view) = received else { return };
        // 借 `install_update_view` 换进去：它会保留扩展实时上报的那个版本号。
        crate::gui::install_update_view(&self.update, view);
        self.update_rx = None;
        ctx.request_repaint();
    }

    /// 「立即检查」：起一个后台线程拉清单，结果走通道回来。
    ///
    /// 已经在查就不再起第二个 —— 连点会打出多个并发请求，而清单走 CDN 有缓存，
    /// 多打一个是纯粹的浪费。
    pub(super) fn check_update_now(&mut self, ctx: &egui::Context) {
        let Some(url) = self.update_url.clone() else {
            return;
        };
        if self.update_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("dlm-gui-update-manual".into())
            .spawn(move || {
                let _ = tx.send(crate::gui::check_update(&url));
            });
        if spawned.is_ok() {
            self.update_rx = Some(rx);
            ctx.request_repaint();
        }
    }

    /// 状态栏右侧那枚标记。没更新、没失败就什么都不画。
    pub(super) fn update_badge(&mut self, ui: &mut egui::Ui) {
        let checking = self.update_rx.is_some();
        let view = self.update_view();
        if checking {
            ui.label(
                egui::RichText::new("检查更新…")
                    .size(11.0)
                    .color(theme::INK_3),
            );
            return;
        }
        if view.available {
            let latest = view.latest.clone().unwrap_or_default();
            let text = if latest.is_empty() {
                "有新版".to_string()
            } else {
                format!("有新版 {latest}")
            };
            let button =
                egui::Button::new(egui::RichText::new(text).size(11.0).color(theme::ACCENT))
                    .frame(false);
            if ui
                .add(button)
                .on_hover_text("点开看这个版本更新了什么")
                .clicked()
            {
                self.update_open = true;
            }
            return;
        }
        if view.error.is_some() {
            let button = egui::Button::new(
                egui::RichText::new("更新检查失败")
                    .size(11.0)
                    .color(theme::INK_3),
            )
            .frame(false);
            if ui.add(button).on_hover_text("点开看原因").clicked() {
                self.update_open = true;
            }
        }
    }

    /// 更新面板：三个版本号 + 说明 + 不能自动装的原因。
    pub(super) fn update_panel(&mut self, ctx: &egui::Context) {
        let mut open = self.update_open;
        let mut check_now = false;
        let mut close = false;
        let mut open_page = None;

        egui::Window::new("更新")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let view = self.update_view();
                let checking = self.update_rx.is_some();

                ui.add_space(6.0);
                // 发行版：唯一决定「要不要更新」的那个号。
                if view.available {
                    let latest = view.latest.clone().unwrap_or_default();
                    ui.label(
                        egui::RichText::new(format!("发行版 {} → {latest}", view.current))
                            .size(14.0)
                            .strong()
                            .color(theme::ACCENT),
                    );
                } else {
                    let current = view.latest.clone().unwrap_or(view.current.clone());
                    ui.label(
                        egui::RichText::new(format!("已是最新（发行版 {current}）"))
                            .size(13.0)
                            .color(theme::INK_2),
                    );
                }

                ui.add_space(8.0);
                // 桌面端：这次会不会换三个 exe。
                let core_target = view.core.clone().unwrap_or_default();
                let core_line = if core_target.is_empty() {
                    format!("桌面端 {}", view.core_current)
                } else if core_target == view.core_current {
                    format!("桌面端 {}（不变）", view.core_current)
                } else {
                    format!("桌面端 {} → {core_target}", view.core_current)
                };
                ui.label(egui::RichText::new(core_line).size(11.5).color(theme::INK_2));

                // 扩展：不上商店，所以永远要用户手动重载。
                // `extension_running` 是扩展自己报的「我现在跑的是哪个版本」——
                // 它等于清单版本才说明用户真的重载过了。
                if let Some(ext) = &view.extension {
                    let line = match &view.extension_running {
                        Some(running) if running == ext => {
                            format!("扩展 {ext}（浏览器里已在运行）")
                        }
                        Some(running) => format!(
                            "扩展 {ext}（浏览器里跑的还是 {running}，需到 chrome://extensions 点『重新加载』）"
                        ),
                        None => {
                            format!("扩展 {ext}（需到浏览器里点『重新加载』）")
                        }
                    };
                    ui.label(egui::RichText::new(line).size(11.5).color(theme::INK_2));
                }

                if let Some(notes) = &view.notes
                    && !notes.trim().is_empty()
                {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("这个版本改了什么")
                            .size(10.5)
                            .color(theme::INK_3),
                    );
                    ui.label(egui::RichText::new(notes).size(11.5).color(theme::INK_2));
                }

                // 有新版但不能自动装：原因必须写出来，否则用户只会看到「有新版」
                // 然后找不到任何能点的按钮。
                if let Some(blocked) = &view.blocked
                    && !blocked.trim().is_empty()
                {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(blocked).size(11.0).color(theme::BAD));
                }
                if let Some(err) = &view.error
                    && !err.trim().is_empty()
                {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(err).size(11.0).color(theme::BAD));
                }

                ui.add_space(4.0);
                let meta = match (view.enabled, self.update_url.as_deref()) {
                    (false, _) => "未启用更新检查".to_string(),
                    (true, None) => "已关闭更新检查（--no-update-check）".to_string(),
                    (true, Some(url)) => {
                        if view.checked_at.is_some() {
                            format!("清单：{url}")
                        } else {
                            format!("尚未检查 · 清单：{url}")
                        }
                    }
                };
                ui.label(egui::RichText::new(meta).size(10.0).color(theme::INK_3));

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_check = self.update_url.is_some() && !checking;
                    if ui
                        .add_enabled(can_check, egui::Button::new("立即检查"))
                        .clicked()
                    {
                        check_now = true;
                    }
                    if ui.button("关闭").clicked() {
                        // 不能直接改 `open`：它已经借给 `Window::open()` 了。
                        close = true;
                    }
                    if let Some(page) = view.url.as_deref().and_then(release_page) {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("打开发布页").clicked() {
                                open_page = Some(page);
                            }
                        });
                    }
                });
            });

        if check_now {
            self.check_update_now(ctx);
        }
        if let Some(page) = open_page {
            open_external(&page);
        }
        if close {
            open = false;
        }
        self.update_open = open;
    }
}

/// 包地址 ⇒ 发布页地址。
///
/// 清单里的 `url` 指向 Release 的**资产**（`/releases/download/v0.1.0/dlm-….zip`），
/// 用户点进去更想看的是发布说明那一页（`/releases/tag/v0.1.0`），不是直接开始下载。
/// 形状不符就返回 `None`，宁可没有这个按钮，也不要给一个 404。
fn release_page(download_url: &str) -> Option<String> {
    let (prefix, _) = download_url.split_once("/releases/download/")?;
    if !prefix.starts_with("https://") && !prefix.starts_with("http://") {
        return None;
    }
    Some(format!("{prefix}/releases"))
}

/// 用系统默认方式打开一个网页。
fn open_external(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_asset_url_maps_to_its_release_page() {
        assert_eq!(
            release_page("https://github.com/o/r/releases/download/v0.1.0/dlm-0.1.0-win-x64.zip")
                .as_deref(),
            Some("https://github.com/o/r/releases")
        );
    }

    /// 不是 Release 资产形状的地址 ⇒ 没有「打开发布页」按钮，
    /// 而不是拼一个点进去 404 的链接。
    #[test]
    fn a_url_that_is_not_a_release_asset_has_no_release_page() {
        assert_eq!(release_page("https://example.invalid/dlm.zip"), None);
        assert_eq!(release_page("file:///releases/download/x"), None);
        assert_eq!(release_page(""), None);
    }
}
