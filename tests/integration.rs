//! 集成测试：真发 HTTP，对端是本仓库自己的 node 探针服务器（`tests/harness/`）。
//!
//! `tests/harness/mod.rs` **不是** cargo 自动发现的目标（只有 `tests/*.rs` 与
//! `tests/<dir>/main.rs` 才是测试目标，派发第 10 条）⇒ 用 `#[path]` 显式接进来。
//! `allow(dead_code)`：harness 是 Task 7/8/10 共用的夹具，本目标里暂时没人调的项不该把
//! `cargo clippy --all-targets -- -D warnings` 这道门禁弄红。
#[allow(dead_code)]
#[path = "harness/mod.rs"]
mod harness;

use std::time::{Duration, Instant};

use anyhow::Context;
use dlm::consts::{PROBE_READ_BYTES, WINDOW_BYTES};
use dlm::fetch::{self, FailKind, Outcome};
use dlm::probe::{self, ProbeResult};
use dlm::scheduler::Interval;
use dlm::task::Task;

/// 简报 Step 4 的限量 Range（`bytes=0-<prelude_max-1>`）。
fn prelude_range() -> String {
    format!("bytes=0-{}", PROBE_READ_BYTES - 1)
}

/// 指定上限的限量 Range，供 `prelude_max` 不是常量的用例对照。
fn range_upto(max: u64) -> String {
    format!("bytes=0-{}", max - 1)
}

// ---------------------------------------------------------------- 简报 Step 1 的两条

/// 简报第 1 条：`/blob` 认 Range，预读满限量，长度取 `Content-Range` 的总数。
///
/// 与简报字面的两处偏差（都是派发点名的更正）：
/// - ETag **不写死** `"\"probe-f675558c108f46fc\""`（那是从上一轮会话抄下来的值，harness 的 ETag
///   由 blob 的 sha256 现算 ⇒ 会漂）。改成与**这台服务器自己宣布**的值比（派发第 7 条）。
/// - 客户端**不带** `Connection: close`（spec §10 勘误 R5 / 计划 E2 作废了简报 Step 2、Step 4 那句）。
#[test]
fn probe_reads_prelude_and_reports_ranges_supported() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let t = Task {
        url: format!("{base}/blob"),
        dir: "D:/x".into(), // 简报字面值；probe 不碰磁盘，所以这个目录不存在也不影响
        ..harness::blank_task()
    };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;

    assert!(r.facts.accept_ranges, "harness 明确回 Accept-Ranges: bytes");
    assert_eq!(
        announced.size,
        48 * 1024 * 1024,
        "夹具核对：这台服务器的 blob 就是 48 MiB（服务器自己报的数）"
    );
    assert_eq!(
        r.facts.total_len, announced.size,
        "长度要取 Content-Range 的分母（整份文件），不是本次响应的 {}", PROBE_READ_BYTES
    );
    assert_eq!(r.prelude.len() as u64, PROBE_READ_BYTES, "预读要正好读满限量");
    // 派发第 7 条：与服务器宣布的值比（而不是历史字面量），且**非空**。
    let etag = r.facts.etag.as_deref().unwrap_or_default();
    assert!(!etag.is_empty(), "ETag 必须取到了非空值：{:?}", r.facts.etag);
    assert_eq!(r.facts.etag.as_deref(), Some(announced.etag.as_str()));
    assert_eq!(r.facts.last_modified.as_deref(), Some(announced.last_modified.as_str()));
    assert_eq!(r.facts.final_url, format!("{base}/blob"), "没有重定向时落地 URL 就是请求的那条");
    // 内容取证：字节本身对得上 blob 的块号（光看长度看不出错位/漏段）。
    harness::assert_blob_block_numbers(&r.prelude, 0)?;
    assert_eq!(r.facts, r.facts(), "ProbeResult::facts() 交回的就是同一份事实（Task 5 的闸按值比）");

    // 发出去的请求确实是限量那一条（响应侧证明不了这件事，见 Server::requests 的文档）。
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs.len(), 1, "试探只该发一条请求：{reqs:?}");
    assert_eq!(reqs[0]["url"], "/blob");
    assert_eq!(reqs[0]["range"], prelude_range(), "试探必须带限量 Range，而不是无 Range 后靠 drop");
    Ok(())
}

/// 简报第 2 条：对端不认 Range ⇒ `accept_ranges` 为假。
///
/// 派发第 8 条同时要求把**真实长度**断出来：`/noranges` 故意把 body 截在 1 MiB，所以 `total_len`
/// 是 1048576 而不是 48 MiB —— 那不是要绕过去的缺陷，不认 Range 的服务器说不出比它这一次交出来的
/// 更多的东西。含义写在 `src/probe.rs` 的模块文档里。
#[test]
fn probe_detects_server_without_ranges() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let t = Task { url: format!("{base}/noranges"), ..harness::blank_task() };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;

    assert!(!r.facts.accept_ranges, "这条路由故意不回 Accept-Ranges");
    // 派发第 8 条：断真实数，不写"应该是 48 MiB"，也不为此再发第二枪去打听长度。
    assert_eq!(r.facts.total_len, 1_048_576, "退化路径的长度就是这一次响应交出来的字节数");
    assert_eq!(r.prelude.len() as u64, PROBE_READ_BYTES, "限量照样守得住（服务端发了 1 MiB）");
    harness::assert_blob_block_numbers(&r.prelude, 0)?;
    assert_eq!(
        r.facts.etag.as_deref(),
        Some(announced.etag.as_str()),
        "/noranges 也发 ETag，不能因为这一支退化就丢掉验证子"
    );
    assert_eq!(r.facts.last_modified, None, "这条路由根本不发 Last-Modified（诚实的形状）");
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs.len(), 1, "退化路径同样只发一枪，不再补一发问长度：{reqs:?}");
    Ok(())
}

// ---------------------------------------------------------------- 线上头的独立取证

/// 用**不经 reqwest** 的手写请求把同一批响应头立起来，再看 probe 的解读与之一致。
///
/// 存在的理由：`accept_ranges == false` 这种"缺某个头"的断言，如果只由解析代码自己去发现，
/// 那"解析代码压根没读那个头"的伪装修法同样会给 false ⇒ 恒绿。线上文本先立起来，probe 再去解读，
/// 两边必须对上。
#[test]
fn prelude_range_and_degraded_headers_are_what_the_wire_actually_carries() -> anyhow::Result<()> {
    let (srv, _base) = harness::spawn()?;
    let announced = srv.announced()?;
    let port = srv.port();

    // ① 认 Range 的那条：限量 Range ⇒ 206 + Content-Range 指回整块 blob 的全长。
    let ranged = harness::raw_response_head(port, "/blob", &[("Range", &prelude_range())])?;
    eprintln!("---- 线上响应头 /blob + Range ----\n{ranged}");
    assert_eq!(harness::head_status(&ranged)?, 206);
    assert_eq!(
        harness::head_header(&ranged, "content-range").as_deref(),
        Some(format!("bytes 0-{}/{}", PROBE_READ_BYTES - 1, announced.size).as_str()),
        "限量 Range 的 206 头（Content-Range 的分母就是 probe 取全长的地方）"
    );
    assert_eq!(
        harness::head_header(&ranged, "content-length").as_deref(),
        Some(PROBE_READ_BYTES.to_string().as_str()),
        "Content-Length 是**本次响应**的字节数，与全长不是一回事"
    );
    assert_eq!(harness::head_header(&ranged, "accept-ranges").as_deref(), Some("bytes"));
    assert_eq!(harness::head_header(&ranged, "etag").as_deref(), Some(announced.etag.as_str()));
    assert_eq!(
        harness::head_header(&ranged, "last-modified").as_deref(),
        Some(announced.last_modified.as_str())
    );

    // ② 不认 Range 的那条：200 + 只有 1 MiB + **没有** Accept-Ranges 这一行。
    let degraded = harness::raw_response_head(port, "/noranges", &[("Range", &prelude_range())])?;
    eprintln!("---- 线上响应头 /noranges + Range ----\n{degraded}");
    assert_eq!(harness::head_status(&degraded)?, 200, "无视 Range ⇒ 200，不是 206");
    assert!(!harness::head_has(&degraded, "accept-ranges"), "退化路由就是不该发这个头");
    assert!(!harness::head_has(&degraded, "content-range"));
    assert_eq!(harness::head_header(&degraded, "content-length").as_deref(), Some("1048576"));

    // ③ 同一个对端、两条线上形状，与 probe 的解读逐字对上。
    let client = harness::client()?;
    let on = probe::run(
        &Task { url: format!("http://127.0.0.1:{port}/blob"), ..harness::blank_task() },
        &client,
        PROBE_READ_BYTES,
    )?;
    let off = probe::run(
        &Task { url: format!("http://127.0.0.1:{port}/noranges"), ..harness::blank_task() },
        &client,
        PROBE_READ_BYTES,
    )?;
    assert!(on.facts.accept_ranges, "认 Range 的那条：{on:?}");
    assert!(!off.facts.accept_ranges, "不认 Range 的那条：{off:?}");
    assert_eq!(on.facts.total_len, announced.size, "全长来自 Content-Range 的分母");
    assert_eq!(off.facts.total_len, 1_048_576, "没有 Content-Range 时退到 Content-Length");
    Ok(())
}

// ---------------------------------------------------------------- 简报 Step 4 的形状

/// 评审 **F1** 裁定：带 `--data-file` 的任务**不许试探** ⇒ `probe::run` 在打出任何请求之前拒。
///
/// 为什么这条从"POST 形状发 GET、不发 Range"改成"拒"（原用例挡的两件事已经不存在了）：
/// - 那种形状取回的 `total_len`/`ETag`/`final_url` 来自**那条 GET**，而真正下载发的是 POST ⇒
///   `resume::decide` 的 ②③④ 与 ⑤ 就在校验"一条我们从未发过的请求确认过的字节"，正是 Task 5
///   补 ①b 要挡的形状。
/// - 反过来"试探也发 POST"要把同一份表单交**两次**（下单/投票/发信都是真副作用），买的是一个用户
///   以为在下一次的下载。两条都不接受 ⇒ 只剩"拒"。
///   下游归属（不在本轮实现）：`engine` 对这种形状跳过 probe，事实取自 `fetch` 真正发出的那一枪。
///
/// 本用例断四件事：① 返回 `Err`；② 错误点名 `--data-file`；③ 错误里没有体的字节；
/// ④ **服务器一条请求都没看到**（拒在上网之前）。再加一个对照组：同一台服务器、同一条 URL，
/// 去掉 `data_file` 必须照常能试探 —— 否则"把所有非 GET 可能性一起拒掉"也算伪修通过。
#[test]
fn a_data_file_task_is_refused_before_the_probe_touches_the_wire() -> anyhow::Result<()> {
    const SENTINEL: &str = "form-body-sentinel-8f31";
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let client = harness::client()?;

    // 体文件是真的（`target/` 下，gitignored）：拿不存在的路径断"没抄体的字节"是恒真断言。
    // 名字固定、入口先扫残留：本用例是唯一用这个名字的，并行跑也不冲突；上一轮 panic 在半路留下的
    // 文件不会被下一次运行读成夹具。
    let body = {
        let t = harness::blank_task();
        let p = t.dir.join("f1-data-file-body.bin");
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, format!("{SENTINEL}&cvv=999"))?;
        p
    };
    let t = Task {
        url: format!("{base}/blob"),
        data_file: Some(body.clone()),
        user: Some("alice".into()), // 只有用户名：明文 HTTP 上合法（口令那一半另有闸）
        ..harness::blank_task()
    };
    assert!(!t.segmentable(), "夹具：带 POST 体的任务不分段（Task::segmentable 的语义）");

    let Err(err) = probe::run(&t, &client, PROBE_READ_BYTES) else {
        panic!("带 --data-file 的任务必须被 probe 拒掉，不许退化成一条无体的 GET");
    };
    let msg = err.to_string();
    eprintln!("---- 带 --data-file 的拒绝原文 ----\n{msg}");
    assert!(msg.contains("--data-file"), "错误要点名那个参数，用户才知道改什么：{msg}");
    assert!(!msg.contains(SENTINEL) && !msg.contains("cvv"), "错误里抄了体的字节：{msg}");
    assert!(
        srv.requests_within(std::time::Duration::from_millis(500))?.is_empty(),
        "被拒的试探一个字节都不该上网（GET 打出去 = 拿另一条请求的事实冒充这条）"
    );

    // 对照：同一 URL、同一客户端，去掉 data_file ⇒ 必须照常试探成功。
    let plain = Task { url: format!("{base}/blob"), data_file: None, ..t.clone() };
    let r = probe::run(&plain, &client, PROBE_READ_BYTES)?;
    assert_eq!(r.facts.total_len, announced.size, "对照组要真下得出结论，否则上面那支可以靠\"全都拒\"绿");
    assert_eq!(r.prelude.len() as u64, PROBE_READ_BYTES);
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs.len(), 1, "被拒那一枪不该留下请求行，只有对照组的：{reqs:?}");
    assert_eq!(reqs[0]["url"], "/blob");
    assert_eq!(reqs[0]["range"], prelude_range(), "对照组走的是限量 Range 那一支");
    let _ = std::fs::remove_file(&body);
    Ok(())
}

/// 上限 0（"只要头、不要体"）时预读必须是空的，而且**不发** `Range`。
///
/// 这条接替原 `post_shaped_task_...` 用例里"没有 Range 可限时靠上限限量"的那一半：F1 之后
/// `!segmentable()` 那一支在 `run()` 里已经到不了（带体的任务整个被拒），没 Range 的形状只剩这一种。
#[test]
fn a_zero_prelude_limit_reads_no_body_and_sends_no_range() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };

    let r = probe::run(&t, &harness::client()?, 0)?;
    assert!(r.prelude.is_empty(), "上限 0 ⇒ 一个字节都不取：{}", r.prelude.len());
    assert_eq!(r.facts.total_len, announced.size, "头照样要读出全长");
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs[0]["range"], serde_json::Value::Null, "上限 0 不许发 `bytes=0--1` 那种谎话：{reqs:?}");
    Ok(())
}

/// 文件比预读量还小时，预读**取实际字节数**，不是"读不满就报错"。
#[test]
fn a_file_smaller_than_the_prelude_is_reported_at_its_real_length() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let t = Task { url: format!("{base}/sz/1024"), ..harness::blank_task() };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;

    assert_eq!(r.facts.total_len, 1024, "Content-Range 的分母才是文件全长");
    assert_eq!(r.prelude.len(), 1024, "整份文件比预读量小 ⇒ 预读就是整份文件");
    harness::assert_blob_block_numbers(&r.prelude, 0)?;
    assert!(r.facts.accept_ranges);
    // 服务器确实只发了 1024 字节（不是 probe 自己截的）：限量 Range 的终点被服务端收到上限截短。
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs[0]["url"], "/sz/1024");
    assert_eq!(reqs[0]["range"], prelude_range());
    let evs = srv.events_while(|evs| evs.iter().any(|e| e["ev"] == "response_done"))?;
    let done = evs.iter().find(|e| e["ev"] == "response_done").expect("上面已等过 response_done");
    eprintln!("---- /sz/1024 的服务端收尾记录 ----\n{done}");
    assert_eq!(done["bytes"], 1024, "服务端自己记的实发字节数");
    assert_eq!(done["status"], 206);
    Ok(())
}

/// 空文件（`/sz/0`）：服务器对我们那条 `bytes=0-…` 回 **416** + `Content-Range: bytes */0`。
///
/// 简报没写的一支，按"读服务器已经报出来的数"处理（评审 **F3** 裁定：**保留**）—— 空文件是一个
/// 合法的对端对象，"拒不下载一个 0 字节的文件"本身就是缺陷。成立条件是**三条并列**：收到 416 +
/// `Content-Range` 的分母是 0 + 我们**确实**发了 Range。反面的两支由
/// `a_416_that_is_not_the_empty_file_shape_is_still_an_error` 钉着。
///
/// 取证分三层，都是**这台 node 服务器**的线上原文（不是手搓的头表）：
/// ① 带 Range 的 `/sz/0` ⇒ 416 + `bytes */0`（这一支的读数来源）；
/// ② 同一-route 不带 Range ⇒ 诚实的 200 + `Content-Length: 0` + `Accept-Ranges: bytes`
///   （证明"0 字节"是服务器的既有事实，不是我们那条 Range 逼出来的边角）；
/// ③ 416 那一支的响应头里**没有** `Accept-Ranges`（harness 的 416 分支只发两个头），所以
///   `accept_ranges = true` 是**解读**（"416 本身就是按 Range 语义给的回答"），不是抄来的头 ——
///   这里把两边都断出来，谁改这一支都得先看见这条理由。
#[test]
fn a_zero_length_file_yields_zero_length_facts_instead_of_an_error() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let t = Task { url: format!("{base}/sz/0"), ..harness::blank_task() };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;

    assert_eq!(r.facts.total_len, 0, "`bytes */0` 的分母就是全长，不是猜出来的");
    assert!(r.prelude.is_empty(), "空文件的预读必须是空的");
    assert!(
        r.facts.accept_ranges,
        "416 是按 Range 语义给的回答 ⇒ 记成认 Range（这一味来自解读，见用例文档 ③）"
    );
    assert_eq!(r.facts.final_url, format!("{base}/sz/0"), "落地 URL 照样要记下");
    // 416 那一支的响应头里没有 ETag/Last-Modified（harness 的 416 分支只发两个头），照实为空。
    assert_eq!(r.facts.etag, None);
    assert_eq!(r.facts.last_modified, None);

    // ① 线上确实是 416 + `bytes */0`（不是我们把它当成 416）。
    let head = harness::raw_response_head(srv.port(), "/sz/0", &[("Range", &prelude_range())])?;
    eprintln!("---- 线上响应头 /sz/0 + Range ----\n{head}");
    assert_eq!(harness::head_status(&head)?, 416);
    assert_eq!(harness::head_header(&head, "content-range").as_deref(), Some("bytes */0"));
    assert_eq!(harness::head_header(&head, "content-length").as_deref(), Some("0"));
    // ③ 解读与线上文本必须对得上：这个 416 里**没有** `Accept-Ranges` 可抄。
    assert!(!harness::head_has(&head, "accept-ranges"), "若哪天它发了这个头，上面的 `true` 就该改成抄头");
    assert!(
        r.facts.accept_ranges,
        "两头都没有却给了 true ⇒ 只能是那三条并列的判据在起作用，不是解析失误"
    );

    // ② 同一-route、不带 Range：诚实的 200 + `Content-Length: 0` + `Accept-Ranges: bytes`。
    let plain = harness::raw_response_head(srv.port(), "/sz/0", &[])?;
    eprintln!("---- 线上响应头 /sz/0 无 Range ----\n{plain}");
    assert_eq!(harness::head_status(&plain)?, 200, "0 字节文件在这台服务器上是既有事实，不是 Range 逼出来的");
    assert_eq!(harness::head_header(&plain, "content-length").as_deref(), Some("0"));
    assert_eq!(harness::head_header(&plain, "accept-ranges").as_deref(), Some("bytes"));
    Ok(())
}

/// 416 的**反面**两支（评审 **F3**：那一支的成立条件必须是三条并列，不是"看见 416 就当空文件"）。
///
/// 这一条用 `harness::spawn_416_stub()`（std 手写的窄对端），**标签照实写**：本仓库的 node 服务器
/// 拿不出这两种形状（`bytes=0-…` 起点为 0 时它的 416 分支只会给 `bytes */0`；对无 Range 的请求它
/// 回 200），而这两支又必须真打一次网络才算测到 —— 拿手搓的 `HeaderMap` 断"解读正确"证明不了
/// `run()` 真的读的是响应头。对端是**测试进程内的线程**，不是真进程，随测试二进制消失。
#[test]
fn a_416_that_is_not_the_empty_file_shape_is_still_an_error() -> anyhow::Result<()> {
    let stub = harness::spawn_416_stub()?;
    let client = harness::client()?;

    // 支一：416 + `bytes */4194304`（分母非 0）⇒ 拒。伪修"416 一律 total_len=0"在这里红：
    // 那份事实会在引擎里读成"这活儿已经干完了"，把一个 4 MiB 的文件当成下完了。
    let t = Task { url: format!("{}/total/4194304", stub.base()), ..harness::blank_task() };
    let Err(err) = probe::run(&t, &client, PROBE_READ_BYTES) else {
        panic!("416 + bytes */4194304 不是空文件形状，必须报错而不是给一份 total_len = 0 的事实");
    };
    let msg = err.to_string();
    eprintln!("---- 416（分母非 0）的拒绝原文 ----\n{msg}");
    assert!(msg.contains("416"), "错误要点名状态码：{msg}");
    assert!(msg.contains("长度"), "错误要说清定不下长度：{msg}");
    assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");

    // 支二：**我们没发 Range**（`prelude_max == 0` 的形状）却收到 416 + `bytes */0` ⇒ 仍要拒。
    // 这一支钉的是"三条并列"里的第三条：分母 0 只有在"我们问过 Range"时才说得通。
    // 放过去的话，一个把 416 当默认回答的畸形对端会让 probe 交出一份"0 字节的空文件"。
    let no_range = Task { url: format!("{}/total/0", stub.base()), ..harness::blank_task() };
    let Err(err) = probe::run(&no_range, &client, 0) else {
        panic!("本次没发 Range，416 就不是\"空文件\"的证据，必须报错");
    };
    let msg = err.to_string();
    eprintln!("---- 416（本次没发 Range）的拒绝原文 ----\n{msg}");
    assert!(msg.contains("416"), "错误要点名状态码：{msg}");
    assert!(msg.contains("没发 Range"), "错误要说清是我们没问 Range：{msg}");

    assert!(
        stub.saw("/total/4194304") && stub.saw("/total/0"),
        "两支都必须真打到对端：{:?}",
        stub.seen()
    );
    Ok(())
}

/// 对端**不说长度**（既没 `Content-Length` 也没 `Content-Range`）⇒ 必须报错，不能猜。
/// 猜成 0 在引擎里读起来就是"这活儿干完了"，比报错坏得多。
#[test]
fn a_response_without_any_length_header_is_refused_not_guessed() -> anyhow::Result<()> {
    let stub = harness::spawn_lengthless_stub()?;
    let t = Task { url: format!("{}/no-length", stub.base()), ..harness::blank_task() };

    let Err(err) = probe::run(&t, &harness::client()?, PROBE_READ_BYTES) else {
        panic!("对端不说长度时 probe 不许猜一个数出来");
    };
    let msg = err.to_string();
    eprintln!("---- 长度取不到时的错误原文 ----\n{msg}");
    assert!(msg.contains("长度"), "错误要说清是长度定不下来：{msg}");
    assert!(stub.saw("/no-length"), "本用例必须真的打到对端上：{:?}", stub.seen());
    Ok(())
}

