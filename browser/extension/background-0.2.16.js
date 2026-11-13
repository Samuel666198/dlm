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
      store({ lastJob: { ...msg, mode }, lastResult: null, lastError: null });
      chrome.action.setBadgeText({ text: "↓" });
      chrome.action.setBadgeBackgroundColor({ color: "#c06340" });
      sendToJobTab(msg.tab, { ...msg, mode, op: "job-accepted" });
      console.log("dlm job accepted", mode, msg);
    } else if (msg && msg.op === "error") {
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
      { target: { tabId: tab.id }, func: () => Array.from(document.links, (a) => a.href) },
      (results) => {
        const all = (results && results[0] && results[0].result) || [];
        let pageHost = "";
        try { pageHost = new URL(pageUrl).hostname; } catch (err) { pageHost = ""; }
        const seen = new Set();
        const picked = [];
        for (const href of all) {
          if (!/^https?:\/\//i.test(href)) continue;
          let host = "";
          try { host = new URL(href).hostname; } catch (err) { continue; }
          if (host !== pageHost) continue;
          if (seen.has(href)) continue;
          seen.add(href);
          picked.push(href);
          if (picked.length >= 50) break;
        }
        for (const href of picked) submitPage(href, pageUrl, /\.m3u8(?:$|[?#])/i.test(href), tab.id);
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
  const url = info.linkUrl || info.srcUrl;
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
    const sent = submitTask({
      op: "download",
      request_id: requestId,
      url: item.finalUrl || item.url,
      referer: item.referrer || "",
      tab: item.tabId,
    });
    if (!sent) {
      clearTimeout(timer);
      pendingTakeover.delete(requestId);
    }
  });
}

loadTakeoverCfg();
watchDownloads();
chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "local" && (changes[TAKEOVER_KEY] || changes.siteBlock)) loadTakeoverCfg();
});