// 扩展静态一致性检查：不启动浏览器，只读文件。
// 1) manifest 指向的 background 文件必须存在，且版本号与文件名一致（扩展用带版本号的 SW 文件名来绕缓存）；
// 2) content.js 与 page-hook.js 各有一份文件后缀正则（MAIN world 不能共享模块），两份必须逐字相同。
// 3) 宿主回给扩展的直连原因（src/host.rs 的 DIRECT_* 常量）必须都被 content.js 认识 ——
//    这是跨语言的两份真相：Rust 改了标记而扩展没跟上，用户就会看到一句不对的原因，
//    或者干脆没有解释（2026-11-27「浏览器下载没出现在桌面端」就是这个形态）。
//
// 用法：node browser/test/check-extension-static.mjs
import { readFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ext = join(here, "..", "extension");
const failures = [];

const manifest = JSON.parse(readFileSync(join(ext, "manifest.json"), "utf8"));
const sw = manifest.background?.service_worker;
if (!sw) {
  failures.push("manifest.background.service_worker 缺失");
} else {
  if (!existsSync(join(ext, sw))) failures.push(`manifest 指向的 ${sw} 不存在`);
  const m = /^background-(.+)\.js$/.exec(sw);
  if (!m) failures.push(`service_worker 文件名不是 background-<version>.js：${sw}`);
  else if (m[1] !== manifest.version) {
    failures.push(`service_worker 版本 ${m[1]} 与 manifest.version ${manifest.version} 不一致`);
  }
}

function fileRegex(file, pattern) {
  const src = readFileSync(join(ext, file), "utf8");
  const m = pattern.exec(src);
  if (!m) {
    failures.push(`${file} 里找不到声明的文件后缀正则`);
    return null;
  }
  return m[1];
}

const contentRe = fileRegex("content.js", /const\s+FILE_LINK_RE\s*=\s*(\/.*\/[a-z]*)\s*;/);
const hookRe = fileRegex("page-hook.js", /const\s+FILE_RE\s*=\s*(\/.*\/[a-z]*)\s*;/);
if (contentRe && hookRe && contentRe !== hookRe) {
  failures.push("content.js 的 FILE_LINK_RE 与 page-hook.js 的 FILE_RE 不一致（两份必须同步改）");
}

const contentSrc = readFileSync(join(ext, "content.js"), "utf8");
const hostRs = readFileSync(join(here, "..", "..", "src", "host.rs"), "utf8");
const directTokens = [...hostRs.matchAll(/pub const DIRECT_[A-Z_]+: &str = "([a-z-]+)";/g)].map(
  (m) => m[1],
);
if (directTokens.length === 0) {
  failures.push("src/host.rs 里找不到任何 DIRECT_* 原因常量（扩展靠它解释为何不在窗口里）");
}
for (const token of directTokens) {
  if (!contentSrc.includes(`"${token}"`)) {
    failures.push(`content.js 不认识直连原因 "${token}"（src/host.rs 里定义了它）`);
  }
}

// 4) 接管必须"收网"。
//
// 2026-11-27：watchDownloads 会登记 pendingTakeover，但当时**没有任何地方在回执到达时
// 取消浏览器那份下载** —— 开着接管开关时浏览器和 dlm 会各下一遍。这类"登记了却没人收网"
// 的缺口不会报错、不会崩，只是用户突然发现同一文件下了两份，所以必须由检查钉住。
const bgSrc = existsSync(join(ext, sw)) ? readFileSync(join(ext, sw), "utf8") : "";
if (!/importScripts\(\s*["']takeover\.js["']\s*\)/.test(bgSrc)) {
  failures.push(`${sw} 没有 importScripts("takeover.js")（接管判定逻辑没接上）`);
}
if (/pendingTakeover\.set/.test(bgSrc)) {
  // 必须带括号：光在注释里提到 `chrome.downloads.cancel 之后没法恢复` 不算数。
  if (!/chrome\.downloads\.cancel\s*\(/.test(bgSrc)) {
    failures.push(`${sw} 登记了 pendingTakeover 却从不 chrome.downloads.cancel —— 会下两遍`);
  }
  if (!/chrome\.downloads\.erase\s*\(/.test(bgSrc)) {
    failures.push(`${sw} 取消了下载却没 chrome.downloads.erase（列表里会留一条红叉）`);
  }
  if (!/lastTakeover/.test(bgSrc)) {
    failures.push(`${sw} 接管结果没写 lastTakeover（选项页的"最近一次接管"永远是空的）`);
  }
} else {
  failures.push(`${sw} 里找不到 pendingTakeover.set（接管登记不见了）`);
}

// 5) 两份扩展名表必须逐字一致。
//
// background 会把 FILE_LINK_RE.source 注入到页面里跑，所以复制了一份。复制就会走样：
// 只改一边时，右键「下载本页所有链接」和页面悬浮球的识别结果会不一样，
// 而且两边都不会报错 —— 用户只会觉得"有时候能认出来有时候不行"。
const fileLinkRe = (src) => {
  const m = src.match(/const FILE_LINK_RE =\s*(\/.+?\/i)/);
  return m ? m[1] : null;
};
const reInContent = fileLinkRe(contentSrc);
const reInBg = fileLinkRe(bgSrc);
if (!reInContent) {
  failures.push("content.js 里找不到 FILE_LINK_RE");
} else if (!reInBg) {
  failures.push(`${sw} 里找不到 FILE_LINK_RE（页面链接识别会两套表）`);
} else if (reInContent !== reInBg) {
  failures.push("content.js 与 " + sw + " 的 FILE_LINK_RE 不一致 —— 两边识别结果会不同");
}

// 6) 「下载本页所有链接」必须**先确认再入队**。
//
// 2026-11-27 之前 downloadPageLinks 直接 for 循环 submitPage：一次误点就是几十条任务，
// 而且它当时抓的是 document.links 全量、**完全不过扩展名**。
// 现在要求：函数体内出现确认层消息，且**不出现** submitPage —— 提交只走 batch-add。
const fnStart = bgSrc.indexOf("function downloadPageLinks(");
if (fnStart < 0) {
  failures.push(`${sw} 里找不到 downloadPageLinks`);
} else {
  const fnBody = bgSrc.slice(fnStart, bgSrc.indexOf("\n}\n", fnStart));
  if (!/show-batch-confirm/.test(fnBody)) {
    failures.push("downloadPageLinks 没有走确认层（show-batch-confirm）—— 会直接开始下载");
  }
  if (/submitPage\s*\(/.test(fnBody)) {
    failures.push("downloadPageLinks 里直接 submitPage —— 批量下载必须等用户确认");
  }
}

// 7) 右键要认得"用 JS 触发下载的按钮"。
if (!/context-target/.test(contentSrc) || !/context-target/.test(bgSrc)) {
  failures.push("右键目标上报（context-target）没接上：按钮上右键会只剩「下载本页所有链接」");
}
if (!/contextTargets\.get\s*\(/.test(bgSrc)) {
  failures.push(`${sw} 收了 context-target 却从不在 onClicked 里取用`);
}

// 8) 链接识别必须带上查询串。
//
// 只看 `url.pathname` 时，`/download?id=9&name=x.zip` 这类链接永远认不出来。
// 用户 2026-11-27 报"检测能力退化"，这就是其中一个真实原因（不是正则被裁了）。
// 所以任何一处 `.test(url.pathname)` 都必须改成 `.test(url.pathname + url.search)`。
const hookPath = join(ext, "page-hook.js");
const hookSrc = existsSync(hookPath) ? readFileSync(hookPath, "utf8") : "";
for (const [name, src] of [
  ["content.js", contentSrc],
  [sw, bgSrc],
  ["page-hook.js", hookSrc],
]) {
  if (!src) continue;
  if (/\.test\(\s*url\.pathname\s*\)/.test(src)) {
    failures.push(`${name} 里有只测 url.pathname 的识别判据 —— 查询串带文件名的链接会漏掉`);
  }
}
if (contentSrc && !/\.test\(url\.pathname \+ url\.search\)/.test(contentSrc)) {
  failures.push("content.js 的链接识别没有带上 url.search（识别能力会退化）");
}

// 9) 文件大小探测：内容脚本问、后台答，且**必须限流**。
//
// 为什么要有这条：探测是"对第三方站点发请求"，一旦没有上限，展开一次就是几十个
// 请求，既慢又像在打人家服务器。8 条是用户那栏默认可见的数量。
if (!/op: "probe-size"/.test(contentSrc)) {
  failures.push("content.js 没有发 probe-size（悬浮菜单不会显示文件大小）");
}
if (!/msg\.op === "probe-size"/.test(bgSrc)) {
  failures.push(`${sw} 没有处理 probe-size`);
}
// 限流必须**在 requestSizes 函数体内**：别的地方出现 slice(0, 8) 不算数
// （第一版就是这么写成假绿的 —— 拿 `ProbeSizeX` 也能通过 `/probeSize/` 那种松检查）。
const reqStart = contentSrc.indexOf("function requestSizes(");
const reqBody =
  reqStart < 0 ? "" : contentSrc.slice(reqStart, contentSrc.indexOf("\n}", reqStart));
if (!reqBody) {
  failures.push("content.js 里找不到 requestSizes");
} else if (!/slice\(0,\s*8\)/.test(reqBody)) {
  failures.push("requestSizes 里没有 slice(0, 8) —— 展开一次会给站点打一堆请求");
}
if (/msg\.op === "probe-size"/.test(bgSrc)) {
  if (!/async function probeSize\(/.test(bgSrc)) {
    failures.push(`${sw} 收到 probe-size 却没有 probeSize 实现（要有函数定义，不能只有调用）`);
  }
  if (!/probeSize\(url\)/.test(bgSrc)) {
    failures.push(`${sw} 没有真的调用 probeSize(url)`);
  }
}

if (failures.length) {
  console.error("extension static check FAILED");
  for (const f of failures) console.error(`- ${f}`);
  process.exit(1);
}
console.log(
  `extension static check ok: version=${manifest.version}, sw=${sw}, suffix-regex 双份一致, direct-reason ${directTokens.length} 个对齐, 接管收网在位, 批量下载先确认, 右键目标上报在位`,
);
