// IDM 分段行为探针服务器（本机 127.0.0.1，只服务自建 blob，不代理任何第三方站点）
// 记录：每条请求的 socket、Range、字节数、连接是否被复用(keep-alive)、以及连接生命周期
// 用法: node probe-server.mjs <port> <日志路径>
import http from "node:http";
import fs from "node:fs";
import crypto from "node:crypto";

const port = parseInt(process.argv[2] || "8077", 10);
const logPath = process.argv[3] || "segment-log.jsonl";
const SIZE = 48 * 1024 * 1024;            // 48 MiB
const BLOB = Buffer.allocUnsafe(SIZE);    // 确定性内容：每 4 KiB 块填块号
for (let i = 0; i < SIZE; i += 4096) BLOB.writeUInt32LE(i / 4096, i);
const SHA = crypto.createHash("sha256").update(BLOB).digest("hex");
const ETAG = '"probe-' + SHA.slice(0, 16) + '"';
const LMOD = new Date(Date.now() - 3600e3).toUTCString();

const t0 = Date.now();
const out = fs.createWriteStream(logPath, { flags: "a" });
const log = (ev, o) => out.write(JSON.stringify({ t: Date.now() - t0, ev, ...o }) + "\n");

let connSeq = 0;
const socks = new Map(); // socket -> {id, reqs, opened, last}

