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

// ---------------------------------------------------------------- T99-G1c：进程级零泄漏判据

/// **表里每一支被闸拒掉的带凭据形状，用真二进制跑一遍 ⇒ 退出码 2 且对端日志里 `authorization`
/// 命中数为 0**（这一轮真正的价值所在：前两发的判据都只在库层打转，所以每发都漏一族）。
///
/// 取证一律按**字段名**：node 对端把非标准头记在 `extra_headers` 那张表里，`authorization` 是其中
/// 的键（`probe_puts_the_task_headers_on_the_wire_but_not_credentials_in_the_clear` 与
/// `fetch_carries_the_nine_item_contract_headers_on_the_wire` 用的是同一张表、同一个读法）。
/// 按列位置读会在头表换序时静默失读，那种"恒绿"本项目已经付过学费。
///
/// 三条对照臂在**同一发**里，各堵一种"看着绿其实什么都没测"的修法：
/// ① **正向控制臂先跑**：规范 `http://` 且不带凭据 ⇒ 退 0，而对端日志**必须真的长出请求行** ——
///    这一臂让后面那句"命中数为 0"不再是"对端压根没在记日志"的假证据（`requests_at_least` 读不到
///    就超时红，所以它自己就是针）。
/// ② 规范 `http://` + `--password` ⇒ 同样退 2，但那句话必须是"明文 HTTP 不发送口令"：
///    证明"退 2"来自口令闸，而不是我们把请求打断在别的地方。
/// ③ 规范 `https://` + `--no-start` ⇒ 退 **0**（`--no-start` 短路在 CLI 层（**E20④**），这一格
///    不联网、不需要真 TLS 夹具）：堵"严格化顺手把合法写法一起杀掉"那一侧。
///
/// 上界那一半按 harness 的纪律下（`requests_at_least` 之后 `requests_quiet`）：先让正向臂那一发
/// 落进账里，再守满一段静置窗口看有没有多出来的请求 —— 少读几条会让"没多出一条请求"碰巧为真。
///
/// ⚠ 本用例**没有**测到的两支，照实登记在 `t99-G1c-report.md`（不写恒真断言凑数）：
///    真 https 上"凭据真上线"那一跳（本仓库的 node 对端不做 TLS），以及**服务器给的**落地 URL
///    长得不规范那一支 —— 原先记在这行里的"reqwest 交回的 `Url` 永远序列化成规范写法，本机造不出
///    那条夹具"是一句**没有实测支撑的话**，T99-G1e 已把它量清楚并钉成测试
///    （[`measure_the_landing_strings_reqwest_hands_back_for_server_supplied_locations`]）：
///    对面交回的串**确实**永远是 `http://<非空 authority>/…`（scheme 小写、`://` 恰好一次、
///    authority 里不会有 `@`）⇒ "非规范形状的落地串"这一支**造不出**；但服务器可以让 `@` 落到
///    **path** 里（L1/L3/L4 实测两闸都放行、`probe::run` 交回 `Ok`），那一族的后果登记在
///    `t99-G1e-report.md` §2，不在本用例的射程内。
#[test]
fn every_rejected_credential_shape_leaves_no_authorization_header_on_the_wire() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("g1c-leak")?;
    let out_dir = dir.path().display().to_string();
    // authority 从 harness 自己报的 base 上取（端口是 bind 之后才知道的，不许写死）。
    let authority = base.strip_prefix("http://")
        .context("harness 的 base 不以 http:// 开头，本用例的形状拼不出来")?
        .to_string();

    // ① 正向控制臂：规范 http、不带凭据 ⇒ 必须真的下载、真的在日志里留下一条请求。
    let ok = run_dlm(&[
        "--url", &format!("{base}/sz/4096"), "--out-dir", &out_dir,
        "--out-name", "control.bin", "--quiet",
    ])?;
    assert_eq!(
        ok.status.code(),
        Some(0),
        "对照臂（规范 http、不带凭据）必须下载成功，否则后面那句'零命中'就是假证据：exit={:?}，stderr：{}",
        ok.status.code(),
        stderr_of(&ok),
    );
    let before = srv.requests_at_least(1)?;
    assert!(!before.is_empty(), "正向臂的请求没落进日志 ⇒ 日志读法本身是坏的");
    assert!(dir.path().join("control.bin").exists(), "对照臂的目标文件要在盘上");

    // ② 被拒的那些：头 14 支每一支都**真指向本对端** ⇒ 一旦放行，`authorization` 就会落在日志里，
    //    而不是"发不出去所以没证据"。形状来源是 G1b 报告 §6.1 的第三族 + G1/G1b 两族 + 规范对照
    //    + 空 host 两臂（R1 裁定三、G1d 裁定一；那两臂不指向对端，只钉"退 2 且不走进装配"）。
    //    每格 = (形状, 凭据走不走 `--password`)。
    const CRED: &str = "u:PASS@";
    let refused: Vec<(String, bool)> = vec![
        (format!("http://{CRED}{authority}/x0.bin"), false),   // 规范写法（对照：这一支从 G1 之前就该退 2）
        (format!("http:/{CRED}{authority}/x1.bin"), false),    // 第三族：单斜杠
        (format!("http:/\\{CRED}{authority}/x2.bin"), false),  // 第三族：斜杠 + 反斜杠
        (format!("http:////{CRED}{authority}/x3.bin"), false), // 第三族：四斜杠
        (format!("http:\\\\{CRED}{authority}/x4.bin"), false), // 反斜杠两支
        (format!("http:{CRED}{authority}/x5.bin"), false),     // 什么分隔符都没有
        (format!("http:///{CRED}{authority}/x6.bin"), false),  // 三斜杠
        (format!("\u{1}http://{CRED}{authority}/x7.bin"), false),  // 前导 C0（G1b 那一族）
        (format!("http\t://{CRED}{authority}/x8.bin"), false),     // scheme 分隔符里插 Tab
        (format!(" \thttp://{CRED}{authority}/x9.bin"), false),    // 前导空白（G1 那一族）
        (format!("ht\ntp://{CRED}{authority}/x10.bin"), false),    // scheme 里插 LF
        // 同一族的另一半：**凭据走 `--password`**、形状仍是分隔符变体。这一组是"零命中"抓不住的那一半 ——
        // 放行它不会让 `authorization` 上线（`url_is_https` 给假），但会**真打出一枪**并交出非 2 的码，
        // 所以下面那条"请求条数不许多"才是它的针。G1b 报告 §6.1 末尾登记的就是这一支。
        (format!("http:/{authority}/pw1.bin"), true),
        (format!("http:/\\{authority}/pw2.bin"), true),
        (format!("http:////{authority}/pw3.bin"), true),
        // 最后一族（T99-G1c-R1 裁定三 ＋ T99-G1d **裁定一**）：**空 host 由我们在参数层当场拒、退 2**，
        // 不再靠 reqwest 那句 `builder error: empty host` 兜底（以前是 `--no-start` 退 0、真下载退 1）。
        // 这一句是**用户可见承诺**，进程级以前没有针（复审 I-3）；R1.6 那六行证据在 `target/` 里，随清理即失。
        // 两臂都不指向本对端 ⇒ 下面那段循环的三条判据（退码 2、stderr 不含"跑不起来"、
        // 请求条数不许多）**自动适用**，不为它们单开断言、不放宽任何一条既有判据。
        ("https://".to_string(), true),            // 空 host：R1 裁定三那一族
        ("https://:8443/d.bin".to_string(), true), // 空 host + 端口：本发裁定一新夹的那一族
    ];
    for (shape, with_password) in &refused {
        let mut args: Vec<&str> = vec![
            "--url", shape.as_str(), "--out-dir", out_dir.as_str(),
            "--out-name", "leak-check.bin", "--quiet",
        ];
        if *with_password {
            args.extend_from_slice(&["--user", "alice", "--password", "hunter2"]);
        }
        let out = run_dlm(&args)?;
        let code = out.status.code();
        let es = stderr_of(&out);
        let so = stdout_of(&out);
        assert_eq!(
            code,
            Some(2),
            "{shape:?} 必须在参数层就被拒（**E23⑤/E26/C3**：'话没说对'只有一种码）：exit={code:?}，stderr：{es}"
        );
        assert!(
            so.is_empty(),
            "{shape:?} 的 stdout 有输出 ⇒ 这一趟走到了打印摘要或结果行那一步，参数闸没拦住：{so}"
        );
        assert!(!es.contains("PASS"), "stderr 不许把 URL 里的口令抄一遍：{es}");
        assert!(!es.contains("hunter2"), "stderr 不许把 --password 抄一遍：{es}");
        assert!(!es.contains("跑不起来"), "出现引擎那句就说明这趟走进了装配：{es}");
        assert!(
            !dir.path().join("leak-check.bin").exists(),
            "{shape:?} 被拒之后磁盘上不该有目标文件（闸排在任何 I/O 之前）"
        );
    }

    // ② 的另一半：规范 http + `--password` 同样退 2，但走的是**口令**那一支。
    let plain = run_dlm(&[
        "--url", &format!("{base}/blob"), "--out-dir", &out_dir, "--out-name", "plain.bin",
        "--user", "alice", "--password", "hunter2", "--quiet",
    ])?;
    assert_eq!(
        plain.status.code(),
        Some(2),
        "规范 http + 口令也要退 2（这一支证明'退 2'不是我们把请求打断在别处）：stderr：{}",
        stderr_of(&plain)
    );
    let es = stderr_of(&plain);
    assert!(es.contains("明文 HTTP 不发送口令"), "对照臂要走的那一支：{es}");
    assert!(!es.contains("hunter2"), "错误消息不许抄口令：{es}");

    // ③ 规范 https + 凭据 + `--no-start` ⇒ 退 0：严格化没把合法写法一起杀（这一格不联网）。
    let secure = run_dlm(&[
        "--url", "https://example.invalid/dir/a.bin", "--out-dir", &out_dir,
        "--out-name", "secure.bin", "--user", "alice", "--password", "hunter2", "--no-start",
    ])?;
    assert_eq!(
        secure.status.code(),
        Some(0),
        "规范 https + 口令 + `--no-start` 必须退 0（**E20④** 的短路在 CLI 层，不联网也不需要真 TLS）：stderr：{}",
        stderr_of(&secure)
    );

    // 上界与零命中：先 `requests_at_least` 保证不漏读，再 `requests_quiet` 守满静置窗口看不许多读。
    let after = srv.requests_at_least(before.len())?;
    let quiet = srv.requests_quiet(std::time::Duration::from_millis(500))?;
    assert_eq!(
        quiet.len(),
        before.len(),
        "被拒的那 {} 发（形状表每一支 + 规范 http 带口令那一支）里有任何一发打到了对端 ⇒ 形状闸没拦住\
         （目标文件没被创建不代表线上没打）：正向臂之后 {before:?}，静置窗口之后 {quiet:?}",
        refused.len() + 1,
    );
    let auth_hits = after
        .iter()
        .filter(|r| r["extra_headers"].get("authorization").is_some())
        .count();
    assert_eq!(
        auth_hits,
        0,
        "对端按字段名记的 `extra_headers.authorization` 命中 {auth_hits} 次 ⇒ 有凭据上了明文线：{after:?}"
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

/// **T99-G1e 任务二 + 任务三（本发是量具，不是判据）**：让**服务器**交出简报里那 10 条形状各异的
/// `Location`，把 reqwest 交回的落地串**钉成手写期望**，并把 `probe::run` 的实际反应照原文登记。
///
/// 动机（简报）：`src/probe.rs:196-201` 与上面
/// `every_rejected_credential_shape_leaves_no_authorization_header_on_the_wire` 那发的表头注释
/// 都写着"reqwest 交回的 `Url` 永远序列化成规范写法，本机造不出那条夹具"，
/// 而这句话**从未被实测支撑**。它决定下一发要不要把
/// 并联闸的第 2 条判据（`canonical_http_authority`）接进 `fetch.rs` 那四处落地检查。
/// 本发**一行本体都没改**：接不接是控制员看完实测之后的裁定。
///
/// ## 实测口径（`--nocapture` 的原文抄在 `t99-G1e-report.md` §1）
///
/// * 每一支交回的 URL 都是 `http://127.0.0.1:端口/…` 的**规范序列**（scheme 小写、`://` 恰好一次、
///   host 非空、authority 里没有 `@`），**除了** L2 那一支 —— 它是唯一被 reqwest 读成 authority
///   userinfo 的（`http://alice:s3cret@…/landed`，`@` 在 authority 里）。
/// * 所以：**造不出**"无 `@` 但非规范"的落地串 ⇒ `fetch.rs` 那四处若接并联闸的第 2 条，
///   在 reqwest 交回的串上是**恒不触发**的一条（简报明令禁止的死分支）。实测唯一被拒的 L2
///   两闸**都**认得（`@` 在 authority ⇒ 半支 1 也看得见）。
/// * ⚠ **另有一族本发才看得见、简报表里没有预判的**：L1/L3/L4 那三支的 `@` 被 reqwest **折进
///   path**（url 2.5.8 `parser.rs:448-459` 的"special relative or authority"分支：同 scheme 且
///   斜杠少于 2 ⇒ 整串按相对引用解析），落地串是 `http://127.0.0.1:端口/alice:s3cret@…/landed`
///   —— 形状规范、authority 干净，**两闸都放行**，`probe::run` 实测交回 `Ok`，于是
///   `alice:s3cret@` 这份凭据串照样落进 `final_url`（⇒ 断点文件、② 的原因串）。
///   这是**现役闸的真缺陷**，与"半支 2 要不要接进四处"是两条独立的问题，报告 §2 单独标出。
///   按"已知坏行为不许钉成测试期望"，这三支**只登记反应、不钉 `Ok`**（见 `Read::RegisterOnly`）。
#[test]
fn measure_the_landing_strings_reqwest_hands_back_for_server_supplied_locations() -> anyhow::Result<()> {
    /// 落地串拿到之后**这一支断什么**（任务三的三个口径，外加两支"对面压根没跟"）。
    #[derive(Copy, Clone)]
    enum Read {
        /// 合法的服务器重定向 ⇒ `probe::run` **必须成功**（正向对照臂 = 本发量具的活体针）。
        Accepted,
        /// authority 里带 `@` ⇒ `probe::run` 必须 `Err`，措辞含"凭据"与"重定向"且不回显
        /// （读法照现役的 `a_credentialed_landing_url_is_refused_and_never_echoed`）。
        RefusedWithCredentials,
        /// 只钉 reqwest 交回的串，**产品反应照原文打印、不钉期望**（实测是 `Ok`，那是坏行为）。
        RegisterOnly,
        /// reqwest 压根不跟：直连就报错 ⇒ 没有落地串，唯一可观察的 URL 是发起请求那一条。
        NotFollowedWithError,
        /// reqwest 不跟也不报错：那条 302 原样交回，`resp.url()` 仍是 `/start`。
        NotFollowedSilently,
    }

    struct Row {
        id: &'static str,
        /// 桩交回的 `Location` **字面量**（简报那张表的原文；`{port}` 由桩换成实际端口）。
        location: &'static str,
        /// **手写的**期望 URL 原文（`{a}` 换成对端 authority `127.0.0.1:端口`）。
        /// 期望值全部由桩自己知道的 authority/端口拼出来，**没有一个字节来自产品函数**
        /// （`canonical_http_authority` 是 `pub(crate)`，集成测试够不到 —— 这不是巧合，是纪律）。
        url: &'static str,
        /// 手写的期望落地请求目标（`{a}` 同上）。`None` = 这一支**不许**有第二条请求打到对端。
        target: Option<&'static str>,
        read: Read,
    }

    const ROWS: [Row; 10] = [
        Row {
            id: "L1",
            location: "http:/alice:s3cret@127.0.0.1:{port}/landed",
            url: "http://{a}/alice:s3cret@{a}/landed",
            target: Some("/alice:s3cret@{a}/landed"),
            read: Read::RegisterOnly,
        },
        Row {
            id: "L2",
            location: "http:////alice:s3cret@127.0.0.1:{port}/landed",
            url: "http://alice:s3cret@{a}/landed",
            target: Some("/landed"),
            read: Read::RefusedWithCredentials,
        },
        Row {
            id: "L3",
            location: "http:alice:s3cret@127.0.0.1:{port}/landed",
            url: "http://{a}/alice:s3cret@{a}/landed",
            target: Some("/alice:s3cret@{a}/landed"),
            read: Read::RegisterOnly,
        },
        Row {
            id: "L4",
            location: "http:/$alice:s3cret@127.0.0.1:{port}/landed",
            url: "http://{a}/$alice:s3cret@{a}/landed",
            target: Some("/$alice:s3cret@{a}/landed"),
            read: Read::RegisterOnly,
        },
        Row {
            id: "L5",
            location: "ftp://127.0.0.1:{port}/landed",
            // 对面不跟 ⇒ 唯一可观察的串还是发起请求那条（`ftp://` 从不成为落地串）。
            url: "http://{a}/start",
            target: None,
            read: Read::NotFollowedWithError,
        },
        Row {
            id: "L6",
            location: "http://127.0.0.1:{port}//landed",
            url: "http://{a}//landed",
            target: Some("//landed"),
            read: Read::Accepted,
        },
        Row {
            id: "L7",
            location: "HTTP://127.0.0.1:{port}/landed",
            // 大写 scheme 由对面**小写化**后再交回 ⇒ 落地口永远看不见 `HTTP://` 这种形状。
            url: "http://{a}/landed",
            target: Some("/landed"),
            read: Read::Accepted,
        },
        Row {
            id: "L8",
            location: "http://127.0.0.1:{port}/landed?a=b\\c",
            // `\` **留在 query 里**，对面不把它换成 `/` ⇒ 半支 1/半支 2 终止符集合不一致那一族
            // 在落地口拿不到任何增量（authority 早在第一个 `/` 就切完了）。
            url: "http://{a}/landed?a=b\\c",
            target: Some("/landed?a=b\\c"),
            read: Read::Accepted,
        },
        Row {
            id: "L9",
            location: "http://:8443/landed",
            // 空 host 那条对面**解析失败** ⇒ 不跟、不报错，那条 302 原样交回。
            url: "http://{a}/start",
            target: None,
            read: Read::NotFollowedSilently,
        },
        Row {
            id: "L10",
            location: "http://127.0.0.1:{port}/landed#frag",
            // fragment **进不了** `resp.url()`（http-1.5.0 `uri/path.rs:28-30` 在建 `Uri` 时
            // 把 `#` 之后整段截掉）⇒ 它既不会落盘、也进不了 ② 的原因串。
            url: "http://{a}/landed",
            target: Some("/landed"),
            read: Read::Accepted,
        },
    ];

    for row in ROWS {
        let (stub, location) = harness::spawn_raw_redirect_stub(row.location)?;
        let authority = stub
            .base()
            .strip_prefix("http://")
            .context("harness 的 base 不以 http:// 开头，本发的形状拼不出来")?
            .to_string();
        let want_url = row.url.replace("{a}", &authority);
        let want_target = row.target.map(|t| t.replace("{a}", &authority));
        let start = format!("{}/start", stub.base());

        eprintln!("\n======== {} ========", row.id);
        eprintln!("{} 桩交回的 Location 原文：{location}", row.id);

        // ── ① reqwest 直连：它交回的落地串（或它挂在那条错误上的 URL）。
        let direct = harness::client()?.get(&start).send();
        match &direct {
            Ok(resp) => {
                eprintln!("{} 直连 status={}，resp.url()={}", row.id, resp.status(), resp.url());
            }
            Err(err) => {
                eprintln!("{} 直连报错：{err}", row.id);
                eprintln!("{} 直连错误链：{err:?}", row.id);
            }
        }
        let observed_url = match &direct {
            Ok(resp) => resp.url().as_str().to_string(),
            Err(err) => err.url().map(|u| u.as_str().to_string()).unwrap_or_default(),
        };
        // 任务三第 2 条：reqwest 交回的串**从此有测试钉着**（而不是有文档说着）。
        assert_eq!(
            observed_url, want_url,
            "{}：对面交回的串必须等于**手写**期望（这一钉的全部意义就是\"reqwest 序列化出来的一定是这个形状\"）",
            row.id
        );
        match row.read {
            Read::NotFollowedWithError => {
                if let Ok(resp) = &direct {
                    panic!("{}：非 http/https 的 Location 本该让 reqwest 当场报错，实际交回 {}", row.id, resp.status());
                }
            }
            Read::NotFollowedSilently => {
                let resp = match &direct {
                    Ok(resp) => resp,
                    Err(err) => panic!("{}：解析不出的 Location 本该被对面当成\"没有重定向\"原样交回，实际直连报错：{err}", row.id),
                };
                assert_eq!(resp.status(), 302, "{}：这一支的状态必须还是那条 302（对面压根没跟）", row.id);
            }
            _ => {
                let resp = match &direct {
                    Ok(resp) => resp,
                    Err(err) => panic!("{}：这一支 reqwest 必须真跟随重定向并交回 200，实际直连报错：{err}", row.id),
                };
                assert_eq!(resp.status(), 200, "{}：落地那条必须是桩交回的诚实 200", row.id);
            }
        }
        // 假断言防护（读法同现役那发凭据用例）：要么落地那条**真打到**对端，要么**一条都不许多**。
        match want_target.as_deref() {
            Some(t) => assert!(
                stub.saw(t),
                "{}：对端没收到落地那条（期望目标 {t}）⇒ 本发没测到\"服务器给的落地串\"这一环，实际收到 {:?}",
                row.id,
                stub.seen()
            ),
            None => assert!(
                !stub.saw("/landed"),
                "{}：这一支对面不许跟随那条 Location，实际收到 {:?}",
                row.id,
                stub.seen()
            ),
        }

        // ── ② 同一条路走 probe::run：每一支都打原文，之后按 `read` 决定钉不钉。
        let t = Task { url: start.clone(), ..harness::blank_task() };
        let outcome = probe::run(&t, &harness::client()?, PROBE_READ_BYTES);
        match &outcome {
            Ok(r) => eprintln!("{} probe::run Ok：{r:?}", row.id),
            Err(err) => {
                eprintln!("{} probe::run Err：{err}", row.id);
                eprintln!("{} probe::run 错误链：{err:?}", row.id);
            }
        }
        eprintln!("{} 桩收到过的请求目标：{:?}", row.id, stub.seen());

        match row.read {
            Read::Accepted => {
                let r = match outcome {
                    Ok(r) => r,
                    Err(err) => panic!(
                        "{}：一条**合法**的服务器重定向必须让 probe::run 成功（正向对照臂 = 本发量具的\
                         活体针；它红了说明落地闸在误杀合法写法）：{err}",
                        row.id
                    ),
                };
                assert_eq!(
                    r.facts.final_url, want_url,
                    "{}：落进 `ProbeFacts.final_url` 的串必须等于手写期望（它就是会落盘、会进 ② 原因串的那一条）",
                    row.id
                );
                // 变异自证的第二发落点：去掉落地桩那条 200 的 `ETag` ⇒ 下面这几支必须红
                // （证明本发读的是**落地那条**，不是 `/start` 那条 302）。
                assert_eq!(
                    r.facts.etag.as_deref(),
                    Some("\"stub\""),
                    "{}：ETag 只能来自落地那条 200（含协议要求的引号）",
                    row.id
                );
                assert_eq!(r.facts.total_len, 0, "{}：落地那条是 Content-Length: 0", row.id);
                assert!(r.prelude.is_empty(), "{}：落地那条没有体 ⇒ 预读为空", row.id);
                assert!(
                    !r.facts.accept_ranges,
                    "{}：对我们那条 Range 回了 200 ⇒ 头里的 Accept-Ranges 不作数（读法同\
                     `an_ordinary_redirect_still_lands_and_is_reported`）：{:?}",
                    row.id, r.facts
                );
            }
            Read::RefusedWithCredentials => {
                let Err(err) = outcome else {
                    panic!(
                        "{}：落地 URL（{want_url}）的 authority 里带着凭据，probe 必须拒绝 —— 这份事实会被落盘并进 ② 的原因串",
                        row.id
                    );
                };
                let msg = err.to_string();
                assert!(msg.contains("凭据"), "{}：错误要说清拒的是凭据：{msg}", row.id);
                assert!(
                    msg.contains("重定向"),
                    "{}：错误要说清凭据来自服务器带的路（不是用户自己写的）：{msg}",
                    row.id
                );
                for leak in ["s3cret", "alice", "landed"] {
                    assert!(
                        !msg.contains(leak),
                        "{}：错误里回显了落地 URL 的一部分（{leak}）：{msg}",
                        row.id
                    );
                }
            }
            Read::RegisterOnly => {
                // **这里一条断言都不写，是刻意的**（简报任务三第 3 条末句 + 本项目"已知坏行为
                // 不许钉成测试期望"的纪律）：实测这三交的的是 `Ok`，而 `final_url` 里带着
                // `alice:s3cret@` 那段凭据串 —— 那是缺陷，钉成期望就等于替它背书、并且让下一发的
                // 修必须来改这条断言。反应已按原文打印在上面，报告 §2 照原文登记。
            }
            Read::NotFollowedWithError | Read::NotFollowedSilently => {
                // 对面不跟这条 Location ⇒ 我们读到的只有那条 302/那个错误，**不许**把它当成
                // "落地成功"收下（两支实测都是 `Err`，措辞不同、打印在上面，本发不钉措辞）。
                if let Ok(r) = outcome {
                    panic!(
                        "{}：对面压根没跟随这条 Location ⇒ 这份\"事实\"来自一条 302 而不是文件，probe 必须报错，实际交回 {r:?}",
                        row.id
                    );
                }
            }
        }
    }
    Ok(())
}

/// **T99-G1f 的进程级正臂**：对面把 `someone:else@…` 这类**口令形状**的串按相对引用折进
/// **path**（G1e L1/L3/L4 实测的那一族），而**本任务没有口令** ⇒ 统一判据（三支合取：userinfo／
/// 形状／本任务口令的字面值）必须**照旧放行**、落地串按原样进 `final_url`。
///
/// 这一臂钉两件事：① 新闸不误杀现役路径（第三支是 **task-aware** 的：没有口令就没有内容可判，
/// "看着像凭据"的自由文本不由我们替用户决定）；② 后来的"顺手修法"——有人若想把落地串里
/// **任何** `@`／口令形状文本一律拒掉（那是简报明令不做的"形状判据第二版"），这一臂会红。
///
/// ⚠ 为什么**没有**同族的进程级反臂与空口令臂（照实登记对简报第 3 条的偏差，理由是实测）：
/// `Task::validate()` 先拒"明文 HTTP ＋ 口令"（spec §4 / E9），而本对端不做 TLS ⇒ 在这台仪器的
/// 明文桩上**带口令的任务永远到不了落地闸**——`a_password_over_plain_http_never_leaves_the_socket`
/// 已经用服务端日志钉过"一个字节都没上网"，简报反臂那句"消息含凭据与重定向"在这种任务上拿不到
/// （先到的那句是"明文 HTTP 不发送口令"）。第三支真正可达的是 https 侧（口令只在那边上线），
/// 真 TLS 维持具名欠账。反臂／空口令臂（连同 `!p.is_empty()` 那一刀的 m1 针）因此钉在
/// `src/probe.rs` 单元层直调 `landed_url_is_refused` / `facts_from_response` 的三发用例里。
#[test]
fn the_unified_landing_gate_still_lands_a_password_shaped_path_without_a_task_password(
) -> anyhow::Result<()> {
    // 量具用现成的（G1e 任务一）：Location 是**字面量**，`{port}` 由桩换成实际端口。
    let (stub, _location) =
        harness::spawn_raw_redirect_stub("http:/someone:else@127.0.0.1:{port}/landed")?;
    let authority = stub
        .base()
        .strip_prefix("http://")
        .context("桩的 base 不以 http:// 开头，期望串拼不出来")?
        .to_string();
    // **手写**期望（纪律：没有一个字节来自产品函数）：G1e L1 实测的折入形状，换成别的口令。
    let want_url = format!("http://{authority}/someone:else@{authority}/landed");
    let want_target = format!("/someone:else@{authority}/landed");

    let t = Task { url: format!("{}/start", stub.base()), ..harness::blank_task() };
    let r = probe::run(&t, &harness::client()?, PROBE_READ_BYTES).map_err(|err| {
        anyhow::anyhow!("任务没有口令 ⇒ 第三判据无从开火，这条落地路必须照旧 Ok（正臂红了＝统一判据在误杀）：{err}")
    })?;
    assert_eq!(r.facts.final_url, want_url, "落地串必须按原样进 final_url（它就是会落盘、会进 ② 原因串的那一条）");
    assert_eq!(r.facts.etag.as_deref(), Some("\"stub\""), "ETag 只能来自落地那条 200（证明真跟随了重定向）");
    assert!(stub.saw(&want_target), "对端没收到落地那条 ⇒ 本发没测到落地这一环，实际收到：{:?}", stub.seen());
    Ok(())
}

/// **T99-G5 第 0 条**：把"`fetch` 那四个落地口只许走统一判据"钉成一根**结构针**（源码文本扫描）。
///
/// 为什么这里需要的是一根结构针而不是一台夹具（G1f 的 **m3 实测**）：把 `src/fetch.rs` 的
/// `run_whole` 落地口退回旧判据（只判 `authority_has_userinfo`）时，本机四闸**全绿、量不出来**；
/// 同一支变异打在 `src/probe.rs:362` 上是**红**的。原因照实登记在 T99-G1e 报告 §8.4：
/// `Task::validate()` 先拒"明文 HTTP ＋ 口令"（`src/task.rs:262`），而本对端**不做 TLS** ⇒
/// "带口令的任务真打穿 `fetch` 那一枪"在这台机器上**造不出夹具**（真 https 是既有的具名欠账）。
/// 控制员裁定＝**不造不真实的夹具，改为钉结构**（本项目已有同类形状：扫源码文本、禁字、连注释一起扫）。
///
/// ## 两条判据必须并联，缺一不可
/// - **禁字**：`authority_has_userinfo(` 不许再出现在 `src/fetch.rs` 里（那四处只许走
///   `probe::landed_url_is_refused`）。⚠ 这条只管本文件：`src/probe.rs` 与 `src/task.rs` 里
///   它**该**出现（统一判据的第一支就是它），所以扫描范围是"那四个落地口住的文件"，不是全树。
/// - **数次数**：`probe::landed_url_is_refused(` 在该文件**恰好 4 次**（四个口一处一个：
///   `run_window` 的错误支与落地支 ＋ `run_whole` 的错误支与落地支）。
///
/// 为什么要两条一起写（这是本发要的写法，理由得留在树里）：**只数次数**会被"把两处合并成一处、
/// 另一处直接删掉"骗过（次数照样对得上，但有个口没人守了 —— 合并后另一处若改走 `authority_has_userinfo`
/// 就没人报警）；**只禁字**会被"改叫别的名字"骗过（在 `fetch` 里再写一份私有判据、或把旧判据换个入口，
/// 禁字清单看不见它）。两条各挡一种骗法 ⇒ 并联。
///
/// ⚠ 计数是**字节串出现次数**，注释里的写法也算：谁在注释里照抄一遍带左括号的调用形状
/// （`probe::landed_url_is_refused(...)`）这一发就会红 —— 那是刻意的，计数要有唯一含义，
/// 注释里的形状不许与调用点长得一样（G1f 那两句注释写的是不带左括号的 `` `probe::landed_url_is_refused` ``）。
///
/// ⚠ **这根针的失效条件**（不许读成"那四处真能拒"）：它量的是**"有没有人偷偷分家"**，
/// **不**保证四处真能拒住带口令的落地串 —— 后者仍是 TLS 那一格欠的账（判据本身由
/// `src/probe.rs` 单元层三臂钉着，进程级只钉得住"无口令不误杀"那一臂）。
/// 哪天对端能上 TLS、行为夹具补上了，这一发**也不该删**："不分家"与"能拒住"是两件事。
///
/// 读文件用运行时 `fs::read_to_string` ＋ `CARGO_MANIFEST_DIR`：不是 `include_str!`，
/// 更不是把源码抄进测试当字符串 —— 抄一份的针改不动实现，也测不到实现。
#[test]
fn the_four_landing_ports_in_fetch_all_go_through_the_unified_criterion() -> anyhow::Result<()> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("fetch.rs");
    let src = std::fs::read_to_string(&path)
        .with_context(|| format!("读 {} 的源码文本失败（本发是源码扫描针，读不到就是没有判据）", path.display()))?;

    // 判据一（禁字）：旧判据不许在这文件里再被直接调用。
    const OLD_CRITERION: &str = "authority_has_userinfo(";
    if let Some(at) = src.find(OLD_CRITERION) {
        let line = src[..at].chars().filter(|c| *c == '\n').count() + 1;
        panic!(
            "src/fetch.rs:{line} 又直接调 `authority_has_userinfo(` —— 那四个落地口只许走 \
             `probe::landed_url_is_refused`（T99-G5 第 0 条；这正是 G1f 的 m3 在本机量不出红的形状）"
        );
    }

    // 判据二（数次数）：四个口一处一个，多一处少一处都算分家。
    const UNIFIED: &str = "probe::landed_url_is_refused(";
    let hits = src.matches(UNIFIED).count();
    assert_eq!(
        hits, 4,
        "src/fetch.rs 里 `probe::landed_url_is_refused(` 应当**恰好 4 次**（run_window 两支 ＋ run_whole 两支），实得 {hits} 次：\
         少了＝有人把某个落地口退回旧判据或整处删掉，多了＝判据又开了第二个入口（与本条的禁字判据配成一对）"
    );
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
    let out = fetch::run_window(&client, &t, last, &store_open, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
    assert!(matches!(out, Outcome::Advanced { bytes } if bytes == WINDOW_BYTES), "{out:?}");

    // ② 非末段：终点在文件中间 ⇒ 必须带上显式终点（少带一个字节就是对端多送一整个窗口）。
    let mid = Interval { start: ALIGNED, end: ALIGNED + 65_536 - 1 };
    let (store, path) = harness::store("t7-closed.bin")?;
    let out = fetch::run_window(&client, &t, mid, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
    assert_eq!(out, Outcome::Complete, "整段读完（区间比窗口窄）⇒ Complete：{out:?}");
    assert_eq!(store.len()?, ALIGNED + 65_536, "终点说了算：只写到 iv.end");
    harness::require_block_alignment(&path, ALIGNED, 65_536)?;

    // ③ 文件真正那一尾：起点 = 全长 - 一个窗口 ⇒ 窗口终点正好是 `total_len - 1` ⇒ 开放终点。
    let tail = Interval { start: total - WINDOW_BYTES, end: total - 1 };
    let (store_tail, path_tail) = harness::store("t7-tail.bin")?;
    let out = fetch::run_window(&client, &t, tail, &store_tail, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
    let Err(err) = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 8 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 327_680, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 8192, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 1_048_576, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
    // **复审 B-Minor 5**：上面那次读数发生在对端**还在写**的时候（节拍 64 KiB / 20 ms，
    // 内核缓冲里还有一截没出手）⇒ 那是一条对增长量读一次就断的上界，与本轮修的恒真窗口同一形状
    // （只是余量 4 倍，实践上稳，但形状不对）。落盘句柄与客户端都放下之后再读一次，
    // 断**同一个**上界：那时对端要么已经写完收手、要么已经撞上断链错误，`written` 不再增长。
    let written_after_teardown = peer.written();
    assert!(
        written_after_teardown < 4 * 1024 * 1024,
        "客户端断开之后对端写出 {written_after_teardown} 字节（整份 body 是 8 MiB）⇒ \
         收手的时机不是由我们决定的，这一格量的是'吸完整个 body'那一支：{written} → {written_after_teardown}"
    );
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
    let result = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed);
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
    let out = fetch::run_window(&client, &t, iv, &store, WINDOW_BYTES, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
    let out2 = fetch::run_window(&client, &t, small, &store2, WINDOW_BYTES, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
        let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, total, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
    let out = fetch::run_window(&client, &t, Interval { start: 0, end: 4_095 }, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&harness::client()?, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &harness::uncapped(), fetch::BodyShape::Windowed)?;
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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
    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &cap, fetch::BodyShape::Windowed)?;
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

    let out = fetch::run_window(&client, &t, iv, &store, 10_000_000, 48 * 1024 * 1024, &cap, fetch::BodyShape::Windowed)?;
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
/// `Server::ranges_at_least` **先守卫条数**、再经 `Server::requests_quiet` **守满一段静置窗口**
/// 再读 —— 两句各钉"不漏读"与"不多读"，见下面那段注释），不是我们拼出来的字符串。
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
        },
    )?;
    assert!(rep.ok, "引擎没跑完：{:?}", rep.message);

    // **先守卫条数再读**（评审 C-I-4；同一缺陷 5B 已在 Abort 用例修过一发，提交 `7b368d2`）：
    // 引擎返回的那一刻日志未必已经落盘，裸读一次会把"最后那一两条 Range"漏掉 —— 而漏掉的
    // 恰好可能是唯一违规的那一枪，判据就会**碰巧**为真。守卫数由这条用例真实期望的条数推出：
    // 线上请求 = 引擎派出的枪数 + 试探那一枪（`Report::dispatched()` 与线上是一对一，T8-R2）。
    //
    // **复审 B-I-4**：守卫数取自**被测自己的**账 ⇒ 光"等到那个数"只审了子集，所以等式要断。
    // **收口复审 B2 第 5 条**：那句批评还得再往前一步 —— 旧注释吹的"多一条就红"当时**是假的**：
    // `requests_at_least` 一到 `n` 就返回，第 `n+1` 条稍后才落盘就永远看不见 ⇒
    // `assert_eq!(ranges.len(), n)` 只审得出"等到的是不是恰好 n 条"，审不出"后来有没有多"。
    // 现在按 `Server::requests_quiet` 文档钦定的**成对用法**来，两句各钉一边：
    // ① `ranges_at_least(k)` 之后先断"那一份快照正好 k 条"⇒ 下面那些逐条判据审的**不是子集**；
    // ② 紧接 `requests_quiet(wait)` **守满**一段时间再断"窗结束仍是 k 条"⇒ 多打一枪当场红。
    // 两句合起来才是"钉平"：①管**不漏读**、②管**不多读**（静置窗口之外的仍然看不见，那是量具的
    // 窗口宽度；够用是因为 `engine::run` 已经交回、所有 worker 线程都已 `join` ⇒ 之后不会再有请求）。
    let expected_lines = rep.dispatched().len() + 1;
    let ranges = srv.ranges_at_least(expected_lines)?;
    assert_eq!(
        ranges.len(),
        expected_lines,
        "线上条数与引擎报的枪数不是一对一（T8-R2）：等到 {} 条，引擎报 {} 枪 + 试探那一枪 = {} \
         ⇒ 条数没钉平之前，上面那些逐条判据审的只是子集",
        ranges.len(),
        rep.dispatched().len(),
        expected_lines,
    );
    let quiet = srv.requests_quiet(Duration::from_millis(300))?;
    assert_eq!(
        quiet.len(),
        expected_lines,
        "静置窗口里冒出了多出来的请求：守满 300 ms 之后线上有 {} 条，引擎只报了 {} 枪 + 试探那一枪 = {} \
         ⇒ ①那句等式审的是'等到的是不是恰好 n 条'，这一句才审得出'后来有没有多'（收口复审 B2 第 5 条）",
        quiet.len(),
        rep.dispatched().len(),
        expected_lines,
    );
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

/// 把一份事件快照里 `conn_close_side` 的**行数**与 `sent` 之和折成一个二元组。
///
/// 只为一件事存在：`the_tightening_bill_on_a_paced_peer_is_priced_by_chunk_not_by_window`
/// 的等待谓词与它后面的读数都要看同一对量（close 的行数 + `sent` 和），而 `events_while` 的谓词
/// 每次只拿到一份快照。抽取写在这里 ⇒ 谓词与用例后面的读数用的是**同一个函数**，不存在两套口径。
/// （收口复审 B2 第 2 条之后谓词不再需要跨采样状态，这个二元组仍然两个调用点共用。）
fn close_side_totals(evs: &[serde_json::Value]) -> (usize, u64) {
    let mut lines = 0usize;
    let mut sent = 0u64;
    for e in evs {
        if e["ev"] == "conn_close_side" {
            lines += 1;
            sent += e["sent"].as_u64().unwrap_or(0);
        }
    }
    (lines, sent)
}

/// **E14①＋③（有节拍对端上的线上账单）**：多打出去的字节按"每笔收紧几块"计价，而不是按窗口计价。
///
/// 为什么这条要挂在**节拍**对端（`/sz/<n>s`：每 64 KiB 之间睡 200 ms）上：无节拍的环回对端会在
/// 我们收手之前把整条 span 一次性塞进 socket，`conn_close_side.sent` 读起来与"我们真需要多少"无关
/// （那是本仪器上测不出一件事，不是它不存在）。有节拍时服务器写多少由我们读多少决定 ⇒ 这一格
/// 才是**账单**。取 `conn_close_side.sent`（每条响应收尾时服务器自己写的字节数）求和，
/// **不取** `response_done`：后者只在服务器写完整条 body 时才发，中途被丢的那些根本不入账。
/// 口径按**计划勘误 E14 附记**：全项目唯一的诚实线上口径就是 `Σ conn_close_side.sent` —— `Σ response_done.bytes`
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
/// - **量具活体针：等到"该有这么多条 close"，再断两条对得上号的等式**（评审 C-I-1① 起了这根针，
///   **复审 B-I-1 修好它的形状**：原先的谓词是 `lines > 0 && sent >= len`，而下面那两条针
///   是对**同一份快照**重算这两个量 ⇒ 数学上不可能红，真正承重的只有那个 wait。
///   **收口复审 B2 第 2 条再改一次**：等的不再是"连续两拍行数与 sent 和都相同"（启发式静置），
///   而是 `conn_close_side` 的**行数到达本用例自己已经推得出、并且下面②要断的那个总数**
///   （`Report::dispatched().len() + 1`，T8-R2 的一对一）且 `sent >= len`。
///   ⚠ 这一发**不是**在修一条会红的用例（控制员在 `cced137` 连跑 5 次全绿）；改的是判据形状：
///   两个相等采样在 200 ms 节拍 + `POLL = 20 ms` 下**可能出现在中途**（那时 `written` 还是下界 ⇒
///   `excess <= bill` 能在真缺陷上碰巧为真），而一条迟到的 close 又能让条数等式假红。
///   换成"等到那个总数"之后：假绿那截残留被去掉、对迟到 close 结构免疫、等待条件与②同源。
///   落全之后断两条**有牙**的等式：close 行数 == `request` 行数（两边都出自对端自己的日志，
///   不问引擎的嘴）、close 行数 == 引擎报的枪数 + 试探那一枪（T8-R2 的那对一对一）。
///   两条联立 ⇒ "线上条数 == 被测枪数"这一句也不再是恒真：引擎多打一枪而没记进 trace
///   （重试、或 trace 漏记）时，第二条当场红。
///   **为什么等待只管"不漏读"、上界仍归断言**：谓词是 `>=`，返回时只会**不少于**那个总数；
///   "恰等于"由②钉。这与 `Server::requests_quiet` 文档里那条分工同形 —— 那里静置窗口当上界仪器，
///   这里由等式当；两处都不许把上界塞进等待条件（塞进去就等于把缺陷等没了）。
///   **为什么这条 close 会迟到**：`conn_close_side` 记在 `res.on("close")` 上
///   （`tests/harness/probe-server.mjs` 的收尾登记，本形状里每条响应恰好一条），而**被客户端
///   中途弃掉**的那些响应，其 close **可以晚于 `engine::run` 交回** ⇒ 单次采样的 `written`
///   只是**下界** ⇒ `excess` 少算 ⇒ 下面那条 `excess <= bill` 的额度会被"少记"白让掉。
///   不许裸读一次 `events()` 就求和。（本条已有的 `tightened > 0` 钉的是**引擎自己的计数**，
///   不算量具活体针。）
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
///
/// **修复轮 8F-3b 改成"等账静置"之后的四次复量**（同一形状、同一台对端；⚠ 那四发用的是**已被换掉**
/// 的"两次采样相等"谓词，收口复审 B2 第 2 条之后等待条件改成"close 行数到达那个已知总数"，
/// 下面这组数不是新形状下的读数 —— 留着只因为它们是"额度不是靠少记让出来的"这句目前唯一的实测出处）：
/// `written` 依次 12,085,786 / 11,345,952 / 11,095,488 / 11,325,488，
/// `excess` 依次 3,697,178 / 2,957,344 / 2,706,880 / 2,936,880，
/// `bill` 依次 7,077,888 / 6,291,456 / 5,505,024 / 6,291,456 ⇒ **最小余量 2,798,144 字节**。
/// 复审 B-I-1 估的"被少记白让掉的额度约 2.6 MiB"落在这一格上 ⇒ 静置之后那条 `excess <= bill`
/// 仍然过，额度不是靠少记让出来的。四次里 `Report.duplicated` 与 `excess` 都**逐字节相等**
/// （8F-1 记的那一次两者差 326）—— 这只是一次观察，本用例**不断**它。
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
        },
    )?;
    assert!(rep.ok, "引擎没跑完：{:?}", rep.message);
    assert_eq!(rep.delivered, len, "收尾时盘上真有这么多字节");

    let tightened = rep.tightened().len() as u64;
    assert!(tightened > 0, "这个形状本来就要收紧好几次；一次都没有 ⇒ 账单这条用例什么都没测（这一条钉的是**引擎自己的计数**，不是量具活体针）");

    // **量具活体针**（评审 C-I-1① 起手，**复审 B-I-1 把它的形状修对**）：日志落盘晚于响应收尾，
    // 裸读一次 `events()` 会在"一条 `conn_close_side` 都没记上"的时候给出 written = 0 ⇒
    // `excess = 0` ⇒ 下面那条 `excess <= bill` **当场恒真**。
    //
    // **收口复审 B2 第 2 条：等的东西换成"这个总数到了"，不再换"两次采样相等"。**
    // 期望总数这条用例本来就推得出来，而且就写在下面那句等式里：每条响应恰好一条
    // `conn_close_side` ⇒ close 行数 = 引擎报的枪数 + 试探那一枪（T8-R2）。原先的谓词是
    // `*prev.borrow() == now`（两次采样相等）加 `sent >= len`，那是个**启发式静置**，两个方向都不牢：
    // ① `POLL = 20 ms`（`tests/harness/mod.rs` 的 `POLL`）对 200 ms 的节奏型对端，两枪之间完全可能
    //    撞到两个相等采样 ⇒ 账还没落全就返回，`written` 只是下界，`excess <= bill` 于是能在真缺陷上
    //    碰巧为真（**假绿**）；② 一条迟到的 close 也能让下面那条条数等式红（**假红**）。
    // ⚠ 措辞纪律（控制员在 `cced137` 连跑 5 次全绿）：**这一发不是在修一条会红的用例**，
    // 它改的是判据的形状 —— 假绿那截残留被去掉（返回时每一枪的 close 都已经在账里）、
    // 对迟到 close 结构免疫（谓词等的就是那个总数，不是"看起来不动了"）、
    // 并且把一次没有对应断言的启发式等待换成"本用例自己已经断言的那个数字"。
    // 谓词只保证**不漏读**（下界）；"不许多"那一半仍然由下面两条等式自己钉 ——
    // 这正是 `Server::requests_quiet` 的文档定的分工，只不过这里"上界"是断言而不是等待条件。
    let expected_closes = rep.dispatched().len() + 1;
    let evs = srv.events_while(|evs| {
        let (lines, sent) = close_side_totals(evs);
        lines >= expected_closes && sent >= len
    })?;
    let (close_lines, written) = close_side_totals(&evs);
    let request_lines = evs.iter().filter(|e| e["ev"] == "request").count();
    // ① **close 行数 == request 行数**：两边都是对端自己日志里的行数，不问引擎的嘴 ⇒ 这一条
    //    红得起来（复审 B-I-1②：原先这里断的是 `close_lines > 0`，是谓词里那个量的复读，恒真）。
    //    `/sz/<n>s` 上的 GET + Range 每一条响应都走到 `probe-server.mjs` 的收尾登记那一路
    //    （`/noranges`、416、HEAD 三支在登记之前就 `return` 了 ⇒ 等式只在**本形状**上成立，
    //    这条用例用的正是本形状）。
    assert_eq!(
        close_lines, request_lines,
        "量具没活着：`conn_close_side` 记了 {close_lines} 行而 `request` 有 {request_lines} 行 ⇒ \
         有响应到交回这一刻还没收尾，written = {written} 只是下界，\
         excess = written - len 量的就不是'多打'而是'少记'"
    );
    // ② **close 行数 == 引擎报的枪数 + 试探那一枪**：与①联立 ⇒ 引擎多打一枪而没记进 trace
    //    （重试、或 trace 漏记）时这一条当场红，①那条只审子集的漏洞因此堵上了。
    //    右边就是上面谓词等的那个 `expected_closes`（同一个数、两处各钉一次）：谓词负责"等到它"，
    //    这一句负责"不多于它" ⇒ 等待条件与判据同源，不存在"等的是一个数、断的是另一个数"的缝。
    assert_eq!(
        close_lines,
        expected_closes,
        "每条响应恰好一条 `conn_close_side` ⇒ close 行数必须等于引擎报的枪数 + 试探那一枪（T8-R2）：\
         close {close_lines} 行 / 线上 {request_lines} 条请求 / 引擎报 {} 枪",
        rep.dispatched().len()
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
//   `Server::requests_at_least()/ranges_at_least()`（复审 B-Minor 4：裸读一次的那条因为没有调用者
//   已被删掉，抽取本身收进 `harness::ranges_of`；"不该有更多条"那一方向走
//   `Server::requests_within()/requests_quiet()`）、`ScriptedPeer::shot_ranges()`、盘上文件与断点文件这些
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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
    // **复审 B-I-4**：守卫数出自**被测自己的**账 ⇒ 只 wait 不断等式 = 只审子集。
    // **收口复审 B2 第 5 条**：光有那条等式也**审不了"多读"** —— `requests_at_least` 一到 `n`
    // 就返回，第 `n+1` 条若稍后才落盘就永远看不见（旧注释"三处都钉平，这个守卫数才可信"里
    // 隐含的那句"多一条会红"当时是吹的）。所以这里也换成 `Server::requests_quiet` 文档钦定的
    // 成对用法：`requests_at_least(k)` 钉**不漏读**，紧接 `requests_quiet(wait)` **守满**一段时间
    // 钉**不多读**，下面那句等式断的是静置窗口结束时的全部请求行。
    // ⚠ 观察窗仍是 `wait` 那么长，不是无穷长；这一格之所以够用，是因为 `engine::run` 已经交回、
    // 所有 worker 线程都已 `join` ⇒ 交回之后不会再有新请求，剩下的只有 node 侧异步写日志的延迟。
    let probe_range = format!("bytes=0-{}", PROBE_READ_BYTES - 1);
    let expected_lines = rep.dispatched().len() + 1;
    srv.requests_at_least(expected_lines)?;
    let requests = srv.requests_quiet(Duration::from_millis(300))?;
    assert_eq!(
        requests.len(),
        expected_lines,
        "线上条数与引擎报的枪数不是一对一（T8-R2）：静置窗口结束时线上有 {} 条请求，\
         引擎报 {} 枪 + 试探那一枪 = {} ⇒ 条数没钉平之前，链接二审的只是子集",
        requests.len(),
        rep.dispatched().len(),
        expected_lines,
    );
    let ranges = harness::ranges_of(&requests);
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
        // 9A（勘误 E21）新增的字段：同上，只补结构体字面量，不改任何判据。
        on_tick: None,
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
        // 9A（勘误 E21）新增的字段：同上，只补结构体字面量，不改任何判据。
        on_tick: None,
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
/// **装得进一个窗口**（`WINDOW_BYTES` = 4 MiB）⇒ 那一支永远只需要一枪，"接缝首尾相接、彼此不重叠"
/// 这条判据在它上面是**空的**（只剩试探与 body 那一枪的接缝）；同一类"仪器没有这个对端"的事 E5 已经记过一次。所以：
/// - **A 段（node `/noranges`）**：钉"结果字节正确"（`expected_sha`，与真 socket 对过的 oracle）
///   + 钉"四个 worker 都报了就绪，线上却只多出一条 Range" + 钉"第一枪问的是**起点最低**的那段缝"。
/// - **B 段（进程内退化对端，5 MiB > 一个窗口）**：钉"试探那一截之后 body 那一枪**接着往下**问到 EOF、
///   两条 Range 首尾相接、互不重叠"，也就是派发点名的**从线上读并发=1**（不是引擎内部的计数器）。
///   ⚠ **E16 R-B2 之后这一段的枪数是 2（试探 + 一枪）**，原先是 3 —— 见下面 B 段那段注释：
///   多出来的那一枪就是"按窗重请求"的平方项本身，裁定明令不许有，所以这里跟着改（方向是收紧）。
///   "多枪时按起点升序"这个观测量随缺陷一起消失，它的归属现在在 A 段那条线上判据 +
///   `src/scheduler.rs` 的 `degraded_dispatch_is_start_ascending` 两处。
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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

    // —— B 段：5 MiB 的退化对端（**E16 R-B2 之后：一枪下完，共 2 条请求**）——
    // 这一段的期望条数**被 E16 改过**（原写的是"一枪装不下 ⇒ 必然两枪 = 3 条"）。那不是放宽，是
    // **收紧**：3 条里多出来的那一枪付的是"从文件头到那里"的整段前缀重取（平方项），裁定明令不许有。
    // 方向自查：`spans.len()` 从 3 改成 2 之后，这条用例只会对"多打一枪"更敏感，没有任何一格变得
    // 更容易通过；`a_degraded_peer_is_fetched_in_one_body_shot_not_one_per_window` 是同一件事的
    // 三窗口版本（带形状守卫），两条一起钉住"退化 = 一枪"。
    //
    // 原来这一格还能顺带比出"多枪时按起点升序"（`Shot::IgnoresRange` 的 body 会一枪读完 ⇒ 修好后
    // 线上不再出现第二枪）。**那个观测量随缺陷一起消失了**，它的归属现在在两处（都在，没有漏）：
    // ① 本用例 A 段那条"第一枪必须是起点最低的那段缝"（线上取证）；
    // ② `src/scheduler.rs` 的 `degraded_dispatch_is_start_ascending`（单元层，派发表本身）。
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
        },
    )?;
    assert!(rep2.ok, "退化形状（进程内对端）也该跑得完：{:?}", rep2.message);
    let spans = harness::wire_spans(
        &peer.shot_ranges().into_iter().map(Some).collect::<Vec<_>>(),
        big,
    )?;
    assert_eq!(spans.len(), 2, "试探 + **一枪**下完剩余整条 body = 2 条（E16 R-B2），实际 {spans:?}");
    assert_eq!(spans[0], (0, PROBE_READ_BYTES - 1), "第一枪是试探");
    // 线上并发=1 的影子：这两条 Range 严格按文件顺序、首尾相接、彼此不重叠
    // （试探读过的那一截之后，body 那一枪必须**接上**而不是**回头**）。
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
    assert_eq!(spans[1].1, big - 1, "那一枪该正好够到 EOF：{spans:?}");
    assert!(rep2.tightened().is_empty(), "退化模式不劈分：{:?}", rep2.tightened());
    assert_eq!(harness::sha256_of(&t2.target_path())?, want, "一枪下完的文件也要一字不差");
    let _ = std::fs::remove_file(&want_file);
    Ok(())
}

