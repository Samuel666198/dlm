// B2/B10/B3b：Native Messaging 长连接 + 右键/页面投递 + 媒体/文件嗅探。
// 只把 url/referer/cookie 交给宿主；页面挂钩、DNR、代理读取不在这一版。
// B10：宿主对无 Cookie 的公开直链优先交给 dlm-gui 队列，回执用 mode 区分 queue/direct。
// B3b：webRequest 只读嗅探 .m3u8/媒体直链/附件与二进制响应；HLS 强制直连 dlm --hls。

importScripts("takeover.js");
const HOST = "com.dlm.host";
const MEDIA_KEY = "mediaByTab";
const mediaStore = chrome.storage.session || chrome.storage.local;
let port = null;
let mediaByTab = new Map();
const mediaReady = loadMedia();
chrome.storage.local.set({ buildVersion: chrome.runtime.getManifest().version, buildHasDone: true });

function store(values) {
  chrome.storage.local.set(values);
}

function loadMedia() {
  return mediaStore
    .get(MEDIA_KEY)
    .then((got) => {
      const raw = got && got[MEDIA_KEY];
      mediaByTab = new Map(Object.entries(raw || {}));
    })
    .catch(() => {
      mediaByTab = new Map();
    });
}

function saveMedia() {
  const raw = Object.fromEntries(mediaByTab);
  return mediaStore.set({ [MEDIA_KEY]: raw }).catch(() => {});
}

