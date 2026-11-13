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
        let local = self.local.clone();
        let spawned = std::thread::Builder::new()
            .name("dlm-gui-update-manual".into())
            .spawn(move || {
                let _ = tx.send(crate::gui::check_update(&url, &local));
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
        let mut install = false;
        let mut copy_ext_page = false;
        let mut open_ext_dir = false;
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
                // 核心版本只在**真的会变**时才说。
                //
                // 以前这里无条件写「桌面端 0.1.0（不变）」，用户看到的是
                // "0.3.1 → 0.5.0" 旁边挂着个 0.1.0，只会以为版本号乱了 ——
                // 那个数是 crate 版本（引擎），跟发行版不是一回事，不变就不该出镜。
                let core_target = view.core.clone().unwrap_or_default();
                if !core_target.is_empty() && core_target != view.core_current {
                    ui.label(
                        egui::RichText::new(format!(
                            "引擎 {} → {}",
                            view.core_current, core_target
                        ))
                        .size(11.5)
                        .color(theme::INK_2),
                    );
                }

                // 扩展：不上商店，所以永远要用户手动重载。
                // 三个版本号（清单 / 磁盘 / 浏览器里跑的）的关系，就是这段话的全部信息量。
                if let Some(line) = extension_status(
                    view.extension.as_deref(),
                    view.extension_disk.as_deref(),
                    view.extension_running.as_deref(),
                ) {
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

                if let Some(err) = &self.update_install_error {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(err).size(11.0).color(theme::BAD));
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    // 能不能自动装，由 `installable` 说：有摘要、满足 min_core。
                    // 不能装时按钮是灰的，原因上面那行 `blocked` 已经写清楚。
                    let can_install = view.installable && self.update_rx.is_none();
                    let install_hint = if view.installable {
                        "下载并安装：会关掉 dlm，装完自动重开"
                    } else {
                        "这个版本不能自动安装（原因见上）"
                    };
                    if ui
                        .add_enabled(can_install, egui::Button::new("立即更新"))
                        .on_hover_text(install_hint)
                        .clicked()
                    {
                        install = true;
                    }
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

                // 扩展要用户手动重载。浏览器**不允许**程序化打开 `chrome://extensions`
                // （`start chrome://…` 会被当成非法地址），所以这里能做的是：
                // 把地址复制到剪贴板、把扩展目录打开。按钮上写清楚是「复制」，不写「打开」。
                if extension_needs_reload(
                    view.extension_disk.as_deref(),
                    view.extension_running.as_deref(),
                ) {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("扩展重载")
                                .size(10.5)
                                .color(theme::INK_3),
                        );
                        if ui
                            .button("复制 chrome://extensions")
                            .on_hover_text("粘到浏览器地址栏回车，点『重新加载』")
                            .clicked()
                        {
                            copy_ext_page = true;
                        }
                        if ui.button("打开扩展目录").clicked() {
                            open_ext_dir = true;
                        }
                    });
                }
            });

        if install {
            match crate::gui::spawn_updater(&self.update_view(), &self.settings_dir) {
                Ok(()) => {
                    // 更新器已经起来了，它会等我们退出再动文件。现在退出。
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(err) => {
                    self.update_install_error = Some(format!("启动更新器失败：{err:#}"));
                }
            }
        }
        if copy_ext_page {
            ctx.copy_text("chrome://extensions".to_string());
        }
        if open_ext_dir {
            open_external(&crate::gui::default_extension_dir().display().to_string());
        }
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

/// 扩展那一行怎么说。
///
/// 三个版本号的关系是这段话的全部信息量：
///
/// | 来源 | 含义 |
/// |---|---|
/// | `target` | 清单（更新包）里带的扩展版本 |
/// | `disk` | 磁盘上已经落地的版本（updater 写进去的） |
/// | `running` | 浏览器里**真正在跑**的版本（扩展自己报的） |
fn extension_status(
    target: Option<&str>,
    disk: Option<&str>,
    running: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match (target, disk) {
        (Some(t), Some(d)) if t != d => parts.push(format!("更新包里的扩展是 {t}，磁盘上还是 {d}")),
        (Some(_), Some(d)) => parts.push(format!("扩展 {d}")),
        (Some(t), None) => parts.push(format!("更新包里的扩展是 {t}")),
        (None, Some(d)) => parts.push(format!("扩展 {d}")),
        (None, None) => {}
    }
    match (disk, running) {
        (Some(d), Some(r)) if d == r => parts.push("浏览器里已在运行".to_string()),
        (_, Some(r)) => parts.push(format!(
            "浏览器里跑的还是 {r}，请到 chrome://extensions 点『重新加载』"
        )),
        (Some(_), None) => {
            parts.push("浏览器还没上报版本；若刚更新过，请点一次『重新加载』".to_string())
        }
        (None, None) => {}
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// 是不是「文件已就位、浏览器还在跑旧的」那种状态 —— 也正是要显示重载按钮的时候。
fn extension_needs_reload(disk: Option<&str>, running: Option<&str>) -> bool {
    matches!((disk, running), (Some(d), Some(r)) if d != r)
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

    /// 「文件已就位、浏览器还在跑旧的」是唯一需要提示重载的状态。
    #[test]
    fn the_extension_line_names_all_three_versions_when_they_disagree() {
        // 装完了但没重载：磁盘 0.2.13，浏览器里 0.2.11。
        let line = extension_status(Some("0.2.13"), Some("0.2.13"), Some("0.2.11")).unwrap();
        assert!(line.contains("0.2.13"), "{line}");
        assert!(line.contains("0.2.11"), "{line}");
        assert!(line.contains("重新加载"), "{line}");
        assert!(extension_needs_reload(Some("0.2.13"), Some("0.2.11")));
    }

    /// 已经重载过 ⇒ 说「已在运行」，且不再显示重载按钮。
    #[test]
    fn a_reloaded_extension_needs_no_action() {
        let line = extension_status(Some("0.2.13"), Some("0.2.13"), Some("0.2.13")).unwrap();
        assert!(line.contains("已在运行"), "{line}");
        assert!(!extension_needs_reload(Some("0.2.13"), Some("0.2.13")));
        assert!(
            !extension_needs_reload(Some("0.2.13"), None),
            "没上报就不催"
        );
    }

    /// 还没装：清单里有新版扩展，磁盘上还是旧的。
    #[test]
    fn an_uninstalled_extension_says_the_package_carries_a_newer_one() {
        let line = extension_status(Some("0.2.13"), Some("0.2.12"), Some("0.2.12")).unwrap();
        assert!(line.contains("更新包里的扩展是 0.2.13"), "{line}");
        assert!(line.contains("磁盘上还是 0.2.12"), "{line}");
    }

    /// 什么都不知道就说空话，别在界面上刷一行「扩展」。
    #[test]
    fn no_extension_information_renders_nothing() {
        assert_eq!(extension_status(None, None, None), None);
    }
}