/// **E16 / 裁定 R-B2 本体（退化模式不许按窗重请求）：对端不认 Range 时，剩余整条 body 必须一枪下完。**
///
/// ## 为什么这条今天会红
/// `src/fetch.rs` 的读上界无条件夹成一个窗口（`stop = min(iv.end + 1, iv.start + WINDOW_BYTES)`），
/// 而 `degraded`（发了 `Range` 却答 `200`）那一支先**流掉 `iv.start` 个字节**再落盘。两条叠起来：
/// 区间每往前挪一个窗口，就重新要一遍"从文件头到那里"的 body，读完窗口就中途弃响应。
/// 本用例把剩余体量放到**三个窗口宽**，于是那一支现状要打 **3 枪**（外加试探 = 4 条请求），
/// 而裁定要的是 **1 枪**（共 2 条）。
///
/// ## 仪器为什么用进程内 `ScriptedPeer` 而不是 node
/// node 唯一那条退化路由 `/noranges` 把 body 夹在 `min(sendLen, 1 MiB)`（`tests/harness/probe-server.mjs`），
/// 1 MiB **装得进一个窗口** ⇒ 平方项根本不出现（8F-1 报的就是这件事）。`Shot::IgnoresRange` 是同一形状
/// 但**没有那 1 MiB 封顶**：`200` + 诚实 `Content-Length` + 不发 `Accept-Ranges` ⇒
/// `probe::peer_accepts_ranges` 读成 `accept_ranges = false`、`total_len` = 整条长度，
/// 引擎于是走进退化支，而退化支今天按窗切 —— 这正是 E16 要点名的那一格。
///
/// ## 判据落在**对端自己记的东西**上（spec §7.2 / E13③：不许问引擎的嘴）
/// 读数来自 `ScriptedPeer::shot_ranges()`。它**不是**裸读一次线上日志：那台对端把每一枪的记录
/// `push` 在**写出响应之前**（`tests/harness/mod.rs` 的 `spawn_scripted_peer`，`arrival_gap` 的文档
/// 就是据此说"不需要守株待兔"），而 `engine::run` 交回时所有 worker 线程已经 `join` ⇒ 记到的条数
/// 就是**终局**，不存在"还没落盘的尾巴"。失败消息里把数到的 `Range` 原样打出来。
///
/// ## 三条判据各自抓什么
/// 1. **枪数**：全程只许 2 条请求（试探 + 一枪 body）。⇒ 抓"按窗重请求"本身（现状红在这里：4 条）。
/// 2. **线上区间**：那一枪问的必须是 `[试探之后的最低缝, EOF]`，而且是**开放终点**写法。
///    两半各自有针（**复审 B-Minor 3 / B-I-5**：原先只有 `wire_spans` 那条闭区间等式，而它把
///    `bytes=236032-` 与 `bytes=236032-12818943` **补成同一个闭区间** ⇒ 闭写法同样绿 ⇒
///    "开放终点"那一轴当时**没有针**）。现在先断**原文串**（`bytes=<起点>-`，没有终点数字），
///    再断补齐后的闭区间等式。⇒ 抓"问得比一枪窄"与"写到 EOF 还发显式终点"两种退化。
/// 3. **内容**：整份文件的 sha256 与独立读者算出的期望相等，另加一段**块号取证**
///    （`require_block_alignment`，起点 4096 对齐 —— 只由那一枪写的尾部区段）。
///    ⇒ 抓"少丢前缀就写错位置"：一枪下完意味着偏移 `iv.start` 之前那批字节仍然要**流式丢掉**，
///    把它写成"整个 body 从 0 落盘"会立刻红在这里，而不是红在枪数上。
///
/// 第 0 条是**形状守卫**（不是判据）：剩余体量必须 > 2 个窗口，否则这条用例退化成
/// "1 MiB / 一个窗口"那种恒真形状（本项目已踩过三次恒绿夹具）。
/// **复审 B-Minor 2 的更正（收口复审 B2 第 1 条）**：上一轮把 `3` 提成 `REMAINING_WINDOWS`，
/// 但守卫句仍写成 `len - PROBE_READ_BYTES > 2 * WINDOW_BYTES` —— 把上一行的构造代进来就是
/// `REMAINING_WINDOWS * WINDOW_BYTES > 2 * WINDOW_BYTES`，即 `3 > 2`：**它断的是构造句本身**，
/// 恒真，且构造句与守卫句仍然同形（上一轮文档吹的那两句当时都是假的）。
/// 现在守卫**只断那个常量**，而且收进 `const` ⇒ 有人把构造改窄（剩余体量 ≤ 2 个窗口 ⇒
/// 按窗切最多两枪，那一支的平方项不再是这条用例能分辨的形状）时，**编译期就红**，
/// 轮不到判据 1 去猜，也轮不到"跑哪个测试"来决定它有没有被走过。
#[test]
fn a_degraded_peer_is_fetched_in_one_body_shot_not_one_per_window() -> anyhow::Result<()> {
    // 试探那一截 + 整整三个窗口 ⇒ 退化支按窗切必然 ≥ 3 枪。
    const REMAINING_WINDOWS: u64 = 3;
    // 守卫断的是**那个常量**（不是由它算出的 `len`）—— 写成 `len - PROBE_READ_BYTES > 2 * WINDOW_BYTES`
    // 就是把构造句代回自己，恒真。`clippy::assertions_on_constants` 不许运行时形状的这个断言
    // （"常量断言该在编译期"），照它的建议收进 `const` ⇒ 改窄常量时**编译就红**，
    // 比运行时红更早，而且没有任何 filter/跳过能让它不出声。
    const _: () = assert!(
        REMAINING_WINDOWS > 2,
        "形状守卫：剩余体量只有这个窗口数 ⇒ 不满足「> 2 个窗口」这条前提，\
         按窗重请求与一枪下完的差额会小到判据 1 分辨不出（本用例的期望枪数 2 也就不成立）。\
         要改就同时改判据 1 与文档，别把这条守卫放宽成恒真。"
    );
    let len = PROBE_READ_BYTES + REMAINING_WINDOWS * WINDOW_BYTES;
    let peer = harness::spawn_scripted_peer(len, vec![harness::Shot::IgnoresRange], harness::Shot::IgnoresRange)?;
    let dir = harness::tmp_dir("e16-degraded-one-shot")?;
    let (want, want_file) = sha_of_expected("e16-degraded-expected.bin", &harness::positioned_body(len as usize))?;
    let t = Task {
        url: format!("{}/blob", peer.base()),
        dir: dir.path().to_path_buf(),
        name: Some("degraded-one-shot.bin".to_string()),
        ..harness::blank_task()
    };
    // 四个 worker 都报就绪，但退化规则保证一次只有一枪在途（`no_ranges_falls_back_…` 钉的那条），
    // 所以枪数 excess 只能来自"按窗重请求"，不能来自并发。
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(want.clone()),
            force_fresh: false,
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
        },
    )?;
    assert!(rep.ok, "退化形状（三个窗口的整条 body）也该跑得完：{:?}", rep.message);

    // —— 判据 1：枪数。对端记下的请求条数（试探 + body）。
    let ranges = peer.shot_ranges();
    assert_eq!(
        ranges.len(),
        2,
        "E16：对端不认 Range 时剩余整条 body 必须**一枪**下完 ⇒ 全程只许 2 条请求（试探 + 1 枪），\
         实际 {} 条：{ranges:?} —— 每多一枪就重新要一遍「从文件头到那里」的 body，那一支的流量是平方级的\
         （推出来的算术，见 E16 正文）",
        ranges.len()
    );

    // —— 判据 2：线上区间（含"那一枪问到 EOF、写法是开放终点"）。
    let spans = harness::wire_spans(&ranges.iter().cloned().map(Some).collect::<Vec<_>>(), len)?;
    assert_eq!(spans[0], (0, PROBE_READ_BYTES - 1), "第一条必须是试探那一枪：{spans:?}");
    // **原文串**那一半（复审 B-I-5：`wire_spans` 把开放终点按 `total_len` 补齐 ⇒ 只看补齐后的
    // 闭区间，`bytes=236032-12818943` 这种显式终点写法**同样绿** ⇒ "开放终点"那一轴没有针）：
    // 这里比的是对端记下来的那串字符本身，不经过任何补齐。
    assert_eq!(
        ranges[1],
        format!("bytes={PROBE_READ_BYTES}-"),
        "E14② 那一轴：窗口够到 EOF 时那一枪必须写成**开放终点**（`bytes=<起点>-`，不带终点数字）；\
         等式左边就是对端记到的那一条 —— 写成显式终点在闭区间读法上与开放写法一模一样，\
         所以这一句是那条形状等式唯一的牙"
    );
    assert_eq!(
        spans[1],
        (PROBE_READ_BYTES, len - 1),
        "body 那一枪必须从「试探已确认字节之后」一直问到 EOF：{spans:?}"
    );

    // —— 判据 3：内容。整份散列（独立读者算的期望）+ 块号取证（起点 4096 对齐的尾部区段）。
    let path = t.target_path();
    assert_eq!(harness::sha256_of(&path)?, want, "一枪下完的 body 也要一字不差");
    // 从 3 MiB 到文件尾这一段（`len - tail_start` = 9,673,216 个字节）只有"那一枪"写过：
    // 起点取 3 MiB 处（4096 的整数倍，取证工具的硬要求）。
    // 复审 B-Minor 1：这一句原先写"尾部这 236,032 个字节"，那是 `PROBE_READ_BYTES`（试探那一枪的量），
    // 与这里检查的区段无关 ⇒ 数错了。
    let tail_start = 3 * 1024 * 1024u64;
    harness::require_block_alignment(&path, tail_start, len - tail_start)?;
    let _ = std::fs::remove_file(&want_file);
    Ok(())
}

