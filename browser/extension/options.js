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

chrome.storage.local.get([TK, BLOCK], (got) => {
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