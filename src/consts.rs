/// 试探请求是否必须带 `Connection: close`。实测结论：**不需要**（spec 勘误 R4 的怀疑不成立）。
///
/// R4 记的"416 请求 `operation timed out`"根因不在 reqwest、也不在连接池，而在本仓库的测试
/// 服务器 `tests/harness/probe-server.mjs`：它推 body 的循环写满后只 `log("response_done")`
/// 就 return，从不 `res.end()`。node 因此把这条 keep-alive 连接的 `_httpMessage` 永久留给那条
/// 未收尾的响应，后续响应的字节全进 `socket._pending` 永不 flush——服务端日志显示它收到了第二个
/// 请求（`reqNoOnConn=2`）也"写了" 416，而客户端一个字节都收不到，直到超时。
/// 判别实验（curl / 裸 node socket / reqwest 三者同样表现，与客户端库无关）：
/// - 前一个响应带 body 且不收尾 → 同连接的第二个请求 0 字节直到超时（206→206 也照样卡，与 416 无关）；
/// - 唯一变量改为 `res.end()` 收尾（`end` / `noend` 两个最小服务器，脚本临时落在 gitignored 的 `target/`，
///   不是交付物）→ 立刻正常；
/// - 416 自身是 `Content-Length: 0`，所以只有"前面还有一个没收尾的带 body 响应"时才暴露。
/// 补上 `res.end()` 之后重测（完整四条转录见 Task 0 报告 `.superpowers/sdd/2026-10-15-dlm-kernel/task-0-report.md`，
/// 该目录同样是 gitignored 暂存区）：
/// - 试探只读 236032 字节就 drop：reqwest 0.13.5 压根不把手工丢弃的连接放回池（harness 侧看到连接
///   被立即拆掉、下一个请求走的是新连接），"脏连接被复用"这个机制不存在；
/// - 前一个 206 的 body 读满后 drop：连接入池并被复用，416 就在那条复用连接上 1 ms 内返回。
/// ⇒ 试探请求无需 `Connection: close`，也无需全局 `pool_max_idle_per_host(0)`；该做的是让 harness
/// 收尾响应（已在本任务修好）。Task 6/7 若仍按计划加 close，只是多拆一条连接，不影响正确性。
pub const PROBE_USES_CONNECTION_CLOSE: bool = false;
