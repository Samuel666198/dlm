// 页面右下角的 dlm 状态 + 一个把当前页交给 dlm 的按钮（B2 的最小可测入口）。
// 真正的右键菜单在 background.js；这个按钮让浏览器验收可以走 DOM 点击。

function ensureBox() {
  let box = document.getElementById("dlm-native-box");
  if (box) return box;
  box = document.createElement("div");
  box.id = "dlm-native-box";
  box.setAttribute(
    "style",
    "position:fixed;right:12px;bottom:12px;z-index:2147483647;display:flex;gap:6px;align-items:stretch"
  );
  const status = document.createElement("div");
  status.id = "dlm-native-status";
  status.setAttribute(
    "style",
    "padding:8px 12px;font:12px/1.4 monospace;color:#fff;border-radius:4px;max-width:60vw;white-space:pre-wrap"
  );
  const button = document.createElement("button");
  button.id = "dlm-submit";
  button.type = "button";
  button.textContent = "dlm ↓";
  button.title = "Download this page with dlm";
  button.setAttribute(
    "style",
    "padding:8px 10px;font:12px/1.2 monospace;color:#fff;background:#1565c0;border:0;border-radius:4px;cursor:pointer"
  );
  button.addEventListener("click", () => {
    chrome.runtime.sendMessage(
      { op: "page-download", url: location.href, referer: location.href },
      () => void chrome.runtime.lastError
    );
  });
  box.appendChild(status);
  box.appendChild(button);
  document.documentElement.appendChild(box);
  return box;
}

function render() {
  ensureBox();
  const el = document.getElementById("dlm-native-status");
  chrome.storage.local.get(["lastPong", "lastError", "lastJob"], (values) => {
    if (values.lastJob) {
      el.textContent = "dlm job: " + (values.lastJob.job || "accepted");
      el.style.background = "#1565c0";
    } else if (values.lastPong) {
      const p = values.lastPong;
      el.textContent = "dlm native: " + p.op + " " + (p.host || "") + " v" + (p.version || "");
      el.style.background = "#1b5e20";
    } else {
      el.textContent = "dlm native: " + (values.lastError || "waiting for pong");
      el.style.background = "#c62828";
    }
  });
}

render();
chrome.storage.onChanged.addListener(render);
// 页面加载时主动唤醒 service worker：未打包扩展刚加载时它可能还没被任何事件拉起。
chrome.runtime.sendMessage({ op: "wake" }, () => void chrome.runtime.lastError);
