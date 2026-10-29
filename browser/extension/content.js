// 页面右下角的 dlm 组件：检测到的下载、进行中进度、确认后交付。
// 组件住在 shadow DOM 里，避免被站点 CSS 污染；所有用户数据只用 textContent 写入。
document.documentElement.setAttribute("data-dlm-version", chrome.runtime.getManifest().version);

const FILE_LINK_RE = /\.(zip|rar|7z|tar|gz|bz2|xz|iso|cab|pdf|docx?|xlsx?|pptx?|txt|md|epub|mobi|csv|exe|msi|apk|dmg|pkg|deb|rpm|mp4|mkv|avi|mov|webm|m4[av]|mp3|flac|wav|aac|ogg|opus|wma|jpg|jpeg|png|gif|bmp|webp|svg|tiff?|heic|ts|bin)(?:$|[?#])/i;

const state = {
  items: [],
  active: new Map(),
  done: new Set(),
  finished: [],
  confirm: null,
  collapsed: true,
  hostReady: false,
};

let renderTimer = null;
let shadow = null;
let host = null;

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text != null) node.textContent = text;
  return node;
}

function fileName(url) {
  try {
    const seg = new URL(url).pathname.split("/").pop() || "";
    return seg || "download.bin";
  } catch {
    return "download.bin";
  }
}

