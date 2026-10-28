// B2/B10/B3b：Native Messaging 长连接 + 右键/页面投递 + 媒体嗅探。
// 只把 url/referer/cookie 交给宿主；页面挂钩、DNR、代理读取不在这一版。
// B10：宿主对无 Cookie 的公开直链优先交给 dlm-gui 队列，回执用 mode 区分 queue/direct。
// B3b：webRequest 只读嗅探 .m3u8/媒体直链；HLS 强制直连 dlm --hls。

const HOST = "com.dlm.host";
const MEDIA_KEY = "mediaByTab";
const mediaStore = chrome.storage.session || chrome.storage.local;
let port = null;
let mediaByTab = new Map();
const mediaReady = loadMedia();

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
  if (path.endsWith(".mpd")) return null; // DASH 需要音视频合并，B3b 不列
  if ([".mp4", ".webm", ".mkv", ".m4a", ".mp3", ".ts", ".mov"].some((ext) => path.endsWith(ext))) {
    return "file";
  }
  if (details.type === "media") return "file";
  return null;
}

function noteMedia(details) {
  if (!details || details.tabId < 0) return;
  const kind = mediaKind(details);
  if (!kind) return;
  mediaReady.then(() => {
    const list = mediaByTab.get(details.tabId) || [];
    if (list.some((item) => item.url === details.url)) return;
    list.push({ url: details.url, kind, at: Date.now() });
    while (list.length > 20) list.shift();
    mediaByTab.set(details.tabId, list);
    saveMedia();
  });
}

chrome.webRequest.onBeforeRequest.addListener(noteMedia, {
  urls: ["<all_urls>"],
  types: ["media", "xmlhttprequest", "other"],
});

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
    if (msg && msg.op === "accepted") {
      const mode = msg.mode || "direct";
      store({ lastJob: { ...msg, mode }, lastError: null });
      chrome.action.setBadgeText({ text: "↓" });
      chrome.action.setBadgeBackgroundColor({ color: "#0f766e" });
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

  port.postMessage({ op: "ping" });
}

function submitTask(task) {
  if (!port) connect();
  if (!port) {
    store({ lastJob: null, lastError: "native host unavailable" });
    return;
  }
  port.postMessage(task);
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

function submitPage(url, referer, hls) {
  if (!/^https?:\/\//i.test(url || "")) {
    store({ lastJob: null, lastError: "only http/https URLs can be sent to dlm" });
    return;
  }
  cookieHeader(url).then((cookie) => {
    const task = { op: "download", url, referer: referer || url, cookie };
    if (hls) task.hls = true;
    submitTask(task);
  });
}

function installMenu() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "dlm-download",
      title: "Download with dlm",
      contexts: ["link", "image", "video", "audio"],
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
    submitPage(msg.url, msg.referer, !!msg.hls);
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
chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId !== "dlm-download") return;
  const url = info.linkUrl || info.srcUrl;
  if (!url) return;
  const hls = /\.m3u8(?:$|[?#])/i.test(url);
  submitPage(url, info.pageUrl || (tab && tab.url) || url, hls);
});
