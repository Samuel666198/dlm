//! dlm 产品层入口：本地 Web UI + 队列。默认依赖仍是 6 个（无 GUI crate）。
//!
//! 运行后打印 `http://127.0.0.1:<port>/?t=<token>`；用浏览器打开。进程活着 = 队列/调度器活着。

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use dlm::consts::PROBE_READ_BYTES;
use dlm::gui::{self, GuiConfig};
use dlm::queue::QueueStore;

fn main() -> ExitCode {
    let config = match parse() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("dlm-gui: {err:#}");
            return ExitCode::from(2);
        }
    };
    match gui::run(config) {
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
        quota_limit: None,
        quota_window_secs: None,
        endpoint: local_app_data()
            .join("dlm")
            .join("gui")
            .join("endpoint.json"),
        update_url: None,
        category_dirs: false,
        open: false,
        max_task_retries: 2,
        retry_backoff: gui::DEFAULT_RETRY_BACKOFF,
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
            }
            "--category-dirs" => config.category_dirs = true,
            "--open" => config.open = true,
            "--max-task-retries" => {
                config.max_task_retries = next(&mut args, "--max-task-retries")?.parse()?
            }
            "--help" | "-h" => {
                println!(
                    "dlm-gui [--port N] [--queue FILE] [--out-dir DIR] [--log-dir DIR] [--dlm EXE] [--concurrency N] [--token T] [--quota BYTES] [--quota-window SECS] [--endpoint FILE] [--update-url URL] [--category-dirs] [--open] [--max-task-retries N]"
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