/// **E16②（义务二）在新形状上的那一格 + "整条那一支真的读得过窗口边界"**。
///
/// 两件事，各由一臂钉：
/// - **`ToEof` 被干净 EOF 收线时不许报 `Complete`**：body 比区间短 ⇒ 只能 `Advanced { bytes }`
///   （`SegmentComplete` 不看盘 ⇒ 报成完成就是把尾巴**永远封存**，E12③ / E7）。
///   变异 **M6**（把 `fetch.rs` 的 `eof` 那一支折成 `Complete`）红在这一臂 —— 这一发**实测过**：
///   红的是本条与 `a_shorter_body_than_the_interval_is_never_reported_complete`、
///   `a_live_but_paced_peer_is_advanced_rather_than_misread_as_stalled` 三条。
/// - **同一台对端、同一个区间，两种形状交回的字节数不同**：`/sz/<5 MiB>` 只肯给 5 MiB，
///   `Windowed` 拿到 4 MiB（一个窗口）而 `ToEof` 拿到 5 MiB（读到对端收线）。
///   ⚠ **这一臂能看见的分辨率要说清**（M7 实测）：把 `plan_shot` 里 `Windowed` 那一支的**窗口夹**
///   摘掉，这一臂**不红** —— 因为 `range_header` 问的还是窗口那一格，诚实的对端**不会多发我们没问的
///   字节**，于是"读到哪儿"的上界被"问到哪儿"遮住了，线上交回的还是 4 MiB。
///   ⇒ 真正能看见 `stop` 那条夹法的只有 `src/fetch.rs` 的单元表
///   （`the_body_shape_decides_the_range_the_read_bound_and_the_budget_together`，M7 在全树**只**红它一条）。
///   这一臂的价值因此不在"拦夹法被摘"，而在**两形状的对照**：`ToEof` 必须真的比 `Windowed` 多拿到
///   那 1 MiB（M1 把 `ToEof` 夹回窗口时红在臂一，不在臂二）。
///
/// **为什么挂 node 的 `/sz/<n>` 而不是退化对端**：本发要单独拎出"形状"这一个变量。
/// `degraded`（我们发了 Range 而它答 200）与 `ToEof`（读到 `iv.end + 1`）在 `fetch` 里是两处**独立**
/// 判据，用一台**认 Range** 的对端配 `ToEof` 正好只动形状。而"退化 + 中途收线"这个组合在进程内
/// 那台对端上造不出来（`Shot::IgnoresRange` 总把整条 blob 写完），要它得给它加一个新的 `Shot` 变体
/// —— 夹具活，登记在 8F-2 报告的"没做的那一格"里。
/// ⚠ 原先这一格打算用 `/drop/<n>`，实测**不对**：那一支把连接**拆掉** ⇒ 走的是 `read_err` 分支而不是
/// `eof` 分支，M6 在它上面**不红**（`/drop/65536` 那一枪交回的 65,536 字节由
/// `a_body_cut_off_midway_keeps_the_bytes_it_actually_wrote` 钉，两条分工不同）。
#[test]
fn a_whole_body_shot_that_ends_at_a_clean_eof_is_advanced_and_reads_past_the_window() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let give = 5 * 1024 * 1024u64; // 对端只肯交 5 MiB：比一个窗口宽、比三个窗口窄
    let iv = Interval { start: 0, end: 3 * WINDOW_BYTES - 1 };
    let total = 48 * 1024 * 1024;

    // —— 臂一：`ToEof` 读到对端收线为止 ⇒ 交回**整条 body**，且绝不报 `Complete`。
    let (store, path) = harness::store("e16-toeof-eof.bin")?;
    let t = Task { url: format!("{base}/sz/{give}"), ..harness::blank_task() };
    let out = fetch::run_window(
        &harness::client()?,
        &t,
        iv,
        &store,
        10_000_000,
        total,
        &harness::uncapped(),
        fetch::BodyShape::ToEof,
    )?;
    assert_eq!(
        out,
        Outcome::Advanced { bytes: give },
        "E16②：body 比整条区间短 ⇒ 交回真落盘的字节数，**不许**报成 `Complete`（那会把尾巴永远封存）：{out:?}"
    );
    assert_eq!(store.len()?, give, "`ToEof` 该一路读到对端收线：交回的就是那 {give} 个字节");
    harness::require_block_alignment(&path, 0, WINDOW_BYTES)?;
    drop(store);
    std::fs::remove_file(&path).context("清测试文件")?;

    // —— 臂二：同一区间、同一对端，`Windowed` 只能拿到**一个窗口**（形状真作用在 `stop` 上）。
    let (store_w, path_w) = harness::store("e16-windowed-eof.bin")?;
    let out_w = fetch::run_window(
        &harness::client()?,
        &t,
        iv,
        &store_w,
        10_000_000,
        total,
        &harness::uncapped(),
        fetch::BodyShape::Windowed,
    )?;
    assert_eq!(
        out_w,
        Outcome::Advanced { bytes: WINDOW_BYTES },
        "窗口那一支必须**在窗口处自己收手**（对端还有 1 MiB 没交完也不读）：{out_w:?}"
    );
    assert_eq!(store_w.len()?, WINDOW_BYTES);
    drop(store_w);
    std::fs::remove_file(&path_w).context("清测试文件")?;
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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
        // 9A（勘误 E21）新增的字段：同上，只补结构体字面量，不改任何判据。
        on_tick: None,
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
        // 9A（勘误 E21）新增的字段：同上，只补结构体字面量，不改任何判据。
        on_tick: None,
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
        },
    )?;
    assert!(rep.ok, "POST 那一枪该把这份文档整份拿回来：{:?}", rep.message);

    // **复审 B-Minor 6**：这一格原先的形状是"守 1 条落盘 → 裸读一次 → 断 `== 1`"——
    // 那条纪律（`Server::requests_within` 的文档）只管"有没有**任何**一条"，管不到"**不许更多**"
    // 那一方向：第二条枪若在路上，裸读一次看不见它，上界断言就会碰巧为真。
    // 现在两步各钉一边：`requests_at_least(1)` 保证不漏读（否则上界会在"还没落盘"上红成假故障），
    // `requests_quiet` **守满**一段时间保证不多读。
    srv.requests_at_least(1)?;
    let requests = srv.requests_quiet(Duration::from_millis(300))?;
    assert_eq!(requests.len(), 1, "带体的形状全程只许有一发请求（试探 + POST = 2 条就是缺陷）：{requests:?}");
    // **收口复审 B2 第 6 条**：这一句原先读的是 `Server::methods()` —— 那个入口裸读一次日志、
    // 不带任何条数守卫，是 `ranges_at_least` 头顶那句"一律带条数守卫"当时**唯一**的反例。
    // 守卫数取上面那条等式刚钉死的 `requests.len()`：等式先保证"至少这么多条已经落盘"，
    // `methods_at_least` 再按同一个数守卫一次读盘 ⇒ 少读不可能，多读由下面那句等式接住。
    assert_eq!(
        srv.methods_at_least(requests.len())?,
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
            // 9A（勘误 E21）新增的字段：集成用例一律不挂观察者（`on_tick` 的形状由 `src/engine.rs`
            // 的 `mod tests` 钉）。这里只是补齐结构体字面量，任何既有断言一个字都没动。
            on_tick: None,
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
    // 先等到"至少有一条请求落了盘"再数总数：不等日志的那次裸读会让"恰好一条"这种
    // 上限断言在缺行时读起来像成立（本项目的老规矩：负向/计数判据都要先守一次落盘时机）。
    // **复审 B-Minor 6**：反过来，"不许有第二条"那一方向裸读一次也看不见在路上的那条枪 ⇒
    // 上面那一步只保证不漏读，下面这一步 `requests_quiet` **守满**一段时间保证不多读。
    srv.requests_at_least(1)?;
    let requests = srv.requests_quiet(Duration::from_millis(300))?;
    assert_eq!(requests.len(), 1, "只许有试探那一枪（判完就拒）：{requests:?}");
    assert_eq!(requests[0]["method"], "GET", "那一枪是试探：{requests:?}");
    assert_eq!(
        dlm::state::load(&state_path)?.context("断点文件读不回来了")?.stats.retries,
        3,
        "断点里那本账必须原样留着"
    );
    Ok(())
}

