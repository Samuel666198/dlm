//! 设置：`settings.json` 的读写与默认值。
//!
//! ## 文件位置
//!
//! `%LOCALAPPDATA%\dlm\settings.json`，与 `queue.json` 平级。
//!
//! ## 优先级（勘误 E5）
//!
//! **CLI > settings.json > 代码默认**。
//! 例：`--out-dir` 给了就用它，没给才看设置文件，都没有才用 `Downloads`。
//! 设置面板对"被命令行覆盖"的项要显示出来，不能假装生效。
//!
//! ## 向前兼容
//!
//! 每个字段都带 `#[serde(default)]`：老文件缺字段就用默认值，不会整份失效。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 设置文件名。与 `queue.json` 同目录。
pub const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // ---- 下载 ----
    /// 每个任务的连接数。
    pub connections: usize,
    /// 同一服务器的最大连接数（IDM `226`）。0 = 不限。
    pub per_host_connections: usize,
    /// 默认保存目录。`None` = `%USERPROFILE%\Downloads`。
    pub out_dir: Option<PathBuf>,
    /// 全局限速（KiB/s）。0 = 不限。
    pub max_speed_kib: u64,
    /// 按类型分子目录（视频 / 音频 / 压缩包…），规则见 `queue::category_for`。
    ///
    /// 落点从 `out_dir/名字` 变成 `out_dir/视频/名字`。**只影响新任务**：
    /// 已有任务不改落点，否则进度统计会对着两个文件说话。
    pub category_dirs: bool,
    /// 完成后用 Windows Defender 扫一遍（默认关：扫描要占资源）。
    pub scan_after_done: bool,

    // ---- 行为 ----
    /// 监视剪贴板里的下载链接。**默认关**（隐私，见设计稿第八节）。
    pub watch_clipboard: bool,
    /// 完成后动作：`""` = 无，`open` / `run:<exe>`。
    pub on_complete: String,
    /// 开机自启（与 `--autostart-install` 同源，这里只反映状态）。
    pub autostart: bool,

    // ---- 界面 ----
    /// 关闭窗口时收进托盘。
    pub close_to_tray: bool,
    /// 收进托盘时提示一次。
    pub tray_hint: bool,
    /// 是否已经提示过（避免每次都说）。与 `tray_hint` 分开，语义才清楚。
    pub tray_hint_shown: bool,

    // ---- 更新 ----
    /// 启动后自动检查更新。**默认开**（用户裁定：默认自动检查）。
    ///
    /// 关掉之后 `gui::run` **不再起检查线程**，一个外网请求都不发 ——
    /// 这是「设置里能关」的实际含义，不能只在界面上打个勾。
    pub auto_check_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: 8,
            per_host_connections: 8,
            out_dir: None,
            max_speed_kib: 0,
            category_dirs: false,
            scan_after_done: false,
            watch_clipboard: false,
            on_complete: String::new(),
            autostart: false,
            close_to_tray: true,
            tray_hint: true,
            tray_hint_shown: false,
            auto_check_update: true,
        }
    }
}

/// 「默认保存目录」输入框的解析：留空 = 用兜底（`--out-dir` / 下载文件夹）。
///
/// 为什么不接受相对路径：这个值会被 `queue::target_path` 直接拼到文件名前面，
/// 相对路径的基准是"进程当前目录"—— GUI 的当前目录是哪里没人说得清，
/// 于是下载会落到一个用户找不到的地方。要绝对路径才让"我设了它"有意义。
pub fn parse_out_dir_input(raw: &str) -> Result<Option<PathBuf>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("要写绝对路径，例如 D:\\Downloads".to_string());
    }
    Ok(Some(path))
}

impl Settings {
    /// 从 `queue.json` 同目录读。文件不存在或读不懂都用默认值 ——
    /// 设置坏了不该让程序起不来。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let Ok(raw) = std::fs::read(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&raw).unwrap_or_default()
    }

    /// 原子写：tmp + rename，避免断电留半截文件。
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("建设置目录失败：{}", dir.display()))?;
        let path = dir.join(FILE_NAME);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("写设置临时文件失败：{}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("提交设置文件失败：{}", path.display()))?;
        Ok(())
    }

    /// 实际生效的保存目录：`out_dir` 没设就用 `Downloads`。
    pub fn effective_out_dir(&self) -> PathBuf {
        self.out_dir.clone().unwrap_or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Downloads")
        })
    }

    /// 完成后动作是否合法。空串 = 无动作。
    pub fn on_complete_ok(&self) -> bool {
        let s = self.on_complete.trim();
        s.is_empty() || s == "open" || s.starts_with("run:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_ones() {
        let d = Settings::default();
        assert_eq!(d.connections, 8);
        assert_eq!(d.per_host_connections, 8);
        assert_eq!(d.max_speed_kib, 0, "默认不限速");
        assert!(
            !d.watch_clipboard,
            "剪贴板监视默认必须关：里面有密码、验证码、私钥"
        );
        assert!(d.close_to_tray, "关窗收托盘是默认行为");
        assert!(d.tray_hint, "首次收进托盘要提示");
        assert!(!d.tray_hint_shown);
        assert!(d.auto_check_update, "更新检查默认开（用户裁定）");
    }

    /// 老文件缺字段必须能读出来 —— 否则升级一次设置就全丢。
    #[test]
    fn a_settings_file_missing_fields_falls_back_per_field() {
        let dir = std::env::temp_dir().join(format!("dlm-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 只有两个字段的"老版本"文件。
        std::fs::write(
            dir.join(FILE_NAME),
            r#"{"connections":4,"watch_clipboard":true}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.connections, 4, "写了的字段要读到");
        assert!(s.watch_clipboard);
        assert_eq!(s.per_host_connections, 8, "没写的字段用默认");
        assert!(s.close_to_tray, "没写的字段用默认");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 文件损坏也要能起 —— 设置不该是启动的硬依赖。
    #[test]
    fn a_corrupt_settings_file_yields_defaults_instead_of_failing() {
        let dir = std::env::temp_dir().join(format!("dlm-settings-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), b"not json at all").unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.connections, 8);
        assert!(!s.watch_clipboard);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_then_load_roundtrips_every_field() {
        let dir = std::env::temp_dir().join(format!("dlm-settings-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let s = Settings {
            connections: 16,
            per_host_connections: 4,
            max_speed_kib: 2048,
            watch_clipboard: true,
            close_to_tray: false,
            on_complete: "open".into(),
            auto_check_update: false,
            ..Default::default()
        };
        s.save(&dir).unwrap();

        let back = Settings::load(&dir);
        assert_eq!(back.connections, 16);
        assert_eq!(back.per_host_connections, 4);
        assert_eq!(back.max_speed_kib, 2048);
        assert!(back.watch_clipboard);
        assert!(!back.close_to_tray);
        assert_eq!(back.on_complete, "open");
        assert!(!back.auto_check_update, "更新检查开关也要落盘、要读回来");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn on_complete_accepts_only_the_documented_shapes() {
        let mut s = Settings::default();
        assert!(s.on_complete_ok(), "空串 = 无动作，合法");
        s.on_complete = "open".into();
        assert!(s.on_complete_ok());
        s.on_complete = "run:notepad.exe".into();
        assert!(s.on_complete_ok());
        s.on_complete = "shutdown".into();
        assert!(
            !s.on_complete_ok(),
            "shutdown/休眠不进队列设置：队列可能同时完成多个任务（与 validate_on_complete 一致）"
        );
        s.on_complete = "wat".into();
        assert!(!s.on_complete_ok());
    }
}
