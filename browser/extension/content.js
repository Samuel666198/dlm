// 在页面右下角显示 native host 的 pong/错误，方便 B1 用浏览器肉眼/CDP 验收。

function render() {
  chrome.storage.local.get(["lastPong", "lastError"], (values) => {
    let el = document.getElementById("dlm-native-status");
    if (!el) {
      el = document.createElement("div");
      el.id = "dlm-native-status";
      el.setAttribute(
        "style",
        "position:fixed;right:12px;bottom:12px;z-index:2147483647;padding:8px 12px;" +
          "font:12px/1.4 monospace;color:#fff;border-radius:4px;max-width:70vw;white-space:pre-wrap"
      );
      document.documentElement.appendChild(el);
    }
    if (values.lastPong) {
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