// ================================================================ Task 9B-2：装配路径上的进度行与 CLI 的退出档
//
// 勘误 **E22 第 5 条 / E24 第 5 条**：`src/engine.rs` 的 `mod tests` 里那些手搭 `Engine` 够不着
// `engine::run` 这条装配路径（试探 → 种子 → worker 线程 → 断点保存），而 9B-1 被禁止建 `report.rs`，
// 于是"挂上 sink 之后到底有没有收到过 tick、收到的那八格是不是可读的数"今天必须由这几发用例交。
// 第一发是**进程内**（真 `engine::run` + node 对端 + 真渲染器），后五发是**真二进制**
// （`CARGO_BIN_EXE_dlm`）—— 因为 `--help` 的退出码、bit0 的短路、`--quiet` 到底挂不挂 sink 这三件事
// 都住在 `main.rs`，进程内没有任何代码路径能观察到它们（`main` 不是库的一部分）。
// 9B-3 又补两发（**E26/C3** 参数层的进程级码与措辞、**E26/C5** `--quiet` + `--no-start` 那一叠），
// 它们同样只能在这个面上钉：一个读的是 `main::parameter_error` 交的码，另一个读的是
// `main::dispatch` 里"短路在两个位都读完之后"那个位置。
//
// 判据方向：**`Report` 始终是权威**，`on_tick` 只是"这一拍的样子"（E21⑦⑧）⇒ 这几发都不断 tick 的条数，
// 只断"至少收到过一发"与"终值那几格与这次下载对得上"。

/// 一格"一位小数的数"的形状（渲染口径 **E25①②**：单位只有 MiB 一档、百分比一位小数）。
fn is_one_decimal(s: &str) -> bool {
    match s.split_once('.') {
        Some((whole, frac)) => {
            !whole.is_empty()
                && whole.chars().all(|c| c.is_ascii_digit())
                && frac.len() == 1
                && frac.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// 把一行进度拆成格子。渲染器**格间用两个空格、格内最多一个空格**（`12.0 MiB/48.0 MiB`），
/// 所以按 `"  "` 切是无歧义的 —— 而这个"无歧义"本身也是被断言的形状之一。
fn tick_cells(line: &str) -> Vec<&str> {
    line.split("  ").collect()
}

/// 起真二进制。存在的理由：`--help` 的退出码（E24②）、bit0 的短路（E20④）、`--quiet` 不挂 sink
/// （E24①）这三条只住在 `main.rs`，进程内观察不到。
fn run_dlm(args: &[&str]) -> anyhow::Result<std::process::Output> {
    let bin = env!("CARGO_BIN_EXE_dlm");
    std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("起二进制失败：{bin}"))
}

fn stdout_of(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}
fn stderr_of(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// **E22⑤ / E24⑤ 的那一发**：真 `engine::run` + node 对端 + 真渲染器，断"确实收到过 tick，
/// 八格都是可读的数"。sink 里做的事与 `main.rs` 挂的那一发同形（在引擎线程里同步调 `report::tick`）。
#[test]
fn a_real_run_hands_the_renderer_cells_a_human_can_read() -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};
    let (_srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("tick-assembly")?;
    let len = 8 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/sz/{len}"),
        dir: dir.path().to_path_buf(),
        ..harness::blank_task()
    };
    let beats: Arc<Mutex<Vec<(String, dlm::engine::RunSnapshot)>>> = Arc::new(Mutex::new(vec![]));
    let collector = beats.clone();
    let on_tick: dlm::engine::OnTick = Arc::new(move |s: &dlm::engine::RunSnapshot| {
        collector.lock().unwrap().push((dlm::report::tick(s), *s));
    });

    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: 4,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
            on_tick: Some(on_tick),
        },
    )?;

    // 权威仍然是 `Report`（E21⑧）：本用例不断 tick 的条数，只断"至少一发"。
    assert!(rep.ok, "引擎没跑完：{:?}", rep.message);
    assert_eq!((rep.bytes_total, rep.delivered), (len, len), "服务器说的长度与盘上真有的字节");

    let seen = beats.lock().unwrap();
    assert!(
        !seen.is_empty(),
        "挂上 sink 之后一发快照都没收到 —— 装配路径（试探→种子→worker→断点）上没有进度源"
    );
    for (line, snap) in seen.iter() {
        // 快照侧：八格各自落在文档说的范围内（**E21①②③⑤**）。
        assert_eq!(snap.total_len, len, "`total_len` 那格取的是试探到的全长（E21②）：{snap:?}");
        assert!(snap.covered_bytes <= snap.total_len, "覆盖集不可能超过全长：{snap:?}");
        assert_eq!(snap.workers_total, 4, "`workers_total` 就是 cfg.workers：{snap:?}");
        assert!(snap.active_workers <= snap.workers_total, "在途的不可能比总数还多：{snap:?}");
        // 渲染侧：形状必须拆得开，而且每一格都是**可读的数**（不是 `None`、不是 `NaN`、不是空白）。
        let cells = tick_cells(line);
        assert!(
            (4..=6).contains(&cells.len()),
            "一行该是 4~6 格（百分比 / 已下对 / 速率 / 连接数 + 可选 dup + 可选 slow-retry）：{line:?}"
        );
        let pct = cells[0].strip_suffix('%').unwrap_or_else(|| panic!("第一格不是百分比：{line:?}"));
        assert!(is_one_decimal(pct), "百分比要是一位小数的数：{line:?}");
        // `unwrap_or(u64::MAX)`：拆不出整数的话就让**这一格红**，而不是让测试炸在解析错误上。
        let pct_whole = pct.split('.').next().unwrap_or("").parse::<u64>().unwrap_or(u64::MAX);
        assert!(
            pct_whole < 100 || (pct_whole == 100 && pct.ends_with(".0")),
            "文件百分比不许超过 100%（`100.0%` 是满的那一格，E25③）：{line:?}"
        );
        let (done, total) = cells[1]
            .strip_suffix(" MiB")
            .and_then(|s| s.split_once(" MiB/"))
            .unwrap_or_else(|| panic!("第二格该是 `<已下> MiB/<全长> MiB`：{line:?}"));
        assert!(is_one_decimal(done) && is_one_decimal(total), "那对 MiB 都要是一位小数：{line:?}");
        assert_eq!(total, "8.0", "8 MiB 的全长手算就是 `8.0`（E25①）：{line:?}");
        assert!(
            done.split('.').next().unwrap_or("").parse::<u64>().unwrap_or(u64::MAX) <= 8,
            "已下的整数部分不许超过全长（8 MiB）：{line:?}"
        );
        let rate = cells[2]
            .strip_suffix(" MiB/s")
            .unwrap_or_else(|| panic!("第三格是速率：{line:?}"));
        assert!(is_one_decimal(rate), "速率要是一位小数的 MiB/s：{line:?}");
        let (active, workers) = cells[3]
            .strip_prefix('x')
            .and_then(|s| s.split_once('/'))
            .unwrap_or_else(|| panic!("第四格是 `x<在途>/<总数>`：{line:?}"));
        assert!(active.chars().all(|c| c.is_ascii_digit()) && !active.is_empty(), "在途那格是整数：{line:?}");
        assert_eq!(
            workers, "4",
            "分母就是 `workers_total`（它的字段文档写着\"进度行里那个 x4 的分母\"）：{line:?}"
        );
        // 后面两格各自独立出现或不出现（E25④⑤）：一旦出现就必须是那个形状。
        for extra in &cells[4..] {
            if let Some(dup) = extra.strip_prefix("dup ") {
                let p = dup.strip_suffix('%').unwrap_or_else(|| panic!("dup 那格该带百分号：{line:?}"));
                assert!(is_one_decimal(p), "dup 那格要是一位小数的数：{line:?}");
            } else if let Some(rounds) = extra.strip_prefix("slow-retry x") {
                assert!(
                    !rounds.is_empty() && rounds.chars().all(|c| c.is_ascii_digit()),
                    "慢重试那格要给得出轮数：{line:?}"
                );
            } else {
                panic!("第五格之后只许是 `dup X%` 或 `slow-retry xN`：{line:?}");
            }
        }
    }
    // 终值那一发（E21⑦ 的第二处调用点）必须说"下完了"：`ok` ⇒ 调度器没有未归属字节 ⇒
    // `covered_bytes == total_len`（判据①那条减法）⇒ 渲染出来就是手算的两格字面量。
    let (last_line, last) = seen.last().expect("上面已经断过非空");
    assert_eq!(last.covered_bytes, last.total_len, "收尾那一拍的覆盖集应当是满的：{last:?}");
    assert!(last.run_delivered > 0, "这一轮在网线上确实动过字节：{last:?}");
    assert!(
        last_line.starts_with("100.0%  8.0 MiB/8.0 MiB  "),
        "终值那一发的头两格是手算的字面量（8 MiB ⇒ `8.0`、覆盖满 ⇒ `100.0%`）：{last_line:?}"
    );
    Ok(())
}

/// **E24②**：`--help` / `--version` 必须以 0 退出，而用法错误保留 clap 自己的码（2）——
/// E20⑦ 那"两档"说的是**下载结果**，不覆盖参数层。一刀切成 `Err ⇒ 1` 会让 `dlm --help` 以 1 退出。
#[test]
fn the_binary_exits_zero_for_help_and_keeps_clap_code_for_a_usage_error() -> anyhow::Result<()> {
    let help = run_dlm(&["--help"])?;
    assert!(
        help.status.success(),
        "`--help` 的退出码必须是 0（E24②），实测 {:?}，stderr：{}",
        help.status.code(),
        stderr_of(&help)
    );
    let h = stdout_of(&help);
    assert!(h.contains("Usage"), "帮助要真打到 stdout 上：{h}");
    for opt in ["--url", "--out-dir", "--data-file", "--sha256", "--quiet", "--no-start"] {
        assert!(h.contains(opt), "帮助里少了 {opt}（第 15 条长选项 `--no-start` 也在这一面上，E23②）：{h}");
    }
    assert!(run_dlm(&["--version"])?.status.success(), "`--version` 与 `--help` 同一档（E24②）");

    let bad = run_dlm(&["--quiet"])?; // 少了必填的 --url
    assert_eq!(
        bad.status.code(),
        Some(2),
        "用法错误走 clap 自己的码而不是下载那两档：exit={:?}，stderr：{}",
        bad.status.code(),
        stderr_of(&bad)
    );
    assert!(stderr_of(&bad).contains("--url"), "那句报错要指出缺的是 --url：{}", stderr_of(&bad));
    Ok(())
}

/// **E20④ / E24③**：bit0 由 CLI 在进入 `engine::run` **之前**短路 —— 打印摘要、退出码 0、
/// 一个字节都不许多打（对端一条请求都没看到、目标文件没被创建）。摘要里口令这个话题整个不出现。
#[test]
fn the_binary_short_circuits_no_start_without_touching_the_wire() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("bin-no-start")?;
    let out_dir = dir.path().display().to_string();
    let url = format!("{base}/blob");

    let out = run_dlm(&["--url", &url, "--out-dir", &out_dir, "--out-name", "blob.bin", "--no-start"])?;
    assert!(
        out.status.success(),
        "`--no-start` 的语义是\"打印任务摘要后退出\"，不是失败（E20④）：exit={:?}，stderr：{}",
        out.status.code(),
        stderr_of(&out)
    );
    let so = stdout_of(&out);
    assert!(so.contains("任务摘要"), "摘要那一行该说清这是什么：{so}");
    assert!(so.contains(&url), "摘要要逐字段打，URL 是其中一格（E23⑥：不许拿 Debug 当渲染器）：{so}");
    assert!(so.contains("blob.bin"), "输出文件名那一格：{so}");
    for verdict in ["下载已完成", "下载未完成"] {
        assert!(!so.contains(verdict), "短路之后不该有最终结果行（引擎压根没进）：{so}");
    }
    assert!(!dir.path().join("blob.bin").exists(), "目标文件被创建了 —— 摘要之后还动了磁盘");
    // 负向判据按 harness 的纪律下：**守满**一段时间而不是裸读一次日志（日志行落盘晚于客户端拿到
    // 响应，裸读的"一条都没有"会在真故障上碰巧为真）。
    let reqs = srv.requests_within(Duration::from_millis(500))?;
    assert!(reqs.is_empty(), "bit0 说\"一个字节都不许多打\"，对端却看到 {} 条请求：{reqs:?}", reqs.len());

    // 口令那一格连 `[redacted]` 都不许出现（E23⑥/E24③）。走 https 是因为 `http://` + 口令
    // 在解析期就被拒（E3），而 https 那条路在 `--no-start` 下根本不会上网。
    let via_alias = run_dlm(&[
        "--url",
        "https://example.invalid/a.bin",
        "--out-dir",
        &out_dir,
        "--user",
        "alice",
        "--password",
        "hunter2",
        "/a",
    ])?;
    assert!(
        via_alias.status.success(),
        "`/a` 与 `--no-start` 同义（E23②）：stderr：{}",
        stderr_of(&via_alias)
    );
    let so2 = stdout_of(&via_alias);
    assert!(so2.contains("alice"), "用户那一格照打（掩码只属于 `Debug`，摘要不是日志）：{so2}");
    for banned in ["hunter2", "[redacted]", "password", "口令"] {
        assert!(!so2.contains(banned), "摘要里不许出现 {banned}（E24③：口令这个话题整个不在摘要里）：{so2}");
    }
    assert_eq!(
        srv.requests_within(Duration::from_millis(300))?.len(),
        0,
        "第二条 `--no-start` 同样不许发请求"
    );
    Ok(())
}

