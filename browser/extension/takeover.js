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

  // 响应头给的 MIME 往往比扩展名更可信（尤其"无扩展名的附件"）。
  // 只认一组明确的"这就是个文件"的类型；application/octet-stream 太泛，不认。
  const DEFAULT_MIME = [
    "application/zip", "application/x-7z-compressed", "application/x-rar-compressed",
    "application/x-tar", "application/gzip", "application/x-iso9660-image",
    "application/vnd.android.package-archive", "application/x-msdownload",
    "application/x-msi", "application/pdf", "application/epub+zip",
    "application/octet-stream+dlm",
  ];

  function mimeAllowed(mime, list) {
    const m = String(mime || "").toLowerCase().split(";")[0].trim();
    if (!m) return false;
    const allow = Array.isArray(list) && list.length ? list : DEFAULT_MIME;
    if (allow.map((x) => String(x).toLowerCase()).includes(m)) return true;
    // 音视频按大类放行：它们几乎一定是"文件"而不是页面
    return /^(video|audio)\//.test(m);
  }

  function fileNameOf(path) {
    const s = String(path || "");
    const cut = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
    return cut >= 0 ? s.slice(cut + 1) : s;
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
    const raw = String(item.finalUrl || item.url || "");
    // 浏览器把数据留在内存/页面上，没有可交给下载器的 URL —— 说清原因，别让用户猜。
    if (/^blob:/i.test(raw)) return { take: false, reason: "blob" };
    if (/^data:/i.test(raw)) return { take: false, reason: "data" };
    const name = item.filename || item.finalUrl || item.url || "";
    if (!extAllowed(name, cfg.whitelist) && !mimeAllowed(item.mime, cfg.mimeWhitelist)) {
      return { take: false, reason: "ext" };
    }
    if (!siteAllowed(item.finalUrl || item.url, cfg.blocklist)) {
      return { take: false, reason: "site" };
    }
    const min = Number(cfg.minBytes) || 0;
    const size = Number(item.fileSize);
    if (min > 0 && size > 0 && size < min) return { take: false, reason: "too-small" };
    return { take: true, reason: "ok" };
  }

  const api = { DEFAULT_WHITELIST, DEFAULT_MIME, extOf, extAllowed, mimeAllowed, fileNameOf, siteAllowed, shouldTakeover };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  root.dlmTakeover = api;
})(typeof globalThis !== "undefined" ? globalThis : this);