/// 评审 **I2**：`206` 上拿 `Content-Length` 当全长 = 拿**本次窗口**当文件长度 ⇒ 必须报错，不许猜。
///
/// 三种"分母不可用"的形状都合法或至少现实（`Content-Range` 的总数可以写成 `*`，见 RFC 9110 §14.3），
/// 而旧写法一律退到 `Content-Length` ⇒ 读出 `prelude_max`（236032）。那份谎**永远自洽**：
/// `accept_ranges` 靠 `range_honored`（真 206 + 有 Content-Range）读成 true、预读正好等于假全长、
/// spec §6 第 7 条那句"没给 `--sha256` 就只校验已写字节数 == total_len"照样通过 ⇒
/// 236 KiB 的截断文件被报成 48 MiB 下载完成，之后每一次续传都同意这个谎。
/// 这正是 `total_len` 自己那条注释（"把它当文件全长会让引擎以为文件只有预读那么大"）警告的形状。
///
/// 用窄对端：本仓库的 node 服务器在 206 上**永远**给得出整数分母（`bytes <s>-<e>/<sendLen>`），
/// 造不出 `/*`、垃圾分母、或干脆不发 `Content-Range` 这三种形状。
/// 第四条 `/known-total` 是**对照组**：分母可用时必须照常读出 48 MiB（不是窗口）——
/// 没有它，"把所有 206 都拒掉"也算修好了。
#[test]
fn a_206_without_a_usable_total_is_refused_instead_of_reading_the_window_as_the_length(
) -> anyhow::Result<()> {
    let fake = PROBE_READ_BYTES.to_string(); // 那个假数字就是窗口大小 = 236032
    let stub = harness::spawn_unknown_total_stub(PROBE_READ_BYTES)?;
    let client = harness::client()?;

    for (route, shape) in [
        ("/unknown-total", "Content-Range 的总长写成 `*`（RFC 9110 允许的未知总长）"),
        ("/garbage-total", "Content-Range 的分母不是数字"),
        ("/no-content-range", "206 根本不带 Content-Range"),
    ] {
        let t = Task { url: format!("{}{route}", stub.base()), ..harness::blank_task() };
        // 用 `match` 而不是 `let Err(..) else`：后者在 else 分支里叫不出那份"成功"，
        // 而本轮要的恰恰是把它的 `total_len` 打出来当证据。
        let err = match probe::run(&t, &client, PROBE_READ_BYTES) {
            Err(err) => err,
            Ok(r) => panic!(
                "{shape} 时 probe 必须报错，不许把本次窗口的字节数当文件全长交回去 —— \
                 它成功了，total_len = {}（预读读了 {} 字节，两者自洽 ⇒ 截断会被报成下载完成）",
                r.facts.total_len,
                r.prelude.len(),
            ),
        };
        let msg = err.to_string();
        eprintln!("---- 206 说不出总长（{route}）的拒绝原文 ----\n{msg}");
        assert!(msg.contains("长度"), "错误要说清是长度定不下来：{msg}");
        assert!(msg.contains("206"), "错误要点名这次的响应形状：{msg}");
        assert!(
            !msg.contains(&fake),
            "错误里把那个当不得全长的数字抄出来了（{fake} 正是窗口大小）：{msg}"
        );
        assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");
        assert!(stub.saw(route), "必须真打到对端：{:?}", stub.seen());
    }

    // 对照组：同一台对端、分母可用 ⇒ 全长来自分母，不是窗口。
    let t = Task { url: format!("{}/known-total", stub.base()), ..harness::blank_task() };
    let r = probe::run(&t, &client, PROBE_READ_BYTES)?;
    eprintln!("---- 对照组（分母可用的 206）----\n{:?}", r.facts);
    assert_eq!(r.facts.total_len, 48 * 1024 * 1024, "读的是 Content-Range 的分母，不是 Content-Length");
    assert_eq!(r.prelude.len() as u64, PROBE_READ_BYTES, "窗口照样读满");
    assert!(r.facts.accept_ranges, "206 + 可用的 Content-Range ⇒ 认 Range（不受 I1 的合取影响）");
    Ok(())
}

// ---------------------------------------------------------------- 9 项契约头

/// probe 发出的请求要带上任务里的 `referer`/`cookies`（以及 `Accept-Encoding: identity`），
/// 但 **`Authorization` 只在 https 上带**（评审 **F2** 的裁定）。
///
/// 为什么试探也要带满这些头（简报没写，按 ①b 的同一逻辑补上）：`resume` 比对的是"当初确认字节
/// 的那个请求"与"这次的请求"是不是同一个；而如果**第一枪**（确认全长与 ETag 的这一枪）与后面几枪
/// 带的头不一样，那 `total_len`/`ETag` 就可能来自另一份文档 —— 同一台服务器换个 Cookie 给不同长度
/// 是很常见的形状。证据取在**服务端日志**那一侧：那才是"确实发出去了"。
///
/// 本用例同时是 F2 那条闸的**线上取证**：harness 是 `http://127.0.0.1`，所以这里断的是
/// `referer`/`cookie` **照样到**（刻意的不对称，spec §4 点名的只有口令），而 `authorization`
/// **不到**（用户名本身也是半副凭据，`Task::validate()` 只管"明文 HTTP + 口令"那一支）。
/// `https` 那一侧的方向在单元层断（`build_request_...` 那几条）：本仓库的 node 对端不做 TLS，
/// 装一条真 https 服务不在本轮授权里，所以那条是"请求构造层"的证据而不是"线上"的证据。
#[test]
fn probe_puts_the_task_headers_on_the_wire_but_not_credentials_in_the_clear() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let t = Task {
        url: format!("{base}/sz/4096"),
        referer: Some("https://ref.example/page".into()),
        cookies: Some("sid=42".into()),
        user: Some("alice".into()), // 只有用户名：明文 HTTP 上 validate() 不管它（F2 的闸管）
        ..harness::blank_task()
    };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;
    let reqs = srv.requests_at_least(1)?;
    eprintln!("---- 服务端看到的请求行 ----\n{}", serde_json::to_string_pretty(&reqs[0])?);
    assert_eq!(reqs[0]["referer"], "https://ref.example/page");
    assert_eq!(reqs[0]["cookie"], "sid=42", "Cookie 不看 scheme 照发（E9/R6：它是按设计持久化的字段）");
    assert_eq!(reqs[0]["accept_encoding"], "identity", "分段下载不能让中间人压体（Content-Length 会失真）");
    // 服务端把非标准头单独记在 `extra_headers` 里（`authorization` 属于那一堆），这里直接查那张表：
    // 用 `["authorization"] == Null` 那种写法的话，`extra_headers` 整个缺失时也等于 Null ⇒ 恒绿。
    let extra = reqs[0]["extra_headers"].as_object().cloned().unwrap_or_default();
    assert!(
        !extra.contains_key("authorization"),
        "明文 HTTP 上 Authorization 一个字节都不许到服务端（用户名本身也是半副凭据）。实际收到的头：{extra:?}"
    );
    assert_eq!(r.facts.total_len, 4096, "带满头的这一枪照样要能读出事实");
    Ok(())
}

/// 口令 + 明文 HTTP 的组合在**发出请求之前**就被拒（spec §4 的硬规约，E9 那条闸口在
/// `Task::validate`）。断"一个字节都没上网"要守着日志看一段时间 —— 直接 `is_empty()` 会在
/// 真故障上碰巧为真（本项目已经踩过三次恒绿夹具）。
#[test]
fn a_password_over_plain_http_never_leaves_the_socket() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let t = Task {
        url: format!("{base}/blob"),
        user: Some("alice".into()),
        password: Some("hunter2".into()),
        ..harness::blank_task()
    };

    let Err(err) = probe::run(&t, &harness::client()?, PROBE_READ_BYTES) else {
        panic!("明文 HTTP 带口令必须被拒，不许降级发送");
    };
    let msg = err.to_string();
    assert!(msg.contains("口令"), "错误要说清是口令规则：{msg}");
    assert!(!msg.contains("hunter2"), "错误消息自己不该把口令抄一遍：{msg}");
    assert!(
        srv.requests_within(std::time::Duration::from_millis(500))?.is_empty(),
        "被拒的试探不该已经打到服务器上"
    );
    Ok(())
}

// ---------------------------------------------------------------- 派发第 9 条（评审 C3）

/// **服务器**把用户带到一条 authority 里写着凭据的 URL ⇒ probe 必须拒绝，且错误里不回显那条 URL。
///
/// 为什么这条必须存在（Task 5 评审 C3 + 派发第 9 条）：`Task::validate()`（勘误 E9）只管**用户自己
/// 给的**那条 URL，管不到响应里的**落地** URL。而 `ProbeFacts.final_url` 会被 `state` 落盘、被
/// `resume` 写进 ② 的原因串再打到终端 —— 一条 `Location: http://alice:s3cret@host/x` 就能把凭据
/// 塞进我们的文件与消息里，绕过 E9 的原意。裁定：**拒**（既不悄悄存，也不悄悄改写规则 ② 要比的串）。
#[test]
fn a_credentialed_landing_url_is_refused_and_never_echoed() -> anyhow::Result<()> {
    let (stub, land_url) = harness::spawn_redirect_stub("alice:s3cret@127.0.0.1:{port}")?;
    let t = Task { url: format!("{}/start", stub.base()), ..harness::blank_task() };

    let Err(err) = probe::run(&t, &harness::client()?, PROBE_READ_BYTES) else {
        panic!(
            "落地 URL（{land_url}）的 authority 里带着凭据，probe 必须拒绝 —— 这份事实会被落盘并进 ② 的原因串"
        );
    };
    let msg = err.to_string();
    eprintln!("---- probe 拒绝落地 URL 的错误原文 ----\n{msg}");
    assert!(msg.contains("凭据"), "错误要说清拒的是凭据：{msg}");
    assert!(msg.contains("重定向"), "错误要说清凭据来自服务器带的路（不是用户自己写的）：{msg}");
    for leak in ["s3cret", "alice", "landed"] {
        assert!(!msg.contains(leak), "错误里回显了落地 URL 的一部分（{leak}）：{msg}");
    }
    // 假断言防护：只有**真的跟随了**重定向，本用例才在测落地 URL 这一环。
    assert!(
        stub.saw("/landed"),
        "对端没收到落地那条请求 ⇒ 这条用例没测到本轮要守的路径，实际收到：{:?}",
        stub.seen()
    );
    Ok(())
}

/// 同一个对端、**不带**凭据的重定向必须照常放行（否则上面那道闸就成了"把重定向一起禁了"的伪修）。
#[test]
fn an_ordinary_redirect_still_lands_and_is_reported() -> anyhow::Result<()> {
    let (stub, land_url) = harness::spawn_redirect_stub("127.0.0.1:{port}")?;
    let t = Task { url: format!("{}/start", stub.base()), ..harness::blank_task() };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;
    assert_eq!(r.facts.final_url, land_url, "落地 URL 要如实记下（spec §6.5：续传按它比对）");
    // **期望更正（评审 I1），不是削弱断言**：落地那条自己发了 `Accept-Ranges: bytes`，却对我们那条
    // `bytes=0-236031` 回 `200` ⇒ 这个对端**不**按 Range 答话。原写法 `assert!(.., "落地那条自己发了
    // Accept-Ranges: bytes")` 钉的正是缺陷本身（把"头怎么说"当成"行为上做没做"）：那种读法下
    // `scheduler` 的 `NoRanges` 退化永不触发（段表不合并，靠后的区间会把内容写错位置），
    // `resume` 的 ⑤ 也会替一份多段快照说话。见 `src/probe.rs` 模块文档第 10 条。
    assert!(
        !r.facts.accept_ranges,
        "对我们那条 Range 回了 200 ⇒ 头里的 Accept-Ranges 不作数（spec §2.2 第二条）：{:?}",
        r.facts
    );
    assert_eq!(r.facts.etag.as_deref(), Some("\"stub\""), "ETag 原样保留，含协议要求的引号");
    assert_eq!(r.facts.total_len, 0, "落地那条是 Content-Length: 0");
    assert!(stub.saw("/landed"), "必须真的跟随了重定向：{:?}", stub.seen());
    Ok(())
}

/// 评审 **I1** 的正面钉：对端**发了** `Accept-Ranges: bytes` 又拿 **200** 回答我们那条 `Range`
/// ⇒ `accept_ranges` 必须为**假**，且长度**退到 `Content-Length`**（退化读法）。
///
/// 为什么 `/noranges` 挡不住这一支：那条路由把头和行为**一起**改坏了（既不发 `Accept-Ranges`
/// 也不答 206），于是"两条证据取其一"的旧写法在它上面照样给 false —— 只有**只坏一条**的对端
/// （头说认 Range、行为上把 Range 当没有）才测得出合取还是析取。这种对端线上很常见：
/// 反代/CDN 吞掉 `Range` 但把源站的 `Accept-Ranges` 透出来。
///
/// 用窄对端而不是 node 服务器：本仓库的服务器是诚实的，`Range` 要么答 206 要么整个不发那个头，
/// 造不出这个自相矛盾的形状（它也不该被改造成骗子 —— `probe-server.mjs` 不在本轮授权里）。
#[test]
fn a_peer_that_answers_200_to_our_range_is_not_range_capable_however_its_header_reads() -> anyhow::Result<()>
{
    const BODY: u64 = 4096;
    let stub = harness::spawn_range_ignoring_stub(BODY)?;
    let t = Task {
        url: format!("{}/claims-bytes-answers-200", stub.base()),
        ..harness::blank_task()
    };

    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;
    assert!(
        !r.facts.accept_ranges,
        "发了 `Accept-Ranges: bytes` 却对 Range 回 200 ⇒ 不认 Range，段表要退化：{:?}",
        r.facts
    );
    assert_eq!(
        r.facts.total_len, BODY,
        "退化读法：全长退到 Content-Length（这一支是诚实 200 交出来的整个 body）"
    );
    assert_eq!(r.prelude.len() as u64, BODY, "body 比预读量小 ⇒ 预读就是整份 body");
    assert_eq!(r.facts.etag.as_deref(), Some("\"stub-ignores-range\""), "退化不丢验证子");
    assert!(
        stub.saw("/claims-bytes-answers-200"),
        "必须真打到对端（否则本用例没测到任何请求行为）：{:?}",
        stub.seen()
    );
    Ok(())
}

// ---------------------------------------------------------------- 勘误 E3 的守卫

/// 勘误 **E3**：**同一条连接**上"读满的 206"之后接一条 **416**，两条都必须完成。
///
/// 这条用例是 harness 那处 `res.end()` 修复（Task 0）的守卫：推流循环一旦改回"写满只 log 不收尾"，
/// 服务端就把这条 keep-alive 连接的后续响应留在 socket 的 pending 队列里不再 flush，第二条请求会
/// 一直挂到客户端超时（本机实测 ~30 s）⇒ 本用例红。
///
/// 范围照实说（派发第 6 条）：Task 0 的结论是"reqwest 0.13.5 的连接池**在这台仪器上**没有涉案"。
/// 本用例同样只在**这台仪器**上守**本仓库的服务器**，不证明、也不声称证明任何外部服务器或任何
/// 客户端库的行为。
#[test]
fn same_connection_drained_206_then_416_both_complete() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let client = harness::client()?;

    // ① 读满一条 206：`.bytes()` 把 body 读到 EOF ⇒ 连接是干净的，应当回到连接池。
    let first = client
        .get(format!("{base}/blob"))
        .header("Range", "bytes=0-196607")
        .send()
        .context("第一条 206 没发出去")?;
    assert_eq!(first.status().as_u16(), 206);
    let body = first.bytes().context("第一条 206 的 body 没读干净")?;
    assert_eq!(body.len(), 196_608);

    // ② 同一个客户端、同一个 origin：这条**必须**复用那条连接（下面的 conn 断言就是这件事的证明）。
    let second = client
        .get(format!("{base}/blob"))
        .header("Range", "bytes=9999999999-")
        .send()
        .context("第二条请求（期望 416）没完成 —— 这正是 Task 0 修掉的那个卡死形状");
    let second = match second {
        Ok(resp) => resp,
        Err(e) => panic!("同连接上的 416 没完成（harness 的 res.end() 又没了？）：{e:#}"),
    };
    assert_eq!(second.status().as_u16(), 416);
    assert_eq!(
        second.headers().get("content-range").and_then(|v| v.to_str().ok()),
        Some(format!("bytes */{}", announced.size).as_str()),
        "416 要把全长说回来（harness 的 416 分支发的是 `bytes */<sendLen>`）"
    );

    // ③ 守卫的承重那一条：两次请求落在**同一条连接**上，否则本用例测的是"新开一条连接"。
    let reqs = srv.requests_at_least(2)?;
    eprintln!("---- E3：服务端看到的请求行 ----\n{}", serde_json::to_string_pretty(&reqs)?);
    assert_eq!(reqs[0]["conn"], reqs[1]["conn"], "两条请求必须在同一条连接上：{reqs:?}");
    assert_eq!(reqs[0]["reqNoOnConn"], 1);
    assert_eq!(reqs[1]["reqNoOnConn"], 2, "第二条必须是那条连接上的第 2 枪（复用）");

    // ④ 第一条的响应确实**收尾**了：`response_done` 出现在第二条请求之前。
    let evs = srv.events_while(|evs| evs.iter().any(|e| e["ev"] == "request" && e["reqNoOnConn"] == 2))?;
    let idx_done = evs
        .iter()
        .position(|e| e["ev"] == "response_done")
        .expect("读满的那条 206 该有 response_done");
    let idx_second = evs
        .iter()
        .position(|e| e["ev"] == "request" && e["reqNoOnConn"] == 2)
        .expect("上面已等过 reqNoOnConn=2");
    assert!(idx_done < idx_second, "第一条响应必须已收尾才谈得上\"干净入池\"：事件序 {idx_done} vs {idx_second}");
    Ok(())
}

/// 与 E3 相邻的一支：**probe 自己**那条读满的 206 之后，同一客户端再打一发 416 也要能完成。
/// 这才是 Task 7/10 的真实形状（试探读满限量预读 ⇒ 分段阶段在复用连接上撞到越界段）。
#[test]
fn probe_own_drained_prelude_then_a_416_on_the_same_client() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let client = harness::client()?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };

    let r = probe::run(&t, &client, PROBE_READ_BYTES)?;
    assert_eq!(r.prelude.len() as u64, PROBE_READ_BYTES, "夹具：这一枪读满了 ⇒ 连接干净");

    let second = client
        .get(format!("{base}/blob"))
        .header("Range", "bytes=9999999999-")
        .send()
        .context("probe 读满预读之后，同一客户端的 416 没完成")?;
    assert_eq!(second.status().as_u16(), 416);
    let reqs = srv.requests_at_least(2)?;
    assert_eq!(reqs[0]["conn"], reqs[1]["conn"], "两枪共用一条连接才是本用例的形状：{reqs:?}");
    assert_eq!(reqs[1]["reqNoOnConn"], 2);
    assert_eq!(r.facts.total_len, announced.size);
    Ok(())
}

// ---------------------------------------------------------------- 形状与契约

/// `ProbeResult` 的对外形状：字段名 + `facts()`（Task 5 的 `decide` 与 Task 8 按这份消费）。
#[test]
fn probe_result_shape_is_the_one_task_5_and_task_8_consume() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let t = Task { url: format!("{base}/sz/8192"), ..harness::blank_task() };
    let r: ProbeResult = probe::run(&t, &harness::client()?, PROBE_READ_BYTES)?;

    let facts = r.facts(); // 派发第 5 条：Task 5 的闸口靠这个方法取事实
    assert_eq!(facts, r.facts, "facts() 取回的就是结构里那份");
    assert_eq!(facts.total_len, 8192);
    assert_eq!(r.prelude.len(), 8192);
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs[0]["url"], "/sz/8192", "这一枪打的确实是本用例那条路由：{reqs:?}");
    // 字段名是 plan 第 717 行的 Produces，用解构把它钉住（改名即编译失败）。
    let ProbeResult { facts: _, prelude: _ } = &r;
    Ok(())
}

/// 调用方给的预读上限要真的说话（不是把 `consts::PROBE_READ_BYTES` 硬绑进实现）。
#[test]
fn prelude_max_is_honoured_rather_than_the_baked_in_constant() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };

    let r = probe::run(&t, &harness::client()?, 4096)?;
    assert_eq!(r.prelude.len(), 4096, "上限小就用小的：Task 9 会把预读量做成可覆盖的参数");
    assert_eq!(r.facts.total_len, srv.announced()?.size, "长度仍取 Content-Range 的分母");
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs[0]["range"], range_upto(4096), "Range 的终点跟着上限走：{reqs:?}");
    assert_ne!(reqs[0]["range"], prelude_range(), "上限换了头也得跟着换（写死常量就是假参数化）");
    Ok(())
}

// ================================================================ Task 7：fetch 窗口化子请求
//
// 简报第 769–792 行给了三条用例：两条是 `unimplemented!` 占位，第三条的期望自相矛盾
// （`Advanced{WINDOW_BYTES}` 与 `store.len() == start + WINDOW_BYTES + 1` 不可能同时成立），
// 还带一句 `|| true` 的恒真断言。下面按派发的六条裁定把它们换成真实断言，并补齐其余形状。
// 对端全是真的：node 探针服务器（`tests/harness/probe-server.mjs`，本轮一字未改）或
// `tests/harness/mod.rs` 里的进程内窄对端（那些形状 node 侧造不出来，理由见各条文档）。

/// 窗口起点：**4096 的整数倍**。简报给的 `1_000_000` 不能用于块号取证 —— harness 的 blob 只在
/// 块首写块号，取证切片必须从块首开始，否则断言读的是未初始化填充字节（一条瞎 oracle）。
const ALIGNED: u64 = 1_048_576; // 256 KiB = 64 × 4096

/// 裁定 5：窗口上限就是**正好 `WINDOW_BYTES` 个字节**，两个数同源（`bytes` 与 `store.len()`），
/// 任何一处差一个字节都当场红。内容用块号取证（不是简报那句 `|| true`）。
#[test]
fn window_writes_at_offset_and_stops_at_window_edge() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let (store, path) = harness::store("t7-window.bin")?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };
    // 区间比一个窗口**宽一倍**：收手的必须是窗口，不是区间终点。
    let iv = Interval { start: ALIGNED, end: ALIGNED + WINDOW_BYTES * 2 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
    eprintln!("---- 裁定 5：窗口那一枪 ----\n{out:?}\nlen={}", store.len()?);
    assert_eq!(out, Outcome::Advanced { bytes: WINDOW_BYTES }, "{out:?}");
    assert_eq!(
        store.len()?,
        ALIGNED + WINDOW_BYTES,
        "写了 {} 字节 ⇒ 长度必须是 start + 那么多（简报写成 start+WINDOW+1，与它自己的字节数断言互斥）",
        WINDOW_BYTES
    );
    // 真内容：每个 4 KiB 块首的块号等于它所在的块 ⇒ 字节确实来自这些偏移，没错位也没掉段。
    harness::require_block_alignment(&path, ALIGNED, WINDOW_BYTES)?;
    assert!(store.len()? <= iv.end, "收手条件写错了（长度越过区间终点）：len={}", store.len()?);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 简报第 2 条（原 `unimplemented!`）：末段发**开放终点** `bytes={start}-`（spec §2 表 / §3 对
/// IDM 的观测），非末段必须带显式终点。取证读的是**服务器收到的那条头**（harness JSONL），
/// 不是我们自己拼出来的串 —— 拼对了但没序列化出去，这里一样红。
///
/// **勘误 E14②（戊）在本条动了一格的判据值**（方向一律没动，三条各钉一件事）：
/// - ① 区间**伸到** EOF 但一个窗口填不满它 ⇒ 现在问的是**窗口那一格**（旧写法发 `bytes=1048576-`，
///   等于授权对端流 47 MiB，而我们最多读 4 MiB ⇒ 那一截全留在连接上，正是阶段4 那笔重复流量的起点）。
/// - ③ 文件**真正那一尾**（窗口终点 == `total_len - 1`）⇒ 仍然发 `bytes={start}-`：IDM 的观察保住。
/// - ② 终点在文件中间 ⇒ 与 E14 之前逐字相同（区间比窗口窄时窗口终点就是区间终点）。
#[test]
fn range_start_open_ended_but_respects_explicit_end() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let client = harness::client()?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };

    // ① 区间伸到 EOF、窗口填不满 ⇒ 问窗口那一格（E14②）。这一枪仍然只读一个窗口。
    let last = Interval { start: ALIGNED, end: total - 1 };
    let (store_open, path_open) = harness::store("t7-open.bin")?;
    let out = fetch::run_window(&client, &t, last, &store_open, 10_000_000, total, &harness::uncapped())?;
    assert!(matches!(out, Outcome::Advanced { bytes } if bytes == WINDOW_BYTES), "{out:?}");

    // ② 非末段：终点在文件中间 ⇒ 必须带上显式终点（少带一个字节就是对端多送一整个窗口）。
    let mid = Interval { start: ALIGNED, end: ALIGNED + 65_536 - 1 };
    let (store, path) = harness::store("t7-closed.bin")?;
    let out = fetch::run_window(&client, &t, mid, &store, 10_000_000, total, &harness::uncapped())?;
    assert_eq!(out, Outcome::Complete, "整段读完（区间比窗口窄）⇒ Complete：{out:?}");
    assert_eq!(store.len()?, ALIGNED + 65_536, "终点说了算：只写到 iv.end");
    harness::require_block_alignment(&path, ALIGNED, 65_536)?;

    // ③ 文件真正那一尾：起点 = 全长 - 一个窗口 ⇒ 窗口终点正好是 `total_len - 1` ⇒ 开放终点。
    let tail = Interval { start: total - WINDOW_BYTES, end: total - 1 };
    let (store_tail, path_tail) = harness::store("t7-tail.bin")?;
    let out = fetch::run_window(&client, &t, tail, &store_tail, 10_000_000, total, &harness::uncapped())?;
    assert_eq!(out, Outcome::Complete, "整尾就是一次窗口：读完 ⇒ Complete：{out:?}");

    let reqs = srv.requests_at_least(3)?;
    eprintln!("---- 开放终点/显式终点：服务端收到的 Range ----\n{}", serde_json::to_string_pretty(&reqs)?);
    assert_eq!(
        reqs[0]["range"],
        format!("bytes={ALIGNED}-{}", ALIGNED + WINDOW_BYTES - 1),
        "E14②：窗口够不着 EOF 的那一枪必须**只问窗口**（旧判据在这里发的是 `bytes={ALIGNED}-`）"
    );
    assert_eq!(
        reqs[1]["range"],
        format!("bytes={ALIGNED}-{}", ALIGNED + 65_536 - 1),
        "非末段要发带显式终点那一条（两条一模一样就是没按窗口/全长分岔）"
    );
    assert_eq!(
        reqs[2]["range"],
        format!("bytes={}-", total - WINDOW_BYTES),
        "文件真正那一尾仍然发开放终点：IDM 的观察（spec §3）不许被 E14② 一起改掉"
    );
    assert_ne!(reqs[0]["range"], reqs[2]["range"], "同样是伸到 EOF 的区间，窗口够得着与够不着必须分岔");
    for r in &reqs {
        assert!(r["connection"].is_null(), "不得发 Connection: close（spec §10 R5 / 计划 E2）：{r:?}");
    }
    drop((store, store_open));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path_open);
    let _ = std::fs::remove_file(&path_tail);
    Ok(())
}