/// **E24① / E20⑦**：`--quiet` 的落点是"一次都不挂 sink"（不是"挂了但里面不打印"），
/// 最终结果照打；非零退出码的**唯一**判据是 `!ok`；`trace` 一个字都不进 stdout。
#[test]
fn the_binary_maps_ok_to_the_exit_code_and_quiet_to_never_installing_the_sink() -> anyhow::Result<()> {
    let (_srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("bin-exit")?;
    let out_dir = dir.path().display().to_string();
    let len = 64 * 1024u64;
    let url = format!("{base}/sz/{len}");
    let good_sha = harness::expected_sha(len);

    // ① 成功 + 非静默 ⇒ 退出码 0，stdout 里既有进度行（`\r`）也有最终结果。
    let ok = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "ok.bin",
        "--connections", "2", "--sha256", &good_sha,
    ])?;
    let sok = stdout_of(&ok);
    assert!(
        ok.status.success(),
        "下完了还非零就是撒谎：exit={:?}，stderr：{}",
        ok.status.code(),
        stderr_of(&ok)
    );
    assert!(sok.contains('\r'), "非静默必须挂上 sink 并打出那一行进度（E24①）：{sok:?}");
    // **E28⑧-3 补的那一格**（本轮追加，上面那几行一字未动）：不只"有 `\r`"，而是**整个 stdout 以进度行
    // 开头** —— `tick_sink` 把 `\r` 放在最前（先回行首再写这一行），所以第一发就写在第 0 列。
    assert!(sok.starts_with('\r'), "非静默那一趟的 stdout **以进度行开头**（`\r` 打在最前，E25 的分工）：{sok:?}");
    assert!(sok.contains("下载已完成"), "最终结果行照打：{sok}");
    assert!(sok.contains("这个断点文件从建立起一共重试"), "那两格是跨断点文件的账（E20⑧/E25⑥）：{sok}");
    assert!(sok.contains("归属之外可能被重下"), "`duplicated` 不是账单，措辞不许漂（E14③）：{sok}");
    // **E28⑧-3 的另一半 + E28①(1) 的对照**（本轮追加）：可分段那一路**收尾那一发换行还在** ——
    // 进度行是 `\r` 原地覆盖的，结果行必须由那一发换行让它独占一行，而不是盖在残行后面，
    // 也不是被两个换行抬成一空行（那正是带体那一路今天犯的错的反面，见
    // `the_data_file_route_downloads_everything_on_one_shot_without_a_single_tick` 的第一臂）。
    assert!(sok.contains("\n下载已完成"), "结果行前面必须有且只有**一个**让行换行：{sok:?}");
    assert!(!sok.contains("\n\n下载已完成"), "让行只该出现一次；出现空行说明有人多打了一发：{sok:?}");
    for trace_word in ["Seeded", "Dispatched", "TraceStep", "Interval"] {
        assert!(!sok.contains(trace_word), "`trace` 一个字都不许进 stdout（E20⑨）：{trace_word} 出现了：{sok}");
    }

    // ② 同一件事走 `--quiet` ⇒ **一个 `\r` 都不许有**（sink 一次都没挂），而结果照打。
    let quiet = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "quiet.bin",
        "--connections", "2", "--quiet",
    ])?;
    let sq = stdout_of(&quiet);
    assert!(quiet.status.success(), "静默不等于失败：stderr：{}", stderr_of(&quiet));
    assert!(!sq.contains('\r'), "`--quiet` 的落点是\"一次都不挂 sink\"（E24①），实测 stdout：{sq:?}");
    assert!(sq.contains("下载已完成"), "静默关的是刷新的那一行，不是结果（E24①）：{sq}");
    // **E28⑧-3 补的那一格**（本轮追加）：静默那一支的 stdout **第一格就是结果行** ——
    // 既不是空行（收尾那发换行跟着 sink 一起没挂），也不是被 `\r` 开头的进度行顶到前面去。
    // 前一句挡住"多一个换行"那一族回归，后一句挡住"少一句话"那一族 ⇒ 两句不是同一条判据。
    assert!(!sq.starts_with('\n'), "静默那一支不许以换行开头（没有进度行就没有让行那一发）：{sq:?}");
    assert!(sq.starts_with("下载已完成"), "静默那一支的 stdout 第一个字符就是结果行：{sq:?}");

    // ③ 下完了但校验不符 ⇒ `ok = false` ⇒ 退出码 1，`message` 那句原样打（裁定 9：不再截断不拼接）。
    let wrong_sha = "0".repeat(64);
    let bad = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "bad.bin",
        "--connections", "2", "--sha256", &wrong_sha,
    ])?;
    let sbad = stdout_of(&bad);
    assert_eq!(
        bad.status.code(),
        Some(1),
        "非零档的唯一判据是 `!ok`（E20⑦）：exit={:?}",
        bad.status.code()
    );
    assert!(sbad.contains("下载未完成"), "结论那一句要跟着 `ok` 走：{sbad}");
    assert!(sbad.contains("sha256 不符"), "引擎那句人话要原样到达用户：{sbad}");

    // ④ 引擎侧 `Err`（断点文件损坏，`run` 根本跑不起来）⇒ 也是非零，且那句原因进 stderr。
    let corrupt_target = dir.path().join("corrupt.bin");
    let corrupt_state = dlm::state::path_for(&corrupt_target);
    std::fs::write(&corrupt_state, "{ 这不是 JSON").context("铺坏断点文件失败")?;
    let err = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "corrupt.bin", "--connections", "2",
    ])?;
    assert_ne!(
        err.status.code(),
        Some(0),
        "`Err` 那一支也必须非零（E20⑦）：exit={:?}",
        err.status.code()
    );
    let se = stderr_of(&err);
    assert!(se.contains("跑不起来"), "那支的措辞要说\"跑不起来\"，与 `ok = false` 分得开：{se}");
    assert!(!se.contains("Seeded"), "错误路径上也不许漏 trace（E20⑨）：{se}");
    Ok(())
}

/// **E26/C3**：形状写错的参数停在**参数层**。真二进制这一支必须退 `BAD_ARGS`（2）并说"参数写错了"，
/// 既不走进引擎换回一句校验结论（**E20⑨**：那是把"你话没说对"报成"东西下坏了"），也不是 clap
/// 的用法块。三者里有两个**故意同码**（E26/C3 追认 `BAD_ARGS = 2` 的理由就是"脚本读到的
/// '话没说对'只该有一种码"）⇒ 码分不开来源，**措辞**是唯一的分档证据，所以这里把三个来源各钉一次：
/// 我们的必须在、引擎的必须不在、clap 的必须不在。
///
/// 为什么这一发要放在**进程级**：今天这一格只被 `main.rs::tests` 那发手搭 `anyhow!` 的函数级用例钉住，
/// 它测的是"`parameter_error` 拿到一句话之后交回哪个码"，**测不到**"`check_boundaries` 那句
/// 到底是不是走到这里的那一句"，也测不到那一趟有没有上网 ⇒ 两头都可能在装配处断掉而无人可见
/// （`--lib` 200 全绿、`--bins` 3 全绿的那种形状，E26 的 C1 已经付过一次学费）。
#[test]
fn the_binary_holds_a_malformed_argument_at_the_parameter_tier() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("bin-bad-args")?;
    let out_dir = dir.path().display().to_string();
    let url = format!("{base}/blob");

    // ① `--sha256` 给的不是"64 个十六进制字符"。选 `xyz` 而不是空串：`Some("xyz")` 在
    //    `Config::verify_sha256` 上是完全合法的形状，**只有** `check_boundaries` 这一层拦得住它
    //    （**E23⑤** 把这一格判给 CLI 的原话），所以"没走到引擎"这句断言在这里不是空话。
    let sha = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "blob.bin", "--sha256", "xyz",
    ])?;
    let es = stderr_of(&sha);
    let so = stdout_of(&sha);
    assert_eq!(
        sha.status.code(),
        Some(2),
        "形状错的退出档是 `BAD_ARGS`（E26/C3，与 clap 的用法错同码）：exit={:?}，stderr：{es}",
        sha.status.code()
    );
    assert!(es.contains("参数写错了"), "参数层的措辞要原样到达 stderr（E23⑤）：{es}");
    assert!(es.contains("--sha256"), "那句话还得点名是哪一格写错了，否则用户没法改：{es}");
    for engine_wording in ["校验不符", "sha256 不符", "跑不起来", "下载未完成"] {
        assert!(
            !es.contains(engine_wording) && !so.contains(engine_wording),
            "两条流里出现 {engine_wording:?} 就说明这趟已经走进了引擎\
             （前两个是引擎的校验结论，`跑不起来` 是 `main` 的 `Err` 那一支，`下载未完成` 是结果行）：\
             stderr：{es}，stdout：{so}"
        );
    }
    assert!(
        !es.contains("Usage"),
        "码是 2，但这一支**不是** clap 的用法块 —— 两个来源同码，靠这句措辞分开（见本用例文档）：{es}"
    );
    assert!(!dir.path().join("blob.bin").exists(), "参数层失败之后磁盘上一个字节都不该动过");
    // 负向判据按 harness 的纪律下：**守满**一段时间（日志行落盘晚于客户端拿到响应）。
    let reqs = srv.requests_within(Duration::from_millis(500))?;
    assert!(
        reqs.is_empty(),
        "对端看到 {} 条请求 ⇒ 形状闸没拦住，白跑一整趟下载才告诉你结论（E23⑤ 拦的正是这一格）：{reqs:?}",
        reqs.len()
    );

    // ② 同一条闸的第二格：`--connections 0`。⚠ 立论按 **E23 附记①** 的更正版走，不抄 E23⑤ 原句：
    //    引擎侧**有** `workers >= 1` 那道 `ensure!`（`engine::run` 开头那几行，在 `build_client` 与
    //    `ensure_parent_dir` 之前 ⇒ 按符号名指，不写行号：本项目"注释抄现状"已经漂过多次），所以 CLI 这一格拦的**不是崩溃**，而是**档与措辞** ——
    //    "话没说对"（2 + "参数写错了"）不该被报成"跑不起来"（1 + 引擎那句）。
    let conns = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "blob.bin", "--connections", "0",
    ])?;
    let ec = stderr_of(&conns);
    assert_eq!(
        conns.status.code(),
        Some(2),
        "`--connections 0` 与 `--sha256 xyz` 同一个档（E26/C3：'话没说对'只有一种码）：exit={:?}，stderr：{ec}",
        conns.status.code()
    );
    assert!(
        ec.contains("参数写错了") && ec.contains("--connections"),
        "参数层措辞 + 点名那一格，两条都要在：{ec}"
    );
    for engine_wording in ["跑不起来", "至少要是 1"] {
        assert!(
            !ec.contains(engine_wording) && !stdout_of(&conns).contains(engine_wording),
            "出现 {engine_wording:?} 就说明这一格是被引擎拦下的，不是 CLI（E23 附记① 的更正就落在这里）：{ec}"
        );
    }
    assert!(!dir.path().join("blob.bin").exists(), "第二格同样不该动磁盘");
    assert_eq!(
        srv.requests_within(Duration::from_millis(300))?.len(),
        0,
        "第二格同样一个字节都不许多打（日志是同一份 ⇒ 这里仍是对端的全量账）"
    );
    Ok(())
}

/// **T99-G5 第 1 条的进程级一对**：`--out-name` 越目录停在**参数层**（退 2 ＋ "参数写错了"），
/// 而相对子目录照旧走通。
///
/// 为什么单元层那张表（`src/task.rs::tests`）不够：它钉的是闸本身，钉不到"闸交回的那句错误经
/// `main.rs` 的 `into_task()` 那一支落到**哪个码**、措辞到不到 **stderr**"—— 这正是 E26/C3 把
/// 这一族判给进程级的原话（`--lib` 全绿而装配处断掉的那种形状，E26 的 C1 已经付过一次学费）。
/// 而这一发要修的缺陷本来就是**用户看见的那两行**（简报实测：摘要里"输出目录"与"落点"自相矛盾），
/// 所以正臂断的是那两行重新自洽。
///
/// 两臂成对（负向判据必须与正向对照成对，E28⑧-1）：
/// - 负臂 `../escaped.bin` ⇒ 退 **2** ＋ 点名 `--out-name` ＋ **摘要根本没打**（短路在 `into_task()`）
///   ＋ 盘上（含上一级目录）没有产物；
/// - 正臂 `sub/inside.bin` ⇒ 退 **0** ＋ 摘要照打 ＋ 落点那一行仍在 `--out-dir` 底下。
///
/// ⚠ 正臂是这一对的牙：没有它，"任何带分隔符的名字都拒"那种改宽（本发的 m2）在这一族上看不出来。
/// 夹具不需要对端 ⇒ 两臂都带 `--no-start`（E20④ 的短路在 `engine::run` 之前，一个字节都不上网）。
#[test]
fn the_binary_holds_an_escaping_out_name_at_the_parameter_tier() -> anyhow::Result<()> {
    let dir = harness::tmp_dir("bin-out-name")?;
    let out_dir = dir.path().display().to_string();
    let url = "http://x.example/A.bin";

    // 负臂：越目录的名字。
    let esc = run_dlm(&[
        "--url", url, "--out-dir", &out_dir, "--out-name", "../escaped.bin", "--no-start",
    ])?;
    let e = stderr_of(&esc);
    assert_eq!(
        esc.status.code(),
        Some(2),
        "`--out-name ../escaped.bin` 是\"话没说对\"那一档（E26/C3：与 `--sha256 xyz`、clap 用法错同码）：exit={:?}，stderr：{e}",
        esc.status.code()
    );
    assert!(
        e.contains("参数写错了") && e.contains("--out-name"),
        "参数层措辞 ＋ 点名是哪一格写错了，两条都要在（否则用户没法改）：{e}"
    );
    assert!(
        !stdout_of(&esc).contains("任务摘要"),
        "闸在 `into_task()` 里 ⇒ 摘要一行都不该打，打了就是把那个自相矛盾的落点交到用户眼前：{}",
        stdout_of(&esc)
    );
    assert!(
        !dir.path().join("escaped.bin").exists() && !dir.path().join("..").join("escaped.bin").exists(),
        "参数层失败之后磁盘上（含 --out-dir 的上一级）一个字节都不该动过"
    );

    // 正臂（对照）：相对子目录不是越目录。
    let sub = run_dlm(&[
        "--url", url, "--out-dir", &out_dir, "--out-name", "sub/inside.bin", "--no-start",
    ])?;
    assert_eq!(
        sub.status.code(),
        Some(0),
        "对照臂：`sub/inside.bin` 落在 --out-dir 底下，必须放行 —— 把它拒了＝没收子目录能力，简报明令不许：exit={:?}，stderr：{}",
        sub.status.code(),
        stderr_of(&sub)
    );
    let s = stdout_of(&sub);
    assert!(s.contains("任务摘要"), "对照臂的摘要照打（E20④/E23⑥）：{s}");
    let landing = s.lines().find(|l| l.starts_with("落点：")).expect("摘要必须有落点那一行");
    assert!(
        landing.contains(&out_dir) && landing.contains("sub/inside.bin"),
        "落点那一行必须仍是 --out-dir 底下的那个子路径（简报实测\"输出目录/落点两行自相矛盾\"的形状不许回来）：{landing}"
    );
    Ok(())
}

/// **E26/C5**：`--quiet` 与 `--no-start` **同时**给。行为由两条裁定叠出来，两条在这一格上都有落点：
/// **E24①** 把 `--quiet` 的语义收窄成"关掉每一拍那一发进度行"（**最终摘要与错误照打**），
/// **E20④** 把 bit0 的短路放在 `engine::run` **之前** —— 而 `dispatch` 的形状是先读 `quiet()`
/// 与 `no_start()` 两个位、再消费 `Cli`，所以短路那一刻两个位都已经读过了 ⇒ 结论是
/// **摘要照打、一个 `\r` 都没有、退出码 0**。
///
/// 这一格今天没有用例，而叠出来的两支各有自己的回归方向："quiet 把摘要也吞了"（把 E24①
/// 读成"什么都不说"）与"no-start 其实下了"（短路位置挪到挂 sink 之后）。所以这里同时断
/// 正向（摘要每一格都在）、反向（无 `\r`、无结果行、stderr 全空）与代价（零请求、无文件）。
#[test]
fn the_binary_still_prints_the_summary_when_quiet_meets_no_start() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("bin-quiet-no-start")?;
    let out_dir = dir.path().display().to_string();
    let url = format!("{base}/blob");
    let without_quiet = ["--url", &url, "--out-dir", &out_dir, "--out-name", "blob.bin", "--no-start"];
    let mut both = without_quiet.to_vec();
    both.push("--quiet");

    let out = run_dlm(&both)?;
    assert_eq!(
        out.status.code(),
        Some(0),
        "`--quiet` + `--no-start` 既不是失败也不是静默退出（E24①/E20④）：exit={:?}，stderr：{}",
        out.status.code(),
        stderr_of(&out)
    );
    let sq = stdout_of(&out);
    assert!(sq.contains("任务摘要"), "`--quiet` 吞掉摘要就是越权（E24①：静默只关刷新的那一行）：{sq}");
    assert!(sq.contains(&url) && sq.contains("blob.bin"), "摘要仍要逐字段打（E23⑥：不许拿 Debug 当渲染器）：{sq}");
    assert!(!sq.contains('\r'), "这一支上永远不该有进度行（引擎压根没进，也就没有进度源）：实测 stdout {sq:?}");
    for verdict in ["下载已完成", "下载未完成"] {
        assert!(!sq.contains(verdict), "短路之后不该有最终结果行（E20④）：{sq}");
    }
    assert!(
        stderr_of(&out).is_empty(),
        "这一支没有任何话要说；stderr 一旦有内容，说明走成了参数层或 `Err` 那一支：{}",
        stderr_of(&out)
    );

    // 对照组：**同一串参数、不给 `--quiet`**。`report::summary` 是纯函数、不含时间，
    // 所以"静默与否"在只入队这一支上必须是一个可观测的**恒等**，而不只是"摘要还在"——
    // 这一句才是那根反针：它挡住"quiet 顺手改了某几格"这种比"整个吞掉"难得多的回归。
    let plain = run_dlm(&without_quiet)?;
    assert_eq!(plain.status.code(), Some(0), "对照组（只 `--no-start`）也是 0：{}", stderr_of(&plain));
    assert_eq!(
        sq,
        stdout_of(&plain),
        "`--quiet` 在 `--no-start` 这一支上不该改变 stdout 的任何一个字节（E24① 的收窄版）"
    );

    assert!(!dir.path().join("blob.bin").exists(), "摘要之后动了磁盘 ⇒ 这一支又开始下载了（E20④）");
    let reqs = srv.requests_within(Duration::from_millis(500))?;
    assert!(
        reqs.is_empty(),
        "bit0 说\"一个字节都不许多打\"，对端却看到 {} 条请求（两发都没进引擎 ⇒ 这本账此刻还是空的）：{reqs:?}",
        reqs.len()
    );
    Ok(())
}

