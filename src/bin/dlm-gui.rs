//! dlm 产品层入口：本地 Web UI + 队列。默认依赖仍是 6 个（无 GUI crate）。
//!
//! 运行后打印 `http://127.0.0.1:<port>/?t=<token>`；用浏览器打开。进程活着 = 队列/调度器活着。
//!
//! ## 为什么是 windows 子系统（不弹那个黑终端）
//!
//! Rust 默认把 bin 编成**控制台**子系统，于是双击 `dlm-gui.exe` 时 Windows 先开一个
//! 黑窗口再跑 GUI —— 用户看到的就是「打开软件附带出现一个终端」，关掉它还会连带杀掉程序。
//! 改成 `windows` 子系统后不分配控制台。
//!
//! 代价与判据：`println!`/`eprintln!` 仍然照常工作，**前提是有人把 stdout/stderr 接走**
//! （集成测试就是 `.stdout(Stdio::piped())` / 重定向到文件）；没人接的时候这些行直接丢弃 ——
//! 对 GUI 来说这正是想要的。`dlm.exe`（CLI）**必须保持控制台子系统**，否则命令行下看不到输出。
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use dlm::consts::PROBE_READ_BYTES;
use dlm::gui::{self, GuiConfig};
use dlm::queue::QueueStore;

fn main() -> ExitCode {
    let headless = std::env::args().any(|a| a == "--headless");
    let config = match parse() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("dlm-gui: {err:#}");
            report_fatal(&format!("{err:#}"), headless);
            return ExitCode::from(2);
        }
    };
    match gui::run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dlm-gui: {err:#}");
            report_fatal(&format!("{err:#}"), headless);
            ExitCode::FAILURE
        }
    }
}

/// 起不来的时候**必须让人看见**。
///
/// 这个 exe 是 `windows` 子系统（为了不弹黑终端），于是没有控制台可以 `eprintln!` 给人看：
/// 双击以后失败，用户只会看到「什么也没发生」。所以 GUI 模式下补一个消息框。
///
/// `headless` 时不弹（集成测试都用 `--headless`，弹框会把用例挂死），只留 stderr。
fn report_fatal(message: &str, headless: bool) {
    if headless {
        return;
    }
    #[cfg(windows)]
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        let text = std::ffi::OsStr::new(message);
        let mut wide: Vec<u16> = text.encode_wide().collect();
        wide.push(0);
        let caption: Vec<u16> = "dlm 启动失败"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        const MB_OK: u32 = 0x0000_0000;
        const MB_ICONERROR: u32 = 0x0000_0010;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut core::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                kind: u32,
            ) -> i32;
        }
        MessageBoxW(
            std::ptr::null_mut(),
            wide.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    #[cfg(not(windows))]
    let _ = message;
}

fn parse() -> Result<GuiConfig> {
    let mut config = GuiConfig {
        queue: local_app_data().join("dlm").join("queue.json"),
        out_dir: user_profile().join("Downloads"),
        log_dir: local_app_data().join("dlm").join("gui").join("logs"),
        dlm_exe: same_dir("dlm.exe"),
        concurrency: 2,
        port: 0,
        token: None,
        quota_limit: None,
        quota_window_secs: None,
        endpoint: local_app_data()
            .join("dlm")
            .join("gui")
            .join("endpoint.json"),
        update_url: Some(gui::DEFAULT_UPDATE_URL.to_string()),
        update_check_delay_secs: gui::UPDATE_CHECK_DELAY_SECS,
        category_dirs: false,
        open: false,
        max_task_retries: 2,
        retry_backoff: gui::DEFAULT_RETRY_BACKOFF,
        force_new_instance: false,
        autostart: false,
        headless: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => config.port = next(&mut args, "--port")?.parse()?,
            "--queue" => config.queue = PathBuf::from(next(&mut args, "--queue")?),
            "--out-dir" => config.out_dir = PathBuf::from(next(&mut args, "--out-dir")?),
            "--log-dir" => config.log_dir = PathBuf::from(next(&mut args, "--log-dir")?),
            "--dlm" => config.dlm_exe = PathBuf::from(next(&mut args, "--dlm")?),
            "--concurrency" => config.concurrency = next(&mut args, "--concurrency")?.parse()?,
            "--token" => config.token = Some(next(&mut args, "--token")?),
            "--quota" => config.quota_limit = Some(next(&mut args, "--quota")?.parse()?),
            "--quota-window" => {
                config.quota_window_secs = Some(next(&mut args, "--quota-window")?.parse()?)
            }
            "--endpoint" => config.endpoint = PathBuf::from(next(&mut args, "--endpoint")?),
            "--update-url" => {
                let raw = next(&mut args, "--update-url")?;
                let parsed = reqwest::Url::parse(&raw)
                    .map_err(|err| anyhow::anyhow!("--update-url 不是合法 URL：{err}"))?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    bail!("--update-url 只接受 http:// 或 https://");
                }
                config.update_url = Some(raw);
                // 显式给了清单地址：有人正在验证更新链路，别让他等那十秒冷启动延迟。
                config.update_check_delay_secs = 0;
            }
            "--category-dirs" => config.category_dirs = true,
            // 默认会查更新（`gui::DEFAULT_UPDATE_URL`）。这一条断网/离线环境用：
            // 关掉后 `gui::run` 一个外网请求都不发。
            "--no-update-check" => config.update_url = None,
            "--open" => config.open = true,
            "--max-task-retries" => {
                config.max_task_retries = next(&mut args, "--max-task-retries")?.parse()?
            }
            // P3：自启项调起来的那个实例。
            "--autostart" => config.autostart = true,
            // P3：显式再起一个（诊断/测试用；正常双击不会走到这里）。
            "--new-instance" => config.force_new_instance = true,
            // 勘误 E2：集成测试用，只起 runner + HTTP，不初始化窗口。
            "--headless" => config.headless = true,
            "--autostart-install" => {
                gui::set_autostart(true)?;
                println!("已写入开机自启（HKCU\\...\\Run\\{}）", gui::AUTOSTART_VALUE);
                std::process::exit(0);
            }
            "--autostart-remove" => {
                gui::set_autostart(false)?;
                println!("已移除开机自启");
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!(
                    "dlm-gui [--port N] [--queue FILE] [--out-dir DIR] [--log-dir DIR] [--dlm EXE] [--concurrency N] [--token T] [--quota BYTES] [--quota-window SECS] [--endpoint FILE] [--update-url URL] [--no-update-check] [--category-dirs] [--open] [--max-task-retries N] [--autostart] [--new-instance] [--headless] [--autostart-install] [--autostart-remove]"
                );
                std::process::exit(0);
            }
            other => bail!("未知参数 {other}（--help 看用法）"),
        }
    }
    if config.concurrency == 0 {
        bail!("--concurrency 至少 1");
    }
    if let Some(table) = test_backoff_from_env() {
        config.retry_backoff = table;
    }
    if config.quota_window_secs == Some(0) {
        bail!("--quota-window 至少 1 秒");
    }
    if config.quota_limit == Some(0) && config.quota_window_secs.is_some() {
        bail!("--quota 0 是关闭配额，不能再给 --quota-window");
    }
    let stored_limit = if config.quota_limit.is_none() && config.quota_window_secs.is_some() {
        QueueStore::load(config.queue.clone())?
            .quota()
            .map(|q| q.limit)
    } else {
        None
    };
    let effective = config.quota_limit.or(stored_limit).unwrap_or(0);
    if config.quota_window_secs.is_some() && effective == 0 {
        bail!("--quota-window 需要先启用 --quota，或队列里已经存有配额");
    }
    if effective > 0 && effective < PROBE_READ_BYTES {
        bail!("--quota 至少 {PROBE_READ_BYTES} 字节（首轮预读要落盘并计入配额）");
    }
    Ok(config)
}

fn next(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next().ok_or_else(|| anyhow::anyhow!("{name} 缺少值"))
}

/// 集成测试专用：`DLM_TEST_RETRY_BACKOFF_MS=50,50` 把退避表按**毫秒**注入，这里换算成秒。
///
/// **生产不会设这个变量**，所以默认仍是 [`gui::DEFAULT_RETRY_BACKOFF`] 的 30/120 **秒**。
/// 存在的理由只有一个：不注入的话，钉"退 1 两次再成功"那条用例要真等 150 秒，
/// 而退避表里那两个数是给人用的产品默认值，不该为了测试改成 1 秒。
///
/// 换算后每一项至少 1 秒 —— 内部时钟是秒粒度，0 秒会让"等到点"立刻成立，
/// 那等于没有退避，测出来的东西就不再是"重排 → 等 → 再跑"。
///
/// 表里任何一项解析失败 ⇒ 当作没设（回退默认），不静默变成空表 —— 空表等于关掉自动重试，
/// 那是会让测试"通过得很安静"的假绿。
fn test_backoff_from_env() -> Option<&'static [u64]> {
    let raw = std::env::var("DLM_TEST_RETRY_BACKOFF_MS").ok()?;
    let secs: Vec<u64> = raw
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<u64>()
                .ok()
                .map(|ms| ms.div_ceil(1000).max(1))
        })
        .collect::<Option<_>>()?;
    if secs.is_empty() {
        return None;
    }
    Some(Box::leak(secs.into_boxed_slice()))
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn user_profile() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn same_dir(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}