function mediaKind(details) {
  const url = details && details.url;
  if (!/^https?:\/\//i.test(url || "")) return null;
  let path = "";
  try {
    path = new URL(url).pathname.toLowerCase();
  } catch {
    return null;
  }
  if (path.endsWith(".m3u8")) return "hls";
  if (path.endsWith(".mpd")) return "dash"; // 只识别：DASH 要音视频合并，dlm 不做
  if ([".mp4", ".webm", ".mkv", ".m4a", ".mp3", ".ts", ".mov"].some((ext) => path.endsWith(ext))) {
    return "file";
  }
  if (details.type === "media") return "file";
  return null;
}

function addMedia(tabId, url, kind) {
  if (tabId == null || tabId < 0 || !/^https?:\/\//i.test(url || "")) return false;
  const list = mediaByTab.get(tabId) || [];
  if (list.some((item) => item.url === url)) return false;
  const keep = kind === "hls" || kind === "dash" ? kind : "file";
  list.push({ url, kind: keep, at: Date.now() });
  while (list.length > 40) list.shift();
  mediaByTab.set(tabId, list);
  return true;
}

function noteMedia(details) {
  if (!details || details.tabId < 0) return;
  const kind = mediaKind(details);
  if (!kind) return;
  mediaReady.then(() => {
    if (addMedia(details.tabId, details.url, kind)) saveMedia();
  });
}

function noteAttachment(details) {
  if (!details || details.tabId < 0) return;
  const headers = details.responseHeaders || [];
  const header = (name) => {
    const found = headers.find((h) => String(h.name || "").toLowerCase() === name);
    return found ? String(found.value || "") : "";
  };
  const disposition = header("content-disposition");
  const contentType = header("content-type");
  const attachment = /attachment/i.test(disposition);
  const binary = /^(application\/(octet-stream|zip|pdf|x-msdownload|x-7z-compressed|x-rar-compressed|x-msi)|application\/vnd\.android\.package-archive)/i.test(contentType);
  if (!attachment && !binary) return;
  mediaReady.then(() => {
    if (addMedia(details.tabId, details.url, "file")) saveMedia();
  });
}

chrome.webRequest.onBeforeRequest.addListener(noteMedia, {
  urls: ["<all_urls>"],
  types: ["media", "xmlhttprequest", "other"],
});
chrome.webRequest.onHeadersReceived.addListener(
  noteAttachment,
  {
    urls: ["<all_urls>"],
    types: ["main_frame", "sub_frame", "object", "xmlhttprequest", "other"],
  },
  ["responseHeaders"]
);

chrome.tabs.onRemoved.addListener((tabId) => {
  mediaReady.then(() => {
    if (mediaByTab.delete(tabId)) saveMedia();
  });
});
chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (!changeInfo.url) return;
  mediaReady.then(() => {
    if (mediaByTab.delete(tabId)) saveMedia();
  });
});

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (err) {
    store({ lastPong: null, lastError: String(err) });
    setTimeout(connect, 1500);
    return;
  }

  port.onMessage.addListener((msg) => {
    if (msg && msg.op === "progress") {
      sendToJobTab(msg.tab, { ...msg, op: "job-progress" });
      return;
    }
    if (msg && msg.op === "cancel") {
      sendToJobTab(msg.tab, { ...msg, op: "job-cancel" });
      return;
    }
    if (msg && msg.op === "done") {
      store({ lastResult: msg });
      chrome.action.setBadgeText({ text: msg.ok ? "✓" : "!" });
      chrome.action.setBadgeBackgroundColor({ color: msg.ok ? "#2e7d32" : "#c62828" });
      sendToJobTab(msg.tab, { ...msg, op: "job-done" });
      console.log("dlm job done", msg);
    } else if (msg && msg.op === "accepted") {
      const mode = msg.mode || "direct";
      // ---- 接管的"收网"----
      //
      // 2026-11-27：这段以前**根本不存在**。watchDownloads 登记了 pendingTakeover，
      // 却没有任何地方在回执到达时去取消浏览器那份下载 —— 于是开着接管开关时，
      // 浏览器和 dlm 会各下一遍，用户看到两个文件。
      //
      // 顺序也不能反：chrome.downloads.cancel 之后没法恢复，
      // 万一 dlm 没接住，这次下载就白丢了。所以是「先投递 → 收到 accepted → 才取消」。
      const taken = msg.request_id ? pendingTakeover.get(msg.request_id) : null;
      if (taken) {
        clearTimeout(taken.timer);
        pendingTakeover.delete(msg.request_id);
        chrome.downloads.cancel(taken.downloadId, () => {
          void chrome.runtime.lastError;
          // erase 只是把取消掉的那条从下载列表里清走，失败无所谓。
          chrome.downloads.erase({ id: taken.downloadId }, () => void chrome.runtime.lastError);
        });
        store({ lastTakeover: { ok: true, mode, at: Date.now() } });
      }
      store({ lastJob: { ...msg, mode }, lastResult: null, lastError: null });
      chrome.action.setBadgeText({ text: "↓" });
      chrome.action.setBadgeBackgroundColor({ color: "#c06340" });
      sendToJobTab(msg.tab, { ...msg, mode, op: "job-accepted" });
      console.log("dlm job accepted", mode, msg);
    } else if (msg && msg.op === "error") {
      // 没接住：**什么都不取消**，浏览器那份照常下。只记一笔原因给选项页看。
      const failed = msg.request_id ? pendingTakeover.get(msg.request_id) : null;
      if (failed) {
        clearTimeout(failed.timer);
        pendingTakeover.delete(msg.request_id);
        store({ lastTakeover: { ok: false, reason: msg.message || "error" } });
      }
      store({ lastPong: null, lastError: msg.message || "native host error" });
      chrome.action.setBadgeText({ text: "!" });
      chrome.action.setBadgeBackgroundColor({ color: "#c62828" });
      console.error("dlm native error", msg.message);
    } else {
      store({ lastPong: msg, lastError: null, at: Date.now() });
      chrome.action.setBadgeText({ text: "OK" });
      chrome.action.setBadgeBackgroundColor({ color: "#2e7d32" });
      console.log("dlm native pong", msg);
    }
  });

  port.onDisconnect.addListener(() => {
    const err = chrome.runtime.lastError;
    port = null;
    store({ lastPong: null, lastError: err ? err.message : "disconnected" });
    chrome.action.setBadgeText({ text: "!" });
    chrome.action.setBadgeBackgroundColor({ color: "#c62828" });
    console.error("dlm native disconnected", err && err.message);
    setTimeout(connect, 1500);
  });

  // 双向版本号：报上自己**正在跑**的 manifest.version，宿主回执里带 core_version。
  //
  // 为什么必须报：扩展是「加载已解压」，浏览器不许程序化替换或重载。
  // 桌面端换了磁盘上的扩展文件之后，只能靠扩展自己说「我现在跑的是 0.2.12」
  // 才能确认用户真的点了「重新加载」—— 否则界面会一直以为磁盘上那份还没生效。
  port.postMessage({ op: "ping", ext_version: chrome.runtime.getManifest().version });
}

function submitTask(task) {
  if (!port) connect();
  if (!port) {
    store({ lastJob: null, lastError: "native host unavailable" });
    return false;
  }
  try {
    port.postMessage(task);
    return true;
  } catch (err) {
    store({ lastJob: null, lastError: String(err) });
    return false;
  }
}

function cookieHeader(url) {
  return new Promise((resolve) => {
    chrome.cookies.getAll({ url }, (cookies) => {
      if (chrome.runtime.lastError) {
        resolve("");
        return;
      }
      resolve((cookies || []).map((c) => `${c.name}=${c.value}`).join("; "));
    });
  });
}