/// 侦测文本里有没有"<数字> 条长选项"这一族**计数形状**（量词写作"条"或"个"，数字与名词之间
/// 允许隔着至多两个空格/量词字符，覆盖 `14条长选项`、`14 条长选项`、`4 个长选项` 等排版）。
/// 不用正则：本项目依赖表里没有 regex，而这一形状窄到手工扫描比引入依赖更便宜。
fn counts_long_options(text: &str) -> bool {
    for (at, _) in text.match_indices("长选项") {
        let before: Vec<char> = text[..at].chars().collect();
        for gap in 0..3 {
            if before.len() <= gap {
                break;
            }
            // 数字落在 gap 位上，中间那些字符只许是空格或量词。
            let mid_ok = before[before.len() - gap..].iter().all(|c| matches!(c, ' ' | '条' | '个'));
            if mid_ok && before[before.len() - 1 - gap].is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

/// 用户表面的禁串清单（**E28③**）。这八条是 `--help` 那一臂**原来就有的**，逐字符照旧，
/// 只是从内联数组搬成常数 —— 搬的理由：下面 T99-G5 第 2 条新加的"真跑一次"那一臂要扫的是
/// **同一份**清单。两处各写一遍，就是下一次改口只改一半的成因（与 `engine::sha256_mismatch_message`
/// 收掉那两份字面量同一课）。
const BANNED_INTERNAL_PROSE: &[&str] =
    &["勘误", "§", "clap", "别名层", "long_about", "暗道", "into_task", "into_config"];

/// **T99-G5 第 2 条**新加的那一族：内部文档坐标（`spec §6.7` 那半句括注就是它的一个实例）。
/// 单列一摊而不是直接塞进上面那份，是为了让"哪一条是哪一发加的"在读时仍然可辨；
/// 两摊在失败文案那一臂上一起扫。`§` 已经在上面那份里，这里补的是`spec`这个**词**。
const BANNED_DOCUMENT_COORDINATES: &[&str] = &["spec"];

/// 串里有没有「`E` 或 `R` ＋ 一个 ASCII 数字」这种**内部编号形状**（勘误编号 E23② 与
/// spec 需求编号 R7 的骨架）。命中时交出那两个字符，红时能直接看见现场。
/// ⚠ 它比上面 `--help` 那一臂内联的那支（只认 `E`）**宽一条**：那一支是既有判据、本发一个字不许动，
/// 而失败文案这一族要同时挡住 `R7` 这种需求编号 ⇒ 这里认两个字母。两支并存是刻意的。
fn internal_number_shape(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .windows(2)
        .find(|w| matches!(w[0], 'E' | 'R') && w[1].is_ascii_digit())
        .map(|w| w.iter().collect())
}

/// **E28③ / E27②**：`Cli` 的结构体文档注释曾被 clap derive 当 `long_about` **原样印进 `--help`** ——
/// 内部勘误论证、漂移过的选项计数、实现纪律（"字段一律私有…暗道"）全到了用户表面。
/// 这一发就是那条纪律终于立成的**针**：扫真二进制的 `--help` stdout，禁内部词与计数形状。
/// 负向扫描不带对照臂就是恒真（纪律要求，也是这条勘误自己的立场）⇒ 同一段 stdout 必须**同时**
/// 证明六条长选项名都在场；哪天帮助面整个坏了，对照臂会先把这一发红掉，而不是让禁串扫描空转。
///
/// **T99-G5 第 2 条把它从"只管 `--help`"扩成"也扫一次真跑的 stdout ＋ stderr"**：失败路径这一格
/// 原本不在任何针的射程里 —— `src/engine.rs` 有两处各抄了同一个字面量，里面带着 `（spec §6.7）`，
/// 一次 `--sha256` 不符的真跑输出里就印着那个内部坐标。**帮助面干净 ≠ 用户表面干净**。
/// 新增那一臂同样带正臂（"算得…期望…"那句话必须在），因为删成空字符串也能骗过纯禁字扫描
/// （**E28⑧-1** 立的形状：负向判据必须与正向对照成对）。
/// ⚠ 上面 `--help` 那几行既有断言的方向与期望值一个字没动（只有禁串数组搬成了常数）。
///
/// **T99-G5b 1c-2 给 `--help` 那一臂又加了一格正臂**（别名 token `/d` 与 `/onboot` 必须在场）：
/// 六条长名的对照臂只断"名字印出来了"，而 16 条名字全在、说明一格的帮助面在那圈判据下是绿的。
/// 别名表从"只住在报错里"搬到 `--help` 里之后，这一格才让它红得下来；结构那一维
/// （每条选项 `get_help()` 非空）住在 `src/cli.rs` 的
/// `every_help_entry_carries_an_explanation_and_the_alias_table_is_printed`。
#[test]
fn the_binary_help_prints_no_internal_review_prose() -> anyhow::Result<()> {
    let out = run_dlm(&["--help"])?;
    assert!(
        out.status.success(),
        "`--help` 仍是退出码 0 那一档（E24②，本用例的扫描对象以能打出帮助为前提）：exit={:?}，stderr：{}",
        out.status.code(),
        stderr_of(&out)
    );
    let h = stdout_of(&out);

    // 对照臂（正向）：这六条长选项必须出现在帮助里。缺任何一条 ⇒ 帮助面坏了，
    // 下面那一串"没印内部措辞"的负向断言就不再是它声称在测的东西。
    for opt in ["--url", "--out-dir", "--out-name", "--connections", "--quiet", "--no-start"] {
        assert!(h.contains(opt), "对照臂：帮助里少了 {opt} —— 此时负向扫描测的已是\"坏帮助\"而非\"禁串\"：{h}");
    }
    // **T99-G5b 1c-2**：同一族判据补第二格正臂，因为上面那圈六条只断"名字在场"，而
    // **名字出现在帮助里 ≠ 帮助教了东西**（`c3a3cfd` 那一版 16 条名字全在、说明一格都没有，
    // 这一发当时四闸全绿）。这里要的是**斜杠写法也到了交付面**：`/d` 是映射项、`/onboot` 是
    // "接受并忽略"项，两种各来一条，缺任何一条 ⇒ 别名表只剩报错信息那一个去处。
    // ⚠ 方向仍是**正向对照**（与下面的禁串扫描成对，E28⑧-1）；既有六条与全部负向判据一字未动。
    for token in ["/d", "/onboot"] {
        assert!(h.contains(token), "对照臂：`--help` 里少了别名 token {token:?}（T99-G5b：别名映射必须进帮助面）：{h}");
    }

    // 禁串：只有仓库内部才该出现的词，出现在交付给用户的帮助面上就是 E28③ 复发。
    for banned in BANNED_INTERNAL_PROSE {
        assert!(!h.contains(banned), "帮助面不许印 {banned:?}（E28③：内部论证漏到了用户表面）：{h}");
    }
    // 禁形状一：`E` + 数字 —— 勘误编号（E23②、E28③ 这种）的骨架。
    let chars: Vec<char> = h.chars().collect();
    let errata_marker = chars.windows(2).any(|w| w[0] == 'E' && w[1].is_ascii_digit());
    assert!(!errata_marker, "帮助面不许印\"E+数字\"的勘误编号形状（E28③）：{h}");
    // 禁形状二："<数字> 条长选项" —— 选项计数（`c6ac42e` 纪律：数量不进文档，而这里进的是用户面）。
    assert!(!counts_long_options(&h), "帮助面不许印\"<数字> 条/个长选项\"的计数（E27②：这个数已经错过一次）：{h}");

    // ---- T99-G5 第 2 条：同一族判据扩到**一次真跑的 stdout ＋ stderr** ----
    // 夹具用现成的形状（`the_binary_maps_ok_to_the_exit_code_…` 的 ③ 那一路）：真下载完成、
    // 但 `--sha256` 给的是不符的那 64 个 0 ⇒ 引擎那句"sha256 不符…"原样到达用户 stdout。
    // 对端不需要新形状 ⇒ 本臂不引入桩、不改 `probe-server.mjs`。
    let (_srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("bin-no-internal-coords")?;
    let out_dir = dir.path().display().to_string();
    let len = 64 * 1024u64;
    let bad = run_dlm(&[
        "--url", &format!("{base}/sz/{len}"), "--out-dir", &out_dir, "--out-name", "nocoords.bin",
        "--connections", "2", "--sha256", &"0".repeat(64),
    ])?;
    assert_eq!(
        bad.status.code(),
        Some(1),
        "本臂的前提是\"下完了但校验不符\"那一档（`ok = false` ⇒ 退 1）；换了档就是什么都没扫到：exit={:?}，stderr：{}",
        bad.status.code(),
        stderr_of(&bad)
    );
    let so = stdout_of(&bad);
    let se = stderr_of(&bad);

    // 正臂（与下面的禁串扫描**成对**，E28⑧-1）：那句话确实到达了用户，三件事一个都不许少 ——
    // 没有这一臂，"把整句删空"与"把坐标删掉但顺手丢了信息"都能骗过纯禁字扫描。
    assert!(so.contains("sha256 不符"), "这一臂扫的必须是校验不符那一路的输出：{so}");
    assert!(
        so.contains("算得 ") && so.contains("期望 "),
        "\"算得 … 期望 …\" 那两格必须在场（否则下面那串禁字扫的是空输出）：{so}"
    );
    assert!(
        so.contains("目标文件与断点文件都保留，不自动删"),
        "「两份文件都保留、不自动删」那句纪律照旧要摆到用户眼前：{so}"
    );

    // 负臂：失败文案这一族的用户表面（stdout ＋ stderr 一起）不许印内部措辞与文档坐标。
    let both = format!("{so}{se}");
    for banned in BANNED_INTERNAL_PROSE.iter().chain(BANNED_DOCUMENT_COORDINATES) {
        assert!(
            !both.contains(banned),
            "真跑一次的用户表面不许印 {banned:?}（T99-G5 第 2 条：用户可见的失败文案里不许有内部文档坐标）：stdout={so} stderr={se}"
        );
    }
    if let Some(hit) = internal_number_shape(&both) {
        panic!(
            "真跑一次的用户表面不许印「E/R ＋ 数字」的内部编号形状（命中 {hit:?}）：stdout={so} stderr={se}"
        );
    }
    Ok(())
}

/// **E28⑧-1**：`--` 之后别名**不再展开**（`expand_idm_aliases` 的 `after_ddash` 那一支）。
/// 全树此前没有任何用例把 `--` 作为参数传进去 ⇒ 三行实测补一发，带对照：
/// 第一行是对照（不给 `--`，`/q` 按 C3 表"接受并忽略"、退 0）；后两行给 `--`，别名位变成
/// clap 的"未预期参数"。**`-- /d` 那行才是牙**：若 `--` 之后仍展开，`/d` 会变成第二个 `--url`，
/// clap 报的是 "the argument '--url <URL>' cannot be used multiple times" 而不是
/// "unexpected argument '/d' found" ⇒ 那一行把"没展开"与"展开了再说错"区分开。
#[test]
fn the_binary_stops_expanding_aliases_after_the_double_dash() -> anyhow::Result<()> {
    // 第一行：不给 `--`，`/q` 被丢掉，其余照常（退出码 0 + 摘要）。
    let plain = run_dlm(&["--url", "http://x.example/A", "/q", "--no-start"])?;
    assert_eq!(
        plain.status.code(),
        Some(0),
        "对照行：`--` 之前的 /q 是\"接受并忽略\"的别名，必须退 0（E23③）：exit={:?}，stderr：{}",
        plain.status.code(),
        stderr_of(&plain)
    );
    assert!(
        stdout_of(&plain).contains("任务摘要"),
        "对照行的摘要照打（`--no-start` 短路在 CLI 层，E20④）：{}",
        stdout_of(&plain)
    );

    // 第二行：`--` 之后的 `/q` 不再是别名 ⇒ 停在参数层的 clap 用法错（码 2），点名 token 原文。
    let ddash_q = run_dlm(&["--url", "http://x.example/A", "--", "/q", "--no-start"])?;
    assert_eq!(
        ddash_q.status.code(),
        Some(2),
        "`--` 之后的 /q 是未预期参数，不许再被展开吞掉：exit={:?}，stderr：{}",
        ddash_q.status.code(),
        stderr_of(&ddash_q)
    );
    let eq = stderr_of(&ddash_q);
    assert!(eq.contains("unexpected argument '/q' found"), "错误要点名 token 原文 '/q'：{eq}");

    // 第三行（有牙）：`--` 之后的 `/d` 同样不展开。
    let ddash_d = run_dlm(&["--url", "http://x.example/A", "--", "/d", "--no-start"])?;
    assert_eq!(
        ddash_d.status.code(),
        Some(2),
        "`--` 之后的 /d 是未预期参数：exit={:?}，stderr：{}",
        ddash_d.status.code(),
        stderr_of(&ddash_d)
    );
    let ed = stderr_of(&ddash_d);
    assert!(
        ed.contains("unexpected argument '/d' found"),
        "`-- /d` 那行必须点名 '/d'；报成 '--url' 就说明 after_ddash 那一支丢了：{ed}"
    );
    assert!(
        !ed.contains("cannot be used multiple times"),
        "出现这句说明 `-- /d` 被展开成了第二个 `--url`（\"展开了再说错\"≠\"没展开\"）：{ed}"
    );
    Ok(())
}

/// **E28①(1) 恢复的那张 C2 处方**（E26/C2 原文 → E26 附记以"端到端不可达"撤销 → **E28①** 用一次
/// 真进程实测把那句前提推翻，处方在此恢复；本项目第十处"可达性断言未复量"）。
///
/// 判据的内容是 **spec §10 R13** 写清的出货形状：一条 `POST`、不带 `Range`、**试探零发**、
/// **不读也不写断点文件**、整份一次落盘、退 **0**；再加 **E21⑧ / E22③** 的 CLI 侧那一半——
/// 这一支全程**一个 `\r` 都没有**（引擎在这条路上一发 tick 都不交，`run_whole_once` 整程阻塞在
/// 那一发 POST 里）。
///
/// 为什么进程级那一发不是库侧那发（`a_data_file_task_is_never_probed_and_posts_exactly_once`）的重复：
/// 那一发证的是**引擎**，而"`main.rs` 在这一支上会不会自己印出一行进度、会不会多打一个空行"
/// 住在装配层，进程内没有任何代码路径观察得到（与 E26/C1 把 `--bins` 升成第四闸同一个理由）。
///
/// ⚠ **本用例同时是它自己量出来的那发缺陷的针**（用户表面）：`main.rs::finish` 以前在
/// `!quiet` 时**无条件** `println!()` 让出那一行 —— 那一发的用途只是"把被 `\r` 原地覆盖的进度行
/// 让出来"，而这一支压根没有进度行 ⇒ 结果行前面凭空多一个**空行**。
/// 下面的判据按**修正后的形状**下（第一行就是结果行），所以它在修复前是红的（红相见报告）。
/// 修复的判据取**任务形状**（`Task::segmentable()`，正是 `engine::run` 用来在 `run_whole_once` 与
/// 多 worker 那条路之间分流的那一位），**不取"这一趟实际收到过几发 tick"**（**E22③** 禁止 CLI 把
/// "收到过 tick"当证据；**E26/C6** 已明否"给 `tick_sink` 加内部可变性去数发数"那条路）。
///
/// 负向判据带**对照臂**（同一台对端、同一条 URL、只是不给 `--data-file`）：
/// 没有对照，那一串"不许有 `\r` / 不许有跨轮账"就是恒真。
#[test]
fn the_data_file_route_downloads_everything_on_one_shot_without_a_single_tick() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("bin-data-file")?;
    let out_dir = dir.path().display().to_string();
    let len = 1024 * 1024u64;
    let url = format!("{base}/sz/{len}");
    let form = b"q=dlm&page=one-shot";
    let body = harness::scratch_body("9b4b-form.bin", form)?;

    // -------------------------------------------------- 第一臂：带体，**不给** `--quiet`
    let out = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "posted.bin",
        "--data-file", &body.display().to_string(),
    ])?;
    let s = stdout_of(&out);
    let target = dir.path().join("posted.bin");
    assert!(
        out.status.success(),
        "带体那一支跑得完就是 `下载已完成`（R13），还非零等于撒谎：exit={:?}，stderr：{}",
        out.status.code(),
        stderr_of(&out)
    );
    assert!(
        !s.contains('\r'),
        "这一支全程不许有任何一行进度：引擎在这条路上一发 tick 都不交（E21⑧/E22③），\
         装配层也不该自己造一个出来。实测 stdout：{s:?}"
    );
    // 那一发缺陷的正身：修正后的形状是"结果行就是 stdout 的第一行"。
    assert!(
        !s.starts_with('\n'),
        "结果行前面多出一个空行 —— 让行那一发只在**能打出进度行**的那一支才有意义（E28① 的缺陷）：{s:?}"
    );
    assert!(s.starts_with("下载已完成"), "第一行就是结果行（与上一句合起来钉死形状）：{s:?}");
    assert!(s.contains("用时 "), "耗时那一格照打（`--quiet` 关的从来不是它）：{s}");
    assert!(s.contains(&format!("全长 {len} B")), "结果行报的全长要等于对端宣布的那个数：{s}");
    assert!(s.contains(&target.display().to_string()), "结果行要点名落点：{s}");
    // 该形状不落断点 ⇒ 跨轮那三格**整格不出现**，而不是印 0 冒充测过（E13④ + E25⑥ 末句）。
    for unbilled in ["这个断点文件从建立起一共重试", "归属之外可能被重下", "归属已定"] {
        assert!(!s.contains(unbilled), "带体那一支不许出现 {unbilled:?}（没有断点就没有跨轮的账）：{s}");
    }
    assert!(
        stderr_of(&out).is_empty(),
        "成功那一支 stderr 该是全空（R13 实测）：{}",
        stderr_of(&out)
    );

    // 落盘的字节数 == 结果行宣布的那个数（一句话里的两个来源各自钉一次）。
    let meta = std::fs::metadata(&target).context("带体那一支没把目标文件落到盘上")?;
    assert_eq!(meta.len(), len, "整份单发要把全长都落到盘上（R13）：{s}");
    assert!(
        !dlm::state::path_for(&target).exists(),
        "该形状既不读也不写断点（E13④/R13）：`--keep-state` 没参与，进程级也不许出现断点文件"
    );

    // 对端的账：**只许一条**、方法 `POST`、**不带 `Range`**、`Content-Length` 等于体文件长度。
    // 上界判据走 harness 的两步（`requests_within` 一见到请求就返回，答不出"不许更多" ⇒
    // 那一入口只服务"一条都不许有"）：`requests_at_least(1)` 保证不漏读，
    // `requests_quiet` **守满**一段时间保证不多读。必须在对照臂之前读，否则读的是两趟的总和。
    srv.requests_at_least(1)?;
    let reqs = srv.requests_quiet(Duration::from_millis(500))?;
    assert_eq!(reqs.len(), 1, "带体的形状全程只许一发请求（试探 + POST = 2 条就是缺陷）：{reqs:?}");
    assert_eq!(reqs[0]["method"], "POST", "那一发是 POST；GET 就是试探（E11①）：{reqs:?}");
    assert_eq!(reqs[0].get("range"), None, "POST 那一枪不许带 `Range`（R7/R13）：{reqs:?}");
    let want_cl = form.len().to_string();
    assert_eq!(
        reqs[0]["extra_headers"]["content-length"].as_str(),
        Some(want_cl.as_str()),
        "上线的请求体长度要等于那份体文件的字节数（R13）：{reqs:?}"
    );

    // -------------------------------------------------- 对照臂：同一条 URL，不给 `--data-file`
    // 这一臂证明的是"进程级观察者**看得见**那一行 `\r`"——于是第一臂那句"看不见"才是结论，
    // 而不是仪器坏了（本项目对负向判据的硬要求：带对照，否则恒真）。
    let seg = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", "segmented.bin", "--connections", "2",
    ])?;
    let ss = stdout_of(&seg);
    assert!(
        seg.status.success(),
        "对照臂自己得先跑完，否则下面的形状断言测的是失败路径：{}",
        stderr_of(&seg)
    );
    assert!(ss.starts_with('\r'), "对照臂（可分段）的 stdout 以进度行开头 —— 仪器看得见 `\\r`：{ss:?}");
    assert!(ss.contains("\n下载已完成"), "可分段那一路的让行换行还在（结果行独占一行）：{ss:?}");
    assert!(!ss.contains("\n\n下载已完成"), "让行只该出现一次：{ss:?}");
    assert!(
        ss.contains("这个断点文件从建立起一共重试"),
        "跨轮那两本账只在可分段那一路出现（与第一臂互为对照，E13④）：{ss}"
    );
    let _ = std::fs::remove_file(&body);
    Ok(())
}

// ================================================================ T10 / spec §7 场景 A：48 MiB 全速
//
// 派发简报写的是"`full_download_is_byte_exact` 已经把 48 MiB 走完 `engine::run`（`workers: 4`）"——
// **实测不是**：那一发用的是 `{base}/sz/8388608`，也就是 **8 MiB**（同一发里 `len` 就写在第一行）。
// spec §7 判据① 点名的尺寸是 48 MiB，而"48 MiB + 全速 + 分段"这一格今天**没有常驻用例**
// （`the_replayed_trace_matches_the_dispatched_shots_and_the_wire` 同样是 8 MiB；
// `the_data_file_route_…` 那一发虽然是 48 MiB 全速，走的是带体整份单发、`workers` 不起作用）。
// ⇒ 本发补上，用的正是计划 Task 10 Step 1 给的那个名字。它不复用场景 B 的判据：B 量的是**限速下的
// 劈分时序**，这一发量的是**全速下 48 MiB 的字节正确 + 线上形状**，两条各自有独立的红法。

/// **spec §7 场景 A / 判据①（+② 的线上形状那一半）**：48 MiB 全速跑到底，落盘字节级正确，
/// 线上枪数与派单一对一，并发真的不止一条连接，且**没有任何一枪要超过一个窗口**。
#[test]
fn scenario_a_fullspeed_48mib_is_byte_exact() -> anyhow::Result<()> {
    const WORKERS: usize = 8;
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("scen-a")?;
    let len = 48 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/blob"),
        dir: dir.path().to_path_buf(),
        name: Some("scen-a.bin".to_string()),
        ..harness::blank_task()
    };

    let started = Instant::now();
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: WORKERS,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
            on_tick: None,
        },
    )?;
    let wall = started.elapsed();
    assert!(rep.ok, "全速 48 MiB 该跑完：{:?}", rep.message);

    // —— 判据①：字节级正确。两边各算一遍：引擎里那道 `--sha256` 闸（`rep.ok` 已含）+ 独立读者 ——
    assert_eq!(rep.bytes_total, len, "服务器宣布的全长");
    assert_eq!(rep.delivered, len, "收尾时盘上真有的字节");
    assert_eq!(
        harness::sha256_of(&t.target_path())?,
        harness::expected_sha(len),
        "落盘那 48 MiB 必须与服务器那份一字不差"
    );

    // —— 判据②的线上形状：条数一对一、试探是第一条、多连接真的上了线、每一枪不超一个窗口 ——
    let probe_range = format!("bytes=0-{}", PROBE_READ_BYTES - 1);
    let expected_lines = rep.dispatched().len() + 1;
    srv.requests_at_least(expected_lines)?;
    let requests = srv.requests_quiet(Duration::from_millis(300))?;
    assert_eq!(
        requests.len(),
        expected_lines,
        "线上 {} 条请求与引擎报的 {} 枪 + 试探那一枪不是一对一（T8-R2 的同一判据）",
        requests.len(),
        rep.dispatched().len()
    );
    let ranges = harness::ranges_of(&requests);
    assert_eq!(
        ranges[0].as_deref(),
        Some(probe_range.as_str()),
        "线上第一条必须是试探那一枪（{probe_range}）：实际 {ranges:?}"
    );
    let spans = harness::wire_spans(&ranges[1..], len)?;
    let mut conns: Vec<u64> = vec![];
    for rec in &requests[1..] {
        let c = rec["conn"].as_u64().context("request 行缺 conn")?;
        if !conns.contains(&c) {
            conns.push(c);
        }
    }
    assert!(
        conns.len() >= 2,
        "8 条 worker 的一跑只在线上留下 {} 条连接 ⇒ 这发的形状退化成单连接了，判据②要审的\
         「多连接的 Range 序列」没人审（派单 {:?}）",
        conns.len(),
        rep.dispatched()
    );
    for (index, (s, e)) in spans.iter().enumerate() {
        assert!(
            e.saturating_sub(*s) < WINDOW_BYTES,
            "线上第 {index} 枪要的 [{s}, {e}] 长 {} B，超过一个窗口 {WINDOW_BYTES} B ⇒ \
             `fetch::range_header` 那道窗口帽断了（E14②：开放终点的条件是**窗口**够到文件尾，不是区间）",
            e - s + 1
        );
    }

    eprintln!(
        "SCENA 取证：用时 {wall:?}、全长 {len} B、Range 枪 {}、连接 {}、派单 {} 条、引擎报 dup {} B",
        spans.len(),
        conns.len(),
        rep.dispatched().len(),
        rep.duplicated
    );
    Ok(())
}

// ================================================================ T10 / spec §7 场景 B：限速对端
//
// 派发简报 **E28⑤**：`/slow` 全尺寸（48 MiB）控制员实测 ≈24 s（`用时 23967 ms`、退 0、落盘
// 50331648 B）。**不许缩尺寸** —— 限速这一档买到的东西是"劈分发生在传输还在飞的那段时间里"这个
// **时序**；把 body 缩到 1 MiB 就只剩劈分粒度（且碰不到 `MIN_SEGMENT_BYTES` 那条下限），判据会量错方向。
// 限速按**响应（≈按连接）**计（`tests/harness/probe-server.mjs`：`resume = () => slow ? setTimeout(push, 200)`），
// 每块 64 KiB ⇒ 单条连接 ≈320 KiB/s，8 条 worker 并发的聚合 ≈ `N × 320 KiB/s`。
// 本文件此前**没有**任何用例走过 `/slow`（`grep slow` 只命中渲染层那几格 `slow-retry` 标签），
// 所以这一发是 spec §7 场景 B 的第一次真实覆盖。

