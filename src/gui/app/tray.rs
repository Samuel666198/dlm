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

/// 托盘图标：32×32 的原始 RGBA 像素，由 `packaging/make-icon.py` 从源图烘出。
///
/// 与 exe 资源段里的 `dlm.ico`、窗口图标是**同一份画**。以前这里是手写循环画的
/// （圆角方块 + 白箭头），换新图标后手绘版和真图标会长得不一样 —— 托盘里一个样、
/// 任务栏里另一个样，比"图标不好看"更难解释。所以三处改成共用一份像素。
///
/// 为什么是裸 RGBA 而不是 PNG：项目没有引入图片解码依赖（`Cargo.lock` 里连 `png`
/// 都没有）。运行时解码要么加一个 crate，要么把像素烘进仓库 —— 选后者，与
/// `updater` 直接调 `powershell` 解压是同一个取舍：不为一件事拖进依赖树。
const TRAY_ICON_SIZE: u32 = 32;
const TRAY_ICON_RGBA: &[u8] = include_bytes!("../../../assets/icon/dlm-32.rgba");

/// 尺寸写在两个地方就必须对得上。对不上的话 `Icon::from_rgba` 只在**运行时**报错，
/// 而那是托盘 —— 用户看到的是"没有图标"，不是一条错误。用编译期断言挡住。
const _: () = assert!(
    TRAY_ICON_RGBA.len() == (TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4) as usize,
    "dlm-32.rgba 字节数必须等于 32*32*4；跑 packaging/make-icon.py 重新生成"
);

pub fn make_icon() -> Icon {
    Icon::from_rgba(TRAY_ICON_RGBA.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE)
        .expect("尺寸已由编译期断言保证，不会失败")
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
            .set_tooltip(Some("Download Manager 仍在下载 — 双击图标恢复窗口"));
    }

    /// 窗口恢复后把 tooltip 换回默认。
    pub fn hint_idle(&self) {
        let _ = self._icon.set_tooltip(Some("Download Manager"));
    }
}

/// 同一个提示至少隔 60 秒才再改一次 —— 反复点 X 不该反复刷。
pub fn should_notify(now_secs: u64, last_secs: u64) -> bool {
    now_secs.saturating_sub(last_secs) >= 60
}

#[cfg(test)]
mod tests {
    use super::{TRAY_ICON_RGBA, TRAY_ICON_SIZE, should_notify};

    #[test]
    fn the_hint_is_throttled_to_once_a_minute() {
        assert!(should_notify(1_000, 0));
        assert!(!should_notify(1_059, 1_000), "60 秒内不重复");
        assert!(should_notify(1_060, 1_000));
        assert!(!should_notify(5, 100), "时钟回拨也不 panic");
    }

    /// 托盘图标必须是**烘进来的那份像素**，不是手绘的。
    ///
    /// 2026-11-19 换了新图标（三支箭汇成一支）。原先 `make_icon()` 是手写循环画的
    /// 圆角方块 + 白箭头；换图后如果只改了 `dlm.ico` 而漏了这里，任务栏里是新图标、
    /// 托盘里还是旧的手绘箭头 —— 同一个程序两个样子。
    ///
    /// 期望值取自实测（跑完 `packaging/make-icon.py` 后读出来的真实像素）：
    /// 四角透明、中心奶白（中间那支箭的杆）、左侧陶土橙（背景）。
    #[test]
    fn the_tray_icon_pixels_are_the_baked_asset() {
        const S: u32 = TRAY_ICON_SIZE;
        let px = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * S + x) * 4) as usize;
            [
                TRAY_ICON_RGBA[i],
                TRAY_ICON_RGBA[i + 1],
                TRAY_ICON_RGBA[i + 2],
                TRAY_ICON_RGBA[i + 3],
            ]
        };

        assert_eq!(TRAY_ICON_RGBA.len(), (S * S * 4) as usize);
        assert_eq!(
            px(0, 0)[3],
            0,
            "squircle 之外必须透明，否则任务栏上是一个白方块"
        );
        let center = px(S / 2, S / 2);
        assert!(
            center[0] > 200 && center[1] > 180 && center[2] > 160,
            "中心应是箭杆的奶白色，实际 {center:?}"
        );
        let bg = px(S / 6, S / 2);
        assert!(
            bg[3] > 200 && bg[0] > 150 && bg[0] > bg[1] && bg[1] > bg[2],
            "左侧应是陶土橙背景（R>G>B），实际 {bg:?}"
        );

        // **三支箭的结构判据**（这条才能区分新旧图标）：
        // 新图标左上、右上各有一支箭，中段空隙是背景色。
        // 旧的手绘版只有一支居中箭头 —— 那两个位置都会是背景，所以这三点一起断言，
        // 换成旧图标（或任何"单箭头"画法）必然挂。
        let cream = |c: [u8; 4]| c[0] > 200 && c[1] > 180 && c[2] > 160;
        let left = px(S * 30 / 100, S * 32 / 100);
        let right = px(S * 70 / 100, S * 32 / 100);
        let gap = px(S * 40 / 100, S * 40 / 100);
        assert!(cream(left), "左上应是第二支箭的奶白色，实际 {left:?}");
        assert!(cream(right), "右上应是第三支箭的奶白色，实际 {right:?}");
        assert!(
            !cream(gap),
            "两支箭之间应是背景色（R>G>B），实际 {gap:?} —— 这是单箭头/纯色块的形态"
        );
        assert!(
            gap[3] > 200 && gap[0] > gap[1] && gap[1] > gap[2],
            "空隙是陶土橙背景，实际 {gap:?}"
        );

        // 奶白像素太少说明图标根本没画上去（全透明或纯色块也会在这里挂）。
        let mut light = 0;
        for y in 0..S {
            for x in 0..S {
                if cream(px(x, y)) {
                    light += 1;
                }
            }
        }
        assert!(light > 20, "奶白色像素只有 {light} 个，图标可能没画上");
    }
}
