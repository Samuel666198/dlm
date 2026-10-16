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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total)?;
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
#[test]
fn range_start_open_ended_but_respects_explicit_end() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let total = srv.announced()?.size;
    let client = harness::client()?;
    let t = Task { url: format!("{base}/blob"), ..harness::blank_task() };

    // ① 末段：`iv.end == total - 1` ⇒ 开放终点。窗口比区间窄，所以这一枪仍然只读一个窗口。
    let last = Interval { start: ALIGNED, end: total - 1 };
    let (store_open, path_open) = harness::store("t7-open.bin")?;
    let out = fetch::run_window(&client, &t, last, &store_open, 10_000_000, total)?;
    assert!(matches!(out, Outcome::Advanced { bytes } if bytes == WINDOW_BYTES), "{out:?}");

    // ② 非末段：终点在文件中间 ⇒ 必须带上显式终点（少带一个字节就是对端多送一整个窗口）。
    let mid = Interval { start: ALIGNED, end: ALIGNED + 65_536 - 1 };
    let (store, path) = harness::store("t7-closed.bin")?;
    let out = fetch::run_window(&client, &t, mid, &store, 10_000_000, total)?;
    assert_eq!(out, Outcome::Complete, "整段读完（区间比窗口窄）⇒ Complete：{out:?}");
    assert_eq!(store.len()?, ALIGNED + 65_536, "终点说了算：只写到 iv.end");
    harness::require_block_alignment(&path, ALIGNED, 65_536)?;

    let reqs = srv.requests_at_least(2)?;
    eprintln!("---- 开放终点/显式终点：服务端收到的 Range ----\n{}", serde_json::to_string_pretty(&reqs)?);
    assert_eq!(reqs[0]["range"], format!("bytes={ALIGNED}-"), "末段要发开放终点那一条");
    assert_eq!(
        reqs[1]["range"],
        format!("bytes={ALIGNED}-{}", ALIGNED + 65_536 - 1),
        "非末段要发带显式终点那一条（两条一模一样就是没按 total_len 分岔）"
    );
    assert_ne!(reqs[0]["range"], reqs[1]["range"], "两条形状必须不同");
    for r in &reqs {
        assert!(r["connection"].is_null(), "不得发 Connection: close（spec §10 R5 / 计划 E2）：{r:?}");
    }
    drop((store, store_open));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path_open);
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total)?;
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
    let Err(err) = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)?;
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
#[test]
fn a_416_whose_denominator_is_our_length_is_complete() -> anyhow::Result<()> {
    let stub = harness::spawn_416_stub()?;
    let (store, path) = harness::store("t7-416-agrees.bin")?;
    let total = 4_194_304;
    let t = Task { url: format!("{}/total/{total}", stub.base()), ..harness::blank_task() };
    let iv = Interval { start: total, end: total * 2 - 1 };

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total)?;
    assert_eq!(out, Outcome::Complete, "N 与我们的 total_len 相等 ⇒ 确实无字节可取：{out:?}");
    assert_eq!(store.len()?, 0);
    assert!(stub.saw("/total/"), "必须真打到对端（否则测的是连接失败）：{:?}", stub.seen());
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
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)?;
    assert_eq!(out, Outcome::Failed(FailKind::Transport), "零字节 + 对端拆线 ⇒ 只能算失败：{out:?}");
    assert_eq!(store.len()?, 0);
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 8 * 1024 * 1024)?;
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

/// 慢**但对端一直在送**不许判成卡死（R1 的窗口化子请求是"卡住的 detector"，不是"慢的 detector"）：
/// `/sz/524288s` 按 64 KiB / 200 ms 的节拍交 512 KiB（E4 实测 ~1.6 s），而我们要 4 MiB。
/// ⇒ body 自然结束在 512 KiB ⇒ 那 512 KiB 是真进度 ⇒ `Advanced`，且**不是** `Stalled`。
#[test]
fn a_live_but_paced_peer_is_advanced_rather_than_misread_as_stalled() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let (store, path) = harness::store("t7-paced.bin")?;
    let t = Task { url: format!("{base}/sz/524288s"), ..harness::blank_task() };
    let iv = Interval { start: 0, end: WINDOW_BYTES - 1 };
    let started = Instant::now();

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 327_680, 48 * 1024 * 1024)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 8192)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 1_048_576)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 1024)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024)?;
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

    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total)?;
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