function fmtBytes(bytes) {
  if (!bytes || bytes < 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return (i === 0 ? Math.round(value) : value.toFixed(1)) + " " + units[i];
}

function fmtEta(seconds) {
  if (!seconds || seconds <= 0) return "";
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return m > 0 ? m + " 分 " + String(s).padStart(2, "0") + " 秒" : s + " 秒";
}

function aggregate() {
  let done = 0;
  let total = 0;
  let percent = 0;
  let count = 0;
  for (const item of state.active.values()) {
    count += 1;
    done += item.done || 0;
    total += item.total || 0;
    percent += item.percent || 0;
  }
  if (total > 0) return { percent: Math.round((done / total) * 100), count };
  return { percent: count ? Math.round(percent / count) : 0, count };
}

const STYLE = `
  :host {
    all: initial;
    position: fixed; right: 12px; bottom: 12px; z-index: 2147483647;
    color-scheme: light dark;
    font-family: "Segoe UI Variable", "Segoe UI", system-ui, "Microsoft YaHei", sans-serif;
    font-size: 13px; line-height: 1.4; color: #171b22;
    --card: light-dark(#ffffff, #1b212a);
    --ink: light-dark(#171b22, #e8edf4);
    --muted: light-dark(#6c7687, #9aa4b2);
    --line: light-dark(#e6e9ee, #2a333f);
    --page: light-dark(#f3f5f8, #0f1318);
    --track: light-dark(#e9edf2, #2a3542);
    --teal: light-dark(#0f766e, #4fd1c5);
    --teal-2: light-dark(#14b8a6, #2dd4bf);
    --teal-soft: light-dark(#e7f4f1, #12312d);
    --teal-ink: light-dark(#ffffff, #08110f);
    --danger: light-dark(#b4232f, #f87171);
    --shadow: light-dark(0 14px 36px rgba(15,23,42,.18), 0 14px 36px rgba(0,0,0,.45));
  }
  * { box-sizing: border-box; }
  button { font: inherit; }
  .wrap { display: flex; justify-content: flex-end; }
  .pill {
    max-width: 336px; background: var(--card); border: 1px solid var(--line);
    border-radius: 12px; box-shadow: var(--shadow); overflow: hidden;
  }
  .pillbtn {
    display: flex; align-items: center; gap: 8px; width: 100%;
    border: 0; background: transparent; color: var(--ink);
    padding: 9px 10px; min-height: 46px; cursor: pointer; text-align: left;
  }
  .mark {
    width: 24px; height: 24px; flex: 0 0 auto; border-radius: 7px;
    display: grid; place-items: center; background: var(--teal); color: var(--teal-ink);
    font: 500 13px/1 ui-monospace, Consolas, monospace;
  }
  .pillbtn b { font-size: 13px; font-weight: 500; }
  .pill .meta { color: var(--muted); font-size: 11px; font-weight: 400; }
  .mini { margin-left: auto; width: 54px; height: 4px; border-radius: 999px; background: var(--track); overflow: hidden; }
  .mini i { display: block; height: 100%; background: var(--teal-2); }
  .pill .pct { font: 500 11px/1 ui-monospace, Consolas, monospace; color: var(--teal); }
  .chev { color: var(--muted); font-size: 11px; }
  .panel {
    width: min(336px, calc(100vw - 24px)); background: var(--card);
    border: 1px solid var(--line); border-radius: 12px; box-shadow: var(--shadow);
    overflow: hidden;
  }
  header { display: flex; align-items: center; gap: 8px; padding: 11px 12px; border-bottom: 1px solid var(--line); }
  header b { font-size: 13px; font-weight: 500; color: var(--ink); }
  header .sub { font-size: 11px; color: var(--muted); }
  header .icon {
    margin-left: auto; min-width: 32px; height: 32px; border: 0; border-radius: 7px;
    background: transparent; color: var(--muted); cursor: pointer; font-size: 12px; padding: 0 7px;
  }
  header .icon + .icon { margin-left: 0; }
  .body { padding: 10px 12px 12px; max-height: min(70vh, 520px); overflow: auto; }
  .ask { border: 1px solid var(--teal); background: var(--teal-soft); border-radius: 10px; padding: 11px; margin-bottom: 10px; }
  .ask-title { font-size: 13px; font-weight: 500; margin-bottom: 6px; color: var(--ink); }
  .ask-name { font-size: 12px; color: var(--ink); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .ask-src { font: 10px/1.4 ui-monospace, Consolas, monospace; color: var(--muted); margin-top: 4px; }
  .ask-actions { display: flex; justify-content: flex-end; gap: 6px; margin-top: 9px; }
  h4 { display: flex; align-items: center; gap: 6px; margin: 0 0 6px; font-size: 11px; font-weight: 500; color: var(--muted); }
  h4 .count {
    min-width: 16px; height: 16px; padding: 0 4px; border-radius: 999px;
    background: var(--teal-soft); color: var(--teal); font: 500 10px/16px ui-monospace, Consolas, monospace; text-align: center;
  }
  section { margin-top: 10px; }
  section:first-of-type { margin-top: 0; }
  .active { border: 1px solid var(--line); border-radius: 9px; padding: 10px; background: var(--page); margin-bottom: 8px; }
  .name { font-size: 12px; font-weight: 500; color: var(--ink); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .bar { width: 100%; height: 6px; border-radius: 999px; background: var(--track); overflow: hidden; margin: 8px 0 6px; }
  .bar i { display: block; height: 100%; border-radius: 999px; background: linear-gradient(90deg, var(--teal), var(--teal-2)); }
  .meta { display: flex; flex-wrap: wrap; gap: 8px; font: 10px/1.3 ui-monospace, Consolas, monospace; color: var(--muted); }
  .actions { display: flex; justify-content: flex-end; margin-top: 7px; }
  .ghost, .primary, .dl {
    border-radius: 7px; font: 500 11px/1 inherit; cursor: pointer; min-height: 32px; padding: 0 10px;
  }
  .ghost { border: 1px solid var(--line); background: var(--card); color: var(--muted); }
  .primary { border: 0; background: var(--teal); color: var(--teal-ink); padding: 0 14px; }
  .done { display: flex; align-items: center; gap: 7px; padding: 7px 2px; border-bottom: 1px solid var(--line); }
  .done:last-child { border-bottom: 0; }
  .done .name { flex: 1; font-weight: 400; }
  .done .meta { font-size: 10px; }
  .check { color: var(--teal); font-weight: 500; }
  .cross { color: var(--danger); font-weight: 500; }
  .link { display: flex; align-items: center; gap: 7px; padding: 6px 2px; border-bottom: 1px solid var(--line); }
  .link:last-child { border-bottom: 0; }
  .chip {
    flex: 0 0 auto; border: 1px solid var(--line); border-radius: 4px; padding: 1px 4px;
    font: 9px/1.4 ui-monospace, Consolas, monospace; color: var(--muted);
  }
  .chip.hls { color: var(--teal); border-color: var(--teal-soft); background: var(--teal-soft); }
  .url {
    flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis;
    font: 10px/1.4 ui-monospace, Consolas, monospace; color: var(--muted);
  }
  .dl { flex: 0 0 auto; border: 0; background: var(--teal-soft); color: var(--teal); padding: 0 9px; }
  .empty, .note { font-size: 11px; color: var(--muted); padding: 6px 2px; }
  .note { color: var(--danger); }
`;

function ensureWidget() {
  if (host) return;
  host = document.createElement("div");
  host.id = "dlm-native-box";
  shadow = host.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = STYLE;
  const wrap = el("div", "wrap");
  wrap.innerHTML = `
    <div id="dlm-pill" class="pill" hidden>
      <button id="dlm-toggle" class="pillbtn" type="button" aria-label="展开 dlm 下载面板">
        <span class="mark">↓</span><b>dlm</b>
        <span id="dlm-pill-meta" class="meta"></span>
        <span id="dlm-pill-bar" class="mini" hidden><i></i></span>
        <span id="dlm-pill-pct" class="pct"></span>
        <span class="chev">⌃</span>
      </button>
    </div>
    <div id="dlm-panel" class="panel" hidden>
      <header>
        <span class="mark">↓</span><b>dlm</b>
        <span id="dlm-sub" class="sub"></span>
        <button id="dlm-submit" class="icon" type="button" title="把当前页交给 dlm">本页</button>
        <button id="dlm-fold" class="icon" type="button" aria-label="收起">—</button>
      </header>
      <div class="body">
        <div id="dlm-confirm" class="ask" hidden>
          <div class="ask-title">用 dlm 下载这个文件？</div>
          <div id="dlm-confirm-name" class="ask-name"></div>
          <div id="dlm-confirm-src" class="ask-src"></div>
          <div class="ask-actions">
            <button id="dlm-confirm-no" class="ghost" type="button">取消</button>
            <button id="dlm-confirm-yes" class="primary" type="button">下载</button>
          </div>
        </div>
        <section id="dlm-active-sec" hidden>
          <h4>进行中 <span id="dlm-active-count" class="count">0</span></h4>
          <div id="dlm-active"></div>
        </section>
        <section id="dlm-finished-sec" hidden>
          <h4>最近</h4>
          <div id="dlm-finished"></div>
        </section>
        <section id="dlm-links-sec" hidden>
          <h4>可下载 <span id="dlm-links-count" class="count">0</span></h4>
          <div id="dlm-list"></div>
        </section>
        <div id="dlm-empty" class="empty" hidden>这个页面还没有检测到下载。</div>
        <div id="dlm-host-note" class="note" hidden>宿主未连接，点击不会接管。</div>
      </div>
    </div>`;
  shadow.append(style, wrap);
  document.documentElement.appendChild(host);

  shadow.getElementById("dlm-toggle").addEventListener("click", () => {
    state.collapsed = false;
    render();
  });
  shadow.getElementById("dlm-fold").addEventListener("click", () => {
    state.collapsed = true;
    render();
  });
  shadow.getElementById("dlm-submit").addEventListener("click", () => {
    sendDownload(location.href, "file", true);
  });
  shadow.getElementById("dlm-confirm-no").addEventListener("click", () => {
    state.confirm = null;
    render();
  });
  shadow.getElementById("dlm-confirm-yes").addEventListener("click", () => {
    const pending = state.confirm;
    state.confirm = null;
    if (pending) sendDownload(pending.url, pending.kind, false);
    render();
  });
}

function scheduleRender() {
  if (renderTimer) return;
  renderTimer = setTimeout(() => {
    renderTimer = null;
    render();
  }, 150);
}

function sendDownload(url, kind, fromPage) {
  chrome.runtime.sendMessage(
    { op: "page-download", url, referer: location.href, hls: kind === "hls" },
    (reply) => {
      if (chrome.runtime.lastError || !reply || reply.ok === false) {
        if (fromPage) {
          window.location.assign(url);
        } else {
          state.finished.unshift({ name: fileName(url), ok: false, code: null, message: "投递失败" });
          state.finished = state.finished.slice(0, 3);
          render();
        }
      }
    }
  );
}

function cancelJob(job) {
  const entry = state.active.get(job);
  if (entry) {
    entry.stopping = true;
    render();
  }
  chrome.runtime.sendMessage({ op: "cancel", job }, (reply) => {
    if (chrome.runtime.lastError || !reply || !reply.ok) {
      const current = state.active.get(job);
      if (current) {
        current.stopping = false;
        current.error = (reply && reply.message) || "停止失败";
      }
      render();
    }
  });
}

function renderActive() {
  const box = shadow.getElementById("dlm-active");
  box.replaceChildren();
  let index = 0;
  for (const [job, entry] of state.active) {
    const row = el("div", "active");
    row.appendChild(el("div", "name", entry.name || "下载任务"));
    const bar = el("div", "bar");
    const fill = el("i");
    fill.style.width = Math.max(0, Math.min(100, entry.percent || 0)) + "%";
    bar.appendChild(fill);
    bar.setAttribute("role", "progressbar");
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    bar.setAttribute("aria-valuenow", String(Math.round(entry.percent || 0)));
    row.appendChild(bar);
    const meta = el("div", "meta");
    if (entry.total > 0) meta.appendChild(el("span", null, fmtBytes(entry.done) + " / " + fmtBytes(entry.total)));
    else meta.appendChild(el("span", null, "连接中…"));
    if (entry.speed > 0) meta.appendChild(el("span", null, fmtBytes(entry.speed) + "/s"));
    if (entry.eta > 0) meta.appendChild(el("span", null, "剩余 " + fmtEta(entry.eta)));
    if (entry.error) meta.appendChild(el("span", null, entry.error));
    row.appendChild(meta);
    const actions = el("div", "actions");
    const stop = el("button", "ghost", entry.stopping ? "停止中…" : "停止");
    stop.type = "button";
    stop.disabled = !!entry.stopping;
    stop.addEventListener("click", () => cancelJob(job));
    actions.appendChild(stop);
    row.appendChild(actions);
    box.appendChild(row);
    index += 1;
  }
  shadow.getElementById("dlm-active-count").textContent = String(index);
  shadow.getElementById("dlm-active-sec").hidden = index === 0;
}

function renderFinished() {
  const box = shadow.getElementById("dlm-finished");
  box.replaceChildren();
  for (const item of state.finished) {
    const row = el("div", "done");
    row.appendChild(el("span", item.ok ? "check" : "cross", item.ok ? "✓" : "✕"));
    row.appendChild(el("span", "name", item.name || "下载任务"));
    let text;
    if (item.queue) text = "已加入队列 " + item.queue;
    else if (item.canceled) text = "已停止（断点保留）";
    else if (item.ok) text = "完成";
    else if (item.message) text = item.message;
    else text = "失败（退出码 " + (item.code ?? "?") + "）";
    row.appendChild(el("span", "meta", text));
    box.appendChild(row);
  }
  shadow.getElementById("dlm-finished-sec").hidden = state.finished.length === 0;
}

function renderLinks() {
  const box = shadow.getElementById("dlm-list");
  box.replaceChildren();
  for (const item of state.items) {
    const row = el("div", "link");
    row.appendChild(el("span", "chip" + (item.kind === "hls" ? " hls" : ""), item.kind === "hls" ? "HLS" : "文件"));
    const url = el("span", "url", fileName(item.url));
    url.title = item.url;
    row.appendChild(url);
    const btn = el("button", "dl", "下载");
    btn.type = "button";
    btn.addEventListener("click", () => sendDownload(item.url, item.kind, false));
    row.appendChild(btn);
    box.appendChild(row);
  }
  shadow.getElementById("dlm-links-count").textContent = String(state.items.length);
  shadow.getElementById("dlm-links-sec").hidden = state.items.length === 0;
}

function renderConfirm() {
  const box = shadow.getElementById("dlm-confirm");
  box.hidden = !state.confirm;
  if (!state.confirm) return;
  shadow.getElementById("dlm-confirm-name").textContent = state.confirm.name;
  shadow.getElementById("dlm-confirm-src").textContent =
    state.confirm.domain + " · " + (state.confirm.kind === "hls" ? "HLS" : "文件");
}

function render() {
  ensureWidget();
  const anyActive = state.active.size > 0;
  const any = state.items.length > 0 || anyActive || state.finished.length > 0 || !!state.confirm;
  host.hidden = !any;
  if (!any) return;

  shadow.getElementById("dlm-pill").hidden = !state.collapsed;
  shadow.getElementById("dlm-panel").hidden = state.collapsed;

  const agg = aggregate();
  const pillMeta = shadow.getElementById("dlm-pill-meta");
  const pillBar = shadow.getElementById("dlm-pill-bar");
  const pillPct = shadow.getElementById("dlm-pill-pct");
  if (agg.count > 0) {
    pillMeta.textContent = agg.count + " 个下载";
    pillBar.hidden = false;
    pillBar.firstElementChild.style.width = agg.percent + "%";
    pillPct.textContent = agg.percent + "%";
  } else {
    pillMeta.textContent = "可下载 " + state.items.length;
    pillBar.hidden = true;
    pillPct.textContent = "";
  }
  shadow.getElementById("dlm-sub").textContent = anyActive ? agg.count + " 个下载" : "";

  renderConfirm();
  renderActive();
  renderFinished();
  renderLinks();

  const empty = shadow.getElementById("dlm-empty");
  empty.hidden = anyActive || state.items.length > 0 || state.finished.length > 0 || !!state.confirm;
  shadow.getElementById("dlm-host-note").hidden = state.hostReady || !any;
  if (!state.collapsed && !state.confirm && !anyActive && state.items.length > 0 && state.finished.length === 0) {
    // 面板里只有检测列表时也保持紧凑；无需额外提示。
  }
}

// ---- 检测与点击接管 -------------------------------------------------------

function addUnique(list, item) {
  if (!item || !item.url) return;
  if (list.some((x) => x.url === item.url)) return;
  list.push(item);
}

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
    out.push({ url: url.href, kind: "file" });
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
    out.push({ url: item.url, kind: item.kind === "hls" ? "hls" : "file" });
    if (out.length >= 40) break;
  }
  return out;
}

function refreshMedia() {
  const local = collectFileLinks();
  chrome.runtime.sendMessage({ op: "page-links", items: local }, () => void chrome.runtime.lastError);
  chrome.runtime.sendMessage({ op: "media-list" }, (reply) => {
    if (chrome.runtime.lastError) {
      state.items = local;
      render();
      return;
    }
    state.items = mergeItems(reply && reply.items, local);
    render();
  });
}

function downloadKind(a, url) {
  if (a.hasAttribute("download")) return "file";
  if (url.pathname.toLowerCase().endsWith(".m3u8")) return "hls";
  if (FILE_LINK_RE.test(url.pathname)) return "file";
  return null;
}

function confirmClick(a, url, kind) {
  state.confirm = {
    url: url.href,
    kind,
    name: fileName(url.href),
    domain: url.host,
  };
  state.collapsed = false;
  render();
}

function fallbackToBrowser(a) {
  if (a.hasAttribute("download")) {
    const copy = a.cloneNode(true);
    copy.removeAttribute("id");
    document.body.appendChild(copy);
    copy.click();
    copy.remove();
    return;
  }
  window.location.assign(a.href);
}

function onDocumentClick(ev) {
  if (ev.defaultPrevented || ev.button !== 0) return;
  if (ev.ctrlKey || ev.metaKey || ev.shiftKey || ev.altKey) return;
  if (ev.composedPath().includes(host)) return;
  const target = ev.target instanceof Element ? ev.target : null;
  const a = target && target.closest("a[href]");
  if (!a) return;
  if ((a.target || "").toLowerCase() === "_blank") return;
  let url = null;
  try {
    url = new URL(a.getAttribute("href"), location.href);
  } catch {
    return;
  }
  if (!/^https?:$/i.test(url.protocol)) return;
  const kind = downloadKind(a, url);
  if (!kind) return;
  if (!state.hostReady) return;
  ev.preventDefault();
  ev.stopPropagation();
  confirmClick(a, url, kind);
}

