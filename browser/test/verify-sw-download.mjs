// Service Worker 的**行为**检查：真的把 background 跑起来，发一条 `page-download`，
// 断言 native 端口收到了 `op: "download"`。
//
// 为什么非要有这一条（2026-11-27 用户报「插件能拦截下载，但点下载没反应、
// 桌面端也没显示」）：
// 上一版把 `isVideoPage` / `probeSize` 插进了 `installMenu()` 的函数体里，
// 成了**函数内局部声明**。`submitPage` 里那句 `isVideoPage(url)` 抛 ReferenceError，
// 后面的 `submitTask(task)` 再也不执行 —— 任务根本没发出去。
//
// 这种错 `node --check` 与文本级静态检查**都看不见**：语法完全合法、
// 关键字也都在文件里。只有真的跑一遍才咬得住。
//
// 用法：node browser/test/verify-sw-download.mjs
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const ext = join(here, "..", "extension");
const manifest = JSON.parse(readFileSync(join(ext, "manifest.json"), "utf8"));
const swFile = manifest.background.service_worker;
const swSrc = readFileSync(join(ext, swFile), "utf8");

const failures = [];
const sent = []; // native 端口收到的每一条消息

// 未捕获的 Promise rejection 必须**判红**。
// 踩过的坑：那一版 bug 是在 `cookieHeader(...).then()` 里抛的 ReferenceError，
// Node 只打印一行、进程照样退 0 —— 用例于是"通过"，而用户那边什么都发不出去。
process.on("unhandledRejection", (err) => {
  failures.push(`未捕获的 Promise rejection：${String(err && err.stack ? err.stack : err)}`);
});
const listeners = { message: [], installed: [], startup: [] };

// ---- chrome 桩：只实现 SW 真的会碰到的入口 ----
const portStub = {
  postMessage(msg) {
    sent.push(msg);
  },
  onMessage: { addListener() {} },
  onDisconnect: { addListener() {} },
  disconnect() {},
};

const storageArea = () => ({
  get: () => Promise.resolve({}),
  set: () => Promise.resolve(),
});

const chromeStub = {
  runtime: {
    lastError: undefined,
    getManifest: () => ({ version: manifest.version }),
    connectNative: () => portStub,
    onMessage: { addListener: (fn) => listeners.message.push(fn) },
    onInstalled: { addListener: (fn) => listeners.installed.push(fn) },
    onStartup: { addListener: (fn) => listeners.startup.push(fn) },
  },
  storage: {
    session: storageArea(),
    local: storageArea(),
    onChanged: { addListener() {} },
  },
  cookies: {
    getAll: (_filter, cb) => cb([]),
  },
  contextMenus: {
    removeAll: (cb) => cb && cb(),
    create: () => {},
    onClicked: { addListener() {} },
  },
  downloads: {
    cancel: () => {},
    erase: () => {},
    onCreated: { addListener() {} },
  },
  scripting: { executeScript: () => Promise.resolve([]) },
  tabs: {
    sendMessage: () => Promise.resolve(),
    onRemoved: { addListener() {} },
    onUpdated: { addListener() {} },
  },
  webRequest: {
    onBeforeRequest: { addListener() {} },
    onHeadersReceived: { addListener() {} },
  },
  action: {
    setBadgeText: () => Promise.resolve(),
    setBadgeBackgroundColor: () => Promise.resolve(),
  },
};

const sandbox = {
  chrome: chromeStub,
  console,
  URL,
  Map,
  Set,
  Promise,
  Array,
  Object,
  JSON,
  String,
  Number,
  RegExp,
  Date,
  Math,
  Error,
  TypeError,
  fetch: () => Promise.resolve({ headers: { get: () => null } }),
  AbortSignal: { timeout: () => undefined },
  setTimeout: (fn) => {
    // 不让重连计时器把测试拖住：延迟到微任务之外不执行
    void fn;
    return 0;
  },
  clearTimeout: () => {},
  importScripts: () => {},
};
sandbox.globalThis = sandbox;
vm.createContext(sandbox);

try {
  vm.runInContext(swSrc, sandbox, { filename: swFile });
} catch (err) {
  console.error(`verify-sw-download FAILED：${swFile} 跑不起来`);
  console.error("-", String(err));
  process.exit(1);
}

// ---- 判据 1：关键函数必须在**顶层**（这正是那一版翻车的地方）----
for (const name of ["isVideoPage", "probeSize", "submitPage", "submitTask"]) {
  const type = vm.runInContext(`typeof ${name}`, sandbox);
  if (type !== "function") {
    failures.push(`${name} 不在顶层（typeof=${type}）—— 它被声明进了某个函数体，调用处会抛 ReferenceError`);
  }
}

// ---- 判据 2：发一条 page-download，native 端口必须真的收到 op:"download" ----
const onMessage = listeners.message[0];
if (!onMessage) {
  failures.push("SW 没有注册 runtime.onMessage 监听");
} else {
  let responded = null;
  onMessage(
    { op: "page-download", url: "https://example.com/files/report.zip", referer: "https://example.com/page" },
    { tab: { id: 7 } },
    (r) => {
      responded = r;
    },
  );

  // cookieHeader 是 Promise 链，给它几个微任务的时间
  await new Promise((resolve) => setTimeout(resolve, 20));

  const download = sent.find((m) => m && m.op === "download");
  if (!download) {
    failures.push(
      `点了下载但 native 端口一条 op:"download" 都没收到（收到的是：${JSON.stringify(sent)})` +
        " —— 这就是「插件能拦截、点下载没反应、桌面端也没有」的形态",
    );
  } else {
    if (download.url !== "https://example.com/files/report.zip") {
      failures.push(`投递的 url 不对：${download.url}`);
    }
    if (!download.referer) {
      failures.push("投递没有带 referer（不少站点会 403）");
    }
    if (download.video !== undefined && typeof download.video !== "boolean") {
      failures.push(`video 标记类型不对：${typeof download.video}`);
    }
  }
  if (!responded || responded.ok !== true) {
    failures.push(`sendResponse 没有回 ok:true（实际：${JSON.stringify(responded)}）—— 内容脚本会以为投递失败`);
  }
}

// ---- 判据 3：视频页要打上 video 标记（否则又去下网页）----
{
  sent.length = 0;
  onMessage(
    { op: "page-download", url: "https://www.bilibili.com/video/BV1atse6EEfx", referer: "https://www.bilibili.com" },
    { tab: { id: 8 } },
    () => {},
  );
  await new Promise((resolve) => setTimeout(resolve, 20));
  const download = sent.find((m) => m && m.op === "download");
  if (!download) {
    failures.push("视频页那条 page-download 没投递出去");
  } else if (download.video !== true) {
    failures.push(`视频页没打 video 标记（实际 video=${download.video}）—— 桌面端会去下一个网页`);
  }
}

if (failures.length) {
  console.error("verify-sw-download FAILED");
  for (const f of failures) console.error("-", f);
  process.exit(1);
}
console.log("verify-sw-download ok：顶层作用域 4 个入口、普通投递、视频页标记 都成立");