function sendToJobTab(tabId, message) {
  if (tabId == null) return;
  chrome.tabs.sendMessage(tabId, message, () => void chrome.runtime.lastError);
}

function submitPage(url, referer, hls, tab) {
  if (!/^https?:\/\//i.test(url || "")) {
    store({ lastJob: null, lastError: "only http/https URLs can be sent to dlm" });
    return false;
  }
  if (!port) connect();
  if (!port) {
    store({ lastJob: null, lastError: "native host unavailable" });
    return false;
  }
  cookieHeader(url).then((cookie) => {
    const task = { op: "download", url, referer: referer || url, cookie };
    if (hls) task.hls = true;
    if (tab != null) task.tab = tab;
    submitTask(task);
  });
  return true;
}

function installMenu() {
  // 页面链接识别用的扩展名表 —— **与 content.js 的 FILE_LINK_RE 必须逐字一致**
// （check-extension-static.mjs 会卡这条）。注入到页面里的脚本通过
// `args: [FILE_LINK_RE.source]` 拿到同一份，所以行为不会走样。
const FILE_LINK_RE =
  /\.(zip|rar|7z|tar|gz|bz2|xz|iso|cab|pdf|docx?|xlsx?|pptx?|txt|md|epub|mobi|csv|exe|msi|apk|dmg|pkg|deb|rpm|mp4|mkv|avi|mov|webm|m4[av]|mp3|flac|wav|aac|ogg|opus|wma|jpg|jpeg|png|gif|bmp|webp|svg|tiff?|heic|ts|bin)(?:$|[?#])/i;

// 右键指向的 URL，按 tab 记着。Chrome 对「用 JS 触发下载的按钮」既不给 linkUrl
// 也不给 srcUrl，只能靠 content.js 在 contextmenu 时上报（见那边的注释）。
const contextTargets = new Map();

/// 探一个 URL 的大小：
/// 1. 先 HEAD 看 `Content-Length`；
/// 2. 有些站点对 HEAD 回 405 或不给长度 ⇒ 退回 `Range: bytes=0-0`，读 `Content-Range` 的总长。
/// 3. 都拿不到就给 `null`（界面显示「—」），**不重试** —— 展开一次最多 8 个请求，
///    对同一个失败地址反复打没有意义。
async function probeSize(url) {
  try {
    const head = await fetch(url, {
      method: "HEAD",
      signal: AbortSignal.timeout(5000),
    });
    const len = Number(head.headers.get("content-length"));
    if (Number.isFinite(len) && len > 0) return len;
  } catch (err) {
    /* 有些站点直接拒绝 HEAD，往下走 Range 那条 */
  }
  try {
    const part = await fetch(url, {
      headers: { Range: "bytes=0-0" },
      signal: AbortSignal.timeout(5000),
    });
    const range = part.headers.get("content-range") || "";
    const total = Number(range.split("/").pop());
    if (Number.isFinite(total) && total > 0) return total;
    const len = Number(part.headers.get("content-length"));
    if (Number.isFinite(len) && len > 0) return len;
  } catch (err) {
    /* 拿不到就认了 */
  }
  return null;
}

chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "dlm-download",
      title: "Download with dlm",
      contexts: ["link", "image", "video", "audio"],
    });
    chrome.contextMenus.create({
      id: "dlm-page-links",
      title: "下载本页所有链接 (dlm)",
      contexts: ["page"],
    });
  });
}

