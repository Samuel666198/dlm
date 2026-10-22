// B1 最小 Native Messaging 长连接：ping 一次，把 pong/错误写进 storage。
// 不做页面挂钩、不接下载；这些归 B2。

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
    store({ lastPong: msg, lastError: null, at: Date.now() });
    chrome.action.setBadgeText({ text: "OK" });
    chrome.action.setBadgeBackgroundColor({ color: "#2e7d32" });
    console.log("dlm native pong", msg);
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

connect();
chrome.runtime.onInstalled.addListener(connect);
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg && msg.op === "wake") connect();
  sendResponse({ ok: true });
});
chrome.action.onClicked.addListener(() => {
  if (port) port.postMessage({ op: "ping" });
  else connect();
});
