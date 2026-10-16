//! 集成测试：真发 HTTP，对端是本仓库自己的 node 探针服务器（`tests/harness/`）。
//!
//! `tests/harness/mod.rs` **不是** cargo 自动发现的目标（只有 `tests/*.rs` 与
//! `tests/<dir>/main.rs` 才是测试目标，派发第 10 条）⇒ 用 `#[path]` 显式接进来。
//! `allow(dead_code)`：harness 是 Task 7/8/10 共用的夹具，本目标里暂时没人调的项不该把
//! `cargo clippy --all-targets -- -D warnings` 这道门禁弄红。
#[allow(dead_code)]
#[path = "harness/mod.rs"]
mod harness;

use anyhow::Context;
use dlm::consts::PROBE_READ_BYTES;
use dlm::probe::{self, ProbeResult};
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
    assert!(r.facts.accept_ranges, "落地那条自己发了 Accept-Ranges: bytes");
    assert_eq!(r.facts.etag.as_deref(), Some("\"stub\""), "ETag 原样保留，含协议要求的引号");
    assert_eq!(r.facts.total_len, 0, "落地那条是 Content-Length: 0");
    assert!(stub.saw("/landed"), "必须真的跟随了重定向：{:?}", stub.seen());
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