http.createServer((req, res) => {
  const s = req.socket;
  if (!socks.has(s)) socks.set(s, { id: ++connSeq, reqs: 0, opened: Date.now() });
  const rec = socks.get(s);
  rec.reqs++;
  rec.last = Date.now();
  const head = {
    method: req.method,
    url: req.url,
    conn: rec.id,
    reqNoOnConn: rec.reqs,
    remotePort: s.remotePort,
    "user-agent": req.headers["user-agent"],
    connection: req.headers.connection,
    accept_encoding: req.headers["accept-encoding"],
    if_range: req.headers["if-range"],
    if_modified_since: req.headers["if-modified-since"],
    cookie: req.headers.cookie,
    referer: req.headers.referer,
    host: req.headers.host,
    range: req.headers.range,
    via: req.headers.via,
    proxy: req.headers["proxy-connection"]
  };
  // 任何非标准头（IDM 可能自加）单独记下来
  const known = new Set(["host","connection","range","user-agent","accept-encoding","if-range","if-modified-since","cookie","referer","via","proxy-connection"]);
  head.extra_headers = Object.fromEntries(Object.entries(req.headers).filter(([k]) => !known.has(k)));
  log("request", head);

  const flaky = req.url.startsWith("/flaky");
  // /sz/<字节数>[s]  s 表示限速；/slow 等价于 /sz/48MiB 且限速
  const mSz = /^\/sz\/(\d+)(s?)/.exec(req.url);
  // /drop/<字节数>  n 计的是**本条响应写出的 body 字节数**（不是文件偏移），可与 Range 组合
  const mDrop = /^\/drop\/(\d+)/.exec(req.url);
  const slow = req.url.startsWith("/slow") || (mSz && mSz[2] === "s");
  const wantSz = mSz ? Math.min(parseInt(mSz[1], 10), SIZE) : (flaky ? 4 * 1024 * 1024 : SIZE);
  const sendLen = wantSz;
  if (req.url.startsWith("/noranges")) {
    // 每条请求只留一条 `request` 记录：上面那条无条件记录就是它。简报片段曾在这里又打一次
    // `log("request", ...)`，于是 /noranges 路径每条请求计两条——Task 6/7/10 用这份 JSONL 断言
    // 请求数与 Range 序列时会偏一位。`served` 判别位改记在本分支的 `response` 记录上
    // （与 416 / HEAD 分支同一形状），既不再重复记请求，也仍然辨得出是谁服务了这条请求。
    const n = Math.min(sendLen, 1024 * 1024);
    res.writeHead(200, { "Content-Type": "application/octet-stream",
      "Content-Length": String(n), "ETag": ETAG });
    // 故意不回 Accept-Ranges，且无视 Range：始终从 0 发满 1 MiB
    res.end(BLOB.subarray(0, n));
    log("response", { ...head, served: "noranges", status: 200, bytes: n });
    return;
  }
  const rng = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || "");
  let start = 0, end = sendLen - 1, status = 200;
  if (rng && (rng[1] || rng[2])) {
    start = rng[1] ? parseInt(rng[1], 10) : Math.max(0, sendLen - parseInt(rng[2], 10));
    end = rng[1] && rng[2] ? Math.min(parseInt(rng[2], 10), sendLen - 1) : sendLen - 1;
    if (start >= sendLen || start > end) { res.writeHead(416, { "Content-Range": "bytes */" + sendLen, "Content-Length": "0" }); res.end(); log("response", { ...head, status: 416, bytes: 0 }); return; }
    status = 206;
  }
  const len = end - start + 1;
  // /drop/<n>：服务端在 body 中间把连接拆掉的对端（Task 0 修复轮 3 新增）。
  // 存在的原因：spec §7 场景 3 写的是"`/flaky`（中途断）"，而 `/flaky` 实测从不中断——它只是
  // 把服务长度改成 4 MiB（`wantSz`）后照常走完 `res.end()`。"响应比 Content-Length 短"这一
  // 病症以前在本 harness 上没有对端，拿 `/flaky` 写中断断言会**碰巧通过**。
  // 本路由不改任何响应头的算法：状态码、Content-Length、Content-Range、Accept-Ranges、ETag、
  // Last-Modified 全都按正常路径如实算出来（也就是说头是诚实的、body 是不完整的），
  // 只在 body 写到第 n 字节之后 destroy socket、永不收尾（`n=0` ⇒ 头发完、body 一个字节不发）。
  // n >= 本条响应总长时没有可截的东西，响应正常结束（等价于不打 `/drop`）。
  // 这不违反下面那段 `res.end()` 纪律：那条缺陷是"连接还活着、响应没收尾"，于是同连接的第二个
  // 请求被挂进 pending 队列。这里我们把 socket 直接拆掉，这条连接不可能再服务第二个请求。
  const dropAt = mDrop && parseInt(mDrop[1], 10) < len ? parseInt(mDrop[1], 10) : -1;
  const bodyEnd = dropAt >= 0 ? start + dropAt - 1 : end;   // 本次要写到的最后一个字节下标
  const headers = {
    "Content-Type": "application/octet-stream",
    "Content-Length": String(req.method === "HEAD" ? sendLen : len),
    "Accept-Ranges": "bytes",
    "ETag": ETAG,
    "Last-Modified": LMOD
  };
  if (status === 206) headers["Content-Range"] = `bytes ${start}-${end}/${sendLen}`;
  res.writeHead(status, headers);
  if (req.method === "HEAD") { res.end(); log("response", { ...head, status, bytes: 0, total: sendLen }); return; }
  // 分块写出，边写边看 IDM 是否在中途换连接
  let off = start, sent = 0, chunkN = 0;
  const CH = 64 * 1024;
  const dropNow = () => {
    // 事件类型是新的 `response_drop`，与顶部那条唯一的 `request`、与正常收尾的 `response_done`
    // 都区分得开（`/noranges` 那条"一条请求只留一条 request 记录"的教训不能再犯一遍）
    log("response_drop", { conn: rec.id, url: req.url, status, bytes: sent, chunks: chunkN,
      expected: len, drop_at: dropAt, range: req.headers.range || null, total: sendLen });
    if (res.socket) res.socket.destroy(); else req.destroy();
  };
  (function push() {
    // 必须 res.end()：本机实测（Task 0）——写满后不收尾时，node 把这条 keep-alive 连接后续响应
    // 的字节留在 socket 的 pending 队列里不再 flush：服务端日志显示它解析并回调了第二个请求、
    // handler 也 writeHead+end 了，而客户端在超时内一个字节收不到（206→206 与 206→416 同样表现）。
    // 在这台 harness 上 curl / 裸 node socket / reqwest 三种客户端观测到同一现象，说明与本服务器的
    // 收尾方式有关。以上只是本 harness 的实测形状，不对其他服务器或客户端库作任何断言。
    if (off > end) { res.end(); log("response_done", { conn: rec.id, url: req.url, status, bytes: sent, chunks: chunkN }); return; }
    const c = BLOB.subarray(off, Math.min(off + CH, bodyEnd + 1));
    off += c.length; sent += c.length; chunkN++;
    if (dropAt >= 0 && off > bodyEnd) {
      // 最后一块已被 `bodyEnd` 截短到"恰好 dropAt 字节"。等 node 把这块 flush 出去（`write` 回调）
      // 再拆 socket：若写完立刻 destroy，还在 socket 发送缓冲里没出手的字节会一起丢掉，
      // 客户端收到的就不是 n 字节，`n` 的语义（"本条响应写出的 body 字节数"）就不可复现了。
      // `dropAt === 0` 也走这里，此时 c 是 0 字节：这次空写的作用是把响应头 flush 出去
      // （`writeHead()` 只缓存、不落 socket；实测直接 destroy 会让客户端一个头都收不到，
      // 那就成了"没有响应"而不是"响应被截断"）。
      res.write(c, dropNow);
      return;
    }
    // `slow` 必须在两条恢复路径上都成立：64 KiB 的写会让 `res.write()` 返回 false（自己限速必然
    // 触发 backpressure），于是恢复只能从 `drain` 回来。若 drain 回调直接 `push()`，节拍就被绕过 ——
    // round 1 实测：`/slow` 48 MiB 只花 114 ms、`/sz/1048576s` 1 ms，"限速"名不副实。
    // 所以 drain 与"写得进去"两条分支统一走 resume()：限速时每块之间 sleep 200 ms，否则 setImmediate。
    const resume = () => (slow ? setTimeout(push, 200) : setImmediate(push));
    if (!res.write(c)) res.once("drain", resume); else resume();
  })();
  res.on("close", () => log("conn_close_side", { conn: rec.id, url: req.url, sent, pending: off <= end, ms: Date.now() - t0 }));
}).on("connection", s => {
  const id = ++connSeq;
  if (!socks.has(s)) socks.set(s, { id, reqs: 0, opened: Date.now() });
  log("socket_open", { remotePort: s.remotePort, seq: id });
  s.on("close", () => { const r = socks.get(s); log("socket_close", { seq: r ? r.id : id, reqs: r ? r.reqs : 0, aliveMs: r ? Date.now() - r.opened : 0 }); });
  s.on("error", e => log("socket_error", { seq: id, msg: e.message }));
}).listen(port, "127.0.0.1", () => {
  log("server_start", { port, size: SIZE, sha256: SHA, etag: ETAG, lastModified: LMOD });
  console.log("探针服务器已起 http://127.0.0.1:" + port + "  blob=" + SIZE + " sha256=" + SHA.slice(0, 16) + "  日志=" + logPath);
});