connect();
chrome.runtime.onInstalled.addListener(() => {
  installMenu();
  connect();
});
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg && msg.op === "wake") connect();
  if (msg && msg.op === "page-download") {
    const tab = sender.tab && sender.tab.id != null ? sender.tab.id : msg.tabId;
    sendResponse({ ok: submitPage(msg.url, msg.referer, !!msg.hls, tab) });
    return true;
  }
  if (msg && msg.op === "cancel") {
    if (!port) {
      sendResponse({ ok: false, message: "native host unavailable" });
      return true;
    }
    try {
      port.postMessage({ op: "cancel", job: msg.job });
      sendResponse({ ok: true });
    } catch (err) {
      sendResponse({ ok: false, message: String(err) });
    }
    return true;
  }
  // E16：继续。宿主按有没有 queue id 分两条路（队列走 /api/resume，直连走重投）。
  // 扩展这边只转发，不判断"能不能续" —— 那是队列与断点文件的事实，不是扩展能猜的。
  if (msg && msg.op === "resume") {
    if (!port) connect();
    if (!port) {
      sendResponse({ ok: false, message: "native host unavailable" });
      return true;
    }
    const payload = { op: "resume" };
    if (msg.queue) payload.queue = msg.queue;
    if (msg.url) {
      payload.url = msg.url;
      if (msg.name) payload.name = msg.name;
      if (msg.referer) payload.referer = msg.referer;
      if (msg.hls) payload.hls = true;
    }
    try {
      port.postMessage(payload);
      sendResponse({ ok: true });
    } catch (err) {
      sendResponse({ ok: false, message: String(err) });
    }
    return true;
  }
  if (msg && msg.op === "probe-size") {
    // **必须在后台做**：`host_permissions: <all_urls>` 让扩展进程能跨域取头部，
    // 而内容脚本发同样的请求会被 CORS 挡掉（大多站点不给 Access-Control-Allow-Origin）。
    const urls = Array.isArray(msg.urls) ? msg.urls.slice(0, 8) : [];
    (async () => {
      const sizes = {};
      for (const url of urls) {
        sizes[url] = await probeSize(url);
      }
      sendResponse({ ok: true, sizes });
    })();
    return true; // 异步回
  }
  if (msg && msg.op === "context-target") {
    const tabId = sender.tab && sender.tab.id;
    if (tabId != null) {
      contextTargets.set(tabId, {
        url: typeof msg.url === "string" ? msg.url : "",
        pageUrl: typeof msg.pageUrl === "string" ? msg.pageUrl : "",
        kind: msg.kind === "hls" ? "hls" : "file",
      });
    }
    sendResponse({ ok: true });
    return true;
  }
  if (msg && msg.op === "batch-add") {
    // 只有用户在二次确认里点了「下载」才会到这里。
    const tabId = sender.tab && sender.tab.id;
    const pageUrl = (sender.tab && sender.tab.url) || "";
    const items = Array.isArray(msg.items) ? msg.items.slice(0, 50) : [];
    let submitted = 0;
    for (const item of items) {
      if (!item || typeof item.url !== "string") continue;
      submitPage(
        item.url,
        pageUrl,
        item.kind === "hls" || /\.m3u8(?:$|[?#])/i.test(item.url),
        tabId,
      );
      submitted += 1;
    }
    sendResponse({ ok: true, count: submitted });
    return true;
  }
  if (msg && msg.op === "page-links") {
    const tabId = sender.tab && sender.tab.id;
    const items = Array.isArray(msg.items) ? msg.items : [];
    mediaReady.then(() => {
      let changed = false;
      for (const item of items) {
        if (item && typeof item.url === "string" && addMedia(tabId, item.url, "file")) changed = true;
      }
      if (changed) saveMedia();
      sendResponse({ ok: true });
    });
    return true;
  }
  if (msg && msg.op === "media-list") {
    mediaReady.then(() => {
      const tabId = msg.tabId == null ? sender.tab && sender.tab.id : msg.tabId;
      sendResponse({ items: (tabId == null ? [] : mediaByTab.get(tabId)) || [] });
    });
    return true;
  }
  sendResponse({ ok: true });
});
// A3：读站点黑名单（options.html 维护）。命中就不接手，安静退出。
function isBlocked(url, cb) {
  chrome.storage.local.get(["siteBlock"], (got) => {
    const rules = Array.isArray(got && got.siteBlock) ? got.siteBlock : [];
    let host = "";
    try { host = new URL(url).hostname.toLowerCase(); } catch (err) { host = ""; }
    cb(rules.some((r) => host === r || host.endsWith("." + r)));
  });
}

