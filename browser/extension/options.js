// A3：站点黑名单。只存域名后缀，匹配用 host === rule || host.endsWith("." + rule)。
const KEY = "siteBlock";
const area = document.getElementById("block");
const saved = document.getElementById("saved");

chrome.storage.local.get([KEY], (got) => {
  const list = Array.isArray(got && got[KEY]) ? got[KEY] : [];
  area.value = list.join("\n");
});

document.getElementById("save").addEventListener("click", () => {
  const list = area.value
    .split(/\r?\n/)
    .map((s) => s.trim().toLowerCase())
    .filter((s) => s && !s.includes("/") && !s.includes(" "));
  chrome.storage.local.set({ [KEY]: list }, () => {
    saved.textContent = `已保存 ${list.length} 条`;
    setTimeout(() => { saved.textContent = ""; }, 1800);
  });
});