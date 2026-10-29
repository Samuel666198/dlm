const $ = (id) => document.getElementById(id);
let currentTab = null;

function setStatus(text, bad) {
  const el = $("status");
  el.textContent = text;
  el.style.color = bad ? "#b4232f" : "#6c7687";
}

function render(items) {
  const list = $("list");
  list.replaceChildren();
  const rows = Array.isArray(items) ? items : [];
  $("count").textContent = rows.length + " 条";
  if (!rows.length) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "这个页面还没有检测到下载。播放视频、展开播放列表或找下载链接后再看。";
    list.appendChild(empty);
    return;
  }
  for (const item of rows) {
    const box = document.createElement("div");
    box.className = "item";
    const row = document.createElement("div");
    row.className = "row";
    const kind = document.createElement("span");
    kind.className = "kind";
    kind.textContent = item.kind === "hls" ? "HLS" : "文件";
    const url = document.createElement("span");
    url.className = "url";
    url.textContent = item.url;
    url.title = item.url;
    const btn = document.createElement("button");
    btn.className = "dl";
    btn.textContent = "下载";
    btn.addEventListener("click", () => {
      if (!currentTab) return;
      chrome.runtime.sendMessage(
        {
          op: "page-download",
          url: item.url,
          referer: currentTab.url || item.url,
          hls: item.kind === "hls",
          tabId: currentTab.id
        },
        () => {
          if (chrome.runtime.lastError) {
            setStatus("投递失败", true);
            return;
          }
          setStatus(item.kind === "hls" ? "已交给 dlm --hls" : "已交给 dlm");
        }
      );
    });
    row.append(kind, url, btn);
    box.appendChild(row);
    list.appendChild(box);
  }
}

function refresh() {
  chrome.tabs.query({ active: true, currentWindow: true }, (tabs) => {
    currentTab = tabs && tabs[0] ? tabs[0] : null;
    if (!currentTab || currentTab.id == null) {
      render([]);
      setStatus("当前页不可用", true);
      return;
    }
    chrome.runtime.sendMessage({ op: "media-list", tabId: currentTab.id }, (reply) => {
      if (chrome.runtime.lastError) {
        render([]);
        setStatus("扩展未响应", true);
        return;
      }
      render(reply && reply.items);
    });
  });
}

chrome.storage.local.get(["lastPong", "lastError", "lastJob", "lastResult"], (values) => {
  if (values.lastResult) {
    setStatus(
      values.lastResult.ok ? "下载完成" : "下载失败（退出码 " + (values.lastResult.code ?? "?") + "）",
      !values.lastResult.ok
    );
  } else if (values.lastJob) setStatus(values.lastJob.mode === "queue" ? "已加入队列" : "已交给 dlm");
  else if (values.lastPong) setStatus("宿主 " + (values.lastPong.version || "ok"));
  else setStatus(values.lastError || "宿主未连接", true);
});
$("refresh").addEventListener("click", refresh);
refresh();