// A1：下载本页所有链接（同站、http(s)、去重、最多 50 条，避免一次灌爆队列）
function downloadPageLinks(tab) {
  if (!tab || tab.id == null) return;
  const pageUrl = tab.url || "";
  isBlocked(pageUrl, (blocked) => {
    if (blocked) return;
    chrome.scripting.executeScript(
      {
        target: { tabId: tab.id },
        // 注入到页面里跑：正则的 source 作为参数传进来，所以两边用的是同一份表，
        // 不会出现「后台一套、页面一套」的走样。
        func: (pattern) => {
          const re = new RegExp(pattern, "i");
          const files = [];
          const seen = new Set();
          let others = 0;
          for (const a of document.querySelectorAll("a[href]")) {
            let url = null;
            try {
              url = new URL(a.getAttribute("href"), location.href);
            } catch {
              continue;
            }
            if (!/^https?:$/i.test(url.protocol)) continue;
            if (seen.has(url.href)) continue;
            seen.add(url.href);
            // 判据是 pathname + search：`/download?id=1&name=x.zip` 这种把文件名
            // 放在查询串里的链接，只看 pathname 会漏掉（用户报的"检测退化"之一）。
            if (re.test(url.pathname + url.search)) {
              files.push({
                url: url.href,
                kind: /\.m3u8(?:$|[?#])/i.test(url.href) ? "hls" : "file",
              });
              if (files.length >= 50) break;
            } else {
              others += 1;
            }
          }
          return { files, others };
        },
        args: [FILE_LINK_RE.source],
      },
      (results) => {
        const out = (results && results[0] && results[0].result) || {};
        let pageHost = "";
        try {
          pageHost = new URL(pageUrl).hostname;
        } catch (err) {
          pageHost = "";
        }
        const files = (out.files || []).filter((item) => {
          try {
            return new URL(item.url).hostname === pageHost;
          } catch (err) {
            return false;
          }
        });
        // **不直接入队**：先把清单交给页面里的确认层，用户点了「下载」才提交。
        // 旧版本这里直接 submitPage 循环，一次误点就是 50 条垃圾任务。
        chrome.tabs.sendMessage(tab.id, {
          op: "show-batch-confirm",
          items: files,
          skipped: out.others || 0,
          domain: pageHost,
        });
      },
    );
  });
}

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "dlm-page-links") {
    downloadPageLinks(tab);
    return;
  }
  if (info.menuItemId !== "dlm-download") return;
  // 顺序有讲究：Chrome 直接给的优先；它给不出来时（用 JS 触发下载的按钮，
  // 既不是 link 也不是 image/video/audio）才用 content.js 记下的那个。
  const remembered =
    (tab && contextTargets.get(tab.id)) || { url: "", pageUrl: "", kind: "file" };
  const url = info.linkUrl || info.srcUrl || remembered.url;
  if (!url) return;
  const hls = /\.m3u8(?:$|[?#])/i.test(url);
  submitPage(url, info.pageUrl || (tab && tab.url) || url, hls, tab && tab.id);
});
// ---------------------------------------------------------------- downloads 接管
// 默认关闭。开启后：浏览器新建的下载若命中白名单/站点规则，就交给宿主；
// **收到 request_id 匹配的 accepted 才 cancel**，2 秒没回执就放手，绝不制造"两边都没下成"。
const TAKEOVER_KEY = "takeover";
const TAKEOVER_ACK_MS = 2000;
let takeoverCfg = { enabled: false, whitelist: [], minBytes: 1048576, blocklist: [] };
const pendingTakeover = new Map();

function loadTakeoverCfg() {
  try {
    chrome.storage.local.get([TAKEOVER_KEY, "siteBlock"], (got) => {
      const saved = (got && got[TAKEOVER_KEY]) || {};
      takeoverCfg = {
        enabled: !!saved.enabled,
        whitelist: Array.isArray(saved.whitelist) ? saved.whitelist : [],
        minBytes: Number(saved.minBytes) || 0,
        mimeWhitelist: Array.isArray(saved.mimeWhitelist) ? saved.mimeWhitelist : [],
        blocklist: Array.isArray(got && got.siteBlock) ? got.siteBlock : [],
      };
    });
  } catch (err) {
    // 扩展上下文失效：保持关闭
  }
}

function takeoverRequestId() {
  return "t" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

function watchDownloads() {
  if (!chrome.downloads || !chrome.downloads.onCreated) return;
  chrome.downloads.onCreated.addListener((item) => {
    if (!takeoverCfg.enabled) return;
    const verdict = globalThis.dlmTakeover.shouldTakeover(item, takeoverCfg);
    if (!verdict.take) return;
    const requestId = takeoverRequestId();
    const timer = setTimeout(() => {
      pendingTakeover.delete(requestId);
      store({ lastTakeover: { ok: false, reason: "no-ack" } });
    }, TAKEOVER_ACK_MS);
    pendingTakeover.set(requestId, { downloadId: item.id, timer });
    const url = item.finalUrl || item.url;
    // 带 cookie：接管下来的下载和右键下载是同一种下载，站点要求登录时
    // 没有 cookie 只会拿到一个登录页。队列从 2026-11-27 起收得下凭证（加密落盘）。
    cookieHeader(url).then((cookie) => {
      const sent = submitTask({
        op: "download",
        request_id: requestId,
        url,
        referer: item.referrer || "",
        tab: item.tabId,
        cookie,
      });
      if (!sent) {
        clearTimeout(timer);
        pendingTakeover.delete(requestId);
      }
    });
  });
}

loadTakeoverCfg();
watchDownloads();
chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "local" && (changes[TAKEOVER_KEY] || changes.siteBlock)) loadTakeoverCfg();
});