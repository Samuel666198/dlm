// 「右键指向的是哪个链接」的回归测试（2026-11-27）。
//
// ## 为什么需要它
//
// 用户报：「右键下载是下载本页所有链接，就算在一个可下载的按钮上右键也不会变」。
//
// 根因不在菜单定义，而在 Chrome 的 `contextMenus.onClicked`：对一个**用 JS 触发下载的按钮**
// （`<button>`、`<div>`、`<a download>` 但 href 为空），它既不给 `linkUrl` 也不给 `srcUrl`。
// 后台原来那句 `const url = info.linkUrl || info.srcUrl; if (!url) return;` 于是直接哑掉，
// 用户眼里只剩「下载本页所有链接」那一条。
//
// 修法是 content.js 在 `contextmenu`（捕获阶段）把被点元素往上找，记住那个 URL，
// 按 tab 上报给后台。这里测的就是那个"往上找"的函数 `urlUnderPointer`。
//
// ## 怎么跑
//
// 不启动浏览器：`urlUnderPointer` 只用到 `el.href` / `el.getAttribute()` / `el.parentElement`
// 三样东西，用普通对象就能喂。浏览器里"真的右键"那一步只能靠人工确认
// （见计划文档「尚未完成」第 1 条）。
//
// 用法：node browser/test/verify-context-target.mjs
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const ext = path.resolve(here, "..", "extension");
const contentSrc = fs.readFileSync(path.join(ext, "content.js"), "utf8");

let failures = 0;
const check = (name, ok, detail) => {
  if (ok) return;
  failures += 1;
  console.error(`FAIL ${name}${detail ? ` — ${detail}` : ""}`);
};

// ---- 从 content.js 里取出 urlUnderPointer 的源码（连同它的函数体） -------------
const startMarker = "function urlUnderPointer(start) {";
const start = contentSrc.indexOf(startMarker);
if (start < 0) {
  console.error("FAIL 在 content.js 里找不到 urlUnderPointer —— 右键指向功能被拿掉了？");
  process.exit(1);
}
let depth = 0;
let end = -1;
for (let i = contentSrc.indexOf("{", start); i < contentSrc.length; i += 1) {
  const ch = contentSrc[i];
  if (ch === "{") depth += 1;
  else if (ch === "}") {
    depth -= 1;
    if (depth === 0) {
      end = i + 1;
      break;
    }
  }
}
if (end < 0) {
  console.error("FAIL urlUnderPointer 的花括号不配对");
  process.exit(1);
}
const fnSrc = contentSrc.slice(start, end);

// ---- 喂一个最小的假环境：只有 location / URL 是必需的 ------------------------
const sandbox = {
  location: { href: "https://site.example/dir/page.html" },
  URL,
};
vm.createContext(sandbox);
const factory = vm.runInContext(`(() => { ${fnSrc}; return urlUnderPointer; })()`, sandbox);

// ---- 造元素的工具 ------------------------------------------------------------
const el = (attrs = {}, extra = {}) => ({
  getAttribute: (name) => (name in attrs ? attrs[name] : null),
  parentElement: null,
  ...extra,
});
const chain = (...nodes) => {
  for (let i = 0; i < nodes.length - 1; i += 1) nodes[i].parentElement = nodes[i + 1];
  return nodes[0];
};

// 1) 直接右键在 <a href> 上
{
  const a = el({}, { href: "https://site.example/files/a.zip" });
  check(
    "a[href] 直接用它的 href",
    factory(a) === "https://site.example/files/a.zip",
    factory(a),
  );
}

// 2) 相对地址要按当前页解析成绝对地址
{
  const a = el({}, { href: "" });
  a.href = new URL("/files/b.zip", sandbox.location.href).href;
  check("相对 href 解析成绝对", factory(a) === "https://site.example/files/b.zip", factory(a));
}

// 3) 按钮上带 data-url（站点常见的做法）
{
  const button = el({ "data-url": "/download?id=9&name=c.zip" });
  check(
    "按钮的 data-url 能用（查询串保留）",
    factory(button) === "https://site.example/download?id=9&name=c.zip",
    factory(button),
  );
}

// 4) 按钮自己什么也没有，但外层 <a href> 有
{
  const span = el({});
  const a = el({}, { href: "https://site.example/files/d.zip" });
  chain(span, a);
  check("往上找到外层的 <a>", factory(span) === "https://site.example/files/d.zip", factory(span));
}

// 5) data-href 也算
{
  const div = el({ "data-href": "https://cdn.example/e.mp4" });
  check("data-href 也算", factory(div) === "https://cdn.example/e.mp4", factory(div));
}

// 6) 什么都没有 ⇒ 空串（后台据此继续走原来的逻辑，不许编一个 URL 出来）
{
  const bare = el({});
  check("找不到时给空串，不许瞎编", factory(bare) === "", JSON.stringify(factory(bare)));
}

// 7) 只认 http(s)：javascript: / data: 之类必须被跳过
{
  const bad = el({ "data-url": "javascript:void(0)" });
  const a = el({}, { href: "https://site.example/files/f.zip" });
  chain(bad, a);
  check(
    "跳过 javascript: 继续往上找",
    factory(bad) === "https://site.example/files/f.zip",
    factory(bad),
  );
}

// 8) 层级再深也要找到（上限 6 层，这里 4 层）
{
  const leaf = el({});
  const mid1 = el({});
  const mid2 = el({});
  const a = el({}, { href: "https://site.example/files/g.zip" });
  chain(leaf, mid1, mid2, a);
  check("4 层深也能找到", factory(leaf) === "https://site.example/files/g.zip", factory(leaf));
}

// 9) 后台真的会用它（否则上报了也白搭）
const bgPath = path.join(ext, (/\bbackground-[\d.]+\.js\b/.exec(
  fs.readFileSync(path.join(ext, "manifest.json"), "utf8"),
) || [])[0] || "");
if (!bgPath || !fs.existsSync(bgPath)) {
  check("能按 manifest 找到 service worker", false, bgPath);
} else {
  const bgSrc = fs.readFileSync(bgPath, "utf8");
  check("后台建了 contextTargets 表", /const contextTargets = new Map\(\)/.test(bgSrc));
  check(
    "后台在 onClicked 里回退到记下的 URL",
    /info\.linkUrl \|\| info\.srcUrl \|\| remembered\.url/.test(bgSrc),
  );
  check("content.js 会上报 context-target", /op: "context-target"/.test(contentSrc));
}

if (failures) {
  console.error(`context-target 检查 FAILED（${failures} 条）`);
  process.exit(1);
}
console.log("context-target ok: 9 类右键形态都能找到 URL，后台会取用");
