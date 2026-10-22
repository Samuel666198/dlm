//! dlm 产品层入口：本地 Web UI + 队列。默认依赖仍是 6 个（无 GUI crate）。
//!
//! 运行后打印 `http://127.0.0.1:<port>/?t=<token>`；用浏览器打开。进程活着 = 队列/调度器活着。

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use dlm::gui::{self, GuiConfig};

fn main() -> ExitCode {
    match parse().and_then(gui::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dlm-gui: {err:#}");
            ExitCode::FAILURE
        }
    }
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
            "--help" | "-h" => {
                println!(
                    "dlm-gui [--port N] [--queue FILE] [--out-dir DIR] [--log-dir DIR] [--dlm EXE] [--concurrency N] [--token T]"
                );
                std::process::exit(0);
            }
            other => bail!("未知参数 {other}（--help 看用法）"),
        }
    }
    if config.concurrency == 0 {
        bail!("--concurrency 至少 1");
    }
    Ok(config)
}

fn next(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next().ok_or_else(|| anyhow::anyhow!("{name} 缺少值"))
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn user_profile() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn same_dir(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}
