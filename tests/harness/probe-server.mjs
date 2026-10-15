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
  const slow = req.url.startsWith("/slow") || (mSz && mSz[2] === "s");
  const wantSz = mSz ? Math.min(parseInt(mSz[1], 10), SIZE) : (flaky ? 4 * 1024 * 1024 : SIZE);
  const sendLen = wantSz;
  if (req.url.startsWith("/noranges")) {
    log("request", { ...head, served: "noranges" });
    res.writeHead(200, { "Content-Type": "application/octet-stream",
      "Content-Length": String(Math.min(sendLen, 1024 * 1024)), "ETag": ETAG });
    // 故意不回 Accept-Ranges，且无视 Range：始终从 0 发满 1 MiB
    res.end(BLOB.subarray(0, Math.min(sendLen, 1024 * 1024)));
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
  (function push() {
    // 必须 res.end()：不收尾的响应会让 node 把这条连接后续所有响应的字节永久排队
    // （socket._httpMessage 一直指向这条未完成的响应），于是同连接第二个请求永远拿不到
    // 响应——curl / 裸 socket / reqwest 都一样卡住。Task 0 的"416 超时"就是这个。
    if (off > end) { res.end(); log("response_done", { conn: rec.id, url: req.url, status, bytes: sent, chunks: chunkN }); return; }
    const c = BLOB.subarray(off, Math.min(off + CH, end + 1));
    off += c.length; sent += c.length; chunkN++;
    if (!res.write(c)) res.once("drain", push); else if (slow) setTimeout(push, 200); else setImmediate(push);
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
