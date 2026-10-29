// MAIN-world 钩子：站点经常 createElement('a') + a.download + a.click()，
// 这种游离节点的点击不经过 DOM 捕获，content script 看不到；这里在页面世界拦下来。
(() => {
  const FILE_RE = /\.(zip|rar|7z|tar|gz|bz2|xz|iso|cab|pdf|docx?|xlsx?|pptx?|txt|md|epub|mobi|csv|exe|msi|apk|dmg|pkg|deb|rpm|mp4|mkv|avi|mov|webm|m4[av]|mp3|flac|wav|aac|ogg|opus|wma|jpg|jpeg|png|gif|bmp|webp|svg|tiff?|heic|ts|bin)(?:$|[?#])/i;

  function downloadUrl(a) {
    if (!(a instanceof HTMLAnchorElement)) return null;
    if (a.dataset && a.dataset.dlmNative === "1") return null;
    const href = a.getAttribute("href") || "";
    let url = null;
    try {
      url = new URL(href, location.href);
    } catch {
      return null;
    }
    if (!/^https?:$/i.test(url.protocol)) return null;
    const named = a.hasAttribute("download");
    if (!named && !url.pathname.toLowerCase().endsWith(".m3u8") && !FILE_RE.test(url.pathname)) return null;
    return url.href;
  }

  const original = HTMLAnchorElement.prototype.click;
  HTMLAnchorElement.prototype.click = function () {
    const url = downloadUrl(this);
    if (!url) {
      original.call(this);
      return;
    }
    const name = this.getAttribute("download") || "";
    window.postMessage({ source: "dlm-page", type: "anchor-download", url, name }, "*");
  };
})();
