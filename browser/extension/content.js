// 页面里的 dlm 悬浮球：检测到的下载、进行中进度、确认后交付。
// 组件住在 shadow DOM 里，避免被站点 CSS 污染；所有用户数据只用 textContent 写入。
// 悬浮球可拖动，位置存 chrome.storage.local；空闲 5 秒后降到 55% 透明。
document.documentElement.setAttribute("data-dlm-version", chrome.runtime.getManifest().version);

const FILE_LINK_RE = /\.(zip|rar|7z|tar|gz|bz2|xz|iso|cab|pdf|docx?|xlsx?|pptx?|txt|md|epub|mobi|csv|exe|msi|apk|dmg|pkg|deb|rpm|mp4|mkv|avi|mov|webm|m4[av]|mp3|flac|wav|aac|ogg|opus|wma|jpg|jpeg|png|gif|bmp|webp|svg|tiff?|heic|ts|bin)(?:$|[?#])/i;

const POS_KEY = "dlmBallPos";
const BALL_SIZE = 48;
const MARGIN = 8;
const DRAG_THRESHOLD = 4;
const IDLE_AFTER_MS = 5000;

const state = {
  items: [],
  active: new Map(),
  done: new Set(),
  finished: [],
  confirm: null,
  collapsed: true,
  hostReady: false,
  tab: "active",
};

let renderTimer = null;
let shadow = null;
let host = null;
let rootEl = null;
let drag = null;
let suppressClick = false;
let lastActivity = Date.now();

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

function hostOf(url) {
  try {
    return new URL(url).host;
  } catch {
    return "";
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
    display: block;
    position: fixed; right: 12px; bottom: 12px; left: auto; top: auto;
    z-index: 2147483647;
    color-scheme: light dark;
    font-family: "Segoe UI Variable", "Segoe UI", system-ui, "Microsoft YaHei", sans-serif;
    font-size: 13px; line-height: 1.4; color: #2a2720;
    /* 暖纸 + 陶土橙：与原生窗口同一套语言（src/gui/app/theme.rs） */
    --card: light-dark(#fffdf8, #221f1b);
    --ink: light-dark(#2a2720, #ede8dc);
    --muted: light-dark(#6c6860, #a09a8e);
    --line: light-dark(#e4dfd2, #332e28);
    --page: light-dark(#f1ede4, #17150f);
    --track: light-dark(#e7e2d5, #2e2a24);
    --accent: light-dark(#c06340, #d97757);
    --accent-2: light-dark(#d0855f, #e08a66);
    --accent-soft: light-dark(#f2e1d7, #3a2a22);
    --accent-ink: light-dark(#fff9f4, #1a1210);
    --danger: light-dark(#a2443a, #e07a6e);
    --shadow: light-dark(0 14px 36px rgba(42,39,32,.18), 0 14px 36px rgba(0,0,0,.5));
  }
  * { box-sizing: border-box; }
  button { font: inherit; }
  .root { position: relative; width: ${BALL_SIZE}px; height: ${BALL_SIZE}px; }
  .ball {
    position: relative; width: ${BALL_SIZE}px; height: ${BALL_SIZE}px; padding: 0;
    border: 0; border-radius: 50%; cursor: grab; touch-action: none;
    display: grid; place-items: center;
    background: linear-gradient(145deg, var(--accent), #a34f31);
    color: #fff9f4; box-shadow: 0 8px 24px rgba(42,39,32,.30);
    transition: opacity .28s ease, transform .15s ease, box-shadow .2s ease;
  }
  .ball:hover { transform: translateY(-1px); box-shadow: 0 10px 30px rgba(42,39,32,.36); }
  .ball:focus-visible { outline: 2px solid var(--accent-2); outline-offset: 3px; }
  .root.dragging .ball { cursor: grabbing; transform: scale(1.04); }
  .root.idle .ball { opacity: .55; }
  .root.idle:hover .ball, .root.dragging .ball { opacity: 1; }
  .ring { position: absolute; inset: -5px; width: 58px; height: 58px; transform: rotate(-90deg); pointer-events: none; }
  .ring circle { fill: none; stroke: var(--accent-2); stroke-width: 3; stroke-linecap: round; }
  .badge {
    position: absolute; top: -4px; right: -4px; min-width: 18px; height: 18px; padding: 0 4px;
    border: 2px solid var(--card); border-radius: 999px; background: var(--danger); color: #fff;
    font: 600 10px/14px ui-monospace, Consolas, monospace; text-align: center;
  }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .panel {
    position: absolute; left: 0; bottom: 58px; width: min(340px, calc(100vw - 24px));
    max-height: min(70vh, 560px); display: flex; flex-direction: column; overflow: hidden;
    background: var(--card); border: 1px solid var(--line); border-radius: 14px; box-shadow: var(--shadow);
  }
  .panel.side-left { left: auto; right: 0; }
  .panel.dir-down { bottom: auto; top: 58px; }
  .panel[hidden] { display: none; }
  .ph { display: flex; align-items: center; gap: 8px; padding: 10px 12px; border-bottom: 1px solid var(--line); background: var(--page); }
  .mark {
    width: 26px; height: 26px; flex: 0 0 auto; border-radius: 8px; display: grid; place-items: center;
    background: var(--accent); color: var(--accent-ink); font: 500 14px/1 ui-monospace, Consolas, monospace;
  }
  .ph b { font-size: 13px; font-weight: 600; color: var(--ink); }
  .ph .sub { min-width: 0; margin-right: auto; font-size: 11px; color: var(--muted); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .ph .icon {
    min-width: 30px; height: 30px; border: 0; border-radius: 7px; background: transparent;
    color: var(--muted); cursor: pointer; font-size: 12px; padding: 0 6px;
  }
  .ph .icon:hover { background: var(--card); color: var(--ink); }
  .ph .icon:last-child { margin-left: 0; }
  .tabs { display: flex; align-items: center; gap: 2px; padding: 6px 8px; border-bottom: 1px solid var(--line); }
  .tab {
    display: flex; align-items: center; gap: 5px; border: 0; border-radius: 7px; padding: 6px 8px;
    background: transparent; color: var(--muted); font-size: 11px; cursor: pointer;
  }
  .tab:hover { background: var(--page); color: var(--ink); }
  .tab.active { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
  .tab .n {
    min-width: 15px; height: 15px; padding: 0 3px; border-radius: 999px; background: var(--page);
    color: var(--muted); font: 500 9px/15px ui-monospace, Consolas, monospace; text-align: center;
  }
  .tab.active .n { background: var(--card); color: var(--accent); }
  .body { padding: 10px 12px 12px; overflow: auto; max-height: min(56vh, 420px); }
  .pane[hidden] { display: none; }
  .ask { border: 1px solid var(--accent); background: var(--accent-soft); border-radius: 10px; padding: 11px; margin-bottom: 10px; }
  .ask-title { font-size: 13px; font-weight: 500; margin-bottom: 6px; color: var(--ink); }
  .ask-name { font-size: 12px; color: var(--ink); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .ask-src { font: 10px/1.4 ui-monospace, Consolas, monospace; color: var(--muted); margin-top: 4px; }
  .ask-actions { display: flex; justify-content: flex-end; gap: 6px; margin-top: 9px; }
  .active { border: 1px solid var(--line); border-radius: 9px; padding: 10px; background: var(--page); margin-bottom: 8px; }
  .active:last-child { margin-bottom: 0; }
  .rowhead { display: flex; align-items: baseline; gap: 8px; }
  .name { flex: 1; min-width: 0; font-size: 12px; font-weight: 500; color: var(--ink); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .pct { flex: 0 0 auto; font: 600 11px/1 ui-monospace, Consolas, monospace; color: var(--accent); }
  .bar { width: 100%; height: 6px; border-radius: 999px; background: var(--track); overflow: hidden; margin: 8px 0 6px; }
  .bar i { display: block; height: 100%; border-radius: 999px; background: linear-gradient(90deg, var(--accent), var(--accent-2)); }
  .meta { display: flex; flex-wrap: wrap; gap: 8px; font: 10px/1.3 ui-monospace, Consolas, monospace; color: var(--muted); }
  .actions { display: flex; justify-content: flex-end; margin-top: 7px; }
  .ghost, .primary, .dl {
    border-radius: 7px; font: 500 11px/1 inherit; cursor: pointer; min-height: 30px; padding: 0 10px;
  }
  .ghost { border: 1px solid var(--line); background: var(--card); color: var(--muted); }
  .ghost:hover { color: var(--ink); }
  .primary { border: 0; background: var(--accent); color: var(--accent-ink); padding: 0 14px; }
  .done { display: flex; align-items: center; gap: 7px; padding: 8px 2px; border-bottom: 1px solid var(--line); }
  .done:last-child { border-bottom: 0; }
  .done .name { flex: 1; font-weight: 400; }
  .done .meta { font-size: 10px; }
  .check { color: var(--accent); font-weight: 500; }
  .cross { color: var(--danger); font-weight: 500; }
  .link { display: flex; align-items: center; gap: 8px; padding: 8px 2px; border-bottom: 1px solid var(--line); }
  .link:last-child { border-bottom: 0; }
  .chip {
    flex: 0 0 auto; border: 1px solid var(--line); border-radius: 4px; padding: 1px 4px;
    font: 9px/1.4 ui-monospace, Consolas, monospace; color: var(--muted);
  }
  .chip.hls { color: var(--accent); border-color: var(--accent-soft); background: var(--accent-soft); }
  .linkinfo { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .url { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font: 11px/1.35 ui-monospace, Consolas, monospace; color: var(--ink); }
  .host { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font: 9px/1.3 ui-monospace, Consolas, monospace; color: var(--muted); }
  .dl { flex: 0 0 auto; border: 0; background: var(--accent-soft); color: var(--accent); padding: 0 9px; }
  .dl:hover { background: var(--accent); color: var(--accent-ink); }
  .empty, .note { font-size: 11px; color: var(--muted); padding: 8px 2px; }
  .note { color: var(--danger); }
  .pf { display: flex; align-items: center; justify-content: space-between; padding: 7px 12px; border-top: 1px solid var(--line); background: var(--page); }
  .linkbtn { border: 0; background: transparent; color: var(--muted); font-size: 10px; cursor: pointer; padding: 4px 0; }
  .linkbtn:hover { color: var(--accent); }
  .ver { font: 9px/1 ui-monospace, Consolas, monospace; color: var(--muted); }
  @media (max-width: 520px) {
    .panel, .panel.side-left, .panel.dir-down {
      position: fixed; left: 12px; right: 12px; top: auto; bottom: 12px; width: auto; max-height: 70vh;
    }
  }
`;

function ensureWidget() {
  if (host) return;
  host = document.createElement("div");
  host.id = "dlm-native-box";
  shadow = host.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = STYLE;
  const root = el("div", "root");
  root.id = "dlm-root";
  root.innerHTML = `
    <button id="dlm-toggle" class="ball" type="button" aria-label="打开 dlm 下载面板" aria-expanded="false" title="dlm">
      <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
        <path d="M12 4v12m-6-6 6 6 6-6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      <svg id="dlm-ring" class="ring" viewBox="0 0 58 58" hidden><circle cx="29" cy="29" r="26"/></svg>
      <span id="dlm-badge" class="badge" hidden>0</span>
      <span id="dlm-pill-meta" class="sr"></span>
    </button>
    <div id="dlm-panel" class="panel" hidden>
      <div class="ph">
        <span class="mark">↓</span><b>dlm</b><span id="dlm-sub" class="sub"></span>
        <button id="dlm-submit" class="icon" type="button" title="把当前页交给 dlm">本页</button>
        <button id="dlm-fold" class="icon" type="button" aria-label="收起面板">×</button>
      </div>
      <div class="tabs" role="tablist" aria-label="下载视图">
        <button class="tab" data-tab="active" role="tab">进行中 <span id="dlm-active-count" class="n">0</span></button>
        <button class="tab" data-tab="links" role="tab">可下载 <span id="dlm-links-count" class="n">0</span></button>
        <button class="tab" data-tab="recent" role="tab">最近 <span id="dlm-recent-count" class="n">0</span></button>
      </div>
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
        <div id="dlm-pane-active" class="pane">
          <div id="dlm-active"></div>
          <div id="dlm-active-empty" class="empty" hidden>没有进行中的下载。</div>
        </div>
        <div id="dlm-pane-links" class="pane" hidden>
          <div id="dlm-list"></div>
          <div id="dlm-links-empty" class="empty" hidden>没有检测到可下载的链接。</div>
        </div>
        <div id="dlm-pane-recent" class="pane" hidden>
          <div id="dlm-finished"></div>
          <div id="dlm-recent-empty" class="empty" hidden>还没有最近记录。</div>
        </div>
        <div id="dlm-host-note" class="note" hidden>宿主未连接，点击不会接管。</div>
      </div>
      <div class="pf">
        <button id="dlm-reset" class="linkbtn" type="button">重置位置</button>
        <span id="dlm-ver" class="ver"></span>
      </div>
    </div>`;
  shadow.append(style, root);
  document.documentElement.appendChild(host);
  rootEl = root;
  shadow.getElementById("dlm-ver").textContent = "v" + chrome.runtime.getManifest().version;

  const ball = shadow.getElementById("dlm-toggle");
  // 点击不挂在这里：统一走 window 捕获路由（onWindowClick）。挂在元素上时，
  // 站点只要在 document 捕获阶段 stopPropagation()，事件就到不了 shadow DOM，
  // 球就点不开了（实测复现）。
  ball.addEventListener("pointerdown", onPointerDown);
  ball.addEventListener("pointermove", onPointerMove);
  ball.addEventListener("pointerup", onPointerUp);
  ball.addEventListener("pointercancel", onPointerUp);
  ball.addEventListener("pointerenter", markActivity);

  // 面板里的按钮和标签页也不挂监听：同样走 onWindowClick 统一路由。

  chrome.storage.local.get([POS_KEY], (values) => {
    const pos = values && values[POS_KEY];
    if (pos && Number.isFinite(pos.x) && Number.isFinite(pos.y)) {
      moveTo(pos.x, pos.y);
    }
  });
}

function scheduleRender() {
  if (renderTimer) return;
  renderTimer = setTimeout(() => {
    renderTimer = null;
    render();
  }, 150);
}

function markActivity() {
  lastActivity = Date.now();
  if (rootEl) rootEl.classList.remove("idle");
}

function updateIdle() {
  if (!rootEl) return;
  const idle =
    state.collapsed &&
    state.active.size === 0 &&
    !state.confirm &&
    Date.now() - lastActivity > IDLE_AFTER_MS;
  rootEl.classList.toggle("idle", idle);
}

function moveTo(x, y) {
  if (!host) return;
  const maxX = Math.max(MARGIN, window.innerWidth - BALL_SIZE - MARGIN);
  const maxY = Math.max(MARGIN, window.innerHeight - BALL_SIZE - MARGIN);
  const left = Math.min(Math.max(MARGIN, x), maxX);
  const top = Math.min(Math.max(MARGIN, y), maxY);
  host.style.left = Math.round(left) + "px";
  host.style.top = Math.round(top) + "px";
  host.style.right = "auto";
  host.style.bottom = "auto";
  if (!state.collapsed) placePanel();
}

function savePosition() {
  const rect = host.getBoundingClientRect();
  chrome.storage.local.set({ [POS_KEY]: { x: Math.round(rect.left), y: Math.round(rect.top) } });
}

function resetPosition() {
  chrome.storage.local.remove(POS_KEY);
  host.style.left = "auto";
  host.style.top = "auto";
  host.style.right = "12px";
  host.style.bottom = "12px";
  markActivity();
  if (!state.collapsed) placePanel();
}

function onPointerDown(ev) {
  if (ev.button !== 0) return;
  // 新的一次按下：清掉上一次拖拽留下的点击抑制。
  suppressClick = false;
  const rect = host.getBoundingClientRect();
  drag = {
    id: ev.pointerId,
    startX: ev.clientX,
    startY: ev.clientY,
    originX: rect.left,
    originY: rect.top,
    moved: false,
  };
  shadow.getElementById("dlm-toggle").setPointerCapture(ev.pointerId);
  markActivity();
  ev.preventDefault();
}

function onPointerMove(ev) {
  if (!drag || ev.pointerId !== drag.id) return;
  const dx = ev.clientX - drag.startX;
  const dy = ev.clientY - drag.startY;
  if (!drag.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
  drag.moved = true;
  rootEl.classList.add("dragging");
  moveTo(drag.originX + dx, drag.originY + dy);
}

function onPointerUp(ev) {
  if (!drag || ev.pointerId !== drag.id) return;
  const moved = drag.moved;
  drag = null;
  rootEl.classList.remove("dragging");
  if (moved) {
    savePosition();
    // 只置位，由随后的 click 消费。原来这里 setTimeout(0) 清零，
    // 和 click 到达的时序是竞态 —— 常常在 click 之前就清了，拖完反而开面板。
    suppressClick = true;
  }
}

function placePanel() {
  if (!host || state.collapsed) return;
  const panel = shadow.getElementById("dlm-panel");
  const rect = host.getBoundingClientRect();
  if (window.innerWidth <= 520) {
    panel.style.maxHeight = "";
    return;
  }
  const spaceRight = window.innerWidth - rect.left - 12;
  const spaceLeft = rect.left + BALL_SIZE + 12;
  panel.classList.toggle("side-left", spaceLeft > spaceRight);
  const spaceBelow = window.innerHeight - rect.top - 70;
  const spaceAbove = rect.top - 70;
  const down = spaceBelow >= spaceAbove;
  panel.classList.toggle("dir-down", down);
  panel.style.maxHeight = Math.max(200, Math.min(560, down ? spaceBelow : spaceAbove)) + "px";
}

function setTab(tab) {
  if (!["active", "links", "recent"].includes(tab)) return;
  state.tab = tab;
  markActivity();
  render();
}

function openPanel() {
  state.collapsed = false;
  if (state.confirm || state.active.size > 0) state.tab = "active";
  else if (state.items.length > 0) state.tab = "links";
  else state.tab = "recent";
  markActivity();
  render();
}

function closePanel() {
  state.collapsed = true;
  render();
}

function togglePanel() {
  markActivity();
  if (state.collapsed) openPanel();
  else closePanel();
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
    const head = el("div", "rowhead");
    head.appendChild(el("span", "name", entry.name || "下载任务"));
    head.appendChild(el("span", "pct", Math.round(entry.percent || 0) + "%"));
    row.appendChild(head);
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
    stop.dataset.job = job;
    actions.appendChild(stop);
    row.appendChild(actions);
    box.appendChild(row);
    index += 1;
  }
  shadow.getElementById("dlm-active-count").textContent = String(index);
  shadow.getElementById("dlm-active-empty").hidden = index > 0;
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
  shadow.getElementById("dlm-recent-count").textContent = String(state.finished.length);
  shadow.getElementById("dlm-recent-empty").hidden = state.finished.length > 0;
}

function renderLinks() {
  const box = shadow.getElementById("dlm-list");
  box.replaceChildren();
  for (const item of state.items) {
    const row = el("div", "link");
    row.appendChild(el("span", "chip" + (item.kind === "hls" ? " hls" : ""), item.kind === "hls" ? "HLS" : "文件"));
    const info = el("div", "linkinfo");
    const url = el("span", "url", fileName(item.url));
    url.title = item.url;
    info.appendChild(url);
    const hostLabel = hostOf(item.url);
    if (hostLabel) info.appendChild(el("span", "host", hostLabel));
    row.appendChild(info);
    const btn = el("button", "dl", "下载");
    btn.type = "button";
    btn.dataset.dlUrl = item.url;
    btn.dataset.dlKind = item.kind;
    row.appendChild(btn);
    box.appendChild(row);
  }
  shadow.getElementById("dlm-links-count").textContent = String(state.items.length);
  shadow.getElementById("dlm-links-empty").hidden = state.items.length > 0;
}

function renderConfirm() {
  const box = shadow.getElementById("dlm-confirm");
  box.hidden = !state.confirm;
  if (!state.confirm) return;
  shadow.getElementById("dlm-confirm-name").textContent = state.confirm.name;
  shadow.getElementById("dlm-confirm-src").textContent =
    state.confirm.domain + " · " + (state.confirm.kind === "hls" ? "HLS" : "文件");
}

function renderTabs() {
  const counts = {
    active: state.active.size,
    links: state.items.length,
    recent: state.finished.length,
  };
  const panes = {
    active: shadow.getElementById("dlm-pane-active"),
    links: shadow.getElementById("dlm-pane-links"),
    recent: shadow.getElementById("dlm-pane-recent"),
  };
  for (const tab of shadow.querySelectorAll(".tab")) {
    const key = tab.dataset.tab;
    const on = key === state.tab;
    tab.classList.toggle("active", on);
    tab.setAttribute("aria-selected", on ? "true" : "false");
    tab.querySelector(".n").textContent = String(counts[key] || 0);
    if (panes[key]) panes[key].hidden = !on;
  }
}

function render() {
  ensureWidget();
  host.dataset.dlmReady = state.hostReady ? "1" : "0";
  const anyActive = state.active.size > 0;
  const any = state.items.length > 0 || anyActive || state.finished.length > 0 || !!state.confirm;
  host.hidden = !any;
  if (!any) return;

  shadow.getElementById("dlm-panel").hidden = state.collapsed;
  shadow.getElementById("dlm-toggle").setAttribute("aria-expanded", state.collapsed ? "false" : "true");

  const agg = aggregate();
  const ring = shadow.getElementById("dlm-ring");
  const badge = shadow.getElementById("dlm-badge");
  if (agg.count > 0) {
    const circumference = 2 * Math.PI * 26;
    ring.hidden = false;
    ring.style.strokeDasharray = `${((agg.percent / 100) * circumference).toFixed(1)} ${circumference.toFixed(1)}`;
    badge.hidden = false;
    badge.textContent = String(agg.count);
  } else {
    ring.hidden = true;
    badge.hidden = true;
  }
  shadow.getElementById("dlm-pill-meta").textContent = agg.count
    ? agg.count + " 个下载"
    : "可下载 " + state.items.length;

  const ball = shadow.getElementById("dlm-toggle");
  ball.title = agg.count ? "dlm：" + agg.count + " 个下载进行中" : "dlm：可下载 " + state.items.length;
  ball.setAttribute("aria-label", state.collapsed ? "打开 dlm 下载面板" : "收起 dlm 下载面板");

  let sub = "";
  if (agg.count > 0) sub = agg.count + " 个进行中";
  else if (state.items.length > 0) sub = "可下载 " + state.items.length;
  else if (state.finished.length > 0) sub = "最近 " + state.finished.length;
  shadow.getElementById("dlm-sub").textContent = sub;

  if (state.confirm) state.tab = "active";
  renderConfirm();
  renderActive();
  renderFinished();
  renderLinks();
  renderTabs();

  shadow.getElementById("dlm-host-note").hidden = state.hostReady;
  updateIdle();
  if (!state.collapsed) placePanel();
}

// ---- 检测与点击接管 -------------------------------------------------------

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
  state.tab = "active";
  markActivity();
  render();
}

function nativeDownload(url, name) {
  const a = document.createElement("a");
  a.href = url;
  const base = (name || "").split("/").pop();
  if (base) a.download = base;
  a.dataset.dlmNative = "1";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

function onPageHookMessage(ev) {
  if (ev.source !== window) return;
  const data = ev.data;
  if (!data || data.source !== "dlm-page" || data.type !== "anchor-download") return;
  if (!/^https?:\/\//i.test(data.url || "")) return;
  if (!state.hostReady) {
    nativeDownload(data.url, data.name);
    return;
  }
  let url = null;
  try {
    url = new URL(data.url);
  } catch {
    return;
  }
  const base = (data.name || "").split("/").pop();
  state.confirm = {
    url: url.href,
    kind: url.pathname.toLowerCase().endsWith(".m3u8") ? "hls" : "file",
    name: base || fileName(url.href),
    domain: url.host,
  };
  state.collapsed = false;
  state.tab = "active";
  markActivity();
  render();
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
// host 内所有点击的统一入口。
//
// ## 为什么在 window 捕获阶段
//
// 悬浮球和面板住在页面的 shadow DOM 里，事件从真实目标冒泡到 window。
// 站点只要在 document 捕获阶段 stopPropagation()，事件就到不了 shadow DOM ——
// 球点不开、面板按钮全哑。实测：干净页能开面板，站点拦 click 后打不开，
// 再拦 pointerdown 后同样打不开。
//
// window 是捕获路径上 document 的上一站，任何 document 级拦截都盖不住它。
//
// ## 为什么不给每个按钮挂监听
//
// 挂了也没用（被拦），而且两处都在时会重复触发。所以这里一处路由全部动作。
function onWindowClick(ev) {
  if (host && ev.composedPath().includes(host)) {
    if (runHostAction(ev)) {
      ev.preventDefault();
      ev.stopPropagation();
    }
    return;
  }
  onDocumentClick(ev);
}

// 按元素 id / data-* 路由 host 内的点击。返回 true 表示已处理。
function runHostAction(ev) {
  const raw = ev.composedPath()[0];
  if (!(raw instanceof Element)) return false;
  // 点到按钮里的 <span> 也要归到按钮上。
  const el = raw.closest("button,[data-tab],[data-job],[data-dl-url]") || raw;

  switch (el.id) {
    case "dlm-toggle":
      // 拖拽结束时抑制紧随其后的这次点击。
      if (suppressClick) {
        suppressClick = false;
        return true;
      }
      togglePanel();
      return true;
    case "dlm-fold":
      closePanel();
      return true;
    case "dlm-submit":
      sendDownload(location.href, "file", true);
      return true;
    case "dlm-reset":
      resetPosition();
      return true;
    case "dlm-confirm-no":
      state.confirm = null;
      markActivity();
      render();
      return true;
    case "dlm-confirm-yes": {
      const pending = state.confirm;
      state.confirm = null;
      if (pending) sendDownload(pending.url, pending.kind, false);
      markActivity();
      render();
      return true;
    }
    default:
      break;
  }

  const data = el.dataset || {};
  if (data.tab) {
    setTab(data.tab);
    return true;
  }
  if (data.job) {
    cancelJob(data.job);
    return true;
  }
  if (data.dlUrl) {
    sendDownload(data.dlUrl, data.dlKind || "file", false);
    return true;
  }
  return false;
}

chrome.runtime.onMessage.addListener((msg) => {
  if (!msg) return;
  if (msg.op === "job-accepted") {
    markActivity();
    if (msg.mode === "queue") {
      state.finished.unshift({ name: msg.name || "下载任务", queue: msg.id });
      state.finished = state.finished.slice(0, 3);
      state.collapsed = false;
      state.tab = "recent";
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
      state.tab = "active";
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
    markActivity();
    state.collapsed = false;
    state.tab = state.active.size > 0 ? "active" : "recent";
    render();
  }
});

window.addEventListener("resize", () => {
  if (!host) return;
  if (host.style.left && host.style.left !== "auto") {
    const rect = host.getBoundingClientRect();
    moveTo(rect.left, rect.top);
  } else if (!state.collapsed) {
    placePanel();
  }
});
document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape" && !state.collapsed) closePanel();
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

window.addEventListener("click", onWindowClick, true);
window.addEventListener("message", onPageHookMessage);
setInterval(updateIdle, 1000);
render();
refreshMedia();
setInterval(refreshMedia, 1500);
chrome.runtime.sendMessage({ op: "wake" }, () => void chrome.runtime.lastError);
