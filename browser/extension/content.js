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
  const mediaButton = document.createElement("button");
  mediaButton.id = "dlm-media-count";
  mediaButton.type = "button";
  mediaButton.textContent = "下载 0";
  mediaButton.hidden = true;
  mediaButton.title = "查看页面里检测到的下载";
  mediaButton.setAttribute(
    "style",
    "padding:8px 10px;font:12px/1.2 monospace;color:#fff;background:#0f766e;border:0;border-radius:4px;cursor:pointer"
  );
  mediaButton.addEventListener("click", () => {
    const panel = document.getElementById("dlm-media-panel");
    if (panel) panel.hidden = !panel.hidden;
  });
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
  box.appendChild(mediaButton);
  box.appendChild(button);
  document.documentElement.appendChild(box);

  const panel = document.createElement("div");
  panel.id = "dlm-media-panel";
  panel.hidden = true;
  panel.setAttribute(
    "style",
    "position:fixed;right:12px;bottom:56px;z-index:2147483647;width:min(360px,calc(100vw - 24px));max-height:45vh;overflow:auto;background:#111820;color:#e8eef7;border:1px solid #2a3644;border-radius:8px;box-shadow:0 12px 32px rgba(0,0,0,.35);font:12px/1.45 monospace"
  );
  const head = document.createElement("div");
  head.textContent = "检测到的下载";
  head.setAttribute("style", "padding:9px 11px;border-bottom:1px solid #263342;color:#9fb0c3;font-size:11px");
  const list = document.createElement("div");
  list.id = "dlm-media-list";
  list.setAttribute("style", "padding:4px");
  panel.appendChild(head);
  panel.appendChild(list);
  document.documentElement.appendChild(panel);
  return box;
}

function renderMedia(items) {
  const button = document.getElementById("dlm-media-count");
  const panel = document.getElementById("dlm-media-panel");
  const list = document.getElementById("dlm-media-list");
  if (!button || !panel || !list) return;
  const rows = Array.isArray(items) ? items : [];
  button.hidden = rows.length === 0;
  button.textContent = "下载 " + rows.length;
  if (!rows.length) panel.hidden = true;
  list.replaceChildren();
  for (const item of rows) {
    const row = document.createElement("div");
    row.setAttribute("style", "display:flex;align-items:center;gap:8px;padding:7px 6px;border-bottom:1px solid #1c2733");
    const kind = document.createElement("span");
    kind.textContent = item.kind === "hls" ? "HLS" : "文件";
    kind.setAttribute(
      "style",
      "flex:0 0 auto;font-size:10px;color:" + (item.kind === "hls" ? "#6ee7d2" : "#9fb0c3") + ";border:1px solid #2a3644;border-radius:3px;padding:1px 4px"
    );
    const url = document.createElement("span");
    url.className = "url";
    url.textContent = item.url;
    url.title = item.url;
    url.setAttribute("style", "min-width:0;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#b8c6d6;font-size:10px");
    const download = document.createElement("button");
    download.type = "button";
    download.textContent = "下载";
    download.setAttribute(
      "style",
      "flex:0 0 auto;padding:4px 8px;font:11px monospace;color:#07110f;background:#6ee7d2;border:0;border-radius:4px;cursor:pointer"
    );
    download.addEventListener("click", () => {
      chrome.runtime.sendMessage(
        {
          op: "page-download",
          url: item.url,
          referer: location.href,
          hls: item.kind === "hls"
        },
        () => void chrome.runtime.lastError
      );
    });
    row.append(kind, url, download);
    list.appendChild(row);
  }
}

const FILE_LINK_RE = /\.(zip|rar|7z|tar|gz|bz2|xz|iso|cab|pdf|docx?|xlsx?|pptx?|txt|md|epub|mobi|csv|exe|msi|apk|dmg|pkg|deb|rpm|mp4|mkv|avi|mov|webm|m4[av]|mp3|flac|wav|aac|ogg|opus|wma|jpg|jpeg|png|gif|bmp|webp|svg|tiff?|heic|ts|bin)(?:$|[?#])/i;

function collectFileLinks() {
  const seen = new Set();
  const out = [];
  for (const a of document.querySelectorAll("a[href]")) {
    let url = null;
    try {
      url = new URL(a.getAttribute("href"), location.href);
    } catch {
      continue;
    }
    if (!/^https?:$/i.test(url.protocol)) continue;
    if (!FILE_LINK_RE.test(url.pathname)) continue;
    if (seen.has(url.href)) continue;
    seen.add(url.href);
    out.push({ url: url.href, kind: "file", at: Date.now() });
    if (out.length >= 30) break;
  }
  return out;
}

function mergeItems(a, b) {
  const seen = new Set();
  const out = [];
  for (const item of [...(a || []), ...(b || [])]) {
    if (!item || !item.url || seen.has(item.url)) continue;
    seen.add(item.url);
    out.push(item);
    if (out.length >= 40) break;
  }
  return out;
}

function refreshMedia() {
  const local = collectFileLinks();
  chrome.runtime.sendMessage({ op: "page-links", items: local }, () => void chrome.runtime.lastError);
  chrome.runtime.sendMessage({ op: "media-list" }, (reply) => {
    if (chrome.runtime.lastError) {
      renderMedia(local);
      return;
    }
    renderMedia(mergeItems(reply && reply.items, local));
  });
}

function render() {
  ensureBox();
  const el = document.getElementById("dlm-native-status");
  chrome.storage.local.get(["lastPong", "lastError", "lastJob"], (values) => {
    if (values.lastJob) {
      const job = values.lastJob;
      if (job.mode === "queue") {
        el.textContent = "dlm queue: " + (job.id || "accepted");
      } else {
        el.textContent = "dlm job: " + (job.job || "accepted");
      }
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
refreshMedia();
setInterval(refreshMedia, 1500);
chrome.storage.onChanged.addListener(render);
// 页面加载时主动唤醒 service worker：未打包扩展刚加载时它可能还没被任何事件拉起。
chrome.runtime.sendMessage({ op: "wake" }, () => void chrome.runtime.lastError);