/// 从 `trace` 按 worker 跟住"这个 worker 手上现在是哪一条区间"，交回每一次劈分的 `(父区间, 收紧后的头)`。
///
/// 为什么不能只看 `Report::tightened()`：那道链接只交回收紧后的**头**，而"劈分点落在父区间的哪一侧"
/// 要的恰恰是劈分**当时**那条在途区间 —— 同一个起点会被连劈多次（1/2 → 1/4 → 1/8 正是这条链），
/// 拿"最终的父"去配"第一次的头"会把祖先链算成同一层。`trace` 的两种步骤都带 worker 序号 ⇒
/// 按序号跟住当前区间就够，**不需要另写一个预测器**（spec §7.2 明令禁的那个）。
fn split_records(rep: &dlm::engine::Report) -> Vec<(Interval, Interval)> {
    let mut current: Vec<Option<Interval>> = vec![];
    let mut out: Vec<(Interval, Interval)> = vec![];
    for step in &rep.trace {
        let (worker, iv, is_tighten) = match step {
            dlm::engine::TraceStep::Dispatched { worker, iv } => (*worker, *iv, false),
            dlm::engine::TraceStep::Tightened { worker, iv } => (*worker, *iv, true),
            _ => continue,
        };
        if current.len() <= worker {
            current.resize(worker + 1, None);
        }
        if is_tighten {
            // 调度器的 `split_tail` 只从 `longest_tail()`（= 某个 `active` 槽的尾巴）劈，
            // 而槽位为空时那条分支根本走不到 ⇒ 这里拿到 `None` 就是实现变了形状，如实崩给读者看。
            let parent = current[worker]
                .expect("调度器收紧了一条手上没有区间的 worker —— 本夹具的跟随逻辑跟不住，去看实现");
            out.push((parent, iv));
        }
        current[worker] = Some(iv);
    }
    out
}

/// **spec §7 场景 B / 判据①②**：48 MiB 走限速对端，断"真的劈了、劈点在线上看得见、下限从没被踩"。
///
/// 每条判据独立一条语句，红在哪一条就说明是哪一环断了：
/// - **字节级正确（判据①）**：与 `full_download_is_byte_exact` 用**同一个** oracle
///   （`harness::expected_sha`，那条 oracle 已由 `the_sha_oracle_matches_what_the_server_serves`
///   对真 socket 证过）。这一条只说"跑得完且下对了"，**它单独存在时本场景是空的**。
/// - **劈分真的发生（判据②的正面）**：`trace` 里有 `Tightened`。调度器的 `Retighten` 只可能来自
///   `split_tail`，而 `split_tail` 只劈**在途**那条区间的尾巴 ⇒ "一次都没劈"与"劈了但不是中点劈"
///   是两种不同的红。
/// - **劈点出现在线上（判据②的线上那一半，E13③ 的口径）**：内部账说"我把这条区间从 `mid` 劈开了"
///   不算数，必须有一条服务端亲手记下的 `Range` **就从 `mid` 开始**。
/// - **中点算术**：劈的是"剩余尾巴 `[written+1, end]`"的中点，所以劈点只可能落在**整条父区间中点
///   或其右侧**（写得越多越往右移），且上半截**绝不超过父区间的一半**。这两条把"1/2 → 1/4 → 1/8 逐层
///   对中点"这条规则钉成数字：把 `split_tail` 改成 1/3 劈会当场红（左移），改成"把整条尾巴给新人"
///   会红在"上半截超过一半"。
/// - **下限（`MIN_SEGMENT_BYTES`）**：每一条被**开出**过的区间都不短于下限。
/// - **时序（限速这一档独有的那一半）**：存在一次劈分，它的劈点请求到达服务端时**父区间那一枪还在飞**
///   （按连接的响应序配对 `response_done`），且到达时刻已在传输开始 2 s 之后 —— 后一句是必要的：
///   8 条 worker 在 t≈0 一起报就绪，那一批劈分**全速对上同样会有**，只有"传输进行中还在劈"才是
///   `/slow` 买到的东西。**顺序判据只取"被劈的那条区间上过线"这一半，不取"父那一枪先到"**：
///   `Dispatched`/`Tightened` 记进账的时刻是**派单**时刻，请求上线要再过一次引擎→worker 的投递，
///   实测 RED #1 正是撞在这一句上（"劈点 37807744 之前线上没有任何一条从 25283840 开始的 Range"）
///   ⇒ 那条顺序义务是**我写错的**，不是实现的缺陷。
///
/// ⚠ 运行时长按派发简报如实记录在 `docs/evidence/`：这一发实测要跑几十秒，**不是**慢用例缺陷。
#[test]
fn scenario_b_throttled_splits_at_midpoints_and_respects_the_floor() -> anyhow::Result<()> {
    const WORKERS: usize = 8;
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("scen-b")?;
    let len = 48 * 1024 * 1024u64;
    let t = Task {
        url: format!("{base}/slow"),
        dir: dir.path().to_path_buf(),
        name: Some("scen-b.bin".to_string()),
        ..harness::blank_task()
    };

    let started = Instant::now();
    let rep = dlm::engine::run(
        &t,
        dlm::engine::Config {
            workers: WORKERS,
            min_seg: dlm::consts::MIN_SEGMENT_BYTES,
            max_retries: None,
            verify_sha256: Some(harness::expected_sha(len)),
            force_fresh: false,
            // 集成用例一律不挂观察者（与全文件同一口径，9A/E21）。
            on_tick: None,
        },
    )?;
    let wall = started.elapsed();
    assert!(rep.ok, "限速那一跑该跑完（`verify_sha256` 那道闸也在引擎里跑过了）：{:?}", rep.message);

    // —— 判据①：字节级正确（同一个 oracle，独立读者）——
    assert_eq!(
        harness::sha256_of(&t.target_path())?,
        harness::expected_sha(len),
        "限速跑完的 48 MiB 必须与服务器那份一字不差"
    );
    assert_eq!(rep.delivered, len, "收尾时盘上真有这么多字节");

    // —— 线上取证：**先守卫条数再读**（评审 C-I-4 的成对用法，与本文件另两处同形）——
    let probe_range = format!("bytes=0-{}", PROBE_READ_BYTES - 1);
    let expected_lines = rep.dispatched().len() + 1;
    srv.requests_at_least(expected_lines)?;
    let requests = srv.requests_quiet(Duration::from_millis(300))?;
    assert_eq!(
        requests.len(),
        expected_lines,
        "线上条数与引擎报的枪数不是一对一（T8-R2 的同一判据）：静置窗口结束时 {} 条，\
         引擎报 {} 枪 + 试探那一枪",
        requests.len(),
        rep.dispatched().len()
    );
    let ranges = harness::ranges_of(&requests);
    assert_eq!(
        ranges[0].as_deref(),
        Some(probe_range.as_str()),
        "要从线上剔掉的必须是试探那一枪（{probe_range}）而且它是第一条：实际 {ranges:?}"
    );
    let shots = &requests[1..];
    let spans = harness::wire_spans(&ranges[1..], len)?;
    let mut conns: Vec<u64> = vec![];
    for rec in shots {
        let c = rec["conn"].as_u64().context("服务端记的 request 没有 conn 字段")?;
        if !conns.contains(&c) {
            conns.push(c);
        }
    }

    // —— 判据②正面：劈分真的发生了（这一条空着，下面三条全是空的）——
    let splits = split_records(&rep);
    assert!(
        !splits.is_empty(),
        "48 MiB 走限速对端一整跑，调度器一次都没劈分 ⇒ 本场景什么都没买到。\
         派单 {:?}，用时 {wall:?}",
        rep.dispatched()
    );

    // `response_done` 按连接分组（组内按日志顺序 = 该连接上的响应顺序；node 对同一条连接的响应
    // 是串行写出的，所以"第 k 条响应"配"第 k 条请求"是成立的）。
    let evs = srv.events()?;

    // 这条连接上第 `nth`（从 1 数）条响应结束的时刻；`None` = 那一枪到现在还没写完（= 还在飞）。
    let response_done_ms = |conn: u64, nth: usize| -> Option<u64> {
        evs.iter()
            .filter(|e| e["ev"] == "response_done" && e["conn"].as_u64() == Some(conn))
            .nth(nth - 1)
            .and_then(|e| e["t"].as_u64())
    };

    let mut late_in_flight_splits = 0usize;
    let mut in_flight_splits = 0usize;
    let mut floor_hits: Vec<Interval> = vec![];
    let mut split_times: Vec<u64> = vec![];
    let mut parent_onlined = 0usize;
    for (index, (parent, head)) in splits.iter().enumerate() {
        assert_eq!(
            head.start, parent.start,
            "第 {index} 次劈分把头段的起点挪了：父 {parent:?} 头 {head:?}（中点劈分只能动终点）"
        );
        assert!(
            head.end < parent.end,
            "第 {index} 次劈分得到的头段不比父区间短：父 {parent:?} 头 {head:?}"
        );
        let mid = head.end + 1; // 上半截的起点就是劈分点
        assert!(
            mid >= parent.start + parent.len() / 2,
            "第 {index} 次劈分点 {mid} 落在父区间中点以左（父 {parent:?}，中点 {}",
            parent.start + parent.len() / 2
        );
        let upper = rep
            .dispatched()
            .into_iter()
            .find(|iv| iv.start == mid && iv.end == parent.end)
            .with_context(|| format!("第 {index} 次劈分后没有把上半截 [{mid}, {}] 派给任何 worker", parent.end))?;
        assert!(
            upper.len() <= parent.len() / 2 + 1,
            "第 {index} 次劈分把超过一半的体量交给了新 worker：父 {parent:?} 上半截 {upper:?}"
        );

        // —— 判据②线上那一半：劈点必须真的以 `Range` 的样子出现过 ——
        let split_shot = spans
            .iter()
            .position(|(s, _)| *s == mid)
            .with_context(|| format!("劈分点 {mid} 只活在调度器的账里：线上没有任何一条 Range 从它开始（父 {parent:?}）"))?;
        // 被劈的那条区间也得真的上过线。**但不许要求"父那一枪先到、劈点那一枪后到"**：
        // `Tightened`/`Dispatched` 记进账的时刻是**派单**时刻，而请求要等 worker 线程真发出去才上线
        // （引擎与 worker 之间隔着一次投递）。一条刚派出去、请求还没上线的区间照样会被下一发
        // `Connected` 劈一刀 —— 这一发第一次就是这么红的（实测：劈点 37807744 之前线上没有
        // 任何一条从 25283840 开始的 Range）。所以顺序判据只取"上过线"这一半，
        // "在飞"那一半走下面的响应收尾配对，且只对**确实已上线**的父区间主张。
        assert!(
            spans.iter().any(|(s, _)| *s == parent.start),
            "被劈的那条区间的起点 {} 从没以 Range 起点的样子上过线（父 {parent:?}）",
            parent.start
        );
        let split_ms = shots[split_shot]["t"].as_u64().context("request 行缺 t")?;
        split_times.push(split_ms);
        let parent_shot = spans[..split_shot].iter().rposition(|(s, _)| *s == parent.start);
        if parent_shot.is_some() {
            parent_onlined += 1;
        }
        if let Some(ps) = parent_shot {
            let rec = &shots[ps];
            let conn = rec["conn"].as_u64().context("服务端记的 request 没有 conn 字段")?;
            let nth = usize::try_from(rec["reqNoOnConn"].as_u64().context("request 行缺 reqNoOnConn")?)
                .context("reqNoOnConn 超出 usize")?;
            // 父那一枪到劈点请求到达时**还没写完** ⇒ 这一刀砍在飞行中的响应上。
            let still_flying = match response_done_ms(conn, nth) {
                Some(done) => done > split_ms,
                None => true, // 到现在都没收尾：那一枪被中途放弃，劈分当然发生在它飞着的时候
            };
            if still_flying {
                in_flight_splits += 1;
                if split_ms >= 2_000 {
                    late_in_flight_splits += 1;
                }
            }
        }

        if upper.len() < dlm::consts::MIN_SEGMENT_BYTES || head.len() < dlm::consts::MIN_SEGMENT_BYTES {
            floor_hits.push(upper);
            floor_hits.push(*head);
        }
    }
    // —— 下限：任何一条被开出的区间都不许短于 `MIN_SEGMENT_BYTES`（放在时序判据**之前**，
    // 这样"下限被踩"这种红不会被"这一跑没在飞"的消息盖住）——
    let floor = dlm::consts::MIN_SEGMENT_BYTES;
    assert!(
        floor_hits.is_empty(),
        "劈分出来的两半里有短于下限 {floor} B 的：{floor_hits:?} ⇒ `split_tail` 的那道 `half < min_seg` 闸断了"
    );
    for (index, iv) in rep.dispatched().iter().enumerate() {
        assert!(
            iv.len() >= floor,
            "第 {index} 条派单 [{}, {}] 长 {} B，短于下限 {floor} B ⇒ 有人开出了一条不该开的段",
            iv.start,
            iv.end,
            iv.len()
        );
    }
    assert!(
        in_flight_splits > 0,
        "没有任何一次劈分发生在「父那一枪还在飞」的时刻 ⇒ 劈的全是没上线的区间，本场景的时序判据落空。\
         共 {} 次劈分、{} 枪、{} 条连接、劈分到达时刻 {split_times:?}、父那一枪已上线 {parent_onlined} 发、用时 {wall:?}",
        splits.len(),
        spans.len(),
        conns.len()
    );
    assert!(
        late_in_flight_splits > 0,
        "没有任何一次劈分发生在「传输已飞满 2 s 且父那一枪还没写完」的时刻 ⇒ 限速这一档买到的只剩 \
         t≈0 那一波（全速对上同样有），本场景的时序判据落空。共 {} 次劈分、{} 枪、{} 条连接、\
         劈分到达时刻 {split_times:?}、父那一枪已上线 {parent_onlined} 发、用时 {wall:?}",
        splits.len(),
        spans.len(),
        conns.len()
    );
    eprintln!(
        "SCENB 取证：用时 {wall:?}、Range 枪 {}、连接 {}、劈分 {}、劈分到达最早 {} ms / 最晚 {} ms、\
         父在飞时劈的 {in_flight_splits} 发（其中 2 s 之后 {late_in_flight_splits} 发）、引擎报 dup {} B",
        spans.len(),
        conns.len(),
        splits.len(),
        split_times.iter().min().copied().unwrap_or(0),
        split_times.iter().max().copied().unwrap_or(0),
        rep.duplicated
    );
    Ok(())
}

// ================================================================ T10 / spec §7 场景 C′：多 worker 真杀进程再续
//
// **这一发为什么要另起一发**：spec §6.6（Ctrl-C 优雅收尾）由 **R11** 裁为不做，承诺改成
// "任意时刻被杀都能续"。既有那一发 `resume_after_kill_does_not_redownload_done_segments` 已经把那个
// 形状钉在**进程内、单 worker、`ScriptedPeer`** 上；而 `tests/harness/mod.rs` 里 `ScriptedPeer` 的文档
// 同时写明它是**单线程顺序服务**（只许给 `workers == 1` 或退化形状用），多 worker 一律走 node 对端 ——
// 而 node 对端做不到"停一半之后同一 URL 还能跑完"（`/drop/0` 连试探一起掐死 ⇒ `engine::run` 在
// `probe` 就 `Err` ⇒ 压根没有断点文件；`/drop/n` 每枪都推进 ⇒ 必然跑到完成；换端口更是直接被
// `resume::decide` 判 `Fresh`）。⇒ **多 worker 那一发的"停"只能从对端搬到进程**：真起 `dlm.exe`，
// 从可观测状态里挑一拍硬杀（`Child::kill()` = TerminateProcess，不给它任何收尾的机会），
// 然后**同一 URL** 再跑一遍。这正是 R11 那句话的字面形状，也是 spec §7 判据③在 8 连接下的那一发。
//
// 用的对端是 `/slow`（48 MiB）而不是 `/blob`：本文件实测全速那一跑 48 MiB 只要 2.7 s，
// "跑到一半被杀"会变成一张计时彩票；限速把窗口拉到几十秒，杀点由**可观测量**决定，不靠 `sleep`。

/// 真二进制的**可中途硬杀**那一发：`run_dlm` 走 `.output()`，会阻塞到 EOF，杀不了。
/// `stdin/stdout/stderr` 全接空设备：没人读的管道写满会把子进程吊死，那测的就不是下载而是管道。
fn spawn_dlm(args: &[&str]) -> anyhow::Result<std::process::Child> {
    use std::process::Stdio;
    let bin = env!("CARGO_BIN_EXE_dlm");
    std::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("起二进制失败：{bin}"))
}

/// 覆盖集之外、`[0, total)` 之内的**第一个字节**。入参按 `start` 升序且两两不重叠
/// （`state::load` 读回来的 `segments` 就是 `snapshot_intervals()` 落盘的形状）。
fn first_uncovered(covered: &[(u64, u64)], total: u64) -> u64 {
    let mut cursor = 0u64;
    for (start, end) in covered {
        if *start > cursor {
            return cursor;
        }
        cursor = cursor.max(end.saturating_add(1));
    }
    cursor.min(total)
}

/// 两条闭区间交叠多少字节。
fn overlap_bytes(a: (u64, u64), b: (u64, u64)) -> u64 {
    let s = a.0.max(b.0);
    let e = a.1.min(b.1);
    if e >= s { e - s + 1 } else { 0 }
}

