//! gui 层的单元测试。原本内嵌在 gui.rs 尾部，拆分后单独成文件。

use super::*;

use std::io::Read;

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
    let plain: Vec<String> = child_args(&task, Path::new("D:/dl"), None, RuntimeTuning::default())
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
    let with_action: Vec<String> = child_args(
        &task,
        Path::new("D:/dl"),
        Some(4096),
        RuntimeTuning::default(),
    )
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

    let argv = |tuning: RuntimeTuning| -> Vec<String> {
        child_args(&task, Path::new("D:/dl"), None, tuning)
            .into_iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect()
    };

    // 设置成 16 连接 + 2 MiB/s。
    let tuning = RuntimeTuning {
        connections: 16,
        max_speed_kib: 2048,
        per_host: 16,
    };
    let got = argv(tuning);
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
    let unlimited = argv(RuntimeTuning {
        connections: 4,
        max_speed_kib: 0,
        per_host: 8,
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
    let clamped = argv(RuntimeTuning {
        connections: 0,
        max_speed_kib: 0,
        per_host: 8,
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
        per_host_connections: 24,
        ..Default::default()
    };

    let tuning = RuntimeTuning::from_settings(&s);
    assert_eq!(tuning.connections, 12);
    assert_eq!(tuning.max_speed_kib, 512);
    assert_eq!(tuning.per_host, 24);
    assert_eq!(tuning.max_speed_bytes(), 512 * 1024);

    // 同一个 tuning 既喂给子进程参数，又决定 host 预算 —— 一个来源。
    let task = Task {
        id: "1".into(),
        url: "https://example.com/a.bin".into(),
        name: "a.bin".into(),
        referer: None,
        status: TaskStatus::Queued,
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
    let argv: Vec<String> = child_args(&task, Path::new("D:/dl"), None, tuning)
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    let from_argv = argv
        .windows(2)
        .find(|w| w[0] == "--connections")
        .map(|w| w[1].clone())
        .expect("必须传 --connections");
    assert_eq!(
        from_argv,
        super::runner::connections_per_task_for_test(tuning).to_string(),
        "argv 里的连接数与 host 预算算的是不是同一个值"
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
    let view = check_update(&format!("http://127.0.0.1:{port}/update.json"));
    handle.join().unwrap();
    assert!(view.enabled, "{view:?}");
    assert_eq!(view.latest.as_deref(), Some("99.0.0"));
    assert!(view.available, "99.0.0 应比当前版本新：{view:?}");
    assert_eq!(view.url.as_deref(), Some("https://example.invalid/dl"));
    assert_eq!(view.notes.as_deref(), Some("test"));
    assert!(view.error.is_none(), "{view:?}");
    assert!(view.checked_at.is_some());
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
