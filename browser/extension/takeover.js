// downloads 接管的纯判定逻辑（无 chrome API 依赖，便于 node 直接测）。
//
// 设计原则：**宁可漏接，不可错接**。一旦 cancel 错了，用户会得到"浏览器没下成、
// dlm 也没下成"的最坏结果。所以每条规则都只往"不接"的方向保守。
(function (root) {
  const DEFAULT_WHITELIST = [
    "zip", "7z", "rar", "gz", "tar", "iso",
    "exe", "msi", "dmg", "pkg",
    "mp4", "mkv", "avi", "mov", "webm", "flac", "mp3", "wav",
    "pdf", "epub", "apk",
  ];

  function extOf(nameOrUrl) {
    let s = String(nameOrUrl || "").split(/[?#]/)[0];
    const slash = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
    if (slash >= 0) s = s.slice(slash + 1);
    const dot = s.lastIndexOf(".");
    if (dot <= 0 || dot === s.length - 1) return "";
    return s.slice(dot + 1).toLowerCase();
  }

  function extAllowed(nameOrUrl, list) {
    const ext = extOf(nameOrUrl);
    if (!ext) return false;
    const allow = Array.isArray(list) && list.length ? list : DEFAULT_WHITELIST;
    return allow.map((e) => String(e).toLowerCase().replace(/^\./, "")).includes(ext);
  }

  function siteAllowed(url, blocklist) {
    if (!Array.isArray(blocklist) || !blocklist.length) return true;
    let host = "";
    try { host = new URL(url).hostname.toLowerCase(); } catch (err) { return true; }
    return !blocklist.some((rule) => {
      const r = String(rule || "").trim().toLowerCase();
      return r && (host === r || host.endsWith("." + r));
    });
  }

  // item 是 chrome.downloads.DownloadItem
  function shouldTakeover(item, cfg) {
    if (!cfg || !cfg.enabled) return { take: false, reason: "disabled" };
    if (!item) return { take: false, reason: "no-item" };
    if (item.state && item.state !== "in_progress") return { take: false, reason: "state" };
    if ((item.bytesReceived || 0) > 0) return { take: false, reason: "already-started" };
    const name = item.filename || item.finalUrl || item.url || "";
    if (!extAllowed(name, cfg.whitelist)) return { take: false, reason: "ext" };
    if (!siteAllowed(item.finalUrl || item.url, cfg.blocklist)) {
      return { take: false, reason: "site" };
    }
    const min = Number(cfg.minBytes) || 0;
    const size = Number(item.fileSize);
    if (min > 0 && size > 0 && size < min) return { take: false, reason: "too-small" };
    return { take: true, reason: "ok" };
  }

  const api = { DEFAULT_WHITELIST, extOf, extAllowed, siteAllowed, shouldTakeover };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  root.dlmTakeover = api;
})(typeof globalThis !== "undefined" ? globalThis : this);