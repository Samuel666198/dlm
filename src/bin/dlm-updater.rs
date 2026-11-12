//! `dlm-updater.exe`：下载 → 校验 → 解压 → 替换 → 回滚 → 重启。
//!
//! ## 为什么要独立一个 exe
//!
//! Windows 上正在运行的程序不能覆盖自己的 exe。所以更新流程必然是
//! `dlm-gui` 确认 → 起 updater → `dlm-gui` 退出 → updater 换文件 → 重启 `dlm-gui`。
//! `dlm-gui` 不是直接跑安装目录里的 updater，而是先把它**拷到临时目录**再跑 ——
//! 这样这个 exe 自己也在可替换的名单里（否则它每次更新都会落后一个版本）。
//!
//! ## 退出码
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 更新完成（或已回滚并重启旧版本） |
//! | 2 | 参数不对 |
//! | 3 | sha256 不符（**不装**） |
//! | 4 | 替换失败（已回滚） |
//! | 5 | 下载失败 |
//! | 6 | 解压失败 |
//! | 7 | 扩展落地失败 |
//! | 8 | 写安装标记失败 |
//!
//! 失败之后只要给了 `--restart` 就要把旧的 `dlm-gui` 拉起来：用户点了「立即更新」，
//! 结果是程序消失、什么都没发生，那比不更新更糟。

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use dlm::updater::{
    BINARIES, Installed, download, extract_zip, install_extension, swap_binaries, verify_file,
    write_installed,
};

fn main() -> ExitCode {
    let cfg = match Config::parse(std::env::args().skip(1).collect()) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS, // --help
        Err(err) => {
            eprintln!("dlm-updater: {err}");
            return ExitCode::from(2);
        }
    };
    match run(&cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, err)) => {
            log(&cfg, &format!("更新失败（退出码 {code}）：{err:#}"));
            // 用户点了「立即更新」，界面已经退了。失败也要把旧的拉起来，
            // 否则屏幕上什么都不剩，用户只能自己去找快捷方式。
            if let Some(exe) = &cfg.restart {
                log(&cfg, &format!("重启旧版本：{}", exe.display()));
                let _ = std::process::Command::new(exe).spawn();
            }
            ExitCode::from(code)
        }
    }
}

struct Config {
    url: String,
    sha256: String,
    install_dir: PathBuf,
    extension_dir: PathBuf,
    data_dir: PathBuf,
    release: String,
    core: String,
    extension: String,
    /// 等这个进程退出之后才动文件（它就是正在跑的 `dlm-gui`）。
    wait_pid: Option<u32>,
    /// 更新成功后要拉起来的程序；`None` = 不重启。
    restart: Option<PathBuf>,
    /// 日志文件（GUI 拉起本进程时 stdout 是丢掉的，日志得落到文件里）。
    log: Option<PathBuf>,
}

impl Config {
    fn parse(args: Vec<String>) -> Result<Option<Self>, String> {
        let mut cfg = Config {
            url: String::new(),
            sha256: String::new(),
            install_dir: PathBuf::new(),
            extension_dir: PathBuf::new(),
            data_dir: PathBuf::new(),
            release: String::new(),
            core: String::new(),
            extension: String::new(),
            wait_pid: None,
            restart: None,
            log: None,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut next = |name: &str| args.next().ok_or_else(|| format!("{name} 缺少值"));
            match arg.as_str() {
                "--url" => cfg.url = next("--url")?,
                "--sha256" => cfg.sha256 = next("--sha256")?,
                "--install-dir" => cfg.install_dir = PathBuf::from(next("--install-dir")?),
                "--extension-dir" => cfg.extension_dir = PathBuf::from(next("--extension-dir")?),
                "--data-dir" => cfg.data_dir = PathBuf::from(next("--data-dir")?),
                "--release" => cfg.release = next("--release")?,
                "--core" => cfg.core = next("--core")?,
                "--extension" => cfg.extension = next("--extension")?,
                "--wait-pid" => {
                    cfg.wait_pid = Some(
                        next("--wait-pid")?
                            .parse()
                            .map_err(|_| "--wait-pid 不是进程号".to_string())?,
                    )
                }
                "--restart" => cfg.restart = Some(PathBuf::from(next("--restart")?)),
                "--no-restart" => cfg.restart = None,
                "--log" => cfg.log = Some(PathBuf::from(next("--log")?)),
                "--help" | "-h" => {
                    println!(
                        "dlm-updater --url URL --sha256 HEX --install-dir DIR --extension-dir DIR \
--data-dir DIR --release V --core V --extension V [--wait-pid N] [--restart EXE] [--log FILE]"
                    );
                    return Ok(None);
                }
                other => return Err(format!("未知参数 {other}（--help 看用法）")),
            }
        }
        // 校验：少一样就不动手。更新器拿不准的东西一律不猜。
        if cfg.url.is_empty() {
            return Err("缺 --url".into());
        }
        if cfg.sha256.trim().is_empty() {
            return Err("缺 --sha256（没有摘要就不许装）".into());
        }
        if cfg.install_dir.as_os_str().is_empty() {
            return Err("缺 --install-dir".into());
        }
        if cfg.extension_dir.as_os_str().is_empty() {
            return Err("缺 --extension-dir".into());
        }
        if cfg.data_dir.as_os_str().is_empty() {
            return Err("缺 --data-dir".into());
        }
        if cfg.release.is_empty() || cfg.core.is_empty() || cfg.extension.is_empty() {
            return Err(
                "--release / --core / --extension 三个版本号都要给（要写进 installed.json）".into(),
            );
        }
        if !cfg.url.starts_with("https://") && !cfg.url.starts_with("http://") {
            return Err("--url 只接受 http(s)".into());
        }
        Ok(Some(cfg))
    }
}

fn run(cfg: &Config) -> Result<(), (u8, anyhow::Error)> {
    use anyhow::Context;

    // 0. 等正在跑的 GUI 退出：它占着要替换的 exe。
    if let Some(pid) = cfg.wait_pid {
        log(cfg, &format!("等待进程 {pid} 退出…"));
        wait_for_exit(pid, Duration::from_secs(60));
    }

    let work = std::env::temp_dir().join(format!("dlm-update-{}", std::process::id()));
    let zip = work.join("dlm-update.zip");
    let stage = work.join("stage");

    // 1. 下载
    log(cfg, &format!("下载更新包：{}", cfg.url));
    let bytes = download(&cfg.url, &zip).map_err(|err| (5, err))?;
    log(cfg, &format!("下载完成：{bytes} 字节"));

    // 2. 校验（不符就不装，一个文件都不动）
    let got = verify_file(&zip, &cfg.sha256).map_err(|err| (3, err))?;
    log(cfg, &format!("sha256 校验通过：{got}"));

    // 3. 解压
    log(cfg, "解压…");
    extract_zip(&zip, &stage)
        .with_context(|| format!("解压到 {} 失败", stage.display()))
        .map_err(|err| (6, err))?;

    // 4. 替换 exe。包里带了 updater 就一并换（GUI 是从临时副本跑的，所以能换）。
    let mut names: Vec<&str> = BINARIES.to_vec();
    if stage.join("dlm-updater.exe").is_file() {
        names.push("dlm-updater.exe");
    }
    log(cfg, &format!("替换 {}", names.join(" / ")));
    swap_binaries(&cfg.install_dir, &stage, &names).map_err(|err| (4, err))?;

    // 5. 扩展落地（浏览器里要用户点一次「重新加载」才生效）
    let copied = install_extension(&stage, &cfg.extension_dir).map_err(|err| (7, err))?;
    log(
        cfg,
        &format!(
            "扩展 {copied} 个文件装到 {}（浏览器里需点一次『重新加载』）",
            cfg.extension_dir.display()
        ),
    );

    // 6. 记下装了什么：下一次启动，GUI 从这份记录读出自己到底是哪一版。
    write_installed(
        &cfg.data_dir,
        &Installed {
            release: cfg.release.clone(),
            core: cfg.core.clone(),
            extension: cfg.extension.clone(),
            installed_at: now_secs(),
        },
    )
    .map_err(|err| (8, err))?;

    // 7. 重启
    if let Some(exe) = &cfg.restart {
        log(cfg, &format!("重新启动：{}", exe.display()));
        std::process::Command::new(exe)
            .spawn()
            .with_context(|| format!("拉起 {} 失败", exe.display()))
            .map_err(|err| (4, err))?;
    }

    let _ = std::fs::remove_dir_all(&work);
    log(cfg, "更新完成");
    Ok(())
}

/// 等到某个进程退出。
///
/// 用内核句柄而不是轮询 `tasklist`：`OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject`
/// 是阻塞等待，不启进程、不占 CPU。拿不到句柄（进程已经没了、或没权限）就直接往下走 ——
/// 「等不到」不该变成「永远等」。
#[cfg(windows)]
fn wait_for_exit(pid: u32, timeout: Duration) {
    use std::ffi::c_void;
    // SAFETY: 只调 kernel32 的打开/等待/关闭，句柄由本调用自己打开、自己关。
    unsafe {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const SYNCHRONIZE: u32 = 0x0010_0000;
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return;
        }
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        WaitForSingleObject(handle, millis);
        CloseHandle(handle);
    }
}

#[cfg(not(windows))]
fn wait_for_exit(_pid: u32, _timeout: Duration) {}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 记一行日志：stdout 给前台跑的人看，文件给「GUI 拉起自己」时看（那时 stdout 被丢掉）。
fn log(cfg: &Config, message: &str) {
    let line = format!("[dlm-updater] {message}");
    println!("{line}");
    if let Some(path) = &cfg.log {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
    }
}
