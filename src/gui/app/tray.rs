//! 系统托盘：图标 + 菜单 + 事件。
//!
//! ## 线程模型（勘误 E3 的更正）
//!
//! `tray-icon` **不要求独立线程**，要求的是「创建它的那个线程上有消息泵」。
//! Windows 上消息泵由 winit 提供，所以标准做法是：
//!
//! 1. 在**主线程**（eframe 初始化时）建 icon
//! 2. 在 `App::ui` 里 poll `MenuEvent::receiver()` 与 `TrayIconEvent::receiver()`
//!
//! 这样不需要自己写 Win32 消息循环，也不用把菜单动作 marshal 回 egui 线程。
//!
//! ## 图标
//!
//! 用代码画一个 32×32 的陶土橙圆角方块 + 白色下载箭头，不依赖外部资源文件
//! （仓库里现在一枚图标都没有，见设计稿第三节）。同一份像素也用来生成窗口图标。

use std::sync::mpsc::{Receiver, channel};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// 托盘菜单动作。UI 线程轮询这个通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    PauseAll,
    NewTask,
    Quit,
}

/// 门禁/单元用例里别建托盘（`DLM_NO_TRAY`）。
///
/// 与 `DLM_NO_SHELL_OPEN` / `DLM_NO_POPUP` 同一类约定：自动化里不许往用户桌面上
/// 扔东西。`gates.ps1` 统一置上，用例不用各自加参数。
pub fn suppressed() -> bool {
    std::env::var_os("DLM_NO_TRAY").is_some()
}

/// 持有托盘句柄。丢掉它图标就消失，所以必须活到进程结束。
pub struct Tray {
    _icon: TrayIcon,
    rx: Receiver<TrayAction>,
}

impl Tray {
    /// 建托盘。必须在主线程调用（Windows 要求创建线程有消息泵）。
    ///
    /// 门禁与单元用例里直接返回 `None`（`DLM_NO_TRAY`，见 [`suppressed`]）：
    /// 那儿的 `App` 建在 cargo 测试线程上，没有消息泵，图标却会真的出现在用户
    /// 任务栏上 —— 跑一次门禁就在桌面上留一枚点了没反应的图标。
    pub fn new() -> Option<Self> {
        if suppressed() {
            return None;
        }
        let menu = Menu::new();

        let show = MenuItem::new("显示窗口", true, None);
        let pause_all = MenuItem::new("全部暂停", true, None);
        let new_task = MenuItem::new("新建任务", true, None);
        let quit = MenuItem::new("退出", true, None);
        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();

        // 菜单项 id 与动作一一对应，事件回来时按 id 查。
        let show_id = show.id().clone();
        let pause_id = pause_all.id().clone();
        let new_id = new_task.id().clone();
        let quit_id = quit.id().clone();

        menu.append_items(&[&show, &sep1, &pause_all, &new_task, &sep2, &quit])
            .ok()?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("dlm")
            .with_icon(make_icon())
            .build()
            .ok()?;

        // 菜单事件：单独一个线程把 id 翻译成动作，塞进 channel。
        // 这个线程只读 tray-icon 的全局 receiver，不碰 UI 状态。
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("dlm-tray-menu".into())
            .spawn(move || {
                let menu_rx = MenuEvent::receiver();
                let tray_rx = TrayIconEvent::receiver();
                loop {
                    if let Ok(ev) = menu_rx.try_recv() {
                        let action = if ev.id == show_id {
                            Some(TrayAction::Show)
                        } else if ev.id == pause_id {
                            Some(TrayAction::PauseAll)
                        } else if ev.id == new_id {
                            Some(TrayAction::NewTask)
                        } else if ev.id == quit_id {
                            Some(TrayAction::Quit)
                        } else {
                            None
                        };
                        if let Some(a) = action
                            && tx.send(a).is_err()
                        {
                            return;
                        }
                    }
                    // 左键单击图标 = 显示窗口（与双击行为一致，双击会连发两次）。
                    if let Ok(TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }) = tray_rx.try_recv()
                    {
                        let _ = tx.send(TrayAction::Show);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            })
            .ok()?;

        Some(Self { _icon: icon, rx })
    }

    /// 非阻塞取一个动作。
    pub fn poll(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }

    /// 更新 tooltip。剪贴板监视命中时用它提示（`tray-icon` 不暴露气泡，见勘误 E13）。
    pub fn set_tooltip(&self, text: &str) {
        let _ = self._icon.set_tooltip(Some(text));
    }
}

/// 生成 32×32 的托盘图标：陶土橙圆角方块 + 白色下箭头。
///
/// 与 `theme::ACCENT` 同色，保证托盘与窗口是同一套视觉。
pub fn make_icon() -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    let accent = [0xC0u8, 0x63, 0x40, 0xFF];
    let white = [0xFFu8, 0xFF, 0xFF, 0xFF];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            // 圆角半径 7：四角裁掉。
            let r = 7i32;
            let in_corner = |x: i32, y: i32| -> bool {
                let dx = if x < r {
                    r - x
                } else if x > SIZE as i32 - 1 - r {
                    x - (SIZE as i32 - 1 - r)
                } else {
                    0
                };
                let dy = if y < r {
                    r - y
                } else if y > SIZE as i32 - 1 - r {
                    y - (SIZE as i32 - 1 - r)
                } else {
                    0
                };
                dx * dx + dy * dy > r * r
            };
            if in_corner(x as i32, y as i32) {
                continue;
            }
            rgba[i..i + 4].copy_from_slice(&accent);

            // 白色下箭头：竖杆 + 三角头。
            let cx = SIZE as i32 / 2;
            let in_shaft = (x as i32 - cx).abs() <= 1 && (9..=18).contains(&(y as i32));
            let in_head = {
                let dy = y as i32 - 17;
                (0..=6).contains(&dy) && (x as i32 - cx).abs() <= 6 - dy
            };
            if in_shaft || in_head {
                rgba[i..i + 4].copy_from_slice(&white);
            }
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).expect("32x32 RGBA 一定合法")
}
impl Tray {
    /// 收进托盘时给个提示。
    ///
    /// 说明：`tray-icon` 0.21 不暴露 `Shell_NotifyIcon` 需要的 hwnd/uID，所以
    /// **做不了真正的气泡**（NIF_INFO）—— 那条路要求自己注册托盘图标。这里退一步，
    /// 把 tooltip 换成"还在下载"，鼠标一悬停就能看见；真正的气泡记为 0.6.0 的选项。
    pub fn hint_downloading(&self) {
        let _ = self
            ._icon
            .set_tooltip(Some("dlm 仍在下载 — 双击图标恢复窗口"));
    }

    /// 窗口恢复后把 tooltip 换回默认。
    pub fn hint_idle(&self) {
        let _ = self._icon.set_tooltip(Some("dlm 下载器"));
    }
}

/// 同一个提示至少隔 60 秒才再改一次 —— 反复点 X 不该反复刷。
pub fn should_notify(now_secs: u64, last_secs: u64) -> bool {
    now_secs.saturating_sub(last_secs) >= 60
}

#[cfg(test)]
mod tests {
    use super::should_notify;

    #[test]
    fn the_hint_is_throttled_to_once_a_minute() {
        assert!(should_notify(1_000, 0));
        assert!(!should_notify(1_059, 1_000), "60 秒内不重复");
        assert!(should_notify(1_060, 1_000));
        assert!(!should_notify(5, 100), "时钟回拨也不 panic");
    }
}