/// **spec §7 场景 C′ / 判据③**：8 条 worker 的真进程被硬杀在半路 ⇒ 断点文件与半截目标文件都在 ⇒
/// 同一 URL 再跑一遍退 0、字节级正确，而且**第二轮一枪都不碰断点文件说"已归我"的字节**。
///
/// 判据与实底：
/// - **覆盖集照它自己说的读**（**E28⑫** 点名的那条）：本发**不假定**粒度。断点保存那一支读成
///   合取闸（`moved && last_save.elapsed() >= 5 s`），所以"杀得早 ⇒ 断点里只有初始那一格种子"是
///   **合法的既有形状**，不是这条用例的失败条件；杀点因此取"断点里的覆盖字节第一次超过种子"这一拍
///   （可观测量，不是 `sleep`），拿不到就在 12 s 那一拍杀并照实登记。**量到的数写进 `docs/evidence/`，
///   不为了好看的数去调时间或尺寸。**
/// - **重复字节从对端自己的账算**：`conn_close_side.sent` 是"这条响应真的写出去了几个字节"，
///   与"我们要求了几个"分开（被中途放弃的那几枪只有前者是真的）。`dup` 取"交出去的字节里落在覆盖集内
///   的那一截"，所以试探那一枪必然贡献 `PROBE_READ_BYTES` 个字节 —— 那正是 **E14③ / E17** 登记过的
///   "真发生了线上代价却没被任何账报出"的那一笔，本发把它量成数而不是留在文档里。
/// - **事件边界**：第一轮那个进程已被确认退掉，死进程发不出新请求 ⇒ 边界之后的 `request` 行
///   只能属于第二轮。再加一道独立交叉：第二轮每条请求的 `remotePort` 必须出现在边界之后的
///   `socket_open` 里（第二轮是新进程，连接不可能复用第一轮已随进程消失的池）。
///   ⚠ **这里选 `remotePort` 而不是 `socket_open.seq` 对号，理由不是"那两个号对不上"**。
///   原先这段写的是"node 在 `.on("connection")` 里把 `connSeq` 自加了两次 ⇒ `socket_open.seq` 恒等于
///   请求行 `conn` 减一"——**控制员实测推翻**（2026-10-18，两种形状：单轮 `/sz/2097152`，以及
///   `/slow` 上"硬杀第一轮 + 第二轮跑完"的双轮）：同一条 `remotePort` 上 `socket_open.seq` 与
///   `request.conn` **相等**（60260→seq 1→conn 1 … 60265→seq 6→conn 6）。读源码也站不住那句：
///   `connection` 那一支是 `const id = ++connSeq;` 之后把**同一个** `id` 同时给 `socks.set` 与
///   `socket_open`，只加一次；另一处 `++connSeq` 住在请求处理里的 `if (!socks.has(s))` 守卫之后，
///   正常次序下根本不走（而且它真走的话给出的方向还是反的）。
///   ⇒ 仍然走 `remotePort`，但用一条**不依赖任何计数器同步**的理由：它是这条 TCP 连接本身的身份，
///   跨进程、跨轮都稳定，也不受"服务器将来怎么改编号"影响。**别把这条当仪器缺陷记着。**
#[test]
fn scenario_c_real_process_kill_then_resume_does_not_redownload_covered_bytes() -> anyhow::Result<()> {
    const WORKERS: &str = "8";
    // 杀点等的是可观测量（覆盖集长到种子之外），不是 `sleep`。上限取 30 s 的理由写在下面那条
    // "前提没成立就如实失败"的判据旁边：`/slow` 上一枪 = 一个 4 MiB 窗口 ≈ 13 s，而断点里的
    // `done` **只在一枪收尾时长**（`Progress` 不往里加），再加上保存闸那 5 s ⇒ 种子之外的那一格
    // 最早也要 ~18 s 才出现。整跑 ≈24 s，所以 30 s 既够用又不会让第二轮等不到东西。
    const SEED_ONLY_TIMEOUT: Duration = Duration::from_secs(30);
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("scen-c")?;
    let announced = srv.announced()?;
    let len = announced.size; // 由**这台服务器自己宣布**（派发第 7 条：不抄历史值）
    let url = format!("{base}/slow");
    let out_dir = dir.path().display().to_string();
    let out_name = "scen-c.bin";
    let target = dir.path().join(out_name);
    let state_path = dlm::state::path_for(&target);
    let args =
        ["--url", &url, "--out-dir", &out_dir, "--out-name", out_name, "--connections", WORKERS, "--quiet"];

    // —— 第一轮：真进程，杀点从可观测量里取（固定 `sleep` 会把这一发变成计时彩票）——
    let started = Instant::now();
    let mut child = spawn_dlm(&args)?;
    let mut covered_at_kill: Vec<(u64, u64)> = vec![];
    let mut first_save: Option<Duration> = None;
    loop {
        if let Some(snap) = dlm::state::load(&state_path)? {
            let covered: Vec<(u64, u64)> = snap.segments.iter().map(|s| (s.start, s.end)).collect();
            let bytes: u64 = covered.iter().map(|(s, e)| e - s + 1).sum();
            if bytes > PROBE_READ_BYTES && first_save.is_none() {
                first_save = Some(started.elapsed());
            }
            covered_at_kill = covered;
            if first_save.is_some() {
                break;
            }
        }
        if child.try_wait()?.is_some() {
            // 只有"已经跑到头"才允许在这里出环：跑完会删断点 ⇒ 下面那句 `state_path.exists()` 会红，
            // 红得有名有姓（这一发买的是"中途被杀"，提前结束就是没买到）。
            break;
        }
        if started.elapsed() >= SEED_ONLY_TIMEOUT {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let kill_reason = if first_save.is_some() {
        "覆盖集长到种子上方那一拍"
    } else {
        "12 s 上限（断点没长出种子之外，E28⑫ 那一格）"
    };
    child.kill().context("硬杀子进程失败")?;
    child.wait().context("等子进程退掉失败")?;
    let killed_after = started.elapsed();

    // —— 杀完之后盘上必须留下那两样东西（R11 的字面承诺）——
    assert!(
        state_path.exists(),
        "硬杀之后没有断点文件 ⇒「任意时刻被杀都能续」根本不成立（杀于 {killed_after:?}，{kill_reason}）"
    );
    assert!(target.exists(), "硬杀之后目标文件不在 ⇒ 连半截文件都没留下");
    let partial_len = std::fs::metadata(&target)?.len();
    assert!(partial_len > 0, "半截目标文件长度是 0 ⇒ 这一拍什么都没落盘");

    let snap = dlm::state::load(&state_path)?.context("上面已断言断点文件在，读回来却什么都没有")?;
    assert_eq!(snap.validator.total_len, len, "断点记的全长要与这台服务器宣布的一致");
    let covered = covered_at_kill;
    let covered_bytes: u64 = covered.iter().map(|(s, e)| e - s + 1).sum();
    assert!(
        covered_bytes <= partial_len,
        "断点说归我的字节（{covered_bytes} B）比盘上真有的（{partial_len} B）还多 ⇒ 断点对磁盘说谎（E17 那本账）"
    );
    let first_gap = first_uncovered(&covered, len);
    assert!(first_gap < len, "覆盖集已经把整份文件说成归我了，第二轮无从续起：{covered:?}");

    // —— 第二轮：同一条 URL、同一个命令，这次等它跑完 ——
    std::thread::sleep(Duration::from_millis(500)); // 让第一轮那些"客户端已经没了"的连接把收尾行写完
    let before = srv.events()?;
    let boundary = before.len();
    let run1_requests = before.iter().filter(|e| e["ev"] == "request").count();
    let second = run_dlm(&args)?;
    assert!(second.status.success(), "续的那一轮该退 0：{}", stderr_of(&second));
    assert_eq!(
        harness::sha256_of(&target)?,
        harness::expected_sha(len),
        "续完之后那 48 MiB 必须与服务器那份一字不差"
    );

    // —— 线上取证：先守卫条数（第二轮至少要有试探 + 一枪），再守满一段时间 ——
    srv.requests_at_least(run1_requests + 2)?;
    srv.requests_quiet(Duration::from_millis(300))?;
    let all = srv.events()?;
    assert!(all.len() >= boundary, "对端日志会缩短？边界之后读回来的行比边界之前还少");
    let run2 = &all[boundary..];
    let run2_requests: Vec<_> = run2.iter().filter(|e| e["ev"] == "request").collect();
    let opened: Vec<u64> =
        run2.iter().filter(|e| e["ev"] == "socket_open").filter_map(|e| e["remotePort"].as_u64()).collect();
    assert!(!run2_requests.is_empty(), "第二轮一条请求都没打出 ⇒ 下面所有判据都是空的（恒绿）");
    let probe_range = format!("bytes=0-{}", PROBE_READ_BYTES - 1);
    assert_eq!(
        run2_requests[0].get("range").and_then(serde_json::Value::as_str),
        Some(probe_range.as_str()),
        "第二轮第一枪该是试探那一枪"
    );
    for rec in &run2_requests {
        let rp = rec["remotePort"].as_u64().context("request 行缺 remotePort")?;
        assert!(opened.contains(&rp), "第二轮的请求挂在一条边界之前就存在的连接上（remotePort {rp}）⇒ 事件边界取错了");
    }

    // 对端自己记的"这条响应真写出了多少字节"：`conn_close_side` 按连接分组，组内第 k 条配该连接上第
    // k 条请求（node 对同一条连接的响应是串行写出的）。配对不齐就当场报错，不许悄悄少算一枪。
    let mut closes: Vec<(u64, Vec<u64>)> = vec![];
    for e in run2.iter().filter(|ev| ev["ev"] == "conn_close_side") {
        let c = e["conn"].as_u64().context("conn_close_side 没有 conn")?;
        let sent = e["sent"].as_u64().context("conn_close_side 没有 sent")?;
        match closes.iter_mut().find(|(k, _)| *k == c) {
            Some((_, v)) => v.push(sent),
            None => closes.push((c, vec![sent])),
        }
    }
    let mut per_conn: Vec<(u64, usize)> = vec![];
    for rec in &run2_requests {
        let c = rec["conn"].as_u64().context("request 行缺 conn")?;
        match per_conn.iter_mut().find(|(k, _)| *k == c) {
            Some((_, n)) => *n += 1,
            None => per_conn.push((c, 1)),
        }
    }
    for (c, n) in &per_conn {
        let got = closes.iter().find(|(k, _)| k == c).map(|(_, v)| v.len()).unwrap_or(0);
        assert_eq!(*n, got, "连接 {c} 上 {n} 条请求只等到 {got} 条 conn_close_side ⇒ 对端那份 sent 账配不平");
    }

    let mut dup_total = 0u64;
    let mut dup_probe = 0u64;
    let mut sent_total = 0u64;
    let mut min_resume_start = u64::MAX;
    for (index, rec) in run2_requests.iter().enumerate() {
        let range = rec
            .get("range")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("第二轮第 {index} 枪没带 Range"))?;
        let (s, _) = harness::wire_span(range, len)?;
        let conn = rec["conn"].as_u64().context("request 行缺 conn")?;
        let nth = usize::try_from(rec["reqNoOnConn"].as_u64().context("request 行缺 reqNoOnConn")?)
            .context("reqNoOnConn 超出 usize")?;
        let sent = closes
            .iter()
            .find(|(k, _)| *k == conn)
            .and_then(|(_, v)| v.get(nth - 1).copied())
            .with_context(|| format!("连接 {conn} 第 {nth} 条响应的 sent 没记下来"))?;
        sent_total += sent;
        // 对端真交出去的那一段（不是我们要求的那一段）：被中途放弃的响应只算它实际写出的字节。
        // `sent == 0`（一头就没写出去）那一支单独给 0，免得 `s + sent - 1` 在减法上翻车。
        let served = (s, s + sent.saturating_sub(1));
        let dup: u64 = if sent == 0 {
            0
        } else {
            covered.iter().map(|c| overlap_bytes(served, *c)).sum()
        };
        dup_total += dup;
        if index == 0 {
            // 试探那一枪：它天生重取预读那一截，是 **E14③ / E17** 那笔"真发生了线上代价却没被
            // 任何账报出"的字节。这一发把它量成数，但它不是判据③的失败（判据管的是"已完成段"，
            // 而试探要的那 236 032 B 从来不是"段"，是引擎每次都要读的头部事实）。
            dup_probe = dup;
            continue;
        }
        assert_eq!(
            dup,
            0,
            "第二轮第 {index} 枪要走了断点文件说已归我的 {dup} B（交出去的那段 {served:?}）⇒ \
             判据③「不重下已完成段」断了。覆盖集 {covered:?}"
        );
        assert!(
            s >= first_gap,
            "第二轮第 {index} 枪从 {s} 开始，而断点说的第一个未覆盖字节是 {first_gap} ⇒ 前缀被回头重取"
        );
        min_resume_start = min_resume_start.min(s);
    }
    assert!(
        min_resume_start != u64::MAX,
        "第二轮除了试探那一枪一枪都没打 ⇒ 上面那两条判据审的是空表（恒绿夹具）"
    );

    eprintln!(
        "SCENC 取证：杀于 {killed_after:?}（{kill_reason}；覆盖集长到种子上方那一拍 {}）；\
         断点覆盖 {} 格 / {covered_bytes} B；半截文件 {partial_len} B；第一个未覆盖字节 {first_gap}；\
         第二轮 {} 枪 / 交出 {sent_total} B / 落在覆盖集内 {dup_total} B（其中试探那一枪 {dup_probe} B）；\
         第二轮剔掉试探后最早起点 {min_resume_start} B",
        first_save.map(|d| format!("{d:?}")).unwrap_or_else(|| "未出现".to_string()),
        covered.len(),
        run2_requests.len(),
    );
    Ok(())
}

// ================================================================ T11 / spec §6.2 的 `slow-retry xN`
//
// 派发背景（`docs/evidence/2026-10-18-spec7-四场景验收.md` §6 第 5 条）：`slow-retry xN` 那一格
// 在渲染层（`report.rs` 的 tick 表）与快照判据（`engine.rs` 判据⑥）两侧都有常驻用例，但**本仓库
// 没有任何一次真进程的真进度行观测到过它**。此前仪器上唯一制造中途失败的对端 `/drop/<n>`：
// `/drop/0` 连试探那一枪也杀 ⇒ `probe::run` 交 `Err` ⇒ `engine::run` 压根进不了重试链；
// `n > 0` 交出真字节 ⇒ `fetch` 交 `Advanced` ⇒ 引擎照常推进、退避不介入。
// ⇒ 本轮加仪器：node 侧 `/probe-only` 路由**只**对试探的限量 Range（`bytes=0-236031`）照常服务，
// 其余每条请求（含无 Range 的整份 GET）一律 503 空 body，并记 `response_refuse` 事件。
// 形状：试探成功 ⇒ 种子 [0, 236031] ⇒ 每条 worker 枪起点 ≥ 236032 ⇒ 全部被拒
//（`Outcome::Failed(Http(503))`，零字节落地）⇒ 快速退避 1+2+4+8+16 s（31 s）用尽 ⇒ 转入 30 s 慢重试
// ⇒ `slow_retry_rounds = attempt - FAST_RETRY_MAX ≥ 1`。主循环在退避期照样每秒醒来交一拍
//（`tick_if_due`；`x0/8` 是 E25⑤ 的合法形状）⇒ 真进度行第一次能带着 `slow-retry xN` 落地。
// `--max-retries 6` 封顶整条链（快速+慢速一起数，spec §6.2 末句）：第 7 枪失败即停 ⇒
// 整跑 ≈ 31 + 30 + 开销。**6 是能量到 `x1` 的最小封顶**：5 只走到快用尽（attempt 到 5，
// `slow_retry_rounds` 恒 0），那一档 `slow-retry` 行根本不会出现 ⇒ 换不来这一发要买的东西。

/// **spec §6.2 / E28⑫ 同族登记（证据文件 §6 第 5 条）的端到端一发**：真 `dlm.exe`（`run_dlm`，
/// 不是 `engine::run`）对 `{base}/probe-only`，**不带 `--quiet`**（它从不挂 sink，E24①，
/// `the_binary_maps_ok_to_the_exit_code_and_quiet_to_never_installing_the_sink` 钉着；带它这一发
/// 量的就不是进度行了），`--max-retries 6` 让整跑能到头而不是无限慢重试。判据一条一个方向：
/// - **主判据**：stdout 里存在带着 `slow-retry x` 且 N ≥ 1 的进度行。这一行必须**同时**带着
///   百分比格与 `MiB/MiB` 对（否则将来渲染器一改，这一发就退化成"只认得 slow-retry"的空壳）。
/// - **退出档**：非零，且措辞是 `main` 的 `ok=false` 结果那一支（"下载未完成" + "重试到顶"），
///   与"参数写错了"（参数层，E26/C3 的禁词写法）和"跑不起来"（`Err` 那一支，试探都没成功）分得开。
/// - **盘上形状**（spec §6.2/§6.7：失败不删用户可能还要的东西）：断点文件活着、能读回来、
///   全长对、种子那一格在；目标文件是**部分但内容自洽**的前缀（按 `positioned_body` 逐字节比）。
/// - **对端自己的账**（仪器侧判据，字段一律**按名字**取——规则 4，两种事件的 JSON 字段顺序相反，
///   按列位对号读的是仪器不是产品）：试探的限量 Range 恰好一条且被**正常服务**（206 / 236 032 B）；
///   至少 6 条 worker 枪被拒（同一链条排下第一发慢重试的前提）；每一条被拒的 Range 起点 ≥ 种子之后。
#[test]
fn the_binary_prints_slow_retry_rounds_when_every_worker_shot_is_refused() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("slow-retry")?;
    let announced = srv.announced()?;
    let len = announced.size; // 由这台服务器自己宣布（不抄历史值，与场景 C 同一口径）
    let url = format!("{base}/probe-only");
    let out_dir = dir.path().display().to_string();
    let out_name = "slow-retry.bin";
    let target = dir.path().join(out_name);
    let state_path = dlm::state::path_for(&target);
    let probe_range = format!("bytes=0-{}", PROBE_READ_BYTES - 1);

    let started = Instant::now();
    let run = run_dlm(&[
        "--url", &url, "--out-dir", &out_dir, "--out-name", out_name,
        "--connections", "8", "--max-retries", "6",
    ])?;
    let wall = started.elapsed();
    let so = stdout_of(&run);
    let se = stderr_of(&run);

    // —— 主判据：真进度行带着 `slow-retry xN`（N ≥ 1），而且那一行不是空壳 ——
    // 分段按 `\r`（`tick_sink` 每发都以 `\r` 开头，E25 的分工），但**最后一发会被 `finish` 的
    // 让行换行与结果块接在后面**（实测 2026-10-18：`… slow-retry x1\n下载未完成 …`）⇒ 每段先
    // 截到第一个 `\n` 为止再拆格。截断只影响"尾巴上挂了什么"，不影响"这一行本身是什么形状"。
    let slow_lines: Vec<&str> = so
        .split('\r')
        .map(|segment| segment.split('\n').next().unwrap_or(segment))
        .filter(|line| line.contains("slow-retry x"))
        .collect();
    assert!(
        !slow_lines.is_empty(),
        "整跑 {wall:?} 没有任何一行进度带 `slow-retry x` ⇒ spec §6.2 那一格端到端仍然没被观测到。\
         stdout 前 400 字节：{:?}；stderr：{se}",
        so.get(..400)
    );
    let mut max_rounds = 0u32;
    for line in &slow_lines {
        let cells = tick_cells(line);
        assert!(cells.len() >= 4, "带 slow-retry 的行至少要有前四格（百分比 / MiB 对 / 速率 / x 连接数）：{line:?}");
        let pct = cells[0]
            .strip_suffix('%')
            .with_context(|| format!("进度行第一格不是百分比：{line:?}"))?;
        assert!(is_one_decimal(pct), "百分比要是一位小数的数：{line:?}");
        let (done, total) = cells[1]
            .strip_suffix(" MiB")
            .and_then(|s| s.split_once(" MiB/"))
            .unwrap_or_else(|| panic!("第二格该是 `<已下> MiB/<全长> MiB`：{line:?}"));
        assert!(is_one_decimal(done) && is_one_decimal(total), "那对 MiB 都要是一位小数：{line:?}");
        assert_eq!(total, "48.0", "全长那一格就是这台服务器宣布的 48 MiB（E25① 的单位口径）：{line:?}");
        for extra in cells.iter().skip(4) {
            if let Some(rounds) = extra.strip_prefix("slow-retry x") {
                assert!(
                    !rounds.is_empty() && rounds.chars().all(|c| c.is_ascii_digit()),
                    "慢重试那一格要给得出轮数：{line:?}"
                );
                // `unwrap_or(0)`：解析炸了就落在上面那条形状判据的红里，不让测试炸在解析上。
                max_rounds = max_rounds.max(rounds.parse::<u32>().unwrap_or(0));
            } else {
                assert!(extra.starts_with("dup "), "第四格之后只许是 `dup X%` 或 `slow-retry xN`：{line:?}");
            }
        }
    }
    assert!(
        max_rounds >= 1,
        "`slow-retry xN` 的 N 至少要到 1（判据⑥的首个非零值：attempt 6 − FAST_RETRY_MAX 5）：{slow_lines:?}"
    );

    // —— 退出档与措辞：跑起来了、但重试到顶；既不是参数层也不是 `Err` 层 ——
    assert_ne!(
        run.status.code(),
        Some(0),
        "全部 worker 枪被拒的整跑不许退 0：exit={:?}，stderr：{se}",
        run.status.code()
    );
    assert!(so.contains("下载未完成"), "结果行要走 `main` 的 `ok=false` 那一支：{so}");
    assert!(so.contains("重试到顶"), "那句说明要指名「重试到顶」，这是封顶那一支的专属措辞：{so}");
    for banned in ["参数写错了", "跑不起来"] {
        assert!(
            !so.contains(banned) && !se.contains(banned),
            "两条流里出现 {banned:?} 就说明档位不对：`参数写错了` 是参数层（E26/C3），\
             `跑不起来` 是 `main` 的 `Err` 那一支（那意味着连试探都没成功，本发要量的形状根本没发生）。\
             stderr：{se}"
        );
    }
    // 退避账：走到第一发慢重试并让第 7 枪撞封顶，至少要在 1+2+4+8+16（31 s）之后再睡 30 s。
    // 这一句防的是"退避根本没真睡"的实现回归；上界不钉（机器负载不可控），如实只钉下界。
    assert!(
        wall >= Duration::from_secs(61),
        "退避链 1+2+4+8+16+30 = 61 s 还没走完就收工 ⇒ 快速/慢速退避至少有一档没真睡（实测 {wall:?}）"
    );

    // —— 盘上形状：断点活着、种子在账、目标文件是部分但内容自洽的前缀 ——
    assert!(
        state_path.exists(),
        "spec §6.2/§6.7：重试到顶的失败不许删断点（那是下一轮续起的依据）：{}",
        state_path.display()
    );
    let snap = dlm::state::load(&state_path)?.context("断点文件存在却读不回来（损坏？）")?;
    assert_eq!(snap.validator.total_len, len, "断点记的全长要与这台服务器宣布的一致");
    let covered: Vec<(u64, u64)> = snap.segments.iter().map(|s| (s.start, s.end)).collect();
    let covered_bytes: u64 = covered.iter().map(|(s, e)| e - s + 1).sum();
    assert!(
        covered.contains(&(0, PROBE_READ_BYTES - 1)),
        "试探那一枪的 [0, {}] 种子必须已被断点认领（它成功了才有后面的形状）：{covered:?}",
        PROBE_READ_BYTES - 1
    );
    assert!(target.exists(), "失败之后目标文件不在 ⇒ 连半截前缀都没留下");
    let partial_len = std::fs::metadata(&target)?.len();
    assert!(
        covered_bytes <= partial_len,
        "断点说归我的字节（{covered_bytes} B）比盘上真有的（{partial_len} B）还多 ⇒ 断点对磁盘说谎（E17 那本账）"
    );
    assert!(
        partial_len >= PROBE_READ_BYTES && partial_len < len,
        "这一跑只有试探那 {} B 真落了盘（其余枪全被拒、零字节落地）：实测半截 {partial_len} B / 全长 {len} B",
        PROBE_READ_BYTES
    );
    let want = harness::positioned_body(usize::try_from(partial_len)?);
    assert_eq!(
        std::fs::read(&target)?,
        want,
        "半截那一段必须与服务器同偏移的内容逐字节一致（部分，但要自洽）：{partial_len} B"
    );

    // —— 对端自己的账：先守卫"两条形状都已落盘"，再取全量计数 ——
    srv.events_while(|evs| {
        evs.iter().any(|e| e["ev"] == "response_refuse")
            && evs.iter().any(|e| e["ev"] == "response_done" && e["url"] == "/probe-only")
    })?;
    let evs = srv.events()?;
    let served: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|e| e["ev"] == "response_done" && e["url"] == "/probe-only")
        .collect();
    assert_eq!(
        served.len(),
        1,
        "整跑只许有**一条**被正常服务的响应，而且它是试探那一枪（其余全被拒）：{served:?}"
    );
    assert_eq!(
        served[0]["status"].as_u64(),
        Some(206),
        "试探那一枪是 206 限量读，不是整份 200：{:?}",
        served[0]
    );
    assert_eq!(
        served[0]["bytes"].as_u64(),
        Some(PROBE_READ_BYTES),
        "试探那一枪交出的字节数正好是限量读（种子就是它）：{:?}",
        served[0]
    );
    let probe_shots = evs
        .iter()
        .filter(|e| e["ev"] == "request" && e["range"].as_str() == Some(probe_range.as_str()))
        .count();
    assert_eq!(probe_shots, 1, "带试探限量 Range 的请求必须恰好一条（多一条就是回头重取/重试探）：{probe_range}");
    let refused: Vec<&serde_json::Value> = evs.iter().filter(|e| e["ev"] == "response_refuse").collect();
    assert!(
        refused.len() >= 6,
        "封顶 6 的那条链要先被拒满 6 次才排下第一发慢重试 ⇒ 到顶时至少已有 6 条拒绝；实测 {} 条。\
         少于 6 而进度行又带着 slow-retry，只能说明这一发走的不是本设计形状。",
        refused.len()
    );
    for e in &refused {
        let range = e["range"].as_str().with_context(|| format!("拒绝行没带 range：{e:?}"))?;
        let (s, _) = harness::wire_span(range, len)?;
        assert!(
            s >= PROBE_READ_BYTES,
            "被拒的那一枪起点 {s} 落在种子 [0, {}) 之内 ⇒ 要么对端把试探形状也拒了（那会退化成 \
             `Err` 那一支），要么引擎回头重取已确认的字节：{e:?}",
            PROBE_READ_BYTES - 1
        );
    }

    eprintln!(
        "T11 取证：用时 {wall:?}、exit={:?}、带 slow-retry 的进度行 {} 行（最大 x{max_rounds}）、\
         拒绝 {} 条、正常服务 {} 条、半截文件 {partial_len} B、断点 {} 格 / {covered_bytes} B；\
         样例行（取最大轮数那一行）：{:?}",
        run.status.code(),
        slow_lines.len(),
        refused.len(),
        served.len(),
        covered.len(),
        slow_lines
            .iter()
            .find(|line| {
                tick_cells(line)
                    .iter()
                    .any(|c| c.strip_prefix("slow-retry x").and_then(|n| n.parse::<u32>().ok()) == Some(max_rounds))
            })
            .copied()
            .unwrap_or_else(|| slow_lines[0])
    );
    Ok(())
}
