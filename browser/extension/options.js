// 扩展设置：接管开关/白名单/最小体积 + 站点黑名单。
const TK = "takeover";
const BLOCK = "siteBlock";
const el = (id) => document.getElementById(id);
const saved = el("saved");

function linesOf(text) {
  return String(text || "")
    .split(/\r?\n|,/)
    .map((s) => s.trim().toLowerCase().replace(/^\./, ""))
    .filter((s) => s && !s.includes("/") && !s.includes(" "));
}

const REASONS = {
  ext: "文件类型不在白名单",
  site: "站点在黑名单里",
  "too-small": "文件太小",
  "already-started": "浏览器已经开始下载",
  state: "下载状态不合适",
  disabled: "接管未开启",
  blob: "blob: 链接（数据在页面内存里，没有可交出的 URL）",
  data: "data: 链接（同上）",
  "no-ack": "宿主 2 秒内没回执，已放手让浏览器自己下",
};
function describeLast(last) {
  if (!last || !last.reason) return "（还没有记录）";
  const t = last.at ? new Date(last.at).toLocaleString() : "";
  if (last.ok) return `${t} 已交给 dlm（${last.mode || "queue"}）`;
  return `${t} 未接管：${REASONS[last.reason] || last.reason}`;
}

chrome.storage.local.get([TK, BLOCK, "lastTakeover"], (got) => {
  document.getElementById("last").textContent = describeLast(got && got.lastTakeover);
  const tk = (got && got[TK]) || {};
  el("tk-enabled").checked = !!tk.enabled;
  el("tk-whitelist").value = Array.isArray(tk.whitelist) && tk.whitelist.length ? tk.whitelist.join(", ") : "";
  el("tk-minbytes").value = Number.isFinite(tk.minBytes) ? tk.minBytes : 1048576;
  const block = Array.isArray(got && got[BLOCK]) ? got[BLOCK] : [];
  el("block").value = block.join("\n");
});

el("save").addEventListener("click", () => {
  const takeover = {
    enabled: !!el("tk-enabled").checked,
    whitelist: linesOf(el("tk-whitelist").value),
    minBytes: Math.max(0, Number(el("tk-minbytes").value) || 0),
  };
  const siteBlock = linesOf(el("block").value);
  chrome.storage.local.set({ [TK]: takeover, [BLOCK]: siteBlock }, () => {
    saved.textContent = "已保存";
    setTimeout(() => { saved.textContent = ""; }, 1800);
  });
});