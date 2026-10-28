// B2/B10：Native Messaging 长连接 + 右键菜单/页面按钮投递。
// 只把 url/referer/cookie 交给宿主；页面挂钩、DNR、代理读取不在这一版。
// B10：宿主对无 Cookie 的公开直链优先交给 dlm-gui 队列，回执用 mode 区分 queue/direct。

const HOST = "com.dlm.host";
let port = null;

function store(values) {
  chrome.storage.local.set(values);
}

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
      chrome.action.setBadgeBackgroundColor({ color: "#1565c0" });
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

function submitPage(url, referer) {
  if (!/^https?:\/\//i.test(url || "")) {
    store({ lastJob: null, lastError: "only http/https URLs can be sent to dlm" });
    return;
  }
  cookieHeader(url).then((cookie) => {
    submitTask({ op: "download", url, referer: referer || url, cookie });
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
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg && msg.op === "wake") connect();
  if (msg && msg.op === "page-download") submitPage(msg.url, msg.referer);
  sendResponse({ ok: true });
});
chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId !== "dlm-download") return;
  const url = info.linkUrl || info.srcUrl;
  if (!url) return;
  submitPage(url, info.pageUrl || (tab && tab.url) || url);
});
chrome.action.onClicked.addListener(() => {
  if (port) port.postMessage({ op: "ping" });
  else connect();
});
