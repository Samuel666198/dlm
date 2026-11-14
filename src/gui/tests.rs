//! gui 层的单元测试。原本内嵌在 gui.rs 尾部，拆分后单独成文件。

use super::*;

use std::io::Read;

use eframe::egui;

// 测试跨多个子模块，逐个引入。
use super::http::{find_bytes, split_target};
use super::runner::{FinishVerdict, child_args, finish_verdict, quota_slice, retry_decision};
use super::snapshot::TaskView;
use crate::queue::{Task, TaskStatus};

#[test]
fn split_target_keeps_path_and_query_pairs() {
    let (path, query) = split_target("/api/log?id=abc&t=deadbeef");
    assert_eq!(path, "/api/log");
    assert!(query.iter().any(|(k, v)| k == "id" && v == "abc"));
    assert!(query.iter().any(|(k, v)| k == "t" && v == "deadbeef"));
}

#[test]
fn find_bytes_finds_the_header_boundary() {
    assert_eq!(
        find_bytes(b"GET / HTTP/1.1\r\n\r\nbody", b"\r\n\r\n"),
        Some(14)
    );
    assert_eq!(find_bytes(b"none", b"\r\n\r\n"), None);
}

#[test]
fn token_is_hex_and_not_constant() {
    let a = random_token();
    let b = random_token();
    assert_eq!(a.len(), 16);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn quota_slice_prefers_one_slot_when_only_one_slice_fits() {
    assert_eq!(
        quota_slice(MIN_QUOTA_SLICE - 1, 1),
        None,
        "不够一轮预读就不派"
    );
    assert_eq!(quota_slice(MIN_QUOTA_SLICE, 1), Some(MIN_QUOTA_SLICE));
    assert_eq!(quota_slice(MIN_QUOTA_SLICE * 2, 2), Some(MIN_QUOTA_SLICE));
    assert_eq!(
        quota_slice(MIN_QUOTA_SLICE + 10, 2),
        Some(MIN_QUOTA_SLICE + 10),
        "均分不够一份时整份给当前槽位，后续槽位看到 0"
    );
}

/// P3：自启命令最硬的形状要求 —— 路径含空格必须整体带引号，
/// 否则 Windows 把 `C:\Program Files\...` 拆成两段，自启项变成打不开的坏命令。
#[test]
fn autostart_command_quotes_only_when_the_path_has_spaces() {
    let spaced = autostart_command(Path::new(r"C:\Program Files\dlm\dlm-gui.exe"));
    assert_eq!(
        spaced, r#""C:\Program Files\dlm\dlm-gui.exe" --autostart"#,
        "含空格的路径必须整体加引号"
    );
    assert!(
        spaced.starts_with('"') && spaced.ends_with(" --autostart"),
        "引号要包住整条路径、且 --autostart 在引号外：{spaced}"
    );

    let plain = autostart_command(Path::new(r"C:\dlm\dlm-gui.exe"));
    assert_eq!(
        plain, r"C:\dlm\dlm-gui.exe --autostart",
        "不含空格时不加引号（多余引号同样会让某些解析器犯迷糊）"
    );
    assert!(!plain.contains('"'), "{plain}");
}

/// P3：reg 参数逐字钉住 —— 值名、类型、以及"删除用同名"这三件事错了会污染别人的自启项。
#[test]
fn autostart_reg_args_write_and_delete_the_same_named_value() {
    let write = autostart_reg_args(Path::new(r"C:\dlm\dlm-gui.exe"));
    assert_eq!(write[0], "add");
    assert_eq!(
        write[1], r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "只碰 HKCU 的 Run 键"
    );
    assert_eq!(write[2], "/v");
    assert_eq!(write[3], AUTOSTART_VALUE);
    assert_eq!(write[4], "/t");
    assert_eq!(write[5], "REG_SZ");
    assert_eq!(write[6], "/d");
    assert_eq!(
        write[7],
        autostart_command(Path::new(r"C:\dlm\dlm-gui.exe"))
    );
    assert_eq!(write[8], "/f", "覆盖同名值，不弹交互提示");

    let remove = autostart_remove_args();
    assert_eq!(remove[0], "delete");
    assert_eq!(remove[1], write[1], "删除必须落在同一个键上");
    assert_eq!(remove[3], write[3], "删除必须用同一个值名");
}

/// P3：端点文件读不懂 / 端口没人听 / token 不对，都不能被当成"已有实例"。
/// 这一格是"陈旧端点文件不阻塞启动"的直接判据。
#[test]
fn probe_existing_instance_says_none_for_every_stale_shape() {
    let dir = std::env::temp_dir().join(format!("dlm-instance-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // 1) 文件不存在。
    assert!(probe_existing_instance(&dir.join("missing.json")).is_none());

    // 2) 文件存在但不是 JSON。
    let garbage = dir.join("garbage.json");
    fs::write(&garbage, b"not json").unwrap();
    assert!(probe_existing_instance(&garbage).is_none());

    // 3) 端口没人听（挑一个刚释放掉的端口）。
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let dead = dir.join("dead.json");
    let endpoint = Endpoint {
        port,
        token: "tok".into(),
        pid: 999_999,
        started_at: 1,
    };
    fs::write(&dead, serde_json::to_vec(&endpoint).unwrap()).unwrap();
    assert!(
        probe_existing_instance(&dead).is_none(),
        "端口没人听时必须回 None（否则 GUI 被硬杀后永远起不来）"
    );

    let _ = fs::remove_dir_all(dir);
}
#[test]
fn finish_verdict_maps_every_contract_exit_code_to_one_outcome() {
    let table = DEFAULT_RETRY_BACKOFF; // [30, 120]
    assert_eq!(finish_verdict(Some(0), 0, 2, table), FinishVerdict::Done);
    assert_eq!(
        finish_verdict(Some(3), 0, 2, table),
        FinishVerdict::QuotaWait,
        "码 3 是配额到量，走等待而不是重试"
    );
    assert_eq!(
        finish_verdict(Some(3), 5, 2, table),
        FinishVerdict::QuotaWait,
        "重试额度用完也不影响配额等待：那是两件事"
    );
    assert_eq!(
        finish_verdict(Some(2), 0, 2, table),
        FinishVerdict::Fail,
        "码 2 是参数错，重试必然同样失败"
    );
    assert_eq!(
        finish_verdict(Some(1), 0, 2, table),
        FinishVerdict::Retry { backoff: 30 }
    );
    assert_eq!(
        finish_verdict(Some(1), 1, 2, table),
        FinishVerdict::Retry { backoff: 120 }
    );
    assert_eq!(
        finish_verdict(Some(1), 2, 2, table),
        FinishVerdict::Fail,
        "两轮用完就判失败"
    );
    assert_eq!(
        finish_verdict(Some(1), 0, 0, table),
        FinishVerdict::Fail,
        "上限 0 = 关掉自动重试"
    );
    assert_eq!(
        finish_verdict(None, 0, 2, table),
        FinishVerdict::Fail,
        "被信号打死（没有退出码）不猜，判失败"
    );
    assert_eq!(
        finish_verdict(Some(7), 0, 2, table),
        FinishVerdict::Fail,
        "契约外的退出码不重试"
    );
}

/// P1 的判据表：退避表与用户上限两条合取，谁先耗尽都停。
#[test]
fn retry_decision_stops_at_whichever_limit_runs_out_first() {
    let table = DEFAULT_RETRY_BACKOFF;
    // 默认上限 2：两轮都能重试，第三轮停。
    assert_eq!(
        retry_decision(0, 2, table),
        Some(30),
        "第 1 轮：30 秒后重排"
    );
    assert_eq!(
        retry_decision(1, 2, table),
        Some(120),
        "第 2 轮：120 秒后重排"
    );
    assert_eq!(retry_decision(2, 2, table), None, "第 3 轮：判 failed");

    // 上限 0 = 关掉自动重试：第一轮就不重试，而不是"先等 30 秒再判失败"。
    assert_eq!(retry_decision(0, 0, table), None, "上限 0 必须当场判失败");

    // 上限 1：只有第一轮。
    assert_eq!(retry_decision(0, 1, table), Some(30));
    assert_eq!(retry_decision(1, 1, table), None);

    // 上限比退避表长：退避表那一侧先耗尽，不会出现"等 0 秒"或负数。
    assert_eq!(retry_decision(2, 99, table), None, "退避表只有两档");
    assert_eq!(retry_decision(u32::MAX, u32::MAX, table), None);
}

/// `attempts` 必须原样进 `TaskView`：UI 的“重试 1/2”靠它，别在视图层被吞掉。
#[test]
fn task_view_carries_the_retry_attempt_count() {
    let task = Task {
        id: "1".into(),
        url: "https://example.com/a.bin".into(),
        name: "a.bin".into(),
        referer: None,
        status: TaskStatus::Queued,
        dir: None,
        pid: None,
        exit_code: Some(1),
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: Some(123),
        on_complete: None,
        attempts: 1,
        total_len: None,
        delivered: None,
    };
    let view = TaskView::from_task(&task, Path::new("D:/dl"));
    assert_eq!(view.attempts, 1);
    assert_eq!(view.hold_until, Some(123), "等待到点的时间要带出去");
}

#[test]
fn child_args_carry_the_completion_action_and_the_quota_slice() {
    let mut task = Task {
        id: "1".into(),
        url: "https://example.com/a.bin".into(),
        name: "a.bin".into(),
        referer: Some("https://example.com/".into()),
        status: TaskStatus::Queued,
        dir: None,
        pid: None,
        exit_code: None,
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: None,
        on_complete: None,
        attempts: 0,
        total_len: None,
        delivered: None,
    };
    let plain: Vec<String> = child_args(&task, None, &RuntimeTuning::default())
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    assert!(
        plain
            .windows(2)
            .any(|w| w[0] == "--referer" && w[1] == "https://example.com/")
    );
    assert!(!plain.iter().any(|s| s == "--on-complete"));
    assert!(!plain.iter().any(|s| s == "--max-bytes"));

    task.on_complete = Some("open".into());
    let with_action: Vec<String> = child_args(&task, Some(4096), &RuntimeTuning::default())
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    assert!(
        with_action
            .windows(2)
            .any(|w| w[0] == "--on-complete" && w[1] == "open")
    );
    assert!(
        with_action
            .windows(2)
            .any(|w| w[0] == "--max-bytes" && w[1] == "4096")
    );
}

/// **E18**：设置里的连接数与限速必须真的出现在子进程 argv 上。
///
/// 这条用例存在的理由：`connections`/`max_speed` 曾经能改能存盘，但 `child_args`
/// 压根不传它们，子进程永远跑 `DEFAULT_CONNECTIONS` 且不限速。界面上写着 16，
/// 实际 8 —— 没有任何东西会红，因为两处代码从来没有互相说过话。
/// 判据只有一条：**改设置 ⇒ argv 跟着变**。
#[test]
fn e18_connections_and_speed_reach_the_child_argv() {
    let task = Task {
        id: "1".into(),
        url: "https://example.com/a.bin".into(),
        name: "a.bin".into(),
        referer: None,
        status: TaskStatus::Queued,
        dir: None,
        pid: None,
        exit_code: None,
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: None,
        on_complete: None,
        attempts: 0,
        total_len: None,
        delivered: None,
    };

    let argv = |tuning: &RuntimeTuning| -> Vec<String> {
        child_args(&task, None, tuning)
            .into_iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect()
    };

    // 设置成 16 连接 + 2 MiB/s。
    let tuning = RuntimeTuning {
        connections: 16,
        max_speed_kib: 2048,
        speed_windows: Vec::new(),
        proxy: None,
        per_host: 16,
        out_dir: std::path::PathBuf::new(),
        category_dirs: false,
        scan_after_done: false,
        rename_template: String::new(),
        on_duplicate: crate::gui::naming::OnDuplicate::AutoNumber,
        rules: Vec::new(),
        night_batch: crate::gui::batch::NightBatch::default(),
    };
    let got = argv(&tuning);
    assert!(
        got.windows(2)
            .any(|w| w[0] == "--connections" && w[1] == "16"),
        "--connections 必须跟着设置走，实际 argv：{got:?}"
    );
    assert!(
        got.windows(2)
            .any(|w| w[0] == "--max-speed" && w[1] == "2097152"),
        "限速要换算成字节/秒（2048 KiB/s = 2097152 B/s），实际 argv：{got:?}"
    );

    // 不限速（0）⇒ 不传 `--max-speed`：传 0 与不传等价，但少一个参数更干净。
    let unlimited = argv(&RuntimeTuning {
        connections: 4,
        max_speed_kib: 0,
        speed_windows: Vec::new(),
        proxy: None,
        per_host: 8,
        out_dir: std::path::PathBuf::new(),
        category_dirs: false,
        scan_after_done: false,
        rename_template: String::new(),
        on_duplicate: crate::gui::naming::OnDuplicate::AutoNumber,
        rules: Vec::new(),
        night_batch: crate::gui::batch::NightBatch::default(),
    });
    assert!(
        !unlimited.iter().any(|s| s == "--max-speed"),
        "限速为 0 时不该出现 --max-speed，实际 argv：{unlimited:?}"
    );
    assert!(
        unlimited
            .windows(2)
            .any(|w| w[0] == "--connections" && w[1] == "4")
    );

    // 手改 JSON 把 connections 写成 0 ⇒ 夹到 1，不能让 `dlm` 报参数错误。
    let clamped = argv(&RuntimeTuning {
        connections: 0,
        max_speed_kib: 0,
        speed_windows: Vec::new(),
        proxy: None,
        per_host: 8,
        out_dir: std::path::PathBuf::new(),
        category_dirs: false,
        scan_after_done: false,
        rename_template: String::new(),
        on_duplicate: crate::gui::naming::OnDuplicate::AutoNumber,
        rules: Vec::new(),
        night_batch: crate::gui::batch::NightBatch::default(),
    });
    assert!(
        clamped
            .windows(2)
            .any(|w| w[0] == "--connections" && w[1] == "1"),
        "connections=0 必须夹到 1，实际 argv：{clamped:?}"
    );
}

/// **E18**：`RuntimeTuning` 从设置投影时，三个字段都要对上；
/// 并且 host 预算用的连接数必须与传给子进程的是同一个值。
///
/// 后半条是关键：`connections_per_task` 与 `child_args` 如果读两个不同的数，
/// 就会出现「预算按 8 算、实际开 16 条」，每 host 上限形同虚设。
#[test]
fn e18_runtime_tuning_projects_settings_and_keeps_one_source_of_truth() {
    let s = crate::gui::settings::Settings {
        connections: 12,
        max_speed_kib: 512,
        speed_windows: Vec::new(),
        proxy: String::new(),
        per_host_connections: 24,
        ..Default::default()
    };

    let tuning = RuntimeTuning::from_settings(&s);
    assert_eq!(tuning.connections, 12);
    assert_eq!(tuning.max_speed_kib, 512);
    assert_eq!(tuning.per_host, 24);
    assert_eq!(tuning.max_speed_now(), 512, "没有窗口时回全局值（KiB/s）");

    // 同一个 tuning 既喂给子进程参数，又决定 host 预算 —— 一个来源。
    let task = Task {
        id: "1".into(),
        url: "https://example.com/a.bin".into(),
        name: "a.bin".into(),
        referer: None,
        status: TaskStatus::Queued,
        dir: None,
        pid: None,
        exit_code: None,
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: None,
        on_complete: None,
        attempts: 0,
        total_len: None,
        delivered: None,
    };
    let argv: Vec<String> = child_args(&task, None, &tuning)
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    let from_argv = argv
        .windows(2)
        .find(|w| w[0] == "--connections")
        .map(|w| w[1].clone())
        .expect("必须传 --connections");
    let planned_connections = super::runner::plan_for_task_for_test(&task, &tuning).1;
    assert_eq!(
        from_argv,
        planned_connections.to_string(),
        "argv 里的连接数与 host 预算算的是不是同一个值"
    );
}

/// **0.6.0 规则引擎接进 runner**：命中的规则要同时改落点与连接数。
///
/// 这条用例钉的是"落到哪个目录"这件事的第一段：规则算出来的目录必须真的出现在
/// 子进程的 `--out-dir` 上。少了它，规则会变成"设置里能写、下载不认"的空壳 ——
/// 与 E18 那次（连接数能存能改、argv 里没有）是同一类错误。
#[test]
fn rules_override_the_child_out_dir_and_connections() {
    let task = Task {
        id: "1".into(),
        url: "https://x.example/releases/a.zip".into(),
        name: "a.zip".into(),
        referer: None,
        status: TaskStatus::Queued,
        dir: None,
        pid: None,
        exit_code: None,
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: None,
        on_complete: None,
        attempts: 0,
        total_len: None,
        delivered: None,
    };
    let tuning = RuntimeTuning {
        out_dir: std::path::PathBuf::from("D:/dl"),
        rules: vec![crate::rules::Rule {
            pattern: "*/releases/*".into(),
            dir: Some("E:/软件".into()),
            connections: Some(2),
        }],
        ..RuntimeTuning::default()
    };

    let argv: Vec<String> = child_args(&task, None, &tuning)
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    assert!(
        argv.windows(2)
            .any(|w| w[0] == "--out-dir" && w[1] == "E:/软件"),
        "规则命中的落点要进 --out-dir，实际 argv：{argv:?}"
    );
    assert!(
        argv.windows(2)
            .any(|w| w[0] == "--connections" && w[1] == "2"),
        "规则的连接数要进 --connections，实际 argv：{argv:?}"
    );

    // 没命中的 URL 仍走全局设置：规则不能变成"所有任务都套第一条"。
    let other = Task {
        url: "https://x.example/other/a.zip".into(),
        ..task.clone()
    };
    let other_argv: Vec<String> = child_args(&other, None, &tuning)
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    assert!(
        other_argv
            .windows(2)
            .any(|w| w[0] == "--out-dir" && w[1] == "D:/dl"),
        "没命中就走全局落点，实际 argv：{other_argv:?}"
    );
    assert!(
        other_argv
            .windows(2)
            .any(|w| w[0] == "--connections" && w[1] == "8"),
        "没命中就用设置的连接数，实际 argv：{other_argv:?}"
    );
}

/// 入队时钉住的落点优先于"现在的规则"：先入队、后改规则不该把排队任务搬家 ——
/// 搬了就等于进度、断点、打开文件全对着另一个路径。
#[test]
fn a_pinned_dir_outranks_a_later_rule() {
    let task = Task {
        id: "1".into(),
        url: "https://x.example/releases/a.zip".into(),
        name: "a.zip".into(),
        referer: None,
        status: TaskStatus::Queued,
        dir: Some(std::path::PathBuf::from("F:/旧规则")),
        pid: None,
        exit_code: None,
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: None,
        on_complete: None,
        attempts: 0,
        total_len: None,
        delivered: None,
    };
    let tuning = RuntimeTuning {
        out_dir: std::path::PathBuf::from("D:/dl"),
        rules: vec![crate::rules::Rule {
            pattern: "*/releases/*".into(),
            dir: Some("E:/新规则".into()),
            connections: None,
        }],
        ..RuntimeTuning::default()
    };

    let (dir, connections) = super::runner::plan_for_task_for_test(&task, &tuning);
    assert_eq!(dir, std::path::PathBuf::from("F:/旧规则"), "钉住的落点优先");
    assert_eq!(connections, 8, "连接数没有钉住的理由：每次派发现算");

    let argv: Vec<String> = child_args(&task, None, &tuning)
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    assert!(
        argv.windows(2)
            .any(|w| w[0] == "--out-dir" && w[1] == "F:/旧规则"),
        "argv 也要按钉住的落点，实际：{argv:?}"
    );
}

#[test]
fn version_comparison_is_numeric_by_dotted_segment() {
    assert!(!version_is_newer("0.1.0", "0.1.0"));
    assert!(version_is_newer("0.1.0", "0.1.1"));
    assert!(
        version_is_newer("0.1.0", "0.10.0"),
        "10 > 1，不是字符串比较"
    );
    assert!(version_is_newer("0.9.9", "0.10.0"));
    assert!(version_is_newer("0.9.0", "v1.0"));
    assert!(!version_is_newer("1.0.0", "0.99.99"), "远端更旧不算新版本");
    assert!(!version_is_newer("1.0.0", "not-a-version"));
}

#[test]
fn update_check_reads_a_local_manifest() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = r#"{"version":"99.0.0","url":"https://example.invalid/dl","notes":"test"}"#;
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let view = check_update(
        &format!("http://127.0.0.1:{port}/update.json"),
        &local_now(),
    );
    handle.join().unwrap();
    assert!(view.enabled, "{view:?}");
    assert_eq!(view.latest.as_deref(), Some("99.0.0"));
    assert!(view.available, "99.0.0 应比当前版本新：{view:?}");
    assert_eq!(view.url.as_deref(), Some("https://example.invalid/dl"));
    assert_eq!(view.notes.as_deref(), Some("test"));
    assert!(view.error.is_none(), "{view:?}");
    assert!(view.checked_at.is_some());
}

fn manifest(json: &str) -> Value {
    serde_json::from_str(json).expect("测试用的清单 JSON 要合法")
}

/// 本机现状：发行版版本 = 编译期常量、磁盘上还没装扩展。
///
/// 这些用例钉的是**清单那一侧**的解析，所以本机这一侧固定住；
/// 「装过一次更新之后本机版本从 `installed.json` 来」由 `updater::tests` 单独钉。
fn local_now() -> crate::updater::LocalState {
    crate::updater::LocalState {
        release: RELEASE.to_string(),
        extension: None,
    }
}

/// 清单 JSON → 视图，配一份固定的本机现状。
fn parse(value: &Value) -> std::result::Result<UpdateView, String> {
    parse_manifest(value, &local_now())
}

/// 新清单（三个版本号 + 摘要）要把每个字段都搬到视图上。
///
/// 这是更新功能的地基：清单里的 `release` 决定「要不要更新」，`core` / `extension`
/// 只用来告诉用户「这次会不会连带换扩展」。任何一个字段没接上，界面就会少一句话。
#[test]
fn update_manifest_carries_the_three_version_numbers_and_the_digest() {
    let view = parse(&manifest(
        r#"{
            "release": "9.0.0",
            "core": "0.2.0",
            "extension": "0.3.0",
            "url": "https://example.invalid/dlm.zip",
            "sha256": "AABBCC",
            "size": 1234,
            "notes": "修了粘贴",
            "min_core": "0.1.0",
            "published_at": "2026-11-12T11:25:00Z"
        }"#,
    ))
    .expect("清单合法");
    assert!(view.enabled);
    assert_eq!(view.current, RELEASE, "本机发行版版本来自 RELEASE 常量");
    assert_eq!(view.core_current, env!("CARGO_PKG_VERSION"));
    assert_eq!(view.latest.as_deref(), Some("9.0.0"));
    assert!(view.available);
    assert!(view.installable, "有摘要、满足 min_core ⇒ 可自动装");
    assert!(view.blocked.is_none(), "{:?}", view.blocked);
    assert_eq!(view.core.as_deref(), Some("0.2.0"));
    assert_eq!(view.extension.as_deref(), Some("0.3.0"));
    assert_eq!(view.url.as_deref(), Some("https://example.invalid/dlm.zip"));
    assert_eq!(view.sha256.as_deref(), Some("aabbcc"), "摘要统一小写");
    assert_eq!(view.size, Some(1234));
    assert_eq!(view.notes.as_deref(), Some("修了粘贴"));
    assert_eq!(view.min_core.as_deref(), Some("0.1.0"));
    assert_eq!(view.published_at.as_deref(), Some("2026-11-12T11:25:00Z"));
    assert!(view.error.is_none());
}

/// 没有摘要 ⇒ **照样说「有新版」**，但不可自动装，原因写进 `blocked`。
///
/// 这两件事混成一个布尔值就会出事：清单坏了会显示成「没有更新」，用户永远等不到提示。
#[test]
fn a_manifest_without_a_digest_is_announced_but_not_installable() {
    let view = parse(&manifest(
        r#"{"release":"9.0.0","url":"https://x.invalid/a"}"#,
    ))
    .expect("清单合法");
    assert!(view.available, "有更新的发行版就是有");
    assert!(!view.installable, "没摘要不许装");
    let blocked = view.blocked.expect("要给原因");
    assert!(blocked.contains("sha256"), "{blocked}");
    assert!(view.error.is_none(), "这不是检查失败，是清单内容不完整");
}

/// 清单自己声明了 `min_core`，本机内核低于它 ⇒ 只能整包重装。
#[test]
fn a_manifest_whose_min_core_is_ahead_of_us_is_not_installable() {
    let view = parse(&manifest(
        r#"{"release":"9.0.0","sha256":"aa","min_core":"99.0.0"}"#,
    ))
    .expect("清单合法");
    assert!(view.available);
    assert!(!view.installable);
    let blocked = view.blocked.expect("要给原因");
    assert!(
        blocked.contains("99.0.0"),
        "原因里要点明要求的内核版本：{blocked}"
    );
    assert!(blocked.contains("手动"), "{blocked}");
}

/// 清单里的版本与本机相同 ⇒ 没有更新，也谈不上可安装。
#[test]
fn the_same_release_is_not_an_update() {
    let view = parse(&manifest(&format!(
        r#"{{"release":"{RELEASE}","sha256":"aa"}}"#
    )))
    .expect("清单合法");
    assert!(!view.available);
    assert!(!view.installable);
    assert!(view.blocked.is_none());
}

/// 老格式（C3 骨架期的 `version` / `latest`）还得认：清单一旦发出去，旧更新器就会一直来读。
#[test]
fn the_legacy_manifest_still_parses_as_a_release() {
    let by_version = parse(&manifest(
        r#"{"version":"99.0.0","url":"https://x.invalid/a"}"#,
    ))
    .expect("老格式要认");
    assert_eq!(by_version.latest.as_deref(), Some("99.0.0"));
    assert!(by_version.available);
    assert!(!by_version.installable, "老格式没有摘要，不许自动装");

    let by_latest = parse(&manifest(r#"{"latest":"99.0.0"}"#)).expect("老格式要认");
    assert_eq!(by_latest.latest.as_deref(), Some("99.0.0"));
}

/// 一个版本号都没有 ⇒ 这是坏清单，必须报错而不是安静地当「没有更新」。
#[test]
fn a_manifest_without_any_version_is_an_error() {
    let err = parse(&manifest(r#"{"url":"https://x.invalid/a","notes":"嗯"}"#))
        .expect_err("缺版本要报错");
    assert!(err.contains("release"), "{err}");
}

/// 更新源固定在 `main` 分支的仓库根：这一条是发布约定，不是实现细节。
/// 改了它，所有已发出去的更新器都会找不到清单（或找到一个不存在的）。
#[test]
fn the_default_update_url_is_the_release_branch_manifest() {
    assert_eq!(
        DEFAULT_UPDATE_URL,
        "https://raw.githubusercontent.com/Samuel666198/dlm/main/latest.json"
    );
    assert_eq!(
        UPDATE_CHECK_DELAY_SECS, 10,
        "启动后延迟查更新的秒数：别跟首帧抢时间（E15）"
    );
}

#[test]
fn endpoint_file_contains_the_port_and_token() {
    let dir = std::env::temp_dir().join(format!("dlm-endpoint-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let path = dir.join("endpoint.json");
    let endpoint = Endpoint {
        port: 51234,
        token: "tok".into(),
        pid: 7,
        started_at: 9,
    };
    write_endpoint(&path, &endpoint).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["port"], 51234);
    assert_eq!(value["token"], "tok");
    assert_eq!(value["pid"], 7);
    assert_eq!(value["started_at"], 9);
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------- 速度 / 剩余 / 抽屉（本轮）

fn task_view_named(name: &str, status: TaskStatus) -> TaskView {
    let task = Task {
        id: name.to_string(),
        url: "https://example.com/a.bin".into(),
        name: name.to_string(),
        referer: None,
        status,
        dir: None,
        pid: None,
        exit_code: None,
        log: "a.log".into(),
        created_at: 0,
        updated_at: 0,
        start_at: None,
        hold_until: None,
        on_complete: None,
        attempts: 0,
        total_len: None,
        delivered: None,
    };
    TaskView::from_task(&task, std::path::Path::new("D:/dl"))
}

/// `fmt_speed` 只负责把字节数翻译成人话：K/M 的进位、以及 0 也要有话说。
#[test]
fn fmt_speed_reads_like_a_rate() {
    assert_eq!(super::app::list::fmt_speed(0.0), "0 B/s");
    assert_eq!(super::app::list::fmt_speed(900.0), "900 B/s");
    assert_eq!(super::app::list::fmt_speed(1024.0), "1.0 KiB/s");
    assert_eq!(
        super::app::list::fmt_speed(1.5 * 1024.0 * 1024.0),
        "1.5 MiB/s"
    );
    assert_eq!(
        super::app::list::fmt_speed(f64::INFINITY),
        "—",
        "算坏了就别硬显示一个数"
    );
}

/// 剩余时间的量级：秒 / 分 / 小时。60 秒不许显示成「1 分 0 秒」。
#[test]
fn fmt_eta_picks_a_readable_unit() {
    assert_eq!(super::app::list::fmt_eta(45), "45 秒");
    assert_eq!(super::app::list::fmt_eta(60), "1 分");
    assert_eq!(super::app::list::fmt_eta(90), "1 分 30 秒");
    assert_eq!(super::app::list::fmt_eta(3600), "1 小时");
    assert_eq!(super::app::list::fmt_eta(3900), "1 小时 5 分");
}

/// 速度列的三条判据：没在跑 ⇒「—」；跑着但还没采到两个点 ⇒「—」；有速率 ⇒ 真数字。
#[test]
fn speed_text_refuses_to_invent_a_number() {
    use super::app::list::speed_text;
    let running = task_view_named("a.bin", TaskStatus::Running);
    assert_eq!(speed_text(&running, None), "—", "一个点算不出斜率，不许编");
    assert_eq!(speed_text(&running, Some(1536.0)), "1.5 KiB/s");

    let paused = task_view_named("a.bin", TaskStatus::Paused);
    assert_eq!(
        speed_text(&paused, Some(1536.0)),
        "—",
        "暂停还显示速度就是界面在说谎"
    );
}

/// 剩余时间要有速率才敢算；算不出来就老实说「—」。
#[test]
fn eta_text_needs_both_a_rate_and_a_total() {
    use super::app::list::eta_text;
    let mut view = task_view_named("a.bin", TaskStatus::Running);
    view.total_len = Some(1000);
    view.delivered = Some(400);

    assert_eq!(eta_text(&view, None), "—", "没速率不算剩余");
    assert_eq!(eta_text(&view, Some(100.0)), "6 秒", "(1000-400)/100");

    let done = task_view_named("a.bin", TaskStatus::Done);
    assert_eq!(eta_text(&done, Some(100.0)), "—");

    let mut waiting = task_view_named("a.bin", TaskStatus::Queued);
    waiting.hold_until = Some(1);
    assert_eq!(eta_text(&waiting, Some(100.0)), "等待");
}

/// 抽屉必须**真的能画**：跑一帧 egui，不 panic、不偷偷返回"用户点了按钮"。
///
/// 这条存在的理由：抽屉是纯 UI，肉眼不点就看不出画崩了；而它一旦 panic，
/// 用户是点一下任务整个窗口就没了。
#[test]
fn the_drawer_renders_a_task_without_panicking() {
    use super::app::drawer::{DrawerActions, drawer_body};
    let view = task_view_named("a.bin", TaskStatus::Running);
    let ctx = egui::Context::default();
    let mut got: Option<DrawerActions> = None;
    // egui 0.37 的入口是 `run_ui(RawInput, |ui|)`：直接给一个根 `Ui`，
    // 与 eframe 0.37 的 `App::ui(&mut Ui)` 是同一个形状。
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        got = Some(drawer_body(ui, &view, Some(1024.0), "line 1\nline 2"));
    });
    // 没有真实渲染后端，字体图集的 delta 没人消费；不 clear 会在 Drop 时 panic
    // （epaint 的 "Dropped TexturesDelta with 1 unapplied deltas"）。
    output.textures_delta.clear();
    assert_eq!(
        got,
        Some(DrawerActions::default()),
        "只是画一帧，不该产生任何动作"
    );
}

/// 日志尾巴只留最后 N 行：抽屉里显示整份日志会把面板撑爆。
#[test]
fn the_log_tail_keeps_only_the_last_lines() {
    use super::app::drawer::read_log_tail;
    let mut path = std::env::temp_dir();
    path.push(format!("dlm-drawer-log-{}.txt", std::process::id()));
    std::fs::write(&path, "1\n2\n3\n4\n5\n").unwrap();
    assert_eq!(read_log_tail(&path, 2), "4\n5");
    assert_eq!(read_log_tail(&path, 99), "1\n2\n3\n4\n5");
    let _ = std::fs::remove_file(&path);
    assert_eq!(read_log_tail(&path, 5), "", "读不到就是空串，不许 panic");
}

/// 「默认保存目录」的解析：留空 = 用兜底；相对路径必须拒（它会按"进程当前目录"落地，
/// 用户根本找不到文件）。
#[test]
fn out_dir_input_is_optional_but_must_be_absolute() {
    use crate::gui::settings::parse_out_dir_input;
    assert_eq!(parse_out_dir_input("").unwrap(), None);
    assert_eq!(parse_out_dir_input("   ").unwrap(), None);
    assert_eq!(
        parse_out_dir_input("  D:\\Downloads  ").unwrap(),
        Some(std::path::PathBuf::from("D:\\Downloads"))
    );
    assert!(
        parse_out_dir_input("Downloads").is_err(),
        "相对路径不许通过：落点会跟着进程当前目录跑"
    );
}

/// 落点优先级：设置里的 > `--out-dir` 的兜底。反了就会把测试和便携用法指到 Downloads。
#[test]
fn the_settings_out_dir_wins_over_the_cli_fallback() {
    use crate::gui::RuntimeTuning;
    let mut s = crate::gui::settings::Settings::default();
    let fallback = std::path::Path::new("E:/tmp/dlm-fallback");

    let tuning = RuntimeTuning::from_settings_with(&s, fallback, false);
    assert_eq!(tuning.out_dir, fallback, "没设就用兜底");

    s.out_dir = Some(std::path::PathBuf::from("D:\\dl"));
    let tuning = RuntimeTuning::from_settings_with(&s, fallback, false);
    assert_eq!(
        tuning.out_dir,
        std::path::PathBuf::from("D:\\dl"),
        "用户显式设了就以它为准"
    );
}

/// 搜索：名字与 URL 都参与、大小写不敏感；空串（含全空白）全通过。
#[test]
fn search_matches_names_and_urls_case_insensitively() {
    use super::app::list::matches_search;
    let mut view = task_view_named("Movie.TS", TaskStatus::Running);
    view.url = "https://Example.COM/a/B.zip".into();

    assert!(matches_search(&view, ""), "空串 = 不过滤");
    assert!(matches_search(&view, "   "), "全空白也不算关键词");
    assert!(matches_search(&view, "movie"), "名字要大小写不敏感");
    assert!(matches_search(&view, "example.com"), "URL 也要参与");
    assert!(matches_search(&view, "b.zip"), "URL 的路径部分也要能搜到");
    assert!(!matches_search(&view, "nope"), "不匹配就是不匹配");
}

/// 关窗判定：只有「有活 + 能回到窗口」才藏进托盘。
#[test]
fn closing_hides_only_when_there_is_work_and_a_tray_to_come_back_from() {
    use super::app::hide_on_close;

    assert!(
        !hide_on_close(true, false, true),
        "托盘没建起来就绝不能藏 —— 窗口一藏就再也没有入口找回来"
    );
    assert!(
        !hide_on_close(true, true, false),
        "全空闲时点 X 应该真的退出"
    );
    assert!(hide_on_close(true, true, true), "有任务在跑才收进托盘");
    assert!(
        !hide_on_close(false, true, true),
        "用户关掉「收进托盘」就照做"
    );
}

/// **界面上真正会出现的符号，必须在装好的字体里有字形**。
///
/// 这台机器的 Microsoft YaHei 缺一批符号字形，缺的会画成**空心方框**。0.6.0 阶段 1~2
/// 的截图连抓三回（`▾`、`✓`、`⋯`/`＋` 早先也踩过）。眼睛看不出是因为方框很小、
/// 只有那几处；这条用例把它变成一次机械检查：列清楚我们打算画出去的符号，
/// 缺一个就红，并指名道姓。
///
/// 往界面里加新符号时，先把它加进这个表 —— 加得进、跑得绿，才说明字体认它。
#[test]
fn every_symbol_we_put_in_the_ui_has_a_glyph() {
    let ctx = egui::Context::default();
    assert!(
        super::app::fonts::install(&ctx),
        "这台机器上装不到中文字体，后面的检查没意义"
    );
    // 字体表要等第一帧才建起来（"No fonts available until first call to Context::run()"）：
    // 空画一帧再问，不然后面每个字都会被当成缺字形。
    let mut warmup = ctx.run_ui(frame_input(), |_ui| {});
    warmup.textures_delta.clear();
    // 现役符号：省略号（截断、菜单里的「…」）、中点（已选 · / 速度分隔）、
    // 乘号（删一条限速计划 / 关抽屉），以及中文标点。
    //
    // ⚠ 已知缺字形、**界面里一律不许直接用**：`▾ ✓ ✕ ⋯ ＋ ▶`。
    // 前四个都是 0.6.0 自己的截图抓出来的（各画成一个空心方框），`▶`/`＋` 更早也踩过。
    // 箭头与三点一律用 `icons::draw_icon` 画，其它位置换文字或 `×`。
    let used = ['…', '·', '×', '↓', '→', '「', '」', '（', '）', '，', '：'];
    let font = egui::FontId::proportional(12.0);
    for ch in used {
        assert!(
            // `has_glyph` 要 `&mut`（缺字形时它会顺手记一笔），所以走 `fonts_mut`。
            ctx.fonts_mut(|f| f.has_glyph(&font, ch)),
            "字体里没有 {ch:?} 的字形 —— 界面会画成空心方框，得换成画出来的图标或别的字符"
        );
    }
}

/// **抓取窗口画得完**（0.6.0 阶段 3）。
///
/// 清单是一张 `Grid` + `ScrollArea`：列数、勾选数组、跳过折叠都容易写错一格就 panic。
/// 这里塞一份"有本体 / 有镜像 / 有跳过"的清单画一帧 —— 正好是三条分支都要走到的那份。
#[test]
fn the_grab_window_renders_a_plan_with_duplicates_and_skips() {
    use crate::grab::{GrabFile, GrabPlan};

    let (mut app, dir) = test_app("grab-window", Vec::new());
    let plan = GrabPlan {
        files: vec![
            GrabFile {
                url: "https://a.example/f/1.zip".into(),
                from: "起始页".into(),
                duplicate: false,
            },
            GrabFile {
                url: "https://a.example/f/1.zip?v=2".into(),
                from: "/more.html".into(),
                duplicate: true,
            },
        ],
        pages: vec!["https://a.example/more.html".into()],
        skipped: vec![(
            "https://b.example/x.zip".into(),
            "不同站点（b.example）".into(),
        )],
    };
    app.grab_open = true;
    app.grab_url = "https://a.example/".into();
    app.grab_checked = vec![true, false];
    app.grab_plan = Some(plan);

    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(frame_input(), |ui| app.ui_frame(ui));
    output.textures_delta.clear();

    // 空清单 + 扫描中也要能画（这时候分支完全不同）。
    app.grab_plan = None;
    app.grab_busy = true;
    app.grab_started = Some(std::time::Instant::now());
    let mut output = ctx.run_ui(frame_input(), |ui| app.ui_frame(ui));
    output.textures_delta.clear();

    let _ = fs::remove_dir_all(dir);
}

/// **夜间批次在 runner 侧的两个接线点**（0.6.0 阶段 4-1）。
///
/// 纯逻辑在 `batch` 模块里测过了；这里测的是"设置里那个开关真的接到了 runner 的两个出口"：
/// 派发门（`may_dispatch_now`）与限速（`max_speed_now`）。少接一个，写进设置也不会生效，
/// 而界面上看起来一切正常。
#[test]
fn the_night_batch_switch_reaches_the_dispatch_gate_and_the_speed_cap() {
    use crate::gui::batch::NightBatch;
    use crate::gui::speed_schedule::local_minute_of_day;

    let now_hour = (local_minute_of_day() / 60) as u8;
    let mk = |batch: NightBatch| crate::gui::settings::Settings {
        max_speed_kib: 4096,
        night_batch: batch,
        ..Default::default()
    };

    // 1) 开着 + 窗口覆盖此刻（start == end 表示全天）⇒ 用批次那一档限速。
    let tuning = RuntimeTuning::from_settings(&mk(NightBatch {
        on: true,
        start_hour: 9,
        end_hour: 9,
        speed_kib: 256,
        shutdown_when_done: false,
    }));
    assert_eq!(
        tuning.max_speed_now(),
        256,
        "窗口内用批次那一档，不叠加全局限速"
    );
    assert!(tuning.may_dispatch_now(), "全天窗口里当然可以派发");

    // 2) 窗口在两小时后（1 小时长）⇒ 此刻不许派发，限速回全局。
    let tuning = RuntimeTuning::from_settings(&mk(NightBatch {
        on: true,
        start_hour: (now_hour + 2) % 24,
        end_hour: (now_hour + 3) % 24,
        speed_kib: 256,
        shutdown_when_done: false,
    }));
    assert!(!tuning.may_dispatch_now(), "窗口外不派发新任务");
    assert_eq!(tuning.max_speed_now(), 4096, "窗口外用全局限速");

    // 3) 关掉总开关 ⇒ 两道门都放开（与没有这个功能时完全一致）。
    let tuning = RuntimeTuning::from_settings(&mk(NightBatch {
        on: false,
        speed_kib: 256,
        ..NightBatch::default()
    }));
    assert!(tuning.may_dispatch_now());
    assert_eq!(tuning.max_speed_now(), 4096);
}

/// 造一个只连着空队列的 `App`，给"整帧渲染"类用例用。
///
/// 托盘在门禁里不建（`DLM_NO_TRAY=1`，见 `tray::suppressed`）：测试线程没有消息泵，
/// 图标会留在用户任务栏上且点了没反应。
fn test_app(tag: &str, tasks: Vec<TaskView>) -> (super::app::App, std::path::PathBuf) {
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    let dir = std::env::temp_dir().join(format!("dlm-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let snapshot: super::app::SnapshotSlot = Arc::new(Mutex::new(tasks));
    let quota: super::app::QuotaSlot = Arc::new(Mutex::new(None));
    let update: super::app::UpdateSlot =
        Arc::new(Mutex::new(super::snapshot::UpdateView::disabled()));
    let (tx, _rx) = channel();
    let app = super::app::App::new(
        snapshot,
        quota,
        update,
        tx,
        super::app::AppWiring {
            settings_dir: dir.clone(),
            default_out_dir: dir.join("dl"),
            default_category_dirs: false,
            update_url: None,
        },
        crate::updater::LocalState {
            release: "0.0.0".to_string(),
            extension: None,
        },
    );
    (app, dir)
}

/// 1120×700 的一帧输入。窗口尺寸给死，好让面板算宽度时有个确定的分母。
fn frame_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1120.0, 700.0),
        )),
        ..Default::default()
    }
}

/// **整帧渲染**（0.6.0 阶段 1）：工具条的自绘按钮与 `⋯` 菜单、上下文动作条、
/// 状态栏的限速胶囊，全都只在"真的画一帧"时才被走到。
///
/// 这条用例存在的理由：这些东西一旦 panic，用户是**打开窗口就没了**；而它们
/// 又都是自绘的（不画字符，改用 `Painter` 画线 —— 字符会变成方框），
/// 光看代码看不出有没有画歪或算错矩形。判据只有一条：这一帧跑得完、且选中状态
/// 真的让动作条出现了。
#[test]
fn a_whole_frame_renders_with_and_without_a_selection() {
    let (mut app, dir) = test_app("frame", vec![task_view_named("a.bin", TaskStatus::Running)]);
    let ctx = egui::Context::default();
    let input = frame_input();

    // 1) 没选中：动作条不该占位（整帧照样要画得完）。
    assert_eq!(app.selected, None);
    let mut output = ctx.run_ui(input.clone(), |ui| app.ui_frame(ui));
    let shapes_without_selection = output.shapes.len();
    output.textures_delta.clear();

    // 2) 选中一条：动作条出现，整帧仍然画得完。
    //
    //    这一格的判据是"真的画了东西"：动作条是自绘的（一条浅蓝底 + 三颗描边按钮），
    //    选中后帧里的图元必然变多。这不是版面断言（egui 不给外面的代码看面板矩形），
    //    但它能挡住"条件写了、内容没画"这种一半实现。
    app.selected = Some("a.bin".to_string());
    assert!(super::app::context_bar::task_actions_visible(
        app.selected.is_some()
    ));
    let mut output = ctx.run_ui(input, |ui| app.ui_frame(ui));
    assert!(
        output.shapes.len() > shapes_without_selection,
        "选中后动作条该多画出图元：选中 {} 个 vs 没选中 {} 个",
        output.shapes.len(),
        shapes_without_selection
    );
    output.textures_delta.clear();

    let _ = fs::remove_dir_all(dir);
}

/// **设置面板每一页都画得完**（0.6.0 阶段 2）。
///
/// 七个分类 + 搜索态各渲染一帧。这条替代不了人眼，但它挡的是这类大改版最容易伤到
/// 用户的那种错：某一页一打开就 panic（点一下设置，整个窗口消失）。
/// 每条设置项走的是 `SettingKey` 的穷举 `match`，所以这一轮也等于把每个控件都画了一遍。
#[test]
fn every_settings_page_renders() {
    use super::app::settings_spec::Category;

    let (mut app, dir) = test_app(
        "settings-pages",
        vec![task_view_named("a.bin", TaskStatus::Done)],
    );
    let ctx = egui::Context::default();
    app.settings_open = true;

    for category in Category::ALL {
        app.settings_category = *category;
        let mut output = ctx.run_ui(frame_input(), |ui| app.ui_frame(ui));
        output.textures_delta.clear();
    }

    // 搜索态：跨分类的命中列表。
    app.settings_search = "连接".to_string();
    let mut output = ctx.run_ui(frame_input(), |ui| app.ui_frame(ui));
    output.textures_delta.clear();
    assert!(
        !super::app::settings_spec::search(&app.settings_search).is_empty(),
        "搜索『连接』必须有命中，否则这一段什么都没画"
    );

    // 搜不到时也要有话说，不能一片空白。
    app.settings_search = "绝对不会命中的关键词".to_string();
    let mut output = ctx.run_ui(frame_input(), |ui| app.ui_frame(ui));
    output.textures_delta.clear();
    assert!(super::app::settings_spec::search(&app.settings_search).is_empty());

    // Agent 接入页：CLI 复制一直可用、MCP 复制看有没有装 dlm-mcp。
    // 画得完就是这条用例的判据（上面每次 run_ui 都是），这里只补一句为什么单列它：
    // 那一页有 `ui.ctx().copy_text(...)` 与置灰分支，是本面板里唯一带副作用的页。
    app.settings_search.clear();
    app.settings_category = Category::Agent;
    let mut output = ctx.run_ui(frame_input(), |ui| app.ui_frame(ui));
    output.textures_delta.clear();

    let _ = fs::remove_dir_all(dir);
}