/// 9 项契约里进头的几项必须**真的上线**（复用 `probe::with_contract_headers`，两枪同源），
/// 而 `Authorization` 只在 https 上发（Task 6 评审 F2 的闸）。取证 = 服务器收到的头。
#[test]
fn fetch_carries_the_nine_item_contract_headers_on_the_wire() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let (store, path) = harness::store("t7-headers.bin")?;
    // 明文 http + 只给用户名（`Task::validate` 管的是"明文 + **口令**"，用户名那半归 F2 那道闸）。
    let t = Task {
        url: format!("{base}/blob"),
        referer: Some("http://example.com/from".into()),
        cookies: Some("sid=42; pref=fast".into()),
        user: Some("alice".into()),
        ..harness::blank_task()
    };
    let iv = Interval { start: ALIGNED, end: ALIGNED + 4096 * 3 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
    assert!(matches!(out, Outcome::Complete), "{out:?}");
    let reqs = srv.requests_at_least(1)?;
    eprintln!("---- 契约头：服务端收到什么 ----\n{}", serde_json::to_string_pretty(&reqs)?);
    let r = &reqs[0];
    assert_eq!(r["referer"], "http://example.com/from", "Referer 要上线（退化路径靠它拿同一份文档）");
    assert_eq!(r["cookie"], "sid=42; pref=fast", "Cookie 要上线（同上）");
    assert_eq!(r["accept_encoding"], "identity", "Accept-Encoding: identity（压缩过的字节不能按偏移落盘）");
    assert_eq!(r["extra_headers"]["accept"], "*/*", "Accept: */* 由契约构造器带上");
    assert!(
        r["extra_headers"].get("authorization").is_none(),
        "明文 http 上半副凭据也不许出去（F2 那道闸在 fetch 这一枪同样成立）：{r:?}"
    );
    assert!(r["connection"].is_null(), "不得发 Connection: close（R5/E2）：{r:?}");
    assert_eq!(r["method"], "GET", "本轮 fetch 只发 GET（POST 那一支是待裁事项，见报告）");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// `Task::validate()` 是**发请求之前**的闸（与 `probe::run` 同一个立场）：明文 HTTP + 口令 ⇒
/// 当场拒，一枪都不许打出去。用 `requests_within` 守一段时间，而不是拿到响应再看日志
/// （日志落盘晚于响应，那种"空日志"断言会在真故障上碰巧为真）。
#[test]
fn the_password_gate_is_enforced_before_fetch_touches_the_wire() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-gate.bin")?;
    let t = Task {
        url: format!("{base}/blob"),
        password: Some("hunter2".into()),
        ..harness::blank_task()
    };
    let iv = Interval { start: ALIGNED, end: ALIGNED + 4096 - 1 };
    let Err(err) = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())
    else {
        panic!("明文 HTTP 带口令的任务必须被 validate() 拒掉，不许发出请求");
    };
    assert!(err.to_string().contains("--password"), "错误要指使用法：{err}");
    assert!(srv.requests_within(Duration::from_millis(300))?.is_empty(), "拒在上网之前：服务器一条请求都不该看到");
    assert_eq!(store.len()?, 0, "被拒的任务不该动过盘");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 3(a)：`/flaky` **只承认 4 MiB**（E5：它从不在中途打断，只是把服务长度改小）。
/// 我们要一个越过该长度的区间 ⇒ 服务器按 Range 语义答 416，而分母 `4194304` 与我们手里的
/// `total_len`（48 MiB）不一致 ⇒ 这是"长度在我们脚下变了"，不许折成 `Complete`（E7）。
#[test]
fn a_beyond_length_interval_on_a_shorter_server_is_validator_changed() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-flaky.bin")?;
    let t = Task { url: format!("{base}/flaky"), ..harness::blank_task() };
    let iv = Interval { start: 5_000_000, end: 5_100_000 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    eprintln!("---- 裁定 3(a)/2：/flaky 越界那一枪 ----\n{out:?}");
    assert_eq!(out, Outcome::Failed(FailKind::ValidatorChanged), "{out:?}");
    assert_eq!(store.len()?, 0, "416 这一支一个字节都不该落盘");
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs[0]["range"], format!("bytes={}-{}", iv.start, iv.end), "线上发的确实是越界那一条");
    let evs = srv.events_while(|evs| evs.iter().any(|e| e["ev"] == "response" && e["status"] == 416))?;
    let resp416 = evs.iter().rev().find(|e| e["ev"] == "response" && e["status"] == 416).expect("上面已等到 416");
    eprintln!("---- 416 的服务器侧记录 ----\n{}", serde_json::to_string_pretty(resp416)?);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 2 的正支：416 的 `Content-Range: bytes */N`，**N == total_len** ⇒ 这个区间确实不需要字节
