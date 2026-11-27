//! 集成测试用的假 `dlm`：按"被调用了几次"返回不同退出码。
//!
//! 为什么需要它：P1 的自动重试只能在**真跑一个子进程**之后才看得出来 —— 单元层能钉
//! "退出码 1 该重排"，但钉不住"队列真的又起了一次子进程、并且次数上限生效"。
//! 用真 `dlm` 造不出稳定可控的失败（要挑对端、等超时），所以这里用一个只数数、
//! 只退出的小程序。
//!
//! 契约（由 `queue_retries_exit_1_and_stops_at_the_cap` 等用例使用）：
//! - `DLM_FAKE_COUNTER`：计数文件路径。每被调用一次就把里面的整数加一。
//! - `DLM_FAKE_CODES`：逗号分隔的退出码序列，第 N 次调用取第 N 项；用完之后一直取最后一项。
//! - 参数里带 `--out-dir` 时，按第几次调用写一个同名的空文件，让 `done` 那一支有落点可查
//!   （P2 的 reveal 只对 `done` 放行，需要一个真存在的文件）。
//! - `DLM_FAKE_SLEEP_MS`：退出前先睡这么多毫秒。用来造"一个还在跑的任务"，
//!   好让用例能在它跑着的时候暂停 —— 否则 `fake-dlm` 秒退，暂停永远抢不到时机。
//! - **冒充桌面端**：把自己的可执行文件**复制并改名为 `fake-gui.exe`**，它就会写一个端点文件
//!   然后退出。端口与令牌从**自己所在的目录**里的 `fake-gui.json` 读（`{"port":N,"token":"T"}`），
//!   端点写到同目录的 `endpoint.json`。
//!   这样"宿主自动拉起桌面端"那条路可以在**不弹真窗口**的前提下端到端测通。
//!   为什么不用环境变量触发：测试是多线程并行的，环境变量是进程级的，
//!   一个用例设了就会漏到别的用例里。按文件名认身份就没有这个问题。
//! - `DLM_FAKE_CONSOLE_REPORT`：往里写一个 `1`/`0` —— 本进程**有没有被分配到控制台**。
//!   用来钉"下载时不该弹出黑色终端窗口"：GUI 是 windows 子系统，若它起控制台子进程时
//!   不加 `CREATE_NO_WINDOW`，Windows 就会分配一个新控制台，这里就会读到 `1`。
//!
//! 退出码 0/1/2/3 与 README「退出码」一节同义，本程序只是把它们按剧本发出去。

use std::env;
use std::fs;
use std::io::ErrorKind;

fn main() {
    // 冒充桌面端：写端点文件就退出（见文件头"冒充桌面端"）。
    if let Some(dir) = own_dir_named("fake-gui") {
        if let Ok(spec) = fs::read_to_string(dir.join("fake-gui.json")) {
            // spec 形如 {"port":12345,"token":"tok"}
            let port = between(&spec, "\"port\":", ",").unwrap_or_else(|| "0".into());
            let token = between(&spec, "\"token\":\"", "\"").unwrap_or_default();
            let text = format!(
                r#"{{"port":{port},"token":"{token}","pid":{}}}"#,
                std::process::id()
            );
            let _ = fs::write(dir.join("endpoint.json"), text);
        }
        std::process::exit(0);
    }

    let counter_path = env::var("DLM_FAKE_COUNTER").unwrap_or_default();
    let codes: Vec<i32> = env::var("DLM_FAKE_CODES")
        .unwrap_or_else(|_| "0".to_string())
        .split(',')
        .filter_map(|s| s.trim().parse::<i32>().ok())
        .collect();

    // 计数：读到多少就加一（文件不存在按 0 起算）。
    let rounds = match fs::read_to_string(&counter_path) {
        Ok(text) => text.trim().parse::<u32>().unwrap_or(0),
        Err(err) if err.kind() == ErrorKind::NotFound => 0,
        Err(_) => 0,
    };
    let _ = fs::write(&counter_path, (rounds + 1).to_string());

    let code = codes
        .get(rounds as usize)
        .copied()
        .or_else(|| codes.last().copied())
        .unwrap_or(0);

    // 成功那一轮留下落点文件，供"打开文件"那条路线使用。
    if code == 0
        && let Some(out_dir) = arg_value("--out-dir")
    {
        let name = arg_value("--out-name").unwrap_or_else(|| "fake.bin".to_string());
        let _ = fs::create_dir_all(&out_dir);
        let _ = fs::write(std::path::Path::new(&out_dir).join(name), b"FAKE");
    }

    // 报告自己有没有控制台（见文件头 DLM_FAKE_CONSOLE_REPORT）。
    if let Ok(path) = env::var("DLM_FAKE_CONSOLE_REPORT") {
        let has = has_console();
        let _ = fs::write(&path, if has { "1" } else { "0" });
    }

    // 挂住一会儿：给"暂停"留出窗口（见文件头 DLM_FAKE_SLEEP_MS）。
    if let Ok(ms) = env::var("DLM_FAKE_SLEEP_MS")
        && let Ok(ms) = ms.trim().parse::<u64>()
        && ms > 0
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    // 让 `--quiet` 之类的参数照收不误：本程序不解析，只是不因多余参数失败。
    std::process::exit(code);
}

fn arg_value(flag: &str) -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
        if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

/// 本进程当前有没有控制台窗口。没被分配控制台时 `GetConsoleWindow` 返回 NULL。
#[cfg(windows)]
fn has_console() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }
    // SAFETY: 只查一个全局状态，不传任何指针进去。
    !unsafe { GetConsoleWindow() }.is_null()
}

#[cfg(not(windows))]
fn has_console() -> bool {
    false
}

/// 自己的可执行文件在某个目录里、且文件名的词干是 `stem` 时，返回那个目录。
fn own_dir_named(stem: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_stem()?.to_string_lossy().to_lowercase();
    if name == stem {
        exe.parent().map(std::path::Path::to_path_buf)
    } else {
        None
    }
}

/// 从一段扁平 JSON 里取两个标记之间的文本（够用就行，不引 serde）。
fn between(text: &str, head: &str, tail: &str) -> Option<String> {
    let start = text.find(head)? + head.len();
    let rest = &text[start..];
    let end = rest.find(tail)?;
    Some(rest[..end].trim().to_string())
}