chrome.runtime.onMessage.addListener((msg) => {
  if (!msg) return;
  if (msg.op === "job-accepted") {
    if (msg.mode === "queue") {
      state.finished.unshift({ name: msg.name || "下载任务", queue: msg.id });
      state.finished = state.finished.slice(0, 3);
      state.collapsed = false;
    } else {
      state.active.set(msg.job, {
        job: msg.job,
        name: msg.name || "下载任务",
        percent: 0,
        done: 0,
        total: 0,
        speed: 0,
        eta: 0,
        stopping: false,
      });
      state.collapsed = false;
    }
    render();
    return;
  }
  if (msg.op === "job-progress") {
    if (state.done.has(msg.job)) return;
    let entry = state.active.get(msg.job);
    if (!entry) {
      entry = { job: msg.job, name: msg.name || "下载任务", stopping: false };
      state.active.set(msg.job, entry);
    }
    entry.percent = msg.percent || 0;
    entry.done = msg.done || 0;
    entry.total = msg.total || 0;
    entry.speed = msg.speed || 0;
    entry.eta = msg.eta || 0;
    scheduleRender();
    return;
  }
  if (msg.op === "job-cancel") {
    const entry = state.active.get(msg.job);
    if (entry && !msg.ok) {
      entry.stopping = false;
      entry.error = msg.message || "停止失败";
    }
    scheduleRender();
    return;
  }
  if (msg.op === "job-done") {
    state.done.add(msg.job);
    state.active.delete(msg.job);
    state.finished.unshift({
      name: msg.name || "下载任务",
      ok: !!msg.ok,
      canceled: !!msg.canceled,
      code: msg.code,
    });
    state.finished = state.finished.slice(0, 3);
    state.collapsed = false;
    render();
  }
});

chrome.storage.local.get(["lastPong"], (values) => {
  state.hostReady = !!values.lastPong;
  render();
});
chrome.storage.onChanged.addListener(() => {
  chrome.storage.local.get(["lastPong"], (values) => {
    state.hostReady = !!values.lastPong;
    render();
  });
});

document.addEventListener("click", onDocumentClick, true);
render();
refreshMedia();
setInterval(refreshMedia, 1500);
chrome.runtime.sendMessage({ op: "wake" }, () => void chrome.runtime.lastError);