/// ⇒ `Complete`（Task 8 把它折成 `Event::SegmentComplete`，所以 `Complete` 必须留给"真没活儿"）。
///
/// **修复轮 C 改的是这一条的输入**：原先取 `iv.start == total_len`，于是"`total_len`"与"`iv.start`"
/// 是两个**同一个数**，判据写成 `reported == Some(iv.start)`（或干脆 `reported.is_some()`）在它身上
/// 读起来一模一样 ⇒ 变异存活。按 **E12②**，可达的形状是 `iv.start >= total_len`（续传时区间起点
/// 已落在 EOF **上或之后**），所以这里把起点放到 `total_len + 4096`：两个量从此是两个不同的数，
/// 正支只认分母那一个。反支（分母 = 我们的 `iv.start` 而不是 `total_len`）归
/// `a_416_that_answers_with_our_interval_start_is_validator_changed`。
#[test]
fn a_416_whose_denominator_is_our_length_is_complete() -> anyhow::Result<()> {
    let stub = harness::spawn_416_stub()?;
    let (store, path) = harness::store("t7-416-agrees.bin")?;
    let total = 4_194_304;
    let t = Task { url: format!("{}/total/{total}", stub.base()), ..harness::blank_task() };
    // 起点**越过**全长 4096 个字节（不是正好落在全长上）：`total_len` 与 `iv.start` 必须是两个数。
    let iv = Interval { start: total + 4_096, end: total * 3 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
    assert_eq!(
        out,
        Outcome::Complete,
        "N 与我们的 total_len 相等（而 `iv.start` 是另一个数）⇒ 确实无字节可取：{out:?}"
    );
    assert_eq!(store.len()?, 0);
    assert!(stub.saw("/total/"), "必须真打到对端（否则测的是连接失败）：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **修复轮 C 的反支**：416 报回来的分母**正好等于我们问的那个起点**（`iv.start`）、却不等于
/// `total_len` ⇒ 必须 `Failed(ValidatorChanged)`。
///
/// 为什么单独立一条而不是把既有那条的输入改改了事：既有那条原本取 `iv.start == total_len`，两个量
/// 在那条里是**同一个数** ⇒ 判据怎么写都看不出来（那正是评审 C 说的存活变异）。把它改成
/// `iv.start = total_len + 4096` 之后正支自己就能挡 `Some(iv.start)` 那一版了，但**反方向**还需要
/// 一条：分母**正好等于 `iv.start`** 而对端的全长另有其数 —— 那才是野外真会出现的形状（区间由旧的
/// `total_len` 推出来、服务端长度已经变小）。两条合起来把判据的两个方向都钉住：
///
/// - 写成 `reported == Some(iv.start)` ⇒ 这条给 `Complete`（红）、正支那条也给 `Complete`（红）；
/// - 写成 `reported.is_some()` ⇒ 这条与既有那条 `a_416_with_a_disagreeing_or_missing_…` 一起红。
///
/// 另外照派发要求带上"没有 `Content-Range`"与"分母读不出来"两格：这两格在既有那条里已经钉过，
/// 这里在**同一台对端、同一条 `iv`**（`iv.start > total_len` 的那个形状）上再钉一遍，是为了让
/// "起点越过全长"这一支的三种分母形状各有归属，而不是共用一条旧 `iv`。
///
/// 三枪打在同一台新桩上（`PER_CONNECTION = 4`，未越界；既有那条反支用它自己的 4 枪）。
#[test]
fn a_416_that_answers_with_our_interval_start_is_validator_changed() -> anyhow::Result<()> {
    let stub = harness::spawn_416_stub()?;
    let client = harness::client()?;
    let (store, path) = harness::store("t7-416-start-not-length.bin")?;
    let total = 4_194_304;
    let iv = Interval { start: total + 4_096, end: total * 3 - 1 };

    for (route, shape) in [
        (format!("/total/{}", iv.start), "分母**等于我们问的起点**、却不等于 total_len（C 点名的那一格）"),
        (String::from("/no-content-range"), "416 压根不带 Content-Range"),
        (String::from("/garbage-total"), "416 的分母读不出来"),
    ] {
        let t = Task { url: format!("{}{route}", stub.base()), ..harness::blank_task() };
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
        eprintln!("---- C 反支：我们的 total_len={total}、iv.start={} ----\n{route} ⇒ {out:?}", iv.start);
        assert_eq!(
            out,
            Outcome::Failed(FailKind::ValidatorChanged),
            "{shape} ⇒ 只能算「服务端长度在我们脚下变了」，不许封成 Complete：{out:?}"
        );
        assert_eq!(store.len()?, 0, "{shape} 这一支不许写过盘");
    }
    assert_eq!(stub.seen().len(), 3, "三枪都要真打出去：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 2 的反支：`N != total_len`、分母是垃圾、以及**根本没有** `Content-Range` 三种形状，
/// 都必须 `Failed(ValidatorChanged)` —— "看到 416 就当这活儿干完了"会把截断文件悄悄封成完成。
#[test]
fn a_416_with_a_disagreeing_or_missing_denominator_is_validator_changed() -> anyhow::Result<()> {
    let stub = harness::spawn_416_stub()?;
    let client = harness::client()?;
    let total = 4_194_304;
    let iv = Interval { start: total, end: total * 2 - 1 };
    let (store, path) = harness::store("t7-416-disagrees.bin")?;

    for (route, want) in [
        (format!("/total/{}", total - 1), "分母比我们的长度小 1"),
        (String::from("/total/999"), "分母完全对不上"),
        (String::from("/garbage-total"), "分母不是数字"),
        (String::from("/no-content-range"), "压根没发 Content-Range"),
    ] {
        let t = Task { url: format!("{}{route}", stub.base()), ..harness::blank_task() };
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
        assert_eq!(
            out,
            Outcome::Failed(FailKind::ValidatorChanged),
            "{want}（{route}）⇒ 只能是 ValidatorChanged，实际 {out:?}"
        );
        assert_eq!(store.len()?, 0, "{want} 这一支不许写过盘");
    }
    assert_eq!(stub.seen().len(), 4, "四枪都要真打出去：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 3(b) + 裁定 4：`/drop/<n>` 是**诚实头 + body 中途拆连接**的对端（E5 之后本仓库唯一一支
/// 真掐的对端）。写下去的 65536 字节是真进度 ⇒ `Advanced`，一个字节都不许丢在失败路径上。
#[test]
fn a_body_cut_off_midway_keeps_the_bytes_it_actually_wrote() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-drop.bin")?;
    let t = Task { url: format!("{base}/drop/65536"), ..harness::blank_task() };
    let iv = Interval { start: ALIGNED, end: ALIGNED + 2 * 1024 * 1024 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    eprintln!("---- 裁定 4：/drop/65536 那一枪 ----\n{out:?}\nlen={}", store.len()?);
    assert_eq!(out, Outcome::Advanced { bytes: 65_536 }, "被掐断前写下去的字节必须交回，不许折成 0：{out:?}");
    assert_eq!(store.len()?, ALIGNED + 65_536, "长度就是交回的那 65536 字节");
    harness::require_block_alignment(&path, ALIGNED, 65_536)?;
    let evs = srv.events_while(|evs| evs.iter().any(|e| e["ev"] == "response_drop"))?;
    let dropped = evs.iter().rev().find(|e| e["ev"] == "response_drop").expect("上面已等到 response_drop");
    eprintln!("---- 服务器侧的截断记录 ----\n{}", serde_json::to_string_pretty(dropped)?);
    assert_eq!(dropped["bytes"], 65_536, "对端确实只写出 65536 字节就拆了 socket");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 4 的零字节支：`/drop/0` 把头发完、body 一个字节不发就拆连接。
/// 零进度才是重试上限（spec §6.1/§6.2）该计的数 ⇒ `Failed(Transport)`。
#[test]
fn a_body_that_never_starts_is_a_countable_transport_failure() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-drop0.bin")?;
    let t = Task { url: format!("{base}/drop/0"), ..harness::blank_task() };
    let iv = Interval { start: ALIGNED, end: ALIGNED + 65_536 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    // 本轮 D：变体判断没有变松（`take_transport_note` 不是 Transport 就当场 panic），只是 additionally
    // 要求这一支**带着那句原因**回来 —— 这一支的 `err` 是 reqwest 包过一层的 `io::Error`（对端拆线，
    // 不是超时），所以那句话取自库的 message + `source()` 链首句。
    let note = take_transport_note(&out, "零字节 + 对端拆线 ⇒ 只能算失败");
    eprintln!("---- D：/drop/0 那一枪的原因原文 ----\n{note}");
    assert_eq!(store.len()?, 0);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 4 的另一支零进度：**干净的 EOF**、body 长度是 0（`Content-Length: 0` 的降级 `200`）。
///
/// 为什么单独立一条而不是靠上面那条 `/drop/0`：那条走的是"读的时候报错"那一支，这一条走的是
/// "读到 `Ok(0)`"那一支 —— 两条都是零字节，但代码路径不同。变异 M9（把 EOF 支改成无条件
/// `Advanced{bytes}`）在这仓库里**只有这一条**会红：`Advanced { bytes: 0 }` 交回 Task 8 就是
/// "写了 0 个字节，请再派一次"，重试上限一个也数不到 ⇒ 一条永不给字节的对端可以让我们无限循环。
/// 零进度必须是 `Failed`，这样 §6.1/§6.2 的封顶才有东西可计。
#[test]
fn an_empty_degraded_body_for_a_non_empty_interval_is_a_countable_failure() -> anyhow::Result<()> {
    let stub = harness::spawn_range_ignoring_stub(0)?;
    let (store, path) = harness::store("t7-empty-body.bin")?;
    let t = Task { url: format!("{}/claims-bytes-answers-200", stub.base()), ..harness::blank_task() };
    let iv = Interval { start: 0, end: 4096 * 4 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    // 本轮 D：这一支的 `read` 给的是 `Ok(0)`（**没有**错误对象），所以那句原因是我们自己写的
    // （`src/fetch.rs` 的 `WINDOW_EOF_WITHOUT_BYTES`），它要**说清是哪一种零进度**，
    // 而不是留给 Task 8 一句"网络错了"。M9 那条纪律（零进度必须是可数的失败）原样保留。
    let note = take_transport_note(&out, "区间要 16 KiB、对端交了 0 字节 ⇒ 零进度必须计一次失败");
    eprintln!("---- D：干净 EOF 而零字节那一支的原因原文 ----\n{note}");
    assert!(note.contains("响应体") && note.contains("干净结束"), "原因要说清是\"body 干净结束、一个字节都没到\"：{note}");
    assert_eq!(store.len()?, 0, "零进度不许悄悄写过盘");
    assert!(stub.saw("/claims-bytes-answers-200"), "必须真打到对端：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 3(c) 的正支（真·卡死）：对端收了请求**永不回答**。窗口预算到点 ⇒ `Stalled`。
///
/// 为什么这一条要 15 s：预算是 plan 第 796 行那个式子，下界 `WINDOW_MIN_TIMEOUT_SECS = 15`，
/// 任何调用方都拿不到更小的数（留一个能把预算调小的 seam = 测一条真代码不会走到的形状）。
/// 时长照实登记：不配 timeout 不等于永不等（Task 0 实测无 timeout 的请求在 30.005 s 自己死掉）。
#[test]
fn a_peer_that_never_answers_is_stalled() -> anyhow::Result<()> {
    let stub = harness::spawn_silent_stub()?;
    let (store, path) = harness::store("t7-silent.bin")?;
    let t = Task { url: format!("{}/silent", stub.base()), ..harness::blank_task() };
    let iv = Interval { start: 0, end: 65_536 - 1 };
    let started = Instant::now();

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    let took = started.elapsed();
    eprintln!("---- 裁定 3(c)：静默对端 ----\n{out:?} 用时 {took:?}");
    assert_eq!(out, Outcome::Stalled, "预算内一个字节都没等到 ⇒ Stalled：{out:?}");
    assert_eq!(store.len()?, 0);
    assert!(took >= Duration::from_secs(14), "预算下界 15 s，不该提前收手：{took:?}");
    assert!(stub.saw("/silent"), "必须真打到对端：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 4 + `is_timeout` 分类里**只有这一条**能钉住的形状：`Response::read` 的超时在 reqwest
/// blocking 里是 `io::Error`（`ErrorKind::Other` + 装着 `reqwest::Error{Decode ← TimedOut}` 的
/// payload），**不是** `reqwest::Error`。只按 `kind()` 分的话这一支会掉进 `Failed(Transport)`，
/// 于是"卡住"被重试上限计成"传输失败"，而两者的措辞与退避时长都不一样。
#[test]
fn a_peer_that_answers_but_sends_no_body_is_stalled_not_a_transport_failure() -> anyhow::Result<()> {
    let stub = harness::spawn_head_only_stub(8 * 1024 * 1024)?;
    let (store, path) = harness::store("t7-headonly.bin")?;
    let t = Task { url: format!("{}/hangs", stub.base()), ..harness::blank_task() };
    let iv = Interval { start: 0, end: WINDOW_BYTES - 1 };
    let started = Instant::now();

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 8 * 1024 * 1024, &harness::uncapped())?;
    let took = started.elapsed();
    eprintln!("---- 读侧超时（is_timeout 分类）----\n{out:?} 用时 {took:?}");
    assert_eq!(out, Outcome::Stalled, "头收了、body 一个字节没到 ⇒ 判 Stalled，不是 Transport：{out:?}");
    assert_eq!(store.len()?, 0);
    assert!(took >= Duration::from_secs(14), "读侧预算同样是 15 s 下界：{took:?}");
    assert!(stub.saw("/hangs"), "必须真打到对端：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 慢**但对端一直在送**不许判成卡死（R1 的窗口化子请求给"卡住"下的定义是"预算到点且零字节落盘"）。
/// `/sz/524288s` 按 64 KiB / 200 ms 的节拍交 512 KiB（E4 实测 ~1.6 s），而我们要 4 MiB。
/// ⇒ body 自然结束在 512 KiB ⇒ 那 512 KiB 是真进度 ⇒ `Advanced`，且**不是** `Stalled`。
///
/// 这条只钉"活着 ≠ 卡死"这一件事（它 1.6 s 就交完，远在预算之内）。"预算到点时会发生什么"——
/// 包括"对端活着也照样被掐、但交回的是 `Advanced` 而不是 `Stalled`"—— 归
/// `the_window_budget_is_a_whole_request_deadline_on_the_read_side_too`。
#[test]
fn a_live_but_paced_peer_is_advanced_rather_than_misread_as_stalled() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-paced.bin")?;
    let t = Task { url: format!("{base}/sz/524288s"), ..harness::blank_task() };
    let iv = Interval { start: 0, end: WINDOW_BYTES - 1 };
    let started = Instant::now();

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 327_680, 48 * 1024 * 1024, &harness::uncapped())?;
    let took = started.elapsed();
    eprintln!("---- 限速对端 ----\n{out:?} 用时 {took:?}");
    assert!(!matches!(out, Outcome::Stalled), "一直在送字节 ⇒ 不是卡死：{out:?}");
    assert_eq!(out, Outcome::Advanced { bytes: 524_288 }, "节拍送完的 512 KiB 都是真进度：{out:?}");
    assert!(took < Duration::from_secs(10), "预算之内就该收手（这条对端 ~1.6 s 送完）：{took:?}");
    assert_eq!(store.len()?, 524_288);
    harness::require_block_alignment(&path, 0, 524_288)?;
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// body 比区间短 ⇒ **绝不是** `Complete`：`Complete` 在 Task 8 那里折成 `Event::SegmentComplete`，
/// 会把这一段的尾巴**永远不再取**。短了就交回真写下的字节数，让 Task 8 重新决定。
#[test]
fn a_shorter_body_than_the_interval_is_never_reported_complete() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-short.bin")?;
    let t = Task { url: format!("{base}/sz/4096"), ..harness::blank_task() };
    let iv = Interval { start: 0, end: 8191 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 8192, &harness::uncapped())?;
    assert!(!matches!(out, Outcome::Complete), "只拿到 4096 字节却报 Complete ⇒ 尾巴会被封存：{out:?}");
    assert_eq!(out, Outcome::Advanced { bytes: 4096 }, "{out:?}");
    assert_eq!(store.len()?, 4096);
    harness::require_block_alignment(&path, 0, 4096)?;
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 6（计划 E7② 交给 Task 7 的那条下游义务）：**发了 `Range` 而对端回 `200`** 时，body 的第一个
/// 字节对应**文件偏移 0**，不是 `iv.start` ⇒ 必须先流掉前 `iv.start` 个字节，再从 `iv.start` 往后落盘。
/// 真对端 = node 的 `/noranges`（无视 Range、从 0 交 1 MiB、头是诚实的）。
#[test]
fn a_degraded_200_body_is_skipped_not_written_at_the_wrong_offset() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-degraded.bin")?;
    let t = Task { url: format!("{base}/noranges"), ..harness::blank_task() };
    let half: u64 = 512 * 1024; // 1 MiB 的后半段：起点是 4096 的整数倍，块号取证才成立
    let iv = Interval { start: half, end: 1_048_576 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 1_048_576, &harness::uncapped())?;
    assert_eq!(out, Outcome::Complete, "区间 = body 的后半段 ⇒ 丢掉前半后正好读完：{out:?}");
    assert_eq!(store.len()?, 1_048_576, "写到 iv.end 就该停：len={}", store.len()?);
    // 内容取证：**这是唯一分辨"丢没丢"的断言**。不丢的话 `iv.start` 处躺的是块 0 的字节。
    harness::require_block_alignment(&path, half, half)?;
    // 丢掉的那 512 KiB 不许在任何偏移留下痕迹（`[0, iv.start)` 仍是从未写过的空洞）。
    let head = harness::bytes_at(&path, 0, 4096)?;
    assert!(head.iter().all(|b| *b == 0), "区间之前的字节仍是空洞 ⇒ 降级支不许写偏移 0");
    let reqs = srv.requests_at_least(1)?;
    assert_eq!(reqs[0]["range"], format!("bytes={half}-"), "我们确实发了 Range（才会撞上 200 这一支）");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 裁定 6 的第二半：丢字节是**流式**的，不是"把整个 body 吸进内存再切片"。
/// 对端按 64 KiB / 20 ms 交 8 MiB，而我们要的只是 `iv.start` 之后那 128 KiB：
/// - `written()`（客户端挂断前对端写到哪儿）钉住"我们没把整个 body 吸完"；
/// - 块号取证钉住"剩下的字节落在正确的偏移"。
///
/// 为什么用进程内对端而不是 node：`/noranges` 只交 1 MiB，比"跳过量 + 窗口"还短，两种写法
/// （流式 / 先吸完）都会把 body 读完 ⇒ 那条断言是瞎的。
#[test]
fn a_degraded_200_is_skipped_as_stream_not_as_a_buffered_body() -> anyhow::Result<()> {
    let peer = harness::spawn_paced_range_ignoring_stub(8 * 1024 * 1024, 64 * 1024, Duration::from_millis(20))?;
    let (store, path) = harness::store("t7-degraded-stream.bin")?;
    let t = Task { url: format!("{}/ignores-range", peer.base()), ..harness::blank_task() };
    let iv = Interval { start: 262_144, end: 262_144 + 131_072 - 1 };
    let started = Instant::now();

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    let took = started.elapsed();
    let written = peer.written();
    eprintln!("---- 裁定 6：流式跳字节 ----\n{out:?} 用时 {took:?} 对端写出 {written}");
    assert_eq!(out, Outcome::Complete, "{out:?}");
    assert_eq!(store.len()?, iv.end + 1, "只写到 iv.end");
    harness::require_block_alignment(&path, iv.start, iv.len())?;
    assert!(
        written < 4 * 1024 * 1024,
        "对端写出 {written} 字节（整份 body 是 8 MiB）⇒ 我们把整个 body 吸完了 = 缓冲进内存"
    );
    assert!(peer.saw("/ignores-range"), "必须真打到对端：{:?}", peer.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// `206` 却说不出它交的是**哪一段**（没有 `Content-Range`）⇒ 偏移无从核对，一律
/// `Failed(ValidatorChanged)`，不许"按我们想要的偏移硬写"。对端 = Task 6 评审 I2 那条窄桩。
#[test]
fn a_206_that_cannot_show_its_offset_is_validator_changed() -> anyhow::Result<()> {
    let stub = harness::spawn_unknown_total_stub(1024)?;
    let (store, path) = harness::store("t7-206-blind.bin")?;
    let t = Task { url: format!("{}/no-content-range", stub.base()), ..harness::blank_task() };
    let iv = Interval { start: 0, end: 1023 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 1024, &harness::uncapped())?;
    assert_eq!(out, Outcome::Failed(FailKind::ValidatorChanged), "206 没带 Content-Range ⇒ 偏移不可核对：{out:?}");
    assert_eq!(store.len()?, 0, "核对不了偏移就一个字节都不许写");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 空区间 ⇒ 不发请求，直接 `Complete`（那一支是"真的没活儿"，Task 8 会把它封成 `SegmentComplete`）。
#[test]
fn an_empty_interval_needs_no_request() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-empty.bin")?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };
    let iv = Interval { start: ALIGNED, end: ALIGNED - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    assert_eq!(out, Outcome::Complete, "{out:?}");
    assert_eq!(store.len()?, 0);
    assert!(srv.requests_within(Duration::from_millis(300))?.is_empty(), "空区间不许打出任何一枪");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// fetch 版的 **E3 守卫**：`fetch` 不许有 `Connection: close`、也不许设 `pool_max_idle_per_host(0)`
/// （spec §10 R5 / 计划 E2）。行为侧的证法与 Task 6 那两条同一形状 —— **整段读完**（body 与
/// `Content-Length` 严格一致）的那一枪把连接留干净，下一枪在**同一条连接**上打出去。
///
/// 这条同时钉住另一半：窗口**没用满**就收手的那一枪（`Advanced`）确实把 body 剩在连接里，那条连接
/// 是被拆掉的（`probe.rs` 模块文档第 2 条对退化支的同一说明）—— 所以本用例走的是"读满整段"那一支。
#[test]
fn a_drained_window_leaves_the_connection_reusable() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let client = harness::client()?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };
    let (store, path) = harness::store("t7-reuse.bin")?;
    let iv = Interval { start: 0, end: 65_536 - 1 };

    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
    assert_eq!(out, Outcome::Complete, "区间比窗口窄 ⇒ 读满整段：{out:?}");
    assert_eq!(store.len()?, 65_536);

    // 同一个客户端、同一个 origin 上的第二枪：必须复用那条连接。
    let second = client
        .get(format!("{base}/blob"))
        .header("Range", "bytes=9999999999-")
        .send()
        .context("fetch 读满整段之后，同一客户端的下一枪没完成（连接被关了？还是 harness 的 res.end() 又没了？）")?;
    assert_eq!(second.status().as_u16(), 416);

    let reqs = srv.requests_at_least(2)?;
    eprintln!("---- fetch 的 E3 守卫：服务端看到的两条请求 ----\n{}", serde_json::to_string_pretty(&reqs)?);
    assert_eq!(reqs[0]["conn"], reqs[1]["conn"], "两条请求必须在同一条连接上：{reqs:?}");
    assert_eq!(reqs[0]["reqNoOnConn"], 1);
    assert_eq!(reqs[1]["reqNoOnConn"], 2, "第二枪必须是那条连接上的第 2 个请求（真复用）");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

// ================================================================ Task 7 修复轮：F1（POST 那一支）与 F2（事实收割口）
//
// 派发裁定把 `--data-file` 那一支的**方法**与**事实来源**都交给了 fetch：
// F1 = 那一支必须发 POST（体 = 文件字节、不带 Range），而不是把一条无体 GET 的字节当成下载内容；
// F2 = 那种形状没有试探（`probe::run` 硬拒，Task 6 评审 F1），所以事实由 `run_whole` 从**自己那一枪**
// 的响应头里收割，读法与 probe **共用同一个函数**。下面每条断言的凭据都取在**对端收到的那一侧**。

/// **F1**：带 `--data-file` 的任务**不许**从窗口这条路打出一条无体的 GET。
///
/// 为什么这条必须先红：本轮之前 `run_window` 无条件 `agent.get(..)` + 无条件发 `Range`，
/// 于是那种任务落盘的字节来自**一条我们从未发过的请求** —— 与 Task 6 评审 F1 拒绝"对带体任务做
/// GET 试探"是同一条理由（那次的裁定是"拒，不许猜一个形状"）。裁定给的修法是多一个入口
/// `run_whole`，所以窗口这条路本身必须**当场拒**，而不是"没人调就行"。
///
/// 取证 = 收体对端**实际收到**的东西（`spawn_post_stub` 连 body 一起收，node 服务器看不到 body）。
#[test]
fn a_data_file_task_never_reaches_the_wire_as_a_bodyless_get_through_the_window_path() -> anyhow::Result<()> {
    let peer = harness::spawn_post_stub(None, b"the-guessed-response-body".to_vec())?;
    let (store, path) = harness::store("t7-f1-window.bin")?;
    let body = harness::scratch_body("f1-window-body.bin", b"session=abc&cvv=999")?;
    let t = Task {
        url: format!("{}/submit", peer.base()),
        data_file: Some(body.clone()),
        ..harness::blank_task()
    };
    let iv = Interval { start: 0, end: 4096 * 4 - 1 };
    let result = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped());
    let reqs = peer.requests();
    eprintln!(
        "---- F1：窗口那条路对带体任务实际发出去什么 ----\n{}\n结论 {result:?}\nlen={}",
        reqs.iter()
            .map(|r| format!("method={} range={:?} body={:?}", r.method, r.header("range"), r.body_text()))
            .collect::<Vec<_>>()
            .join("\n"),
        store.len()?
    );
    let Err(err) = result else {
        panic!(
            "带 --data-file 的任务不该被窗口那条路接受。对端收到的是：{}",
            reqs.iter()
                .map(|r| format!("method={} body={} 字节", r.method, r.body.len()))
                .collect::<Vec<_>>()
                .join(" / ")
        );
    };
    let msg = err.to_string();
    assert!(msg.contains("--data-file"), "错误要点名那个参数，调用方才知道换哪条路：{msg}");
    assert!(msg.contains("run_whole"), "错误要指出正确的入口（Task 8 照着这句接线）：{msg}");
    assert!(!msg.contains("cvv") && !msg.contains("session=abc"), "错误里抄了体的字节：{msg}");
    assert!(reqs.is_empty(), "拒在上网之前：对端一条请求都不该看到：{reqs:?}");
    assert_eq!(store.len()?, 0, "被拒的任务不许动过盘");
    let _ = std::fs::remove_file(&body);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 体文件里那句哨兵：既用来证明"上线的 body 确实是这份文件的字节"，也用来证明
/// "错误消息一个字都不抄它"。里面刻意带着口令形状的东西（spec §4 那条硬规约管的就是它）。
const FORM_BODY: &[u8] = b"user=alice&password=hunter2&cvv=999";

/// **F1 的正面取证**：`--data-file` 的那一枪发出去的是 **POST**、体 = 文件字节、**不带 Range**，
/// 而**落盘的字节是响应体**（不是请求体，也不是别的什么）。
///
/// 凭据全部取自**对端收到的那一侧**（`spawn_post_stub` 连 body 一起收：node 服务器只记头，
/// "body 到底有没有上线"这件事不能由我们自己拼的字符串自证）。同一台对端还回了一份**位置可辨**的
/// 响应体，于是"落盘的是响应体"这一半也不用同一份内容自证（两份字节本来就不同长、也不同内容）。
///
/// 顺带钉住三条：契约头与 `probe` 同源（`Referer`/`Cookie`/`identity`/`*/*`）、明文 socket 上
/// `Authorization` 仍然一条不出去（Task 6 评审 F2 那道闸在这一支同样成立）、不发 `Connection: close`（R5/E2）。
#[test]
fn a_data_file_task_is_posted_with_the_file_as_its_body_and_lands_the_response_body() -> anyhow::Result<()> {
    let response_body = harness::positioned_body(65_536); // 与请求体刻意不同长也不同内容
    let peer = harness::spawn_post_stub(Some("bytes"), response_body.clone())?;
    let (store, path) = harness::store("t7-f1-post.bin")?;
    let file = harness::scratch_body("f1-post-body.bin", FORM_BODY)?;
    let file_bytes = std::fs::read(&file)?;
    let t = Task {
        url: format!("{}/submit", peer.base()),
        data_file: Some(file.clone()),
        referer: Some("http://example.com/form".into()),
        cookies: Some("sid=42; pref=fast".into()),
        user: Some("alice".into()), // 明文 http 上连用户名都不许出去
        ..harness::blank_task()
    };

    let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
    let reqs = peer.requests();
    let facts = facts.expect("这一枪看到了 2xx 的响应，事实必须有");
    eprintln!(
        "---- F1/F2：POST 那一枪的线上取证 ----\n收到 {} 条: method={} range={:?} content-length={:?} body={} 字节\n结论 {out:?}  len={} 事实 {facts:?}",
        reqs.len(),
        reqs[0].method,
        reqs[0].header("range"),
        reqs[0].header("content-length"),
        reqs[0].body.len(),
        store.len()?
    );
    assert_eq!(reqs.len(), 1, "整份那一枪就该只有一发请求：{reqs:?}");
    let req = &reqs[0];
    assert_eq!(req.method, "POST", "带体的任务必须发 POST（GET 就是把另一条请求的字节当成果子）");
    assert!(!req.has("range"), "POST 上一条 Range 都不许出现（spec §2.2 第一条）：{:?}", req.headers);
    assert_eq!(req.body, file_bytes, "上线的 body 必须逐字节等于 `--data-file` 那份文件");
    assert_eq!(
        req.header("content-length"),
        Some(file_bytes.len().to_string().as_str()),
        "Content-Length 由体文件的长度写出（reqwest 在发送那一步补，`build()` 那一步看不到）"
    );
    assert!(!req.has("transfer-encoding"), "不许把这份体写成 chunked（本夹具据此收 body）：{:?}", req.headers);
    // 契约头同源 + 口令闸（F2）+ 不发 close（R5/E2）。
    assert_eq!(req.header("referer"), Some("http://example.com/form"));
    assert_eq!(req.header("cookie"), Some("sid=42; pref=fast"));
    assert_eq!(req.header("accept-encoding"), Some("identity"), "压缩过的字节不能按偏移落盘");
    assert_eq!(req.header("accept"), Some("*/*"));
    assert!(!req.has("authorization"), "明文 socket 上半副凭据也不许出去（F2 那道闸在这一支同样成立）：{:?}", req.headers);
    assert!(!req.has("connection"), "不发 Connection: close（R5/E2）：{:?}", req.headers);
    assert!(!req.has("content-type"), "不许凭空发明 Content-Type（留给 Task 9 的开放问题）：{:?}", req.headers);

    // 落盘的是**响应体**：长度、内容、偏移三样都对上响应那一侧。
    assert_eq!(out, Outcome::Complete, "{out:?}");
    assert_eq!(store.len()?, 65_536, "落盘长度 = 响应体长度，不是请求体长度（{} 字节）", file_bytes.len());
    harness::require_block_alignment(&path, 0, 65_536)?;
    assert_ne!(harness::bytes_at(&path, 0, file_bytes.len())?, file_bytes, "盘上的字节居然是**请求体** ⇒ 那是把表单当成了成果子");
    assert_eq!(harness::bytes_at(&path, 0, response_body.len())?, response_body, "盘上的字节必须逐字节等于响应体");

    // **F2**：事实取自**这一枪自己的响应**。
    assert_eq!(facts.total_len, 65_536, "长度取自这一枪的 Content-Length");
    assert_eq!(facts.etag.as_deref(), Some("\"stub-post-peer\""), "验证子取自这一枪的头");
    assert_eq!(facts.last_modified, None, "这一枪的头里没有它就是没有（不许编一个）");
    assert!(facts.accept_ranges, "对端这一枪说了 `Accept-Ranges: bytes`");
    assert_eq!(facts.final_url, format!("{}/submit", peer.base()), "落地 URL 就是这一枪的 URL");
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **F2 的下半场**：`accept_ranges` 这一味必须**随这一枪的响应头变**，不是常数。
///
/// 为什么不能只测"true 那一格"：只测 true 的话"永远给 true"的写法（等于没读头）照样绿。
/// 三格一起过才算"读的是这一枪的头"：`bytes` ⇒ 真、`none` ⇒ 假、压根不发这个头 ⇒ 假。
/// 读法与 `probe` 共用同一个函数（`probe::facts_from_response`），所以那一侧的合取判据
/// （评审 **I1**：我们发过 Range 而对方回 200 ⇒ 无条件假）在这里由 `asked_for_range = false`
/// 那一格代劳 —— 这一支从不发 Range，看的就是头。
#[test]
fn the_harvested_facts_are_read_from_this_posts_own_response() -> anyhow::Result<()> {
    for (declared, want) in [(Some("bytes"), true), (Some("none"), false), (None, false)] {
        let peer = harness::spawn_post_stub(declared, harness::positioned_body(4096))?;
        let (store, path) = harness::store("t7-f1-facts.bin")?;
        let file = harness::scratch_body("f1-facts-body.bin", FORM_BODY)?;
        let t = Task {
            url: format!("{}/submit", peer.base()),
            data_file: Some(file.clone()),
            ..harness::blank_task()
        };
        let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
        let facts = facts.expect("2xx 的响应必须有事实");
        let shown = declared.unwrap_or("(没有这个头)");
        eprintln!("---- F2：这一枪的头说 {shown} ⇒ accept_ranges = {} ----", facts.accept_ranges);
        assert_eq!(facts.accept_ranges, want, "对端说 {shown} ⇒ 该读成 {want}，实际 {:?}", facts.accept_ranges);
        assert_eq!(out, Outcome::Complete, "{out:?}");
        assert_eq!(store.len()?, 4096, "这一格的字节也得真落盘（否则测的是失败路径）");
        assert_eq!(facts.total_len, 4096);
        let _ = std::fs::remove_file(&file);
        drop(store);
        std::fs::remove_file(&path).context("清测试文件")?;
    }
    Ok(())
}

/// 没拿到 2xx 的那一枪**不许**交回一份事实：`ProbeFacts` 那五个字段全部来自响应头，
/// 而这次没有可用来定长度的响应头。交 `None`（而不是"猜一份"或"拿上一次那份"）是 F2 那条
/// 五字段契约唯一站得住的读法。
#[test]
fn a_whole_shot_without_a_success_response_hands_back_no_facts() -> anyhow::Result<()> {
    let peer = harness::spawn_post_stub(Some("bytes"), harness::positioned_body(4096))?;
    let (store, path) = harness::store("t7-f1-404.bin")?;
    let file = harness::scratch_body("f1-404-body.bin", FORM_BODY)?;
    // 认不出的路由 ⇒ 对端回 404（体照样发出去了，所以这不是传输失败，是"服务器有说法"）。
    let t = Task {
        url: format!("{}/typo-route", peer.base()),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
    eprintln!("---- 非 2xx 那一支 ----\n{out:?} 事实 {facts:?} len={}", store.len()?);
    assert_eq!(out, Outcome::Failed(FailKind::Http(404)), "状态码要交给 Task 8 记账：{out:?}");
    assert!(facts.is_none(), "404 的响应头里没有一个可用长度，不许硬凑一份事实：{facts:?}");
    assert_eq!(store.len()?, 0, "非 2xx 不许写盘");
    let reqs = peer.requests();
    assert_eq!(reqs.len(), 1, "这一枪确实打出去了（否则测的是连接失败）：{reqs:?}");
    assert_eq!(reqs[0].method, "POST");
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 诚实的**空文档**（`200` + `Content-Length: 0`）在整份这一支是 `Complete` + 一份全长 0 的事实，
/// **不是**那条"零进度必须可数"的失败。
///
/// 两支的区别在 `run_window` 里是判据（区间要字节而对端给 0 ⇒ 失败），在这里必须是另一读法：
/// 整份这一支**没有区间**，"文档本来就是空的"是合法的对端对象（Task 6 评审 **F3** 同一条立场）。
/// 弄混的话，空文件的下载会变成无限重试。
#[test]
fn an_empty_document_is_a_complete_with_zero_length_facts_not_a_countable_failure() -> anyhow::Result<()> {
    let peer = harness::spawn_post_stub(Some("bytes"), vec![])?;
    let (store, path) = harness::store("t7-f1-empty-doc.bin")?;
    let file = harness::scratch_body("f1-empty-doc-body.bin", FORM_BODY)?;
    let t = Task {
        url: format!("{}/submit", peer.base()),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
    eprintln!("---- 空文档那一支 ----\n{out:?} 事实 {facts:?}");
    assert_eq!(out, Outcome::Complete, "对端声明 0 字节并干净收尾 ⇒ 这一份文档拿全了：{out:?}");
    let facts = facts.expect("2xx 的响应必须有事实");
    assert_eq!(facts.total_len, 0, "长度照着响应说，不许改成 1 或猜一个");
    assert_eq!(store.len()?, 0, "0 字节的文档不该把盘撑大");
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 整份那一支的**短 body**：头是诚实的（`Content-Length: 48 MiB`），body 在第 65536 字节被拆线
/// （node 的 `/drop/<n>`，E5 之后本仓库唯一真掐的对端）。
/// ⇒ 写下去的 65536 字节是真进度（裁定 4），而结论**绝不是** `Complete`；事实仍然交得出来，
/// 因为它来自**响应头**，与"字节拿没拿全"是两件事。
///
/// 这一条同时是"没有 Range"的第二台观察者：node 的 JSONL 里 `range` 字段必须是 null、
/// `method` 必须是 POST —— 与 `spawn_post_stub` 那台对端的取证互相对照（两台都不由我们自证）。
#[test]
fn a_short_body_on_the_whole_shot_keeps_its_bytes_and_is_never_complete() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-f1-short.bin")?;
    let file = harness::scratch_body("f1-short-body.bin", FORM_BODY)?;
    let t = Task {
        url: format!("{base}/drop/65536"),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
    eprintln!("---- 整份 + 短 body ----\n{out:?} len={} 事实 {facts:?}", store.len()?);
    assert_eq!(out, Outcome::Advanced { bytes: 65_536 }, "被掐断前写下去的字节必须交回，且不许折成 Complete：{out:?}");
    assert_eq!(store.len()?, 65_536);
    harness::require_block_alignment(&path, 0, 65_536)?;
    let facts = facts.expect("响应头先到，事实就有了（与 body 拿没拿全无关）");
    assert_eq!(facts.total_len, 48 * 1024 * 1024, "长度是**对端声明的** 48 MiB，不是我们拿到的 65536");
    assert!(facts.accept_ranges, "/drop 的头是诚实的（`Accept-Ranges: bytes`）");
    let reqs = srv.requests_at_least(1)?;
    eprintln!("---- node 那台观察者看到的 ----\n{}", serde_json::to_string_pretty(&reqs)?);
    assert_eq!(reqs[0]["method"], "POST", "第二台观察者也得看到 POST（不是我们自证的）");
    assert!(reqs[0]["range"].is_null(), "POST 那一支一条 Range 都不许上线：{:?}", reqs[0]);
    assert!(reqs[0]["connection"].is_null(), "不发 Connection: close（R5/E2）：{:?}", reqs[0]);
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **E9 那道闸在整份这一支同样成立**：`run_whole` 交回的 `ProbeFacts.final_url` 就是落地 URL，
/// 而落地时带着 userinfo ⇒ 拒，且那句消息**不回显**那条地址（与 `probe` 同一个常量、同一个判据）。
#[test]
fn a_data_file_task_redirected_onto_a_credentialed_landing_url_is_refused_and_never_echoed() -> anyhow::Result<()> {
    let (stub, _land_url) = harness::spawn_redirect_stub("alice:s3cret@127.0.0.1:{port}")?;
    let (store, path) = harness::store("t7-f1-landing.bin")?;
    let file = harness::scratch_body("f1-landing-body.bin", FORM_BODY)?;
    let t = Task {
        url: format!("{}/start", stub.base()),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let Err(err) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000) else {
        panic!("落地 URL 的 authority 里带着凭据 ⇒ 这一份事实必须被拒（E9 / Task 5 评审 C3）");
    };
    let msg = err.to_string();
    eprintln!("---- 落地 URL 带凭据那一支（消息照抄，它本身不含任何地址成分）----\n{msg}");
    assert!(msg.contains("凭据") && msg.contains("重定向"), "措辞要可行动：{msg}");
    for leak in ["alice", "s3cret", "@", "http://", "https://"] {
        assert!(!msg.contains(leak), "拒绝文本里出现了 URL 的成分（{leak}）：{msg}");
    }
    assert!(!msg.contains("cvv") && !msg.contains("hunter2"), "顺手抄了请求体：{msg}");
    assert_eq!(store.len()?, 0, "被拒的这一枪不许落过任何字节");
    assert!(stub.saw("/landed"), "重定向必须真跟随到了那条带凭据的地址（否则测的是连接失败）：{:?}", stub.seen());
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 体文件读不出来 ⇒ 报错，且**一枪都不许打出去**、错误里一个字都不抄那份体。
///
/// "不抄体"这一半不是恒真断言：同一句哨兵在
/// `a_data_file_task_is_posted_with_the_file_as_its_body_and_lands_the_response_body`
/// 里**确实**被我们读出来并送上了线 ⇒ 那些字节到过我们手里；这里断的是它们不进消息。
#[test]
fn a_missing_data_file_stops_the_whole_shot_before_the_wire_and_never_echoes_the_body() -> anyhow::Result<()> {
    let peer = harness::spawn_post_stub(Some("bytes"), harness::positioned_body(4096))?;
    let (store, path) = harness::store("t7-f1-nofile.bin")?;
    let file = harness::scratch_body("f1-gone-body.bin", FORM_BODY)?;
    std::fs::remove_file(&file).context("先证明这份体存在过（删掉它才有'读不出来'这一支）")?;
    let t = Task {
        url: format!("{}/submit", peer.base()),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let Err(err) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000) else {
        panic!("体文件读不出来时必须报错，不许悄悄发一条空体/缺体的 POST");
    };
    let msg = err.to_string();
    eprintln!("---- 体文件不在 ----\n{msg}");
    assert!(msg.contains("--data-file"), "错误要点名那个参数：{msg}");
    assert!(msg.contains("f1-gone-body.bin"), "路径要点名（那是用户能给对的东西）：{msg}");
    for leak in ["cvv", "hunter2", "session", "user=alice"] {
        assert!(!msg.contains(leak), "错误里抄了体的字节（{leak}）：{msg}");
    }
    assert!(peer.requests().is_empty(), "拒在上网之前：对端一条请求都不该看到：{:?}", peer.requests());
    assert_eq!(store.len()?, 0);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 整份那一支的**零进度**：对端把响应头交出来、body 一个字节都不发（`spawn_head_only_stub`），
/// 于是预算在"什么都没有"上到点 ⇒ `Stalled`（定义：预算到点**且零字节落盘**）。
///
/// 为什么 `/drop/65536` 那条不够：那条有进度 ⇒ 走 `Advanced`，而 Task 8 的重试封顶数的正是
/// "零进制的卡住"。这一条另外钉住一件容易写错的事：**`Stalled` 也可以带着事实** —— 响应头确实到了，
/// 那五个字段就是真读到的东西；把它跟失败结论一起丢掉等于让调用方以为这一枪连状态码都没看到。
#[test]
fn a_whole_shot_with_a_head_and_no_body_is_stalled_with_the_facts_it_saw() -> anyhow::Result<()> {
    let stub = harness::spawn_head_only_stub(8 * 1024 * 1024)?;
    let (store, path) = harness::store("t7-f1-headonly.bin")?;
    let file = harness::scratch_body("f1-headonly-body.bin", FORM_BODY)?;
    let t = Task {
        url: format!("{}/hangs", stub.base()),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let started = Instant::now();
    let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
    let took = started.elapsed();
    eprintln!("---- 整份 + 只有头 ----\n{out:?} 用时 {took:?} 事实 {facts:?}");
    assert_eq!(out, Outcome::Stalled, "头收了、body 一个字节没到 ⇒ 零进制的预算到点，只能是 Stalled：{out:?}");
    assert!(took >= Duration::from_secs(14), "读侧预算同样是 15 s 下界：{took:?}");
    assert_eq!(store.len()?, 0, "零进度不许悄悄写过盘");
    let facts = facts.expect("响应头到了，事实就是读到的东西，不许跟失败结论一起丢");
    assert_eq!(facts.total_len, 8 * 1024 * 1024, "长度取自这一枪的 Content-Length");
    assert!(facts.accept_ranges, "对端这一枪说了 `Accept-Ranges: bytes`");
    assert!(stub.saw("/hangs"), "必须真打到对端：{:?}", stub.seen());
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// 整份那一支的**掐在第一个字节之前**：node 的 `/drop/0` 把头发完就拆线（诚实的
/// `Content-Length: 48 MiB` + body 0 字节）。⇒ 零进度 + **不是**超时 ⇒ `Failed(Transport)`，
/// 而事实照样交得出来（响应头到了）。
///
/// 这条与 `a_whole_shot_with_a_head_and_no_body_is_stalled_with_the_facts_it_saw` 是同一个三元组
/// 里的另两支：那个对端**挂着不拆线**（超时 ⇒ `Stalled`），这个**拆线**（不是超时 ⇒ `Failed`）。
/// Task 8 的 §6.1/§6.2 封顶数的就是这两支，写混的话一条永不给字节的对端会拿到错误的退避时长
/// （而 `Advanced { bytes: 0 }` 那种写法更是让封顶一个也数不到 —— 上一轮变异 M9 的教训）。
#[test]
fn a_whole_shot_cut_off_before_the_first_byte_is_a_countable_failure() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-f1-drop0.bin")?;
    let file = harness::scratch_body("f1-drop0-body.bin", FORM_BODY)?;
    let t = Task {
        url: format!("{base}/drop/0"),
        data_file: Some(file.clone()),
        ..harness::blank_task()
    };
    let (out, facts) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000)?;
    eprintln!("---- 整份 + 头发完就拆线 ----\n{out:?} 事实 {facts:?}");
    // 本轮 D：`run_whole` 的这一支与 `run_window` 走的是**同一个**构造器（措辞不许在两条入口上分岔）。
    // 这一枪的请求体（`FORM_BODY`，带着口令形状的那 33 字节）**确实**被我们打开过并送上了线 ⇒
    // "原因里没有一个字的体"在这里不是恒真断言。
    let note = take_transport_note(&out, "零字节 + 对端拆线 ⇒ 只能算失败");
    eprintln!("---- D：整份那一支的原因原文 ----\n{note}");
    for leak in ["cvv", "hunter2", "session", "user=alice"] {
        assert!(!note.contains(leak), "原因里抄了请求体（{leak}）：{note}");
    }
    assert_eq!(store.len()?, 0, "零进度不许悄悄写过盘");
    let facts = facts.expect("响应头到了，事实就是读到的东西");
    assert_eq!(facts.total_len, 48 * 1024 * 1024, "长度照响应自己说的，不改成我们拿到的 0");
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **命名这条性质**（派发点名"做成用例，别留在注释里"）：窗口预算那一枪的期限是
/// **整请求期限**，body 也算在里面 —— 对端**一直活着**也会被掐。
///
/// 这条用例推翻了上一轮的记录（当时写"每次 `read()` 重新起算的静默期限，慢但活着的节拍打不中它"，
/// 而它自己的对照用例 `/sz/524288s` 1.6 s 就交完 body，两种语义给同一个结论 ⇒ 那条断言是瞎的）。
/// 本轮把它做成能分辨的实验：节拍 260 ms / 64 KiB ⇒ 连续静默从没超过 260 ms，而预算是 15 s。
/// - 整请求期限 ⇒ 15 s 到点，交回**少于一个窗口**的字节；
/// - 静默期限 ⇒ 根本不会响，交回**正好一个窗口** 4 MiB。
///
/// 第二段是同一台对端、同一个节拍，只要**总时长**在预算内（512 KiB ⇒ ~1.8 s）就必须完整读完：
/// 于是"掐我们的确实是这一枪已经用了多久"，不是"哪一块来得慢"。
///
/// `execute` 那一侧（响应头之前）的期限同样是整请求期限，由 `a_peer_that_never_answers_is_stalled`
/// 钉着；两条路径语义**相同**这一点由这两条用例合起来覆盖（派发要求"若不同就各钉一条"）。
#[test]
fn the_window_budget_is_a_whole_request_deadline_on_the_read_side_too() -> anyhow::Result<()> {
    let peer = harness::spawn_paced_partial_stub(6 * 1024 * 1024, 64 * 1024, Duration::from_millis(260))?;
    let client = harness::client()?;
    let t = Task { url: format!("{}/blob", peer.base()), ..harness::blank_task() };
    let (store, path) = harness::store("t7-budget-cut.bin")?;
    let iv = Interval { start: 0, end: WINDOW_BYTES * 2 - 1 };
    let started = Instant::now();

    // 速率取 `WINDOW_BYTES` ⇒ 式子给 2 s，被下界夹成 15 s —— 正是要让"填满一个窗口"比预算更久。
    let out = fetch::run_window(&client, &t, iv, &store, WINDOW_BYTES, 48 * 1024 * 1024, &harness::uncapped())?;
    let took = started.elapsed();
    let Outcome::Advanced { bytes } = out else {
        panic!("被自己的期限掐在一个窗口中间 ⇒ 必须是 Advanced（带着已落盘的字节），实际 {out:?}");
    };
    eprintln!(
        "---- 整请求期限（读侧）----\n{out:?} 用时 {took:?} 对端写出 {} 字节（它承认的 body 是 6 MiB）",
        peer.written()
    );
    assert!(took >= Duration::from_secs(14), "这一枪要跑到预算那一档才说明响的是期限，不是别的东西：{took:?}");
    assert!(took < Duration::from_secs(30), "到点就收手，不许再等第二个 15 s 或撞上客户端默认的 30 s：{took:?}");
    assert!(bytes < WINDOW_BYTES, "对端每块之间只静默 260 ms（远小于 15 s 预算）仍然被掐 ⇒ 期限算的是整请求；若是静默期限，这里应该是正好 {WINDOW_BYTES}");
    assert!(bytes > 0, "被掐之前写下去的字节是真进度（裁定 4）");
    assert_eq!(store.len()?, bytes, "交回的字节数与盘上的量同源");
    harness::require_block_alignment(&path, 0, bytes)?;
    assert!(peer.written() >= bytes, "对端至少写出了我们读到的那么多 ⇒ 它没停，停的是我们");
    assert_eq!(
        peer.ranges()[0],
        format!("bytes=0-{}", WINDOW_BYTES - 1),
        "这一枪发的确实是带显式终点的那一条，而且**终点是窗口那一格**（E14②：旧写法在这里发的是\
         区间终点 `bytes=0-8388607`；区间终点在文件中间，两种写法都该带显式终点 ⇒ 判据方向没动，\
         动的是那个终点的取法）。对端按它答了 206"
    );

    // 同一个节拍、总时长在预算内 ⇒ 必须完整读完（否则上一段红的原因就说不清是哪一种）。
    let small = Interval { start: 0, end: 512 * 1024 - 1 };
    let (store2, path2) = harness::store("t7-budget-ok.bin")?;
    let started2 = Instant::now();
    let out2 = fetch::run_window(&client, &t, small, &store2, WINDOW_BYTES, 48 * 1024 * 1024, &harness::uncapped())?;
    let took2 = started2.elapsed();
    eprintln!("---- 同一台对端，只要总时长在预算内 ----\n{out2:?} 用时 {took2:?}");
    assert_eq!(out2, Outcome::Complete, "512 KiB 在 ~1.8 s 内读完 ⇒ 同一个期限没响，区间读完就是 Complete：{out2:?}");
    assert!(took2 < Duration::from_secs(10), "这一段必须明显快过预算，否则对照不成立：{took2:?}");
    harness::require_block_alignment(&path2, 0, 512 * 1024)?;

    drop((store, store2));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path2);
    Ok(())
}

// ================================================================ Task 7 修复轮 1：A / B（存活变异的两格边界）
//
// 评审在 `ba906f4` 上跑出的三格**存活变异**：判据改坏之后整个套件照样绿。C（416 的分母读成
// `iv.start`）改的是 `a_416_whose_denominator_is_our_length_is_complete` 那条既有用例，见那里；
// 下面两条补的是 A（206 的起点不符）与 B（`iv.end` 含与不含的那一条边界）。

/// **A**：206 **说得出**一个起点，但那个起点不是我们问的那一个 ⇒ `Failed(ValidatorChanged)`，
/// 而且**一个字节都不许落盘**。
///
/// 为什么这一格比"对端说不出起点"危险得多：缺头那一支挡的是"按我们想要的偏移硬写"，而起点不符
/// 那一支写下去的字节**来自文件的另一段** —— 长度对、块数对、`Advanced`/`Complete` 照样给，
/// 于是错位的字节被报成"这一段下完了"。`E7`/`E9` 之后封存不看盘，所以错位只有到 sha256 门槛
/// （Task 10）才现形，而那时已经"完成"了。
///
/// 既有那条 `a_206_that_cannot_show_its_offset_is_validator_changed` 钉不住这一格：它走
/// `/no-content-range`（`asked_at` 是 `None`）且 `iv.start == 0`，所以把判据
/// `asked_at != Some(iv.start)` 写成 `asked_at.is_none()` 在它身上读起来一模一样。
///
/// 两条形状分开钉，差的是"差多少"：
/// - `/known-total` ⇒ 对端声称起点是 **0**，我们问的是 4096（差 `iv.start` 本身）
/// - `/wrong-start` ⇒ 对端声称起点是 **1024**，我们问的是 4096（差一个第三方数）
///   第二条另外挡掉"把 0 当成我们的起点"那种写法（`asked_at != Some(0)`），那种写法在第一条上
///   与正确实现同判、在真服务器上（起点 0 是合法起点）却会把骗子放行。
#[test]
fn a_206_that_claims_another_offset_than_the_one_we_asked_for_writes_nothing() -> anyhow::Result<()> {
    let stub = harness::spawn_unknown_total_stub(1024)?;
    let client = harness::client()?;
    let (store, path) = harness::store("t7-206-wrong-offset.bin")?;
    let iv = Interval { start: 4_096, end: 4_096 + 1_024 - 1 };

    for (route, claimed) in [("/known-total", 0u64), ("/wrong-start", 1_024)] {
        let t = Task { url: format!("{}{route}", stub.base()), ..harness::blank_task() };
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
        eprintln!(
            "---- A：我们问 {}，{route} 声称 {claimed} ----\n{out:?} len={}",
            iv.start,
            store.len()?
        );
        assert_eq!(
            out,
            Outcome::Failed(FailKind::ValidatorChanged),
            "{route}：对端声称的起点 {claimed} 与我们问的 {} 不是同一个 ⇒ 偏移核对不了，\
             只能是 ValidatorChanged（写成 `asked_at.is_none()` 的判据会在这一枪放行，\
             于是那 1024 个来自文件别处的字节被写进偏移 {} 起）：{out:?}",
            iv.start,
            iv.start
        );
        assert_eq!(store.len()?, 0, "{route}：起点不符 ⇒ 一个字节都不许落盘");
    }
    // "没被吸进文件"由**独立读者**证（不经 Store）：文件长度真的还是 0，不是"写过又缩回去"。
    assert_eq!(std::fs::metadata(&path)?.len(), 0, "响应体被落进文件了（长度不是 0）");
    assert_eq!(stub.seen().len(), 2, "两枪都要真打到对端（否则测的是连接失败）：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **B** 的左半边：区间**正好**一个窗口（长度 = `WINDOW_BYTES`）⇒ 区间的最后一个字节
/// （`iv.end`，含）**确实**落到了盘上 ⇒ 必须 `Complete`。
///
/// `iv.end` 是**含端点**的（`Interval::len() = end - start + 1`），而 `run_window` 的收手条件是
/// `stop = min(iv.end + 1, iv.start + WINDOW_BYTES)` ⇒ 区间长度 = 窗口时 `cursor` 恰好走到
/// `iv.end + 1`，**越过了** `iv.end`。`Complete` 承诺的是"`iv.end` 那个字节到了"，所以判据只能是
/// `cursor > iv.end`。
#[test]
fn an_interval_of_exactly_one_window_is_complete() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let (store, path) = harness::store("t7-window-exact.bin")?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };
    let iv = Interval { start: ALIGNED, end: ALIGNED + WINDOW_BYTES - 1 };
    assert_eq!(iv.len(), WINDOW_BYTES, "这一条要的就是「正好一个窗口」那个形状");

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
    eprintln!("---- B 左半边：len(iv) == WINDOW_BYTES ----\n{out:?} len={}", store.len()?);
    assert_eq!(out, Outcome::Complete, "区间正好一个窗口 ⇒ 最后一个字节落盘了，必须 Complete：{out:?}");
    assert_eq!(store.len()?, iv.end + 1, "含端点：写完 iv.len() 个字节 ⇒ 长度是 iv.end + 1");
    // 块号取证覆盖窗口内每一个 4 KiB 块首（最后一块是第 1279 块，起点 5238784）⇒ 落盘的内容
    // 确实来自 `[ALIGNED, ALIGNED+WINDOW_BYTES)`，没错位也没掉段。
    harness::require_block_alignment(&path, ALIGNED, WINDOW_BYTES)?;
    // 区间最后一个字节（下标 `iv.end`）**读得出来** —— 与右半边那条互为对照，这一对才把
    // "`Complete` 意味着 `iv.end` 到了" 钉成字节级的凭据，而不是一句我们自己的措辞。
    // 刻意不断它的内容：blob 只在块首写块号，块内其余字节是 `allocUnsafe` 的填充（不是契约）。
    assert!(
        harness::bytes_at(&path, iv.end, 1).is_ok(),
        "报了 Complete 却连区间最后一个字节都不在盘上（长度 {}，下标 {} 读不出来）",
        store.len()?,
        iv.end
    );
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **B** 的右半边：区间比窗口**多一个字节** ⇒ 那一个字节（`iv.end`）**没有**落盘 ⇒ 只能
/// `Advanced { bytes: WINDOW_BYTES }`。
///
/// 这一条是 `>` 与 `>=` 的分水岭：`cursor` 在这里最多走到 `stop = iv.start + WINDOW_BYTES = iv.end`，
/// 永远越不过 `iv.end`。判据写成 `cursor >= iv.end` 的话，一个长度为 `WINDOW_BYTES + 1` 的区间会
/// 把**尾巴那个字节从没落盘**的一段封成 `Complete` ⇒ `E7`/`E9` 之后 `SegmentComplete` 不看盘，
/// 那个字节就再也不会被取。所以这一条不许靠注释、要靠 `store.len()` 与"读 `iv.end` 读不出来"钉。
#[test]
fn an_interval_one_byte_wider_than_the_window_is_never_complete() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let (store, path) = harness::store("t7-window-plus-one.bin")?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };
    let iv = Interval { start: ALIGNED, end: ALIGNED + WINDOW_BYTES };
    assert_eq!(iv.len(), WINDOW_BYTES + 1, "多一个字节 ⇒ 窗口填不满这个区间");
    let edge = iv.end; // 区间最后一个字节所在的**下标**（格式串里不支持字段访问，取个局部名）

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped())?;
    eprintln!("---- B 右半边：len(iv) == WINDOW_BYTES + 1 ----\n{out:?} len={}", store.len()?);
    assert_eq!(
        out,
        Outcome::Advanced { bytes: WINDOW_BYTES },
        "区间比窗口多一个字节 ⇒ 交回「正好一个窗口」，并如实说那个尾巴还没到：{out:?}"
    );
    assert_eq!(store.len()?, edge, "长度停在 iv.end ⇒ 下标 iv.end 那个字节不在盘上");
    let on_disk = std::fs::metadata(&path)?.len();
    assert!(
        on_disk <= edge,
        "盘上长度越过了窗口上界（{edge}）⇒ stop 算错了：len={on_disk}"
    );
    assert!(
        harness::bytes_at(&path, edge, 1).is_err(),
        "区间的最后一个字节（下标 {edge}）**必须**读不出来 —— 它没落盘，而 Complete 会把它封掉"
    );
    harness::require_block_alignment(&path, ALIGNED, WINDOW_BYTES)?;
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

// ================================================================ Task 7 修复轮 1：D（Transport 带原因）+ drive-by
//
// 契约改动（控制方裁定，勘误 **E12**⑦已登记）：`FailKind::Transport` 从单元变体变成
// `Transport(String)` —— 那句 `String` 是**这一次**失败的原因（reqwest / `io::Error` 的 message 加
// `source()` 链首句，压成一行、≤200 字符、不抄 body、不重复 URL）。理由：Task 8 要把这句话交给用户，
// 裸枚举到那里只剩"网络错了"，而"网络错了"没法行动。
//
// 四条形状规则由 `src/fetch.rs` 的单元用例逐条钉（假错误喂进去，那里能构造任意外形）；**这一节**钉的是
// 每个真生产支交回来的确实是"一句能用的话"，并且线上真的没有 body 字节、没有第二条 URL。

/// 把 `Outcome::Failed(FailKind::Transport(note))` 里那句原因取出来，顺带钉住它的**形状契约**
/// （E12⑦：非空、一行、最多 200 个**字符**）。不是这一支就当场 panic —— 调用方的用例于是仍然
/// 只测它要测的那一支，"变体对不对"这条判断没有因为换成 `Transport(_)` 而变松。
fn take_transport_note(out: &Outcome, shape: &str) -> String {
    let Outcome::Failed(FailKind::Transport(note)) = out else {
        panic!("{shape} ⇒ 期待 Failed(Transport(那句原因))，实际是 {out:?}");
    };
    assert!(!note.is_empty(), "{shape}：Transport 不许带空原因（空话与裸枚举一样没法行动）");
    assert!(!note.contains('\n') && !note.contains('\r'), "{shape}：原因必须是一行：{note:?}");
    assert!(
        note.chars().count() <= 200,
        "{shape}：原因超过 200 个字符（{} 个）：{note}",
        note.chars().count()
    );
    note.clone()
}

/// **D**：`execute` 那一支（连响应头都没等到）也必须带着原因回来。
///
/// 形状：向一个**刚刚放开**的临时端口打枪（`bind` 到端口 0 拿到号之后立刻 drop ⇒ 本机立刻回 RST），
/// 于是 reqwest 交回一次连接失败。这是 `run_window` / `run_whole` 两处 `execute` 支唯一能真构造出来
/// 的入口（其余 Transport 支要么要真断线、要么要真拆线）。
///
/// 钉三件事：① 两支都交回 `Transport` **且带着话**（不是空串、不是一行里的半截）；② 那句话里
/// **没有 URL** —— reqwest 把 URL 抄在 `Display` 的尾巴上，摘掉是契约（URL 不是原因，而 Task 8
/// 打印这句时旁边就站着这条 URL，留着等于说两遍）；③ 还没看到响应 ⇒ `run_whole` 交不回事实（`None`），
/// 且盘上仍然是空的。
#[test]
fn a_transport_failure_from_the_execute_side_names_the_cause_without_repeating_the_url() -> anyhow::Result<()> {
    let port = {
        let listener = std::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .context("向 OS 要一个空闲端口失败")?;
        listener.local_addr().context("拿不到临时端口")?.port()
    }; // 离开作用域即放开：下一次连这个号就是"没人听"。
    let dead = format!("http://127.0.0.1:{port}");
    let client = harness::client()?;
    let (store, path) = harness::store("t7-transport-exec.bin")?;

    // ① 窗口那一支（GET + Range）
    let t = Task { url: format!("{dead}/blob"), ..harness::blank_task() };
    let out = fetch::run_window(&client, &t, Interval { start: 0, end: 4_095 }, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    let note = take_transport_note(&out, "窗口那一枪连响应头都没等到（端口没人听）");
    eprintln!("---- D：run_window 的 execute 支 ----\n{out:?}\n原因原文：{note}");
    assert!(!note.contains("http"), "原因里还有 URL 的成分：{note}");
    assert!(!note.contains(&port.to_string()), "原因里抄了这次的目标端口（Task 8 打印时旁边就有 URL）：{note}");
    assert_eq!(store.len()?, 0, "连接都没成 ⇒ 盘上必须还是空的");

    // ② 整份那一支（POST）：同一个构造器、同一条规则，而且**没有**事实可交（没看到响应）。
    let file = harness::scratch_body("f1-exec-dead-body.bin", FORM_BODY)?;
    let pt = Task { url: format!("{dead}/submit"), data_file: Some(file.clone()), ..harness::blank_task() };
    let (out2, facts) = fetch::run_whole(&client, &pt, &store, 10_000_000)?;
    let note2 = take_transport_note(&out2, "整份那一枪连响应头都没等到（同一个端口）");
    eprintln!("---- D：run_whole 的 execute 支 ----\n{out2:?}\n原因原文：{note2}");
    assert!(facts.is_none(), "还没看到响应就失败 ⇒ 不许硬凑一份事实：{facts:?}");
    assert!(!note2.contains("http") && !note2.contains(&port.to_string()), "同一支规则不许在两条入口上分岔：{note2}");
    for leak in ["cvv", "hunter2", "session", "user=alice"] {
        assert!(!note2.contains(leak), "原因里抄了请求体（{leak}）：{note2}");
    }
    assert_eq!(store.len()?, 0, "两支都不该动过盘");
    let _ = std::fs::remove_file(&file);
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **D 的"不抄 body"那一半**：那 4096 个字节**确实到过我们的读缓冲**，而句原因里一个字节都不许有。
///
/// 形状：对端回 `200` + `Content-Length: 4096` + 4096 个 `'r'`（`spawn_range_ignoring_stub`，
/// 刻意挑全同字节的那种 body —— 于是"原因里有没有 ASCII 字母/数字"这条断言就是"有没有 body 字节"
/// 这条断言，没有中间地带），而区间起点正好等于 `iv.start = 4096` ⇒ 降级支把整份 body 当成要丢的
/// **前导**流过去，一个字节都没落盘 ⇒ 零进度 ⇒ `Transport`（`run_window` 的 EOF 支，那句原因是
/// 我们自己写的，见 `WINDOW_EOF_WITHOUT_BYTES`）。
///
/// 为什么这条不是恒真断言：字节真的经过了 `resp.read()` 的那个 `buf`，所以"把最后一块 / 它的长度 /
/// 它的 Debug 形状抄进原因"那种写法会在这里红（`is_ascii_alphanumeric` 同时挡住 `'r'` 与 `[114,`）。
/// 而 `run_window` 与 `run_whole` 共用同一个构造器 ⇒ 这一支挡住的就是那两支的规则。
#[test]
fn a_transport_note_holds_no_byte_of_the_body_it_handed_back() -> anyhow::Result<()> {
    const BODY: u64 = 4_096;
    let stub = harness::spawn_range_ignoring_stub(BODY)?;
    let (store, path) = harness::store("t7-note-holds-no-body.bin")?;
    let t = Task { url: format!("{}/claims-bytes-answers-200", stub.base()), ..harness::blank_task() };
    let iv = Interval { start: BODY, end: BODY * 2 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped())?;
    let note = take_transport_note(&out, "区间要 4096 字节、对端那 4096 字节全是要丢的前导 ⇒ 零进度");
    eprintln!("---- D：那句原因的原文（4096 个 'r' 一个字节都没进账）----\n{note}");
    assert!(
        note.chars().all(|c| !c.is_ascii_alphanumeric()),
        "原因里出现了 ASCII 字母或数字 ⇒ 那是 body 的字节（对端交的 4096 个 'r'）或它的长度：{note}"
    );
    assert!(note.contains("响应体") && note.contains("干净结束"), "原因要说清是哪一种零进度：{note}");
    assert_eq!(store.len()?, 0, "零进度不许悄悄写过盘");
    assert_eq!(std::fs::metadata(&path)?.len(), 0, "那 4096 个字节没被落进文件（独立读者）");
    assert!(stub.saw("/claims-bytes-answers-200"), "必须真打到对端（否则测的是连接失败）：{:?}", stub.seen());
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

/// **drive-by（派发点名的第二条）**：`--data-file` 指向**目录**时必须**当场**拒，且不碰网络。
///
/// 派发给的理由（"Windows 上 `File::open` 对目录是成功的，错误要到 reqwest 的驱动线程读体时才冒
/// 出来 ⇒ POST 已经上了线"）在本机**实测不成立**：`File::open` 对一个目录在打开那一步就返回
/// `PermissionDenied / 拒绝访问。 (os error 5)`（Windows 要求目录句柄带 `FILE_FLAG_BACKUP_SEMANTICS`，
/// std 不带）。所以这条用例钉的不是"少一次上线"，而是**那句错误话说对了没有**：旧写法把"你给的是
/// 目录"报成"拒绝访问" —— 路径明明存在、明明可读，用户照着去查 ACL 就白跑一趟。同时把"绝不碰网络"
/// 也钉住（`spawn_post_stub` 是独立观察者，连请求体都收得下来），因为这道闸住在打开之前，
/// 那条性质从此由构造保证而不是由运气保证。摘掉 `ensure!` 之后本用例红在"普通文件"那一条断言上
/// （消息变成"读 --data-file 的请求体文件失败：…"），取证见报告。
#[test]
fn a_directory_handed_to_data_file_is_refused_before_the_wire_and_names_the_path() -> anyhow::Result<()> {
    let peer = harness::spawn_post_stub(Some("bytes"), harness::positioned_body(4096))?;
    let (store, path) = harness::store("t7-dir-as-body.bin")?;
    // 一个**真的目录**：`harness::blank_task()` 的 `dir`（`target/harness-scratch`，夹具建好的那个）。
    let scratch = harness::blank_task();
    let dir = scratch.dir.clone();
    assert!(dir.is_dir(), "这一条的输入必须是目录（{dir:?} 不是）");
    let t = Task {
        url: format!("{}/submit", peer.base()),
        data_file: Some(dir.clone()),
        ..harness::blank_task()
    };

    let Err(err) = fetch::run_whole(&harness::client()?, &t, &store, 10_000_000) else {
        panic!("目录当请求体必须当场报错，不许把这份体交给 reqwest 去读");
    };
    let msg = err.to_string();
    eprintln!("---- drive-by：目录当体 ----\n{msg}\n原因链：{err:?}");
    assert!(msg.contains("--data-file"), "错误要点名那个参数，用户才知道改什么：{msg}");
    assert!(msg.contains("普通文件"), "错误要说清是「不是文件」，而不是「打不开」：{msg}");
    assert!(!msg.contains("拒绝访问") && !msg.contains("权限"), "把「你给的是目录」报成权限问题会把用户支到 ACL 上去查：{msg}");
    assert!(msg.contains("harness-scratch"), "路径要点名（那是用户能给对的东西）：{msg}");
    assert!(!msg.contains('\n'), "错误消息是一行话：{msg}");
    assert!(peer.requests().is_empty(), "拒在上网之前：对端一条请求都不该看到（POST 一旦上线就是真提交）：{:?}", peer.requests());
    assert_eq!(store.len()?, 0, "被拒的这一枪不许动过盘");
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;
    Ok(())
}

// ================================================================ Task 8：引擎主循环

/// 简报 Task 8 Step 1 的第一条：8 MiB 走 node 对端、`workers = 4`、带 `verify_sha256`。
///
/// 简报给这条用例起的全名是 `full_download_is_byte_exact_and_ranges_match_scheduler`。
/// **本轮只实现前半截**（"跑得完 + 字节级正确"）：后半截那道"派发的区间与线上 `Range`、与
/// 回放给全新 `Scheduler` 的动作序列三方对齐"要用到 `Report::replay()` / `dispatched()` /
/// `tightened()` 这三道链接，按派发归**阶段 5**，本轮只保证 `trace` 填得足够它写出来。
///
/// 为什么 `8 MiB × 4 worker` 这个形状顺手钉住了模块文档那条"派发公式差一字节"的更正：
/// 第一条被派出去的区间是 `[236032, 8 MiB-1]`（7.75 MiB）**长过一个窗口**（`WINDOW_BYTES`
/// = 4 MiB），所以这一路必然出现 `Advanced{bytes}` 那一支；写成 `iv.start + bytes`（不含端点）
/// 会把**从没写过的那一个字节**报成已确认，于是文件里留一个洞。洞在 blob 上落到非块首的概率是
/// 4095/4096 ⇒ **这条集成用例不是那个差一的可靠判据**，真正的判据是单元用例
/// `the_advanced_mapping_is_inclusive_and_bounded_to_the_interval`；这条用例钉的是更大的面：
/// 引擎真能把七个模块接成一次跑到底的下载，并且落盘的字节与服务器那份一字不差。
#[test]
fn full_download_is_byte_exact() -> anyhow::Result<()> {
    // `_srv` 要活到断言跑完（`Server::Drop` 会杀进程并**删掉日志文件**）。
    let (_srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng1")?;
    let len = 8 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/sz/{len}"),
        dir: dir.path().to_path_buf(),
        ..harness::blank_task()
    };

    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "引擎没跑完：{:?}", rep.message);
    assert_eq!(rep.bytes_total, len, "服务器说的长度");
    assert_eq!(rep.delivered, len, "收尾时盘上真有这么多字节");

    // 独立读者（harness 自己那份 sha2）与引擎的判定分开：两边都说对，才算对。
    assert_eq!(harness::sha256_of(&t.target_path())?, harness::expected_sha(len));
    Ok(())
}

// ================================================================ Task 8 阶段5A：E14（乙：终点帽 / 戊：窗口终点 / ③：重复账）
//
// 勘误 **E14** 裁定了两件事并要第三件事有个说法。这一节的五条用例按那三条分：
// - **乙（终点帽）**：`a_shot_stops_at_the_end_cap_lowered_mid_body_...` 与
//   `an_end_cap_already_lowered_before_the_shot_starts_...`（都走**进程内节拍对端** ⇒ 不是计时彩票）。
//   这两条就是"每笔收紧 ≤ 一块"那一支的**全部**证据，它们是**单发级**的（一次一枪、帽由测试自己下）。
//   这里**没有**引擎层的同判据用例，而且它按现有可观测量**写不出来**：`Report::trace` 只在一枪
//   **结束时**才交回一个 `Progress{abs_end}`（`Advanced`/`Complete` 才是它的进度），而帽可以落在
//   那一枪早已读过的偏移之下（E14 附记"残余那一截"的成因就是这句）—— 于是"落点 ≤ 它收到的第一个
//   帽终点 + 一块"这个量在 trace 里根本没有对应的观测量（要它就得让 fetch 每块上报进度，
//   那是 `fetch` 的契约变更，归 8F-2 之后另裁）。原先这里点的那个用例名是**悬空引用**（T8 评审 C-I-3），
//   按 E15 的口径删掉，改成把"拿不到"这件事写在明处。
// - **戊（开放终点的判据是窗口）**：`every_wire_request_asks_for_at_most_a_window_...`（取证在**线上**，
//   即 node 服务器记下的 `Range`，不是我们自己拼的字符串）。
// - **③（重复账）**：`the_tightening_bill_on_a_paced_peer_is_priced_by_chunk_not_by_window` —— 判据是**线上流量**，
//   对端有节拍（`/sz/<n>s`，每 64 KiB 之间 200 ms）⇒ 服务器写出去的字节跟着我们读的走，
//   那本账才量得出东西（无节拍的环回对端会把整条 span 一次塞进 socket，见报告）。

/// **E14① 乙（主用例）**：帽在一枪读到一半时降下去 ⇒ 那一枪**停手**，交回 `Advanced { bytes }`，
/// 并且盘上没有任何"帽 + 一块"之外的字节。
///
/// 形状与为什么它是决定性的：对端是**诚实 206 的节拍对端**（每 15 ms 交一块 64 KiB），区间
/// `[0, WINDOW_BYTES)` 正好一个窗口（64 块），帽降到第 12 块末。看盘的人（另一个线程）在写到第 8 块
/// 之后把帽降下去 ⇒ 正确实现最多再写**一块**就收手（落点 ≤ 13 块），而把读循环改成"不查帽"的变异
/// 会读满整个窗口（64 块 = 4 MiB）—— 两者差 5 倍，不是能碰巧通过的断言。
/// 取证另加两条：`store.len()` 与交回的字节数同源（裁定 4/5 那条纪律在帽这一支同样成立），
/// 以及块号对齐（那些字节确实来自这些偏移，帽没有把内容打乱）。
#[test]
fn a_shot_stops_at_the_end_cap_lowered_mid_body_and_keeps_what_it_wrote() -> anyhow::Result<()> {
    const CHUNK: u64 = 64 * 1024;
    const TRIGGER_BYTES: u64 = 8 * CHUNK; // 写到第 8 块之后，看盘的人把帽降下去
    const CAP_BYTES: u64 = 20 * CHUNK; // 帽：允许写到偏移 `CAP_BYTES - 1`（含端点）
    const WINDOW_CHUNKS: u64 = WINDOW_BYTES / CHUNK;

    let peer = harness::spawn_paced_partial_stub(6 * 1024 * 1024, CHUNK as usize, Duration::from_millis(15))?;
    let client = harness::client()?;
    let t = Task { url: format!("{}/blob", peer.base()), ..harness::blank_task() };
    let (store, path) = harness::store("t8a-cap-mid-body.bin")?;
    let iv = Interval { start: 0, end: WINDOW_BYTES - 1 };
    let cap = std::sync::Arc::new(dlm::fetch::EndCap::new(dlm::fetch::NO_END_CAP));

    let (watch_cap, watch_path, lowered) = (
        std::sync::Arc::clone(&cap),
        path.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let watch_flag = std::sync::Arc::clone(&lowered);
    let watcher = std::thread::spawn(move || {
        // 独立读者（不经 `Store`）：盘上长度就是这一枪写到哪儿了。
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let len = std::fs::metadata(&watch_path).map(|m| m.len()).unwrap_or(0);
            if len >= TRIGGER_BYTES {
                dlm::fetch::lower_end_cap(&watch_cap, CAP_BYTES - 1);
                watch_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &cap)?;
    watcher.join().expect("看盘线程不该崩");

    let Outcome::Advanced { bytes } = out else {
        panic!("帽掐在 body 中间 ⇒ 必须是 Advanced（带着已落盘的字节），实际 {out:?}");
    };
    assert!(lowered.load(std::sync::atomic::Ordering::SeqCst), "夹具：看盘线程没在 30 s 内等到第 8 块 ⇒ 这一枪根本没读起来");
    assert!(
        bytes >= TRIGGER_BYTES,
        "帽之前写下去的字节都是真进度（裁定 4）：{bytes}"
    );
    assert!(
        bytes <= CAP_BYTES + CHUNK,
        "帽之外最多再落**一块**（块首查帽的形状）：bytes={bytes} cap={CAP_BYTES} chunk={CHUNK}"
    );
    assert!(
        bytes < WINDOW_BYTES,
        "这一枪本该读满一个窗口（{WINDOW_CHUNKS} 块）才被帽掐住 ⇒ 帽没起作用（读循环忽略帽的变异就在这里红）：bytes={bytes}"
    );
    assert_eq!(store.len()?, bytes, "交回的字节数与盘上的量同源");
    assert!(
        std::fs::metadata(&path)?.len() <= CAP_BYTES + CHUNK,
        "文件里不该有帽 + 一块之外的字节"
    );
    harness::require_block_alignment(&path, 0, bytes)?;
    assert!(peer.written() >= bytes, "对端至少写出了我们读到的那么多：written={}", peer.written());

    drop(store);
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// **E14① 乙（单调那一半，走真代码路径）**：帽在**开枪之前**就降好了 ⇒ 那一枪自己算出的窗口终点
/// 不许把它放宽回来。
///
/// 这一条钉的是 `run_window` 进门那句 `lower_end_cap(cap, stop - 1)` 为什么必须是 `fetch_min`
/// 而不是 `store`：`store` 会把已经收紧的帽抬回窗口终点 ⇒ 这一枪读满 64 块（4 MiB）。
/// 与单元用例 `end_cap_lowering_is_monotonic_downward_only`（钉 `lower_end_cap` 那个原语本身）的分工：
/// 那里证"写更大的值不算话"，这里证"那一枪真的不会因为自己的窗口而忘掉别人的帽"。
#[test]
fn an_end_cap_already_lowered_before_the_shot_starts_survives_the_shots_own_window() -> anyhow::Result<()> {
    const CHUNK: u64 = 64 * 1024;
    let peer = harness::spawn_paced_partial_stub(6 * 1024 * 1024, CHUNK as usize, Duration::from_millis(5))?;
    let client = harness::client()?;
    let t = Task { url: format!("{}/blob", peer.base()), ..harness::blank_task() };
    let (store, path) = harness::store("t8a-cap-first.bin")?;
    let iv = Interval { start: 0, end: WINDOW_BYTES - 1 };
    // 帽先说话：只许写到第 4 块末（含端点 ⇒ `4 * CHUNK - 1`）。
    let cap = dlm::fetch::EndCap::new(4 * CHUNK - 1);

    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &cap)?;
    let Outcome::Advanced { bytes } = out else {
        panic!("区间比帽宽 ⇒ 只能 Advanced，实际 {out:?}");
    };
    assert!(
        bytes >= 4 * CHUNK,
        "收手条件是 `cursor > 帽` ⇒ 至少把第 4 块写满：bytes={bytes}"
    );
    assert!(
        bytes <= 5 * CHUNK,
        "这一枪自己的窗口终点（64 块）不许把帽放宽：bytes={bytes}（放宽的写法会读满 {}",
        WINDOW_BYTES
    );
    assert_eq!(store.len()?, bytes);
    harness::require_block_alignment(&path, 0, bytes)?;

    drop(store);
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// **E14② 戊（线上取证）**：引擎派出去的每一枪，问的字节数**不超过一个窗口**，
/// 而**只有窗口真够得着 EOF 的那一枪**才许写开放终点。
///
/// 判据全部读 node 服务器自己记下的那条 `Range`（请求行里的 `range` 字段，经
/// `Server::requests_at_least` **先守卫条数**再读 —— 见下面那段注释），不是我们拼出来的字符串。
/// 两条规则合起来是**双向**的：
/// - `bytes=s-`（开放）⇔ `s + WINDOW_BYTES >= total_len` —— 窗口够得着 EOF 才许省掉终点；
///   阶段4 的形状在这里红：它发过 `bytes=236032-`（区间终点 == EOF 但窗口只到 4430335），
///   等于授权对端流 7.75 MiB。
/// - `bytes=s-e`（显式）⇒ `e - s + 1 <= WINDOW_BYTES` **且** `e + 1 < total_len`。
///
/// 顺带钉"两种形状都真的出现过"：一条都没有的话，上面两句就退化成恒真（本项目已经踩过三次恒绿夹具）。
#[test]
fn every_engine_request_asks_for_at_most_a_window_and_only_the_true_tail_is_open_ended() -> anyhow::Result<()> {
    // `_srv` 必须活到断言跑完：`Server::Drop` 会杀进程并**删掉日志**。
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-cap1")?;
    let len = 8 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/sz/{len}"),
        dir: dir.path().to_path_buf(),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "引擎没跑完：{:?}", rep.message);

    // **先守卫条数再读**（评审 C-I-4；同一缺陷 5B 已在 Abort 用例修过一发，提交 `7b368d2`）：
    // 引擎返回的那一刻日志未必已经落盘，裸读一次会把"最后那一两条 Range"漏掉 —— 而漏掉的
    // 恰好可能是唯一违规的那一枪，判据就会**碰巧**为真。守卫数由这条用例真实期望的条数推出：
    // 线上请求 = 引擎派出的枪数 + 试探那一枪（`Report::dispatched()` 与线上是一对一，T8-R2）。
    let expected_lines = rep.dispatched().len() + 1;
    let requests = srv.requests_at_least(expected_lines)?;
    let ranges: Vec<Option<String>> = requests
        .iter()
        .map(|r| r.get("range").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let mut open_ended = 0usize;
    let mut full_window_asks = 0usize;
    for value in ranges.iter().flatten() {
        let spec = value.strip_prefix("bytes=").unwrap_or(value);
        let (raw_start, raw_end) = spec.split_once('-').unwrap_or((spec, ""));
        let start = raw_start.parse::<u64>().unwrap_or(0);
        if raw_end.trim().is_empty() {
            open_ended += 1;
            assert!(
                start.saturating_add(WINDOW_BYTES) >= len,
                "开放终点那一枪的窗口够不着 EOF（{start} + 一个窗口 < {len}）⇒ 按 E14② 必须发显式的窗口终点：{value}"
            );
        } else {
            let end = raw_end.parse::<u64>().unwrap_or(0);
            let span = end - start + 1;
            assert!(
                span <= WINDOW_BYTES,
                "这一枪问了 {span} 字节，比一个窗口（{WINDOW_BYTES}）还宽 ⇒ 对端被授权流我们注定用不上的 body：{value}"
            );
            assert!(
                end + 1 < len,
                "窗口终点落在 EOF 或越过 EOF 时该发开放终点，不该发这一条：{value}"
            );
            if span == WINDOW_BYTES {
                full_window_asks += 1;
            }
        }
    }
    assert!(open_ended > 0, "这一形状里一条开放终点都没有 ⇒ 两头都断不到，判据退化成恒真");
    assert!(full_window_asks > 0, "没有一问就是整个窗口的枪 ⇒ 窗口终点那条分支没被走过（戊的正面形状没测到）");
    Ok(())
}

/// **E14①＋③（有节拍对端上的线上账单）**：多打出去的字节按"每笔收紧几块"计价，而不是按窗口计价。
///
/// 为什么这条要挂在**节拍**对端（`/sz/<n>s`：每 64 KiB 之间睡 200 ms）上：无节拍的环回对端会在
/// 我们收手之前把整条 span 一次性塞进 socket，`conn_close_side.sent` 读起来与"我们真需要多少"无关
/// （那是本仪器上测不出一件事，不是它不存在）。有节拍时服务器写多少由我们读多少决定 ⇒ 这一格
/// 才是**账单**。取 `conn_close_side.sent`（每条响应收尾时服务器自己写的字节数）求和，
/// **不取** `response_done`：后者只在服务器写完整条 body 时才发，中途被丢的那些根本不入账。
/// 口径按**裁定 16**：全项目唯一的诚实线上口径就是 `Σ conn_close_side.sent` —— `Σ response_done.bytes`
/// **漏**半路被丢弃的响应，而两者**相加**又把"两条都记"的正常响应重复计数。
///
/// **这一格只钉"按块计价、不按窗口计价"，钉不住"每笔收紧 ≤ 一块"**（评审 C-I-1②）：
/// 那个 `CHUNKS_PER_TIGHTEN = 6` 是**这台仪器自身的节拍**决定的 —— 跨过帽的一块（`cap_allows`
/// 在块首问 ⇒ 1）+ 帽只能等主循环醒一次（`LOOP_POLL = 1 s` ÷ 每块 200 ms ⇒ 5）。
/// 换一台节拍不同的对端、或把 `LOOP_POLL` 改小，这个数就跟着变 ⇒ **这台仪器看不见 E14③ 钦定的
/// 那道"每笔 ≤ 一块"的阈值**。那一支由本文件里 `a_shot_stops_at_the_end_cap_lowered_mid_body_…`
/// 与 `an_end_cap_already_lowered_before_the_shot_starts_…` 两条**单发级**用例钉（帽由测试自己下，
/// 不经过主循环，所以那里的额度就是一块）。常数**不许**改小 —— 改小只会把这条用例变成摆设。
///
/// 判据的形状（每一项都有出处，不是把常数放宽到通过为止）：
/// - **量具活体针两条**（评审 C-I-1①：本项目第四次"账没落盘就先求和 ⇒ 判据当场恒真"）：
///   `conn_close_side` 的**条数 > 0**，且 `written >= len`（对端至少写出了整条文件那么多字节）。
///   两条都由 `events_while` **等**到位才读，不许裸读一次 `events()` 就求和。
///   （本条已有的 `tightened > 0` 钉的是**引擎自己的计数**，不算量具活体针。）
/// - `excess = 写出总量 - bytes_total`，`excess ≤ tightened × 6 × CHUNK_BYTES`。
/// - `duplicated > 0`：账**不许**为了让用例好看而折掉（E14③ 末句）。
/// - `duplicated ≤ excess + workers × CHUNK_BYTES`：引擎自己那本账不许比线上账单更凶（超出那一点
///   只可能是收尾时还在途、被 `Stop` 掐掉的那几枪写了却没被对端记完）。
///
/// **出处**（评审 C-I-2①：原先这里把 `8,206,900` 写成"实测 `excess`（线上）"，**标错了**）：
/// 那个数是实现者阶段4 一次性取证报的 **`duplicated`**（`task-8-report.md` 顾虑 1），不是线上多打。
/// 按 **E14 附记**的复量表，8 MiB × 4 worker 那一形状有**三个各不相同**的数：线上 `excess = 8,465,356`、
/// 阶段4 `duplicated = 8,334,284`、报告里那句 `8,206,900`。"阶段4 在这一格会红"用的是**前两者**：
/// `excess 8,465,356` 对 15 次收紧的额度 `15 × 6 × 64 KiB = 5,898,240`。⚠ 这两个数出自**两次运行**
/// （excess 取自 E14 附记的复量表，收紧次数 15 取自阶段4 那次取证的 trace），同一形状、时序会漂 ⇒
/// 这里只是量级对照，不是一次判决。
///
/// **本轮（修复轮 8F-1）复量的一次读数**（评审 C-I-2② 要求"要么删数、要么亲测再写"）：
/// 口径 `Σ conn_close_side.sent`，形状 8 MiB × 4 worker、`/sz/8388608s` 节拍对端 ⇒
/// `written = 11,673,632` ⇒ `excess = 3,285,024`，**同一轮** `Report.duplicated = 3,284,698`。
/// ⚠ 这是**会随时序漂的一次读数，不是判据**：本用例对 `duplicated` 只断 `> 0` 与那条一侧界，
/// 没有任何断言在这个量级上钉它（原先那句"实测 3.1-3.5 MB"当时无出处，现在这句有：
/// 8F-1 的变异 M5 那一发的红消息）。
#[test]
fn the_tightening_bill_on_a_paced_peer_is_priced_by_chunk_not_by_window() -> anyhow::Result<()> {
    const CHUNK: u64 = 64 * 1024;
    const WORKERS: usize = 4;
    // 帽只能等主循环醒一次 ⇒ 每笔收紧的额度是一块 + LOOP_POLL 那 1 s 里的五块。
    const CHUNKS_PER_TIGHTEN: u64 = 6;

    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-cap2")?;
    let len = 8 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/sz/{len}s"),
        dir: dir.path().to_path_buf(),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: WORKERS,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "引擎没跑完：{:?}", rep.message);
    assert_eq!(rep.delivered, len, "收尾时盘上真有这么多字节");

    let tightened = rep.tightened().len() as u64;
    assert!(tightened > 0, "这个形状本来就要收紧好几次；一次都没有 ⇒ 账单这条用例什么都没测（这一条钉的是**引擎自己的计数**，不是量具活体针）");

    // **量具活体针**（评审 C-I-1①，本项目第四次同一形状）：日志落盘晚于响应收尾，
    // 裸读一次 `events()` 会在"一条 `conn_close_side` 都没记上"的时候给出 written = 0 ⇒
    // `excess = 0` ⇒ 下面那条 `excess <= bill` **当场恒真**。所以先**等**到
    // "有 conn_close_side 行、且累计写出量 >= 整条文件"，再去求和。
    let evs = srv.events_while(|evs| {
        let mut lines = 0usize;
        let mut sent = 0u64;
        for e in evs {
            if e["ev"] == "conn_close_side" {
                lines += 1;
                sent += e["sent"].as_u64().unwrap_or(0);
            }
        }
        lines > 0 && sent >= len
    })?;
    let mut close_lines = 0usize;
    let mut written = 0u64;
    for e in &evs {
        if e["ev"] == "conn_close_side" {
            close_lines += 1;
            written += e["sent"].as_u64().unwrap_or(0);
        }
    }
    assert!(close_lines > 0, "量具一条 `conn_close_side` 都没记 ⇒ excess 会是 0，那条判据就恒真了");
    assert!(
        written >= len,
        "量具没活着：对端只推了 {written} 字节上网线，比整条文件（{len}）还少 ⇒ \
         excess = written - len 量的不是'多打'，而是'少记'"
    );
    let excess = written.saturating_sub(len);
    let bill = tightened * CHUNKS_PER_TIGHTEN * CHUNK;
    assert!(
        excess <= bill,
        "线上多打了 {excess} 字节，超出 {tightened} 次收紧 × {CHUNKS_PER_TIGHTEN} 块 × {CHUNK} 字节 = {bill} 的额度：\
         帽没赶上在途那一枪（阶段4 的形状：按 E14 附记表线上 excess = 8,465,356，\
         而 15 次收紧的额度是 5,898,240 —— 那两个数出自两次运行，见本条用例的文档）"
    );
    assert!(rep.duplicated > 0, "E14③：这笔账不许折成 0（不许为了让用例好看把它抹掉）");
    assert!(
        rep.duplicated <= excess + (WORKERS as u64) * CHUNK,
        "引擎报的重复账 {} 比线上真实多打的 {excess} 还多出一大块以上 ⇒ 那本账又在替另一件事说话",
        rep.duplicated
    );
    Ok(())
}

// ================================================================ Task 8 阶段5B：§7.2 的三道链接 + 简报 Step 1 剩下的两条 + E13④ 的真退避
//
// 这一节的九条用例按派发的 P1–P5 排。两条纪律贯穿全部：
// - **取证一律先问线上或对端，再问引擎的嘴**：`Report` 的计数只能当旁证，判据落在
//   `Server::requests()/ranges()`、`ScriptedPeer::shot_ranges()`、盘上文件与断点文件这些
//   **不是被测代码自己写的**东西上（spec §7.2、E13③）。
// - **三道链接各断各的**，一条红就说出是哪一环断了：回放↔派单（引擎有没有照调度器办事）、
//   派单↔线上（引擎说派了，线上真是那个 Range 吗）、线上↔字节（下下来的东西对不对）。
// 简报 Step 1 那两条 `unimplemented!`（`resume_after_kill_…` 与 `no_ranges_…`）在这一节里
// **照名字**落地成真实断言；简报给的那条 `harness::replay(observed_ranges …)` 造"预测序列"的写法
// 按 E13③ 作废（spec §7.2 明令"不能直接拿预测序列对比"），取而代之的是 `Report::replay` 那条
// "把本次真实事件流喂给一台全新调度器"的路子。

/// 把 `Report::replay` 交回的动作折成 `(Assign 的区间, Retighten 的区间)` 两列，丢掉 `Idle`。
/// 与 `src/engine.rs` 用例里那个同名私有帮手是同一读法；测试侧要两份是因为它在另一个 crate。
fn replay_pairs(actions: &[dlm::scheduler::Action]) -> (Vec<Interval>, Vec<Interval>) {
    (
        actions
            .iter()
            .filter_map(|a| match a {
                dlm::scheduler::Action::Assign { iv, .. } => Some(*iv),
                _ => None,
            })
            .collect(),
        actions
            .iter()
            .filter_map(|a| match a {
                dlm::scheduler::Action::Retighten { iv, .. } => Some(*iv),
                _ => None,
            })
            .collect(),
    )
}

/// 一份"应当等于盘上内容"的期望字节的散列：把期望内容落成文件、用 harness 那个独立读者算。
/// 存在的理由：`harness::expected_sha` 只服务 node 服务器那份 blob；进程内夹具的 body 由
/// `positioned_body` 现造，散列必须由**第三方读者**（`sha2` + 文件）算出来，不能由被测代码代劳。
/// 交回散列；临时文件由调用方删（`harness::scratch_body` 的纪律）。
fn sha_of_expected(name: &str, bytes: &[u8]) -> anyhow::Result<(String, std::path::PathBuf)> {
    let path = harness::scratch_body(name, bytes)?;
    Ok((harness::sha256_of(&path)?, path))
}

/// **P1 / E13③ / spec §7.2：三道链接一次走通。**
///
/// 简报 Step 1 那条用例写的是 `harness::replay(observed_ranges …)` 造一个 `predicted` 再比 ——
/// §7.2 明令**不许**这么干（"劈分时机取决于真实时序"，预测序列根本无从预测）。这里按 §7.2 的原话做：
/// **把这次运行里真实发生的事件序列回放给一台全新调度器**，然后分三道断言，每道单独一条语句，
/// 红在哪一条就说明是哪一环断了：
/// 1. **回放 ↔ 派单**：回放给出的 `Assign` 区间序列必须与引擎真打出去的那批（`dispatched()`）
///    **逐条同序**相等，`Retighten` 必须与 `tightened()` 逐条相等。这一道抓"引擎没照调度器办事"
///    （或者 trace 记错了对象）。
/// 2. **派单 ↔ 线上**：`dispatched()` 折成闭区间之后必须与 node 服务器自己记下的 `Range` 一一对上 ——
///    **剔掉试探那一枪**，并且**用"剔掉的确实是它"来钉**（断言那条 `bytes=0-236031` 在全程只出现一次、
///    且就是线上第一条），不是"随便少一条也算对"。跨连接的两枪谁先到线上没有保证（四条连接各自
///    `connect`），所以顺序判据取它**因果上必然成立**的那一半：**同一条连接上的请求**必须与
///    `dispatched()` 的相对顺序一致（一条连接同时只被一个 worker 占着 ⇒ 它上面的先后是真的先后）。
///    这一道抓"引擎说派了，可线上不是那个 Range"。
/// 3. **线上 ↔ 字节**：落盘文件与 `harness::expected_sha` 一字不差（那个 oracle 已被
///    `the_sha_oracle_matches_what_the_server_serves` 对真 socket 证过，这里直接复用，不重新推）。
///
/// 为什么用 node 对端：**E13⑤** —— 多 worker 的引擎形状不许用进程内窄桩（`MAX_CONNECTIONS = 8`）。
#[test]
fn the_replayed_trace_matches_the_dispatched_shots_and_the_wire() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-5b-replay")?;
    let len = 8 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/sz/{len}"),
        dir: dir.path().to_path_buf(),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "引擎没跑完，三道链接都无从谈起：{:?}", rep.message);

    // —— 链接一：把录下来的事件流喂给一台全新调度器 ——
    let min_seg = dlm::consts::MIN_SEGMENT_BYTES;
    let (replayed_assigns, replayed_tightens) = replay_pairs(&rep.replay(len, min_seg));
    assert_eq!(
        replayed_assigns,
        rep.dispatched(),
        "链接一：回放给出的 Assign 与引擎真打出去的一枪必须逐条同序相等"
    );
    assert_eq!(
        replayed_tightens,
        rep.tightened(),
        "链接一（收紧那一半）：回放的 Retighten 必须与记下的收紧逐条同序相等"
    );
    // 两头都得真的有东西可比，否则上面两条退化成"空表 == 空表"（本项目踩过三次恒绿夹具）。
    assert!(rep.dispatched().len() > 1, "这个形状该打出很多枪，只记到 {}: {:?}", rep.dispatched().len(), rep.dispatched());
    assert!(!rep.tightened().is_empty(), "这个形状本来要收紧好几次；一次都没记 ⇒ 链接一的第二半没被走过");

    // —— 链接二：引擎派的每一枪，线上真的收到那条 Range 吗 ——
    // **先守卫条数再读**（评审 C-I-4）：这条用例期望的线上条数**可知** ——
    // 引擎派出的枪数 + 试探那一枪（T8-R2 钉死的一对一）。裸读 `requests()` 在日志落盘晚时
    // 会少几条，而"少一条"正好会让下面那条一一对应的判据往错误方向通过或往错误方向红。
    let probe_range = format!("bytes=0-{}", PROBE_READ_BYTES - 1);
    let expected_lines = rep.dispatched().len() + 1;
    let requests = srv.requests_at_least(expected_lines)?;
    let ranges: Vec<Option<String>> =
        requests.iter().map(|r| r.get("range").and_then(|v| v.as_str()).map(str::to_string)).collect();
    let probe_lines: Vec<usize> =
        ranges.iter().enumerate().filter(|(_, r)| r.as_deref() == Some(probe_range.as_str())).map(|(i, _)| i).collect();
    assert_eq!(
        probe_lines,
        vec![0],
        "要从线上剔掉的必须是**试探自己**那一枪（{probe_range}），而且它必须是第一条请求；\
         实际出现位置 {probe_lines:?}，线上 {ranges:?}"
    );
    // 剔掉第 0 条（上面那句已经钉死它就是试探），剩下的必须与派单**一一对应**。
    //
    // 对应关系是"派出去的区间 ∩ 一个窗口"，**不是**区间本身：`fetch::range_header` 发的是这一枪的
    // **窗口** `[iv.start, min(iv.end, iv.start + WINDOW_BYTES - 1)]`（E12① + **E14**②，只有窗口够到
    // EOF 才写成开放终点）。所以第一枪派的 `[236032, 8388607]`（8 MiB 的整条尾巴）在线上就是
    // `bytes=236032-4430335`。把两边写在同一个形状上比，才是 §7.2 那道"线上真是这个 Range 吗"；
    // 拿区间直接比会红得像是引擎没办事，而缺的只是窗口那一层换算。
    let shot_reqs = &requests[1..];
    let shot_ranges = &ranges[1..];
    let wire = harness::wire_spans(shot_ranges, len)?;
    let dispatched: Vec<(u64, u64)> = rep
        .dispatched()
        .iter()
        .map(|iv| (iv.start, iv.end.min(iv.start.saturating_add(WINDOW_BYTES - 1))))
        .collect();
    assert_eq!(
        wire.len(),
        dispatched.len(),
        "线上 {n} 条 Range 对引擎 {m} 枪：一对一关系断了（T8-R2 说 18 条 = 试探 + 17 枪）",
        n = wire.len(),
        m = dispatched.len()
    );
    let mut wire_sorted = wire.clone();
    let mut dispatched_sorted = dispatched.clone();
    wire_sorted.sort();
    dispatched_sorted.sort();
    assert_eq!(
        wire_sorted,
        dispatched_sorted,
        "链接二：引擎派出去的区间集合与线上真收到的 Range 集合不是同一批（不是顺序问题，是**有没有这一条**的问题）"
    );
    // 顺序：同一连接上的请求必须按派单顺序到达（跨连接不保证，见用例文档）。
    let mut conns: Vec<u64> = vec![];
    let mut per_conn: Vec<Vec<(u64, u64)>> = vec![];
    for (rec, span) in shot_reqs.iter().zip(wire.iter()) {
        let conn = rec["conn"].as_u64().context("服务端记的 request 没有 conn 字段")?;
        match conns.iter().position(|c| *c == conn) {
            Some(k) => per_conn[k].push(*span),
            None => {
                conns.push(conn);
                per_conn.push(vec![*span]);
            }
        }
    }
    for (conn, spans) in conns.iter().zip(&per_conn) {
        let mut cursor = 0usize;
        for span in spans {
            let found = dispatched.iter().skip(cursor).position(|d| d == span).with_context(|| {
                format!(
                    "链接二（顺序那一半）：连接 {conn} 上的 Range {span:?} 在它前面那些之后找不到 ⇒ \
                     这条连接上先到的一枪在引擎的派单里反而是后派的"
                )
            })?;
            cursor += found + 1;
        }
    }

    // —— 链接三：下下来的字节是那份文件吗 ——
    assert_eq!(harness::sha256_of(&t.target_path())?, harness::expected_sha(len), "链接三：落盘字节与服务器那份不是一字不差");
    assert_eq!(rep.delivered, len, "收尾时盘上真有这么多字节");
    Ok(())
}

/// **P2 / 简报 Step 1 的第 2 条（原本还是 `unimplemented!`）：真停一半、真续上、已完成的字节一个都不重下。**
///
/// "kill" 怎么落地（**仪器缺口的如实登记**，见 `tests/harness/mod.rs` 里 `ScriptedPeer` 的文档）：
/// node 那台对端里唯一能把一枪掐在半路的是 `/drop/<n>`（E5），可它做不到"停一半之后同一 URL 还能跑完"：
/// `n = 0` 连**试探那一枪**一起掐死 ⇒ `engine::run` 在 `probe` 就 `Err` ⇒ **压根没有断点文件**；
/// `n > 0` 每一枪都交出 n 个真字节 ⇒ `Advanced` ⇒ 引擎一路有进度、必然跑到完成。换台服务器也不行
/// （`resume` 的 ② 拿 `final_url` 比对，端口一变就是另一个资源 ⇒ 判 `Fresh` ⇒ 续传这条路根本不出现）。
/// 所以这里用**按脚本回答的进程内对端**（`workers = 1` ⇒ 顺序模型与真实行为一致）：
/// 第 0 枪诚实（试探）、第 1 枪写出 100 000 个真字节之后被拆线（`Advanced` ⇒ 那批字节算数）、
/// 第 2 枪直接回 503（零字节 ⇒ `Failed`）配上 `--max-retries 0` ⇒ **引擎真的停在半路**，
/// 交回 `ok = false`，**留下半截文件与一份引擎自己写的断点文件**。然后同一 URL 再跑一遍（脚本用完，
/// 之后每枪诚实），判据全在**第二轮的线上**：
/// - 第一轮那一条 `bytes=0-236031` 仍然只出现一次、就是第二轮的第一枪（试探），比对前先把它剔掉；
/// - 第二轮每一枪的起点都 **≥ 断点文件说的覆盖上界**，且**没有任何一枪碰到 `snapshot_intervals()`
///   里已经归我的字节**（判据取自 `state::load` 读回来的段表，**不是**引擎的计数器）；
/// - 最终文件与期望内容一字不差（引擎那道 sha 闸 + harness 那个独立读者各算一遍）。
#[test]
fn resume_after_kill_does_not_redownload_done_segments() -> anyhow::Result<()> {
    const TOTAL: u64 = 512 * 1024;
    const DELIVERED: u64 = 100_000;
    // 试探落盘的字节 + 那一枪真写下去的字节 = 第一轮结束时盘上应有的量。
    let watermark = PROBE_READ_BYTES + DELIVERED;

    let peer = harness::spawn_scripted_peer(
        TOTAL,
        vec![
            harness::Shot::Honest,
            harness::Shot::Truncated(DELIVERED),
            harness::Shot::Status(503),
        ],
        harness::Shot::Honest,
    )?;
    let dir = harness::tmp_dir("eng-5b-kill")?;
    let t = Task {
        url: format!("{}/blob", peer.base()),
        dir: dir.path().to_path_buf(),
        name: Some("resumed.bin".to_string()),
        ..harness::blank_task()
    };
    let state_path = dlm::state::path_for(&t.target_path());
    let (want, want_file) = sha_of_expected("5b-kill-expected.bin", &harness::positioned_body(TOTAL as usize))?;

    // —— 第一轮：真的停在半路 ——
    let cfg = dlm::engine::Config {
        workers: 1,
        min_seg: dlm::consts::MIN_SEGMENT_BYTES,
        max_retries: Some(0),
        verify_sha256: None,
        force_fresh: false,
    };
    let first = dlm::engine::run(&t, cfg.clone())?;
    assert!(!first.ok, "这一轮的脚本就是要它停在半路，`ok` 却给了真：{:?}", first.message);
    assert_eq!(first.delivered, watermark, "第一轮结束时盘上就该有这么多字节：一半都没下完");
    assert!(state_path.exists(), "停在半路必须留下断点文件，否则下一轮无从续（spec §4）");
    let snap = dlm::state::load(&state_path)?.expect("上面已断言文件在，读回来却什么都没有");
    let covered: Vec<(u64, u64)> = snap.segments.iter().map(|s| (s.start, s.end)).collect();
    assert_eq!(covered, vec![(0, watermark - 1)], "断点文件说的覆盖集必须是「已确认的那些字节」：{snap:?}");

    // —— 第二轮：同一 URL、同一台对端，脚本用完之后每枪诚实 ——
    let cfg_ok = dlm::engine::Config {
        workers: 1,
        min_seg: dlm::consts::MIN_SEGMENT_BYTES,
        max_retries: None,
        verify_sha256: Some(want.clone()),
        force_fresh: false,
    };
    let second = dlm::engine::run(&t, cfg_ok)?;
    assert!(second.ok, "第二轮该把剩下的下完：{:?}", second.message);
    assert!(second.fresh_reason.is_none(), "第二轮走的是 `Resume`，没有「从头重来」要解释：{:?}", second.fresh_reason);

    // 线上取证：前 3 枪属于第一轮，第二轮从下标 3 开始。
    let ranges = peer.shot_ranges();
    assert_eq!(ranges.len(), 5, "第一轮 3 枪 + 第二轮 2 枪 = 5，实际 {ranges:?}");
    let (run1, run2) = ranges.split_at(3);
    assert_eq!(
        run1[0],
        format!("bytes=0-{}", PROBE_READ_BYTES - 1),
        "夹具前提：第一轮第一枪是试探（剔掉的就是它）"
    );
    assert_eq!(
        run2[0],
        format!("bytes=0-{}", PROBE_READ_BYTES - 1),
        "第二轮第一枪还是试探，比对时剔掉的是它、不是随便一条"
    );
    let resumed_wire: Vec<Option<String>> = run2[1..].iter().cloned().map(Some).collect();
    let resumed = harness::wire_spans(&resumed_wire, TOTAL)?;
    assert!(!resumed.is_empty(), "第二轮一枪都没打 ⇒ 下面两条判据全是空的（恒绿）");
    for (index, (start, end)) in resumed.iter().enumerate() {
        assert!(
            *start >= watermark,
            "第二轮第 {index} 枪从 {start} 开始，而断点文件说 {} 之前都归我了（已确认的字节被重下）",
            watermark - 1
        );
        for (done_start, done_end) in &covered {
            assert!(
                end < done_start || start > done_end,
                "第二轮第 {index} 枪要的 [{start}, {end}] 与断点里已确认的 [{done_start}, {done_end}] 重叠 ⇒ \
                 简报那句「不重下已完成段」断了"
            );
        }
    }
    assert_eq!(
        resumed[0].0,
        watermark,
        "第二轮第一枪的起点必须**正好**是已确认覆盖的下一个字节（简报 Step 1 第 2 条的原话）"
    );

    assert_eq!(harness::sha256_of(&t.target_path())?, want, "续完之后文件必须与整份内容一字不差");
    assert!(!state_path.exists(), "跑完之后断点文件该删（spec §4，本轮没给 --keep-state）");
    let _ = std::fs::remove_file(&want_file);
    Ok(())
}

/// **P3 / 简报 Step 1 的第 3 条（原本还是 `unimplemented!`）：对端不认 Range ⇒ 一次只一段、按起点升序、字节还对。**
///
/// 两个形状（为什么要有第二个：node 的 `/noranges` 把 body 夹在 `min(sendLen, 1 MiB)`，而 1 MiB
/// **装得进一个窗口**（`WINDOW_BYTES` = 4 MiB）⇒ 那一支永远只需要一枪，"多枪时按起点升序、彼此不重叠"
/// 这条判据在它上面是**空的**；同一类"仪器没有这个对端"的事 E5 已经记过一次。所以：
/// - **A 段（node `/noranges`）**：钉"结果字节正确"（`expected_sha`，与真 socket 对过的 oracle）
///   + 钉"四个 worker 都报了就绪，线上却只多出一条 Range"。
/// - **B 段（进程内退化对端，5 MiB > 一个窗口）**：钉"多枪时 Ranges 严格按文件顺序、首尾相接、
///   互不重叠"，也就是派发点名的**从线上读并发=1**（不是引擎内部的计数器）。
///
/// 调度器这一侧的规则在 `src/scheduler.rs` 的 `dispatch` 退化分支：`NoRanges` 之后**只要还有人在途就
/// 一律 Idle**、且永不产生 `Retighten`（不劈分）。`tightened()` 为空 + 同轮只有一枪在途就是那条规则的
/// 线上影子；`fed()` 里 `Connected` 的条数只用来证明"别的 worker 确实醒着、确实问过活"。
#[test]
fn no_ranges_falls_back_to_single_worker_and_is_correct() -> anyhow::Result<()> {
    // —— A 段：node 的 /noranges（1 MiB，装得进一个窗口 ⇒ 只需一枪）——
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-5b-noranges")?;
    let len = 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/noranges"),
        dir: dir.path().to_path_buf(),
        name: Some("degraded-node.bin".to_string()),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "退化形状也该跑得完：{:?}", rep.message);
    assert_eq!(harness::sha256_of(&t.target_path())?, harness::expected_sha(len), "对端不认 Range 也得一字不差");

    // **先守卫条数再读**（评审 C-I-4 的同一形状，本文件里另外两处同类裸读一并修）：
    // 期望条数由这条用例自己的形状给出 = 试探 + 恰好一枪。裸读一次日志会把还没落盘的尾巴漏掉，
    // 而"漏掉后面的行"正好会让下面那条 `== 2` 在真分段了的情况下碰巧为真。
    let requests = srv.requests_at_least(2)?;
    assert_eq!(requests.len(), 2, "试探 + 恰好一枪 = 2 条请求，实际 {requests:?}：分段了就不止 2 条");
    assert_eq!(rep.dispatched().len(), 1, "退化模式一次只许有一段在途：{:?}", rep.dispatched());
    assert!(rep.tightened().is_empty(), "退化模式永不劈分 ⇒ 不该有任何收紧：{:?}", rep.tightened());
    let asks = rep.fed().iter().filter(|e| matches!(e, dlm::scheduler::Event::Connected(_))).count();
    assert!(asks >= 4, "四个 worker 都该报过就绪（否则「并发=1」是因为只有一个 worker，不是退化规则）：{asks}");
    let asked = requests[1].get("range").and_then(|v| v.as_str()).map(str::to_string);
    let first_shot = harness::wire_spans(&[asked], len)?;
    assert_eq!(
        first_shot[0],
        (PROBE_READ_BYTES, len - 1),
        "退化模式派的第一枪必须是**起点最低**的那段缝（试探已写的字节之后），不是「最长优先」"
    );

    // —— B 段：5 MiB 的退化对端（一枪装不下 ⇒ 必然两枪，这才比得出顺序）——
    let big = 5 * 1024 * 1024u64;
    let peer = harness::spawn_scripted_peer(big, vec![], harness::Shot::IgnoresRange)?;
    let dir2 = harness::tmp_dir("eng-5b-noranges-big")?;
    let (want, want_file) = sha_of_expected("5b-degraded-expected.bin", &harness::positioned_body(big as usize))?;
    let t2 = Task {
        url: format!("{}/blob", peer.base()),
        dir: dir2.path().to_path_buf(),
        name: Some("degraded-scripted.bin".to_string()),
        ..harness::blank_task()
    };
    let rep2 = dlm::engine::run(
        &t2,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(want.clone()),
            force_fresh: false,
        },
    )?;
    assert!(rep2.ok, "退化形状（进程内对端）也该跑得完：{:?}", rep2.message);
    let spans = harness::wire_spans(
        &peer.shot_ranges().into_iter().map(Some).collect::<Vec<_>>(),
        big,
    )?;
    assert_eq!(spans.len(), 3, "试探 + 两枪 = 3 条，实际 {spans:?}");
    assert_eq!(spans[0], (0, PROBE_READ_BYTES - 1), "第一枪是试探");
    // 线上并发=1 的影子：这三条 Range 严格按文件顺序、首尾相接、彼此不重叠。
    for pair in spans.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (prev_end, next_start) = (a.1, b.0);
        assert_eq!(
            b.0,
            a.1 + 1,
            "退化模式下线上收到的 Range 必须首尾相接（前一条到 {prev_end}，后一条从 {next_start} 起）：\
             {spans:?} —— 跳号 = 中间那段没人认领，重叠 = 已确认的字节被重下"
        );
        assert!(b.1 > a.1, "后一条必须往后走，不许回头：{spans:?}");
    }
    assert_eq!(spans[2].1, big - 1, "最后一枪该正好够到 EOF：{spans:?}");
    assert!(rep2.tightened().is_empty(), "退化模式不劈分：{:?}", rep2.tightened());
    assert_eq!(harness::sha256_of(&t2.target_path())?, want, "两枪拼起来的文件也要一字不差");
    let _ = std::fs::remove_file(&want_file);
    Ok(())
}

/// **P4 / E13④ 点名而阶段5A 跳过的那一条：退避真的接进了循环（真等第一个 1 s）。**
///
/// 表驱动的 `next_retry_delay` 只证明"这张表算得对"；本用例证明"表上那一格**真的被等了**"。
/// 形状：第 1 枪（试探之后的第一枪）直接回 503 ⇒ `Failed(Http)` 且**当场就回来**（不像静默对端
/// 那样要把窗口预算的下界 15 s 耗完），于是重试必须等到 `1 s` 之后才出现。取证在**对端那一侧**：
/// `ScriptedPeer` 给每一枪盖一个到达时刻，断言第 1 枪与第 2 枪之间的间隔落在
/// `[950 ms, 1 900 ms)` —— 下界挡"根本没等"（变异：退避直接给 `Idle`），上界挡"等错了那一格"
/// （变异：把 `1 << attempt` 写成 `1 << (attempt + 1)` ⇒ 2 s）。
/// **不造假时钟、不开 `Sleep` 接缝**（E13④ 的禁令），一秒就是实测的一秒。
/// 另外钉两条：断点文件里 `stats.retries == 1`（跨轮的账，不是引擎本轮的计数器），以及
/// 等完之后这一轮**照样跑完**、字节一字不差（退避不是"卡死"也不是"放弃"）。
#[test]
fn the_first_retry_actually_waits_the_whole_one_second_step() -> anyhow::Result<()> {
    const TOTAL: u64 = 512 * 1024;
    let peer = harness::spawn_scripted_peer(
        TOTAL,
        vec![harness::Shot::Honest, harness::Shot::Status(503)],
        harness::Shot::Honest,
    )?;
    let dir = harness::tmp_dir("eng-5b-backoff")?;
    let (want, want_file) = sha_of_expected("5b-backoff-expected.bin", &harness::positioned_body(TOTAL as usize))?;
    let t = Task {
        url: format!("{}/blob", peer.base()),
        dir: dir.path().to_path_buf(),
        name: Some("backoff.bin".to_string()),
        // `--keep-state`：跑完了也留着断点文件，好让"重试计数落进盘上那本账"这条判据有对象可读。
        flags: dlm::consts::FLAG_KEEP_STATE,
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 1,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(want.clone()),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "退避之后该把这一轮跑完：{:?}", rep.message);
    assert_eq!(rep.retries, 1, "只许失败一次 ⇒ 一次重试：{rep:?}");
    assert_eq!(peer.shot_count(), 3, "试探 + 失败那一枪 + 重试那一枪 = 3，实际 {:?}", peer.shot_ranges());
    assert_eq!(
        peer.shot_ranges()[1], peer.shot_ranges()[2],
        "重试打的就是同一区间（等的是时间，不是换个地方下）"
    );
    let gap = peer.arrival_gap(1, 2)?;
    assert!(
        gap >= Duration::from_millis(950),
        "第一次重试只等了 {gap:?} —— 退避表算得对但没接进循环（spec §6.1 的第一档是 1 秒）"
    );
    assert!(
        gap < Duration::from_millis(1_900),
        "第一次重试等了 {gap:?} —— 那不是 1 s 那一档，`next_retry_delay` 的 `1 << attempt` 走错了格子"
    );

    let state_path = dlm::state::path_for(&t.target_path());
    let snap = dlm::state::load(&state_path)?.context("给了 --keep-state，断点文件却不在")?;
    assert_eq!(snap.stats.retries, 1, "重试计数必须落到盘上那本账（跨轮的账不归本轮计数器）：{snap:?}");
    assert_eq!(harness::sha256_of(&t.target_path())?, want, "等过一秒之后下来的字节照样一字不差");
    let _ = std::fs::remove_file(&want_file);
    Ok(())
}

/// **P5 之一：一枪写了字节之后失败/被掐，那批已写前缀算数，且不重下。**
///
/// `fetch` 的契约（E12③）先要说清：**`Failed` 永远不带字节**（状态码/偏移核对都排在读 body 之前），
/// 交出过字节的那一支叫 `Advanced`。所以"失败段的已写前缀"在本模型里只有一个可达形状：
/// 同一个区间**早先那一窗**写下去并已确认的字节，碰上后面一枪失败。这里就是那个形状：
/// 第 1 枪写出 100 000 个真字节后被拆线（`Advanced`），第 2 枪（尾巴）直接回 503（`Failed`），
/// `--max-retries 0` 让引擎当场停下。三条判据各自挡一种写法：
/// - 断点文件的覆盖集**含**那 100 000 字节（`release()` 把已写前缀并进 done）⇒ 挡住"失败就把整段
///   交回队列"那种写法（那会连已写的字节一起丢掉，下一轮从区间起点重来）；
/// - 线上第 2 枪的 Range **正好**从 `前缀末尾 + 1` 起 ⇒ 挡住"重下时从区间起点重发"（简报担心的正是这个）；
/// - 半截文件仍在盘上、那批字节的偏移没错（块号取证）。
#[test]
fn a_failed_segments_written_prefix_survives_and_is_not_downloaded_twice() -> anyhow::Result<()> {
    const TOTAL: u64 = 512 * 1024;
    const DELIVERED: u64 = 100_000;
    let watermark = PROBE_READ_BYTES + DELIVERED;
    let peer = harness::spawn_scripted_peer(
        TOTAL,
        vec![
            harness::Shot::Honest,
            harness::Shot::Truncated(DELIVERED),
            harness::Shot::Status(503),
        ],
        harness::Shot::Honest,
    )?;
    let dir = harness::tmp_dir("eng-5b-prefix")?;
    let t = Task {
        url: format!("{}/blob", peer.base()),
        dir: dir.path().to_path_buf(),
        name: Some("prefix.bin".to_string()),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 1,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: Some(0),
            verify_sha256: None,
            force_fresh: false,
        },
    )?;
    assert!(!rep.ok, "封顶的重试该让这一轮停住：{:?}", rep.message);

    let snap = dlm::state::load(&dlm::state::path_for(&t.target_path()))?
        .context("失败那一支必须留着断点文件（下一轮才续得上）")?;
    assert_eq!(
        snap.segments.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(),
        vec![(0, watermark - 1)],
        "已写前缀必须留在覆盖集里；失败就把整段交回队列的写法在这里红：{snap:?}"
    );
    let ranges = peer.shot_ranges();
    assert_eq!(ranges.len(), 3, "试探 + 两枪：{ranges:?}");
    assert_eq!(
        ranges[2],
        format!("bytes={watermark}-"),
        "失败之后重派的那一枪必须从**已写前缀之后**开始，不许从区间起点 {PROBE_READ_BYTES} 重来"
    );
    assert_eq!(dlm::store::Store::open(&t.target_path(), false)?.len()?, watermark, "已写的字节不许被失败抹掉");
    // 块号取证取一个跨过边界的对齐窗口：证明那 100 000 字节确实落在自己的偏移上，不是抄来的形状。
    harness::require_block_alignment(&t.target_path(), 4096 * 57, 4096)?;
    Ok(())
}

/// **P5 之二：`--max-retries` 到顶 ⇒ `ok = false` + 一句人话 + 断点文件不许被删。**
///
/// 判据分三块，缺一块都是"看着像失败处理"：
/// - `ok == false`（"跑到一个结论但没跑全"，`Err` 是"根本跑不起来"，两条不许混）；
/// - `message` 非空且带上限那一枪的原文（E12⑦：`FailKind` 的原因必须活着走到用户眼前，
///   折成裸枚举就只剩"网络错了"这种没法行动的话）；
/// - **盘上什么都没动**：目标文件还在（`spec §6.7` 那一支的纪律同样适用于失败收尾）、
///   断点文件还在、`state::load` 读得回来（"失败就删断点"等于把用户已下的字节整份扔掉）。
#[test]
fn exhausted_retries_report_failure_with_a_message_and_keep_the_state_file() -> anyhow::Result<()> {
    const TOTAL: u64 = 512 * 1024;
    let peer = harness::spawn_scripted_peer(
        TOTAL,
        vec![harness::Shot::Honest, harness::Shot::Status(503)],
        harness::Shot::Honest,
    )?;
    let dir = harness::tmp_dir("eng-5b-cap")?;
    let t = Task {
        url: format!("{}/blob", peer.base()),
        dir: dir.path().to_path_buf(),
        name: Some("capped.bin".to_string()),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 1,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: Some(0),
            verify_sha256: None,
            force_fresh: false,
        },
    )?;
    assert!(!rep.ok, "--max-retries=0 而第一枪就失败，居然报成功：{rep:?}");
    let note = rep.message.clone().unwrap_or_default();
    assert!(!note.trim().is_empty(), "ok=false 却不带一句人话（Task 9 只能打出个空行）：{rep:?}");
    assert!(note.contains("重试"), "消息没说是重试到顶：{note:?}");
    assert!(note.contains("503"), "消息丢了那一枪的原文（E12⑦ 要求原因走到用户眼前）：{note:?}");
    assert_eq!(rep.retries, 0, "封顶 0 次 ⇒ 一次都不该重试：{rep:?}");

    let state_path = dlm::state::path_for(&t.target_path());
    assert!(state_path.exists(), "失败之后断点文件被删了（下一轮就只能从 0 重下）");
    let snap = dlm::state::load(&state_path)?.context("断点文件在却读不回来")?;
    assert_eq!(
        snap.segments.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(),
        vec![(0, PROBE_READ_BYTES - 1)],
        "断点里的覆盖集就是试探那份字节：{snap:?}"
    );
    assert_eq!(
        std::fs::metadata(t.target_path())?.len(),
        PROBE_READ_BYTES,
        "目标文件不许在失败收尾时被删短或删掉"
    );
    Ok(())
}

/// **P5 之三：断点文件"损坏"与"不存在"是两件事，损坏 ⇒ `Err` 且**一个字节都不许多动**。**
///
/// 这是 `state::load` 的契约（决策 4），但它的**后果**住在引擎里：把损坏读成"没有断点"会把
/// 已经验证过的字节重新算成待下载，还会骗过 `--no-resume` 的判据（`src/state.rs` 模块文档第 4 条）。
/// 所以这里穿过 `engine::run` 来验，而不是只在 `state` 那一层验一次。三条取证：
/// - 损坏 ⇒ `Err`，消息点名"断点文件"；而且**拒在上网之前**（`requests_within` 守着：一条请求都没发）
///   —— 用 `requests().is_empty()` 那种读法本项目已经踩过三次（日志行落在客户端返回之后）；
/// - 那份坏文件**原样留在盘上**（引擎不许"顺手"替用户删掉它看不出内容的东西，spec §6.7 同一条纪律）；
/// - 目标文件**根本没被创建**（`Store::open` 排在这个判定之后）；
/// - 对照支：文件不存在 ⇒ `Ok`，照跑照对（否则"损坏"这一支可以靠"永远报错"蒙过去）。
#[test]
fn a_corrupt_state_file_is_an_error_while_an_absent_one_is_not() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-5b-corrupt")?;
    let len = 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/sz/{len}"),
        dir: dir.path().to_path_buf(),
        name: Some("corrupt.bin".to_string()),
        ..harness::blank_task()
    };
    let state_path = dlm::state::path_for(&t.target_path());
    let target = t.target_path();
    let garbage = "{ \"v\": 1, 这不是合法 JSON".as_bytes().to_vec();
    std::fs::write(&state_path, &garbage).context("写坏断点文件失败")?;

    let cfg = dlm::engine::Config {
        workers: 4,
        min_seg: dlm::consts::MIN_SEGMENT_BYTES,
        max_retries: None,
        verify_sha256: None,
        force_fresh: false,
    };
    let Err(e) = dlm::engine::run(&t, cfg.clone()) else {
        panic!("断点文件是坏的，`engine::run` 却跑成功了 —— 把损坏读成\"没有断点\"就是这句话的后果");
    };
    let text = format!("{e:#}");
    assert!(text.contains("断点文件"), "错误没点名是断点文件的问题：{text:?}");
    assert_eq!(std::fs::read(&state_path)?.as_slice(), &garbage[..], "引擎把看不懂的那份改写了（用户就再也看不出自己坏了什么）");
    assert!(!target.exists(), "拒绝开工就不该碰目标文件");
    assert!(
        srv.requests_within(Duration::from_millis(250))?.is_empty(),
        "断点判错之前就已经上网了：试探那一枪白打一次"
    );

    // 对照支：同一个任务，断点文件不存在 ⇒ 这不是错误，照跑照对。
    std::fs::remove_file(&state_path).context("删掉坏断点失败")?;
    let rep = dlm::engine::run(&t, cfg)?;
    assert!(rep.ok, "没有断点文件时该正常跑完：{:?}", rep.message);
    assert_eq!(harness::sha256_of(&target)?, harness::expected_sha(len));
    Ok(())
}

/// **P5 之四：`Decision::Fresh{reason}` 那句原因必须活着走到 `Report.fresh_reason`。**
///
/// `src/resume.rs` 对 Task 9 的承诺是"中途重来时把这句直接打给用户"；承诺要能被证伪，就得有一条
/// 用例从 `engine::run` 的返回值里把它读出来（E13② 把 `resume` 从 `Config` 里删掉、改由引擎自己走
/// `load → probe → decide` 之后，这条链路的**唯一出口**就是 `Report`）。
/// 形状：第一轮带 `--keep-state` 跑成功（留下一份与本轮任务完全一致的断点文件），第二轮**只换一个
/// 请求字段**（Referer）⇒ `resume` 的 ①b 判"请求本身变了" ⇒ `Fresh{原因}`。
/// 三条断言：第一轮 `fresh_reason` 是 `None`（本来就没有需要解释的事）、第二轮是 `Some` 且点名
/// Referer（不许糊成"要重下"）、第二轮仍然把文件下对（重来这一支该照样成功）。
#[test]
fn the_fresh_reason_reaches_the_report_for_the_cli_to_print() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-5b-reason")?;
    let len = 1024 * 1024u64;
    let url = format!("{base}/sz/{len}");
    let t = Task {
        url: url.clone(),
        dir: dir.path().to_path_buf(),
        name: Some("reason.bin".to_string()),
        flags: dlm::consts::FLAG_KEEP_STATE,
        ..harness::blank_task()
    };
    let cfg = dlm::engine::Config {
        workers: 4,
        min_seg: dlm::consts::MIN_SEGMENT_BYTES,
        max_retries: None,
        verify_sha256: Some(harness::expected_sha(len)),
        force_fresh: false,
    };
    let first = dlm::engine::run(&t, cfg.clone())?;
    assert!(first.ok, "第一轮（根本没有断点）该成功：{:?}", first.message);
    assert_eq!(first.fresh_reason, None, "没有断点就没有冲突，那句原因必须是 `None`（Task 9 不该为没发生的事打一行）");
    assert!(dlm::state::path_for(&t.target_path()).exists(), "给了 --keep-state 就该留着断点文件");

    let second = dlm::engine::run(
        &Task { referer: Some("http://referer.example/page".to_string()), ..t.clone() },
        cfg,
    )?;
    assert!(second.ok, "换了 Referer 判从头重来，那一轮也该成功：{:?}", second.message);
    let reason = second.fresh_reason.clone().unwrap_or_default();
    assert!(!reason.trim().is_empty(), "①b 那条冲突必须带着一句话走到 Report，实际 None/空串");
    assert!(reason.contains("请求与断点不符"), "那句得手到「为什么重来」：{reason:?}");
    assert!(reason.contains("Referer"), "那句得点名换了哪一项：{reason:?}");
    assert_eq!(harness::sha256_of(&t.target_path())?, harness::expected_sha(len), "重来那一支也得把文件下对");
    // 两轮都上过快照：守卫数由**这两轮真实期望的条数**推出（每轮 = 试探那一枪 + 引擎派出的枪数，
    // T8-R2 的"dispatched 与线上一对一"），不是裸读一次日志再断"> 2"（评审 C-I-4 的同一形状）。
    let expected_lines = first.dispatched().len() + second.dispatched().len() + 2;
    let requests = srv.requests_at_least(expected_lines)?;
    assert_eq!(
        requests.len(),
        expected_lines,
        "两轮各留下「试探 + 派出的枪数」那么多条线上证据：期望 {expected_lines} 条，实际 {}",
        requests.len()
    );
    Ok(())
}

/// **P5 之五：带 `--data-file` 的任务**绝不试探**，而那一发 POST **只发一次**。**
///
/// 依据：E11①（GET 试探确认的是"我们从没发过的那一发"，POST 试探＝把表单交两次）、
/// R7 / E12⑧（整份那一枪被预算掐住也只许如实报未完成，**不许重发**）、E13④（该形状既不读也不写断点）。
/// 取证只在对端那一侧：node 服务器把每一条请求的 `method` 与 `range` 都记进 JSONL ——
/// "有没有多打一条 GET"这件事**只有线上答得出**，读引擎的代码答不出。
#[test]
fn a_data_file_task_is_never_probed_and_posts_exactly_once() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("eng-5b-post")?;
    let len = 1024 * 1024u64;
    let body = harness::scratch_body("5b-post-form.bin", b"q=dlm&page=1")?;
    let t = Task {
        url: format!("{base}/sz/{len}"),
        dir: dir.path().to_path_buf(),
        name: Some("posted.bin".to_string()),
        data_file: Some(body.clone()),
        ..harness::blank_task()
    };
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
        },
    )?;
    assert!(rep.ok, "POST 那一枪该把这份文档整份拿回来：{:?}", rep.message);

    srv.requests_at_least(1)?;
    let requests = srv.requests()?;
    assert_eq!(requests.len(), 1, "带体的形状全程只许有一发请求（试探 + POST = 2 条就是缺陷）：{requests:?}");
    assert_eq!(
        srv.methods()?,
        vec!["POST".to_string()],
        "那一条必须是 POST：GET 就是「试探」，E11① 硬拒的那个形状"
    );
    assert_eq!(
        requests[0].get("range"),
        None,
        "POST 那一枪不许带 Range：带了就等于把整份那一条请求切成几发重交（R7）"
    );
    assert_eq!(rep.retries, 0, "这一支不重试（重发＝二次提交表单）：{rep:?}");
    assert!(rep.trace.is_empty(), "这一支没有调度器，trace 里不该有任何派单：{:?}", rep.trace);
    assert!(!dlm::state::path_for(&t.target_path()).exists(), "该形状既不读也不写断点（E13④）");
    assert_eq!(harness::sha256_of(&t.target_path())?, harness::expected_sha(len), "落盘的得是那份响应体，一字不差");
    let _ = std::fs::remove_file(&body);
    Ok(())
}

/// **P5 之六：`Abort`（断点要口令）⇒ `engine::run` 交回 `Err`，而且**不碰断点文件、不碰目标文件**。**
///
/// T5 转派第 ④ 条给 T8 的原话是"`Abort` 之后不许动断点文件"。这一支的后果很具体：断点是"哪些字节
/// 在这把 ETag 下算数"的唯一记录，把它删了/改了就等于替用户做了一个他没说过的决定（`--password`
/// 重新给一次就能续，删掉就得从 0 重下）。
/// 形状：盘上放一份**需要口令**的断点（`needs_secret = true`，spec §4 明写口令本身不落盘，所以
/// 这份文件就是引擎自己会写出去的那一副形状）+ 一份半截目标文件，然后**不给口令**跑一次。
/// 四条判据：`Err` 且点名口令；断点文件**字节与 mtime 都没变**；目标文件长度没变（没被截断）；
/// 线上只有试探那一枪（判 `Abort` 排在 `Store::open` 之前，一个下载字节都没动）。
#[test]
fn an_abort_leaves_the_state_file_and_the_target_untouched() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let announced = srv.announced()?;
    let dir = harness::tmp_dir("eng-5b-abort")?;
    let len = 1024 * 1024u64;
    let url = format!("{base}/sz/{len}");
    let t = Task {
        url: url.clone(),
        dir: dir.path().to_path_buf(),
        name: Some("secret.bin".to_string()),
        ..harness::blank_task()
    };
    let target = t.target_path();
    let state_path = dlm::state::path_for(&target);
    // 半截目标文件 + 一份"当初带了口令"的断点：断点的其余字段与本轮探到的事实完全对得上，
    // 好让 `Abort` 是**唯一**的结论（不是被 ①b/②/④ 抢先判成 `Fresh`）。
    std::fs::write(&target, vec![0xABu8; (PROBE_READ_BYTES / 2) as usize]).context("铺半截目标文件失败")?;
    let snap = dlm::state::Snapshot {
        v: dlm::state::STATE_VERSION,
        task: dlm::state::TaskView::from(&t),
        validator: dlm::state::Validator {
            etag: Some(announced.etag.clone()),
            last_modified: Some(announced.last_modified.clone()),
            total_len: len,
            final_url: url.clone(),
            ignore_mtime: false,
        },
        segments: vec![dlm::state::SegView { start: 0, end: PROBE_READ_BYTES / 2 - 1 }],
        stats: dlm::state::Stats { bytes_delivered: PROBE_READ_BYTES / 2, bytes_duplicated: 0, retries: 3 },
        needs_secret: true,
        saved_at: dlm::state::utc_now(),
    };
    dlm::state::save(&state_path, &snap).context("铺断点文件失败")?;
    let before = std::fs::read(&state_path)?;
    let mtime_before = std::fs::metadata(&state_path)?.modified()?;

    let outcome = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: None,
            force_fresh: false,
        },
    );
    let Err(e) = outcome else { panic!("断点要口令而本次没给，`engine::run` 却跑下去了：{outcome:?}") };
    let text = e.to_string();
    assert!(text.contains("口令"), "那句拒绝得说得出为什么不停：{text:?}");

    assert_eq!(std::fs::read(&state_path)?.as_slice(), &before[..], "Abort 之后断点文件被改写过（T5 转派第 ④ 条）");
    let after_meta = std::fs::metadata(&state_path)?;
    assert_eq!(after_meta.modified()?, mtime_before, "断点文件的 mtime 动了 —— 内容没动也说明有人写过它");
    assert_eq!(
        std::fs::metadata(&target)?.len(),
        PROBE_READ_BYTES / 2,
        "目标文件被动过（截断/覆写）：`Store::open` 本该排在这个判定之后"
    );
    // 先等到"至少有一条请求落了盘"再数总数：`requests()` 不等日志，缺行会让"恰好一条"这种
    // 上限断言读起来像成立（本项目的老规矩：负向/计数判据都要先守一次落盘时机）。
    srv.requests_at_least(1)?;
    let requests = srv.requests()?;
    assert_eq!(requests.len(), 1, "只许有试探那一枪（判完就拒）：{requests:?}");
    assert_eq!(requests[0]["method"], "GET", "那一枪是试探：{requests:?}");
    assert_eq!(
        dlm::state::load(&state_path)?.context("断点文件读不回来了")?.stats.retries,
        3,
        "断点里那本账必须原样留着"
    );
    Ok(())
}
