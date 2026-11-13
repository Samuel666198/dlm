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
        out_dir: std::path::PathBuf::new(),
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
        out_dir: std::path::PathBuf::new(),
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
        out_dir: std::path::PathBuf::new(),
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
    let argv: Vec<String> = child_args(&task, Path::new("D:/dl"), None, tuning.clone())
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
        super::runner::connections_per_task_for_test(&tuning).to_string(),
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

    let tuning = RuntimeTuning::from_settings_with(&s, fallback);
    assert_eq!(tuning.out_dir, fallback, "没设就用兜底");

    s.out_dir = Some(std::path::PathBuf::from("D:\\dl"));
    let tuning = RuntimeTuning::from_settings_with(&s, fallback);
    assert_eq!(
        tuning.out_dir,
        std::path::PathBuf::from("D:\\dl"),
        "用户显式设了就以它为准"
    );
}
