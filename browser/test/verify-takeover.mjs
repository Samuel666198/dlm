// downloads 接管判定逻辑（takeover.js）的纯逻辑验证。不启动浏览器。
//
// 为什么这条要进 CI 门禁：`shouldTakeover` 决定"要不要抢浏览器自己的下载"，
// 判错的代价是**用户两边都没下成**。它的每条规则都刻意往"不接"的方向保守，
// 而保守这件事不会自己保持 —— 后来的人很容易为了"覆盖更多"把某一条件放宽。
//
// takeover.js 本身就是无 chrome 依赖、导出 module.exports 的纯函数（它在注释里
// 写着"便于 node 直接测"），所以这里可以直接 require 进来逐个分支打。
//
// 用法：node browser/test/verify-takeover.mjs
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const tk = require(join(here, "..", "extension", "takeover.js"));

const failures = [];
let checks = 0;

function ok(cond, message) {
  checks += 1;
  if (!cond) failures.push(message);
}

function eq(actual, expected, message) {
  checks += 1;
  if (actual !== expected) failures.push(`${message}（期望 ${expected}，实际 ${actual}）`);
}

const cfg = (over = {}) => ({
  enabled: true,
  whitelist: [],
  mimeWhitelist: [],
  blocklist: [],
  minBytes: 0,
  ...over,
});

const item = (over = {}) =>
  ({
    id: 1,
    state: "in_progress",
    bytesReceived: 0,
    url: "https://example.com/a.zip",
    finalUrl: "https://example.com/a.zip",
    filename: "C:\\Users\\x\\Downloads\\a.zip",
    referrer: "https://example.com/",
    ...over,
  });

// ---- 总开关 ----
eq(tk.shouldTakeover(item(), cfg({ enabled: false })).take, false, "开关关着就绝不动手");
eq(tk.shouldTakeover(item(), cfg({ enabled: false })).reason, "disabled", "关着的原因要能说出来");
eq(tk.shouldTakeover(null, cfg()).take, false, "没有 item 不许崩");
eq(tk.shouldTakeover(item(), null).take, false, "没有配置不许崩");

// ---- 已经开始 / 已经结束的不要碰 ----
eq(
  tk.shouldTakeover(item({ state: "complete" }), cfg()).take,
  false,
  "已经下完的不要再接一次",
);
eq(
  tk.shouldTakeover(item({ bytesReceived: 1024 }), cfg()).take,
  false,
  "已经下了字节的就别抢（抢了等于白下那部分）",
);

// ---- 没有可交给下载器的 URL ----
eq(tk.shouldTakeover(item({ finalUrl: "blob:https://x/1", url: "blob:https://x/1" }), cfg()).take, false, "blob 没有独立 URL");
eq(tk.shouldTakeover(item({ finalUrl: "data:text/plain,hi", url: "data:text/plain,hi" }), cfg()).take, false, "data 同理");

// ---- 扩展名 / MIME 白名单 ----
eq(tk.shouldTakeover(item(), cfg()).take, true, "zip 属于默认白名单，应接");
eq(
  tk.shouldTakeover(item({ url: "https://example.com/page.html", finalUrl: "https://example.com/page.html", filename: "C:\\x\\page.html" }), cfg()).take,
  false,
  "html 页面不在白名单：宁可漏接",
);
eq(
  tk.shouldTakeover(
    item({ url: "https://example.com/x", finalUrl: "https://example.com/x", filename: "C:\\x\\x", mime: "application/pdf" }),
    cfg(),
  ).take,
  true,
  "没有扩展名但 MIME 明确是文件时要接",
);
eq(
  tk.shouldTakeover(
    item({ url: "https://e.com/x", finalUrl: "https://e.com/x", filename: "C:\\x\\x", mime: "application/octet-stream" }),
    cfg(),
  ).take,
  false,
  "application/octet-stream 太泛，不认（否则会把整站当文件）",
);
eq(
  tk.shouldTakeover(
    item({ url: "https://e.com/v", finalUrl: "https://e.com/v", filename: "C:\\x\\v", mime: "video/mp4" }),
    cfg(),
  ).take,
  true,
  "音视频按大类放行",
);

// ---- 站点黑名单 ----
eq(
  tk.shouldTakeover(item(), cfg({ blocklist: ["example.com"] })).take,
  false,
  "命中黑名单就连白名单里的类型也不接",
);
eq(tk.shouldTakeover(item(), cfg({ blocklist: ["example.com"] })).reason, "site", "原因要能区分是站点规则");
eq(
  tk.shouldTakeover(item(), cfg({ blocklist: ["notexample.com"] })).take,
  true,
  "黑名单必须是按域名后缀匹配，不能子串撞上",
);

// ---- 最小体积 ----
eq(
  tk.shouldTakeover(item({ fileSize: 100 }), cfg({ minBytes: 1024 })).take,
  false,
  "小于体积下限的不接",
);
eq(
  tk.shouldTakeover(item({ fileSize: 100 }), cfg({ minBytes: 1024 })).reason,
  "too-small",
  "原因要能说出是体积",
);
eq(
  tk.shouldTakeover(item({ fileSize: 2048 }), cfg({ minBytes: 1024 })).take,
  true,
  "够了就该接",
);
eq(
  tk.shouldTakeover(item({ fileSize: 0 }), cfg({ minBytes: 1024 })).take,
  true,
  "体积未知（0）时不能因为「不够大」就拒 —— 宁可接",
);

// ---- 扩展名解析本身 ----
eq(tk.extOf("https://e.com/a.ZIP?x=1#y"), "zip", "要有查询串/锚点时也能取到扩展名");
eq(tk.extOf("C:\\dl\\b.tar.gz"), "gz", "取最后一段");
eq(tk.extOf("noext"), "", "没有扩展名就是空");
eq(tk.extOf(""), "", "空串不许崩");

if (failures.length) {
  console.error("takeover check FAILED");
  for (const f of failures) console.error(`- ${f}`);
  process.exit(1);
}
console.log(`takeover check ok: ${checks} 条判定`);
