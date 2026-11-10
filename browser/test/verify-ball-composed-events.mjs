// 悬浮球/面板对「站点事件拦截」的回归测试。
//
// ## 这个测试为什么存在
//
// 之前所有悬浮球验收都用 `element.click()` 程序化触发 —— 那条路只发一个
// `click` 事件，绕过了整条指针链路，所以从没发现真实鼠标点不开的问题。
//
// 站点在 document 捕获阶段 `stopPropagation()` 时，事件到不了 shadow DOM，
// 当时挂在球和按钮上的监听器全都收不到，表现为：球点不开、面板按钮全哑。
// 修复是把 host 内所有点击统一到 window 捕获阶段路由（window 比 document 早）。
//
// 本测试用 CDP 的 Input.dispatchMouseEvent（走浏览器真实输入管线）点真鼠标，
// 并且分别在干净页与两种拦截页上跑，钉住这个行为。
//
// 用法：node browser/test/verify-ball-composed-events.mjs
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const argv = process.argv.slice(2);
const arg = (n, d) => {
  const i = argv.indexOf(`--${n}`);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : d;
};
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const chrome = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const browser = path.resolve(arg("browser", fs.existsSync(edge) ? edge : chrome));
const ext = path.resolve(arg("extension", path.join(root, "browser", "extension")));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// 三种页面：干净 / 拦 click / 连 pointerdown+pointerup 一起拦。
// 站点这两种写法都很常见（点击劫持防护、自研事件总线）。
const CASES = [
  { mode: "clean", block: [] },
  { mode: "click-blocked", block: ["click"] },
  { mode: "pointer-blocked", block: ["pointerdown", "pointerup", "click"] },
];

// 注意：页面里**故意不放**可下载链接。
// 曾经这里有一个 <a href="/file.bin">，扩展会把它识别成可下载项，于是
// render() 里的 any 为 true，恰好掩盖了「空页面上点球没反应」的 bug。
const pageFor = ({ mode, block }) => `<!doctype html><meta charset="utf-8">
<title>dlm ball ${mode}</title>
<body style="font-family:sans-serif;padding:24px">
<h1>${mode}</h1><p>页面上没有任何可下载内容。</p>
<script>
${block.map((k) => `document.addEventListener("${k}", (e) => e.stopPropagation(), true);`).join("\n")}
</script></body>`;

const server = http.createServer((req, res) => {
  const hit = CASES.find((c) => req.url === `/page-${c.mode}.html`);
  if (hit) {
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    res.end(pageFor(hit));
    return;
  }
  if (req.url === "/file.bin") {
    res.writeHead(200, { "Content-Type": "application/octet-stream", "Content-Length": 4 });
    res.end("FILE");
    return;
  }
  res.writeHead(404).end();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dlm-composed-"));
const profile = path.join(tmp, "profile");
fs.mkdirSync(profile, { recursive: true });
const freePort = async () => {
  const s = net.createServer();
  await new Promise((r) => s.listen(0, "127.0.0.1", r));
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
};
const debugPort = await freePort();

const child = spawn(
  browser,
  [
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    `--remote-debugging-port=${debugPort}`,
    "--disable-features=DisableLoadExtensionCommandLineSwitch",
    `--load-extension=${ext}`,
    "--window-size=1200,860",
    `http://127.0.0.1:${port}/page-clean.html`,
  ],
  { stdio: "ignore" },
);

let target = null;
const deadline = Date.now() + 20000;
while (Date.now() < deadline) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
    target = list.find((t) => t.type === "page" && t.url.startsWith("http://127.0.0.1:"));
    if (target) break;
  } catch {}
  await sleep(250);
}
if (!target) {
  console.error("✗ 页面 target 没起来");
  child.kill();
  process.exit(1);
}

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.addEventListener("open", res, { once: true });
  ws.addEventListener("error", rej, { once: true });
});
let seq = 0;
const pending = new Map();
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) {
    const { resolve, reject } = pending.get(m.id);
    pending.delete(m.id);
    m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result);
  }
});
const cdp = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expr) => {
  const r = await cdp("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text);
  return r.result?.value;
};

// 真实输入：走渲染器输入管线，pointerdown/up 与 click 都会按浏览器规则产生。
const realClick = async (x, y) => {
  await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, button: "none", clickCount: 0 });
  await sleep(40);
  await cdp("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
  await sleep(60);
  await cdp("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1 });
  await sleep(250);
};

const S = 'document.getElementById("dlm-native-box").shadowRoot';
const centerOf = (expr) =>
  evaluate(`(() => {
    const el = ${expr};
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2), w: Math.round(r.width) };
  })()`);
const panelOpen = () => evaluate(`!${S}.getElementById("dlm-panel").hidden`);

await cdp("Runtime.enable");

const failures = [];
const log = [];

for (const c of CASES) {
  await cdp("Page.navigate", { url: `http://127.0.0.1:${port}/page-${c.mode}.html` });
  await sleep(1200);
  for (let i = 0; i < 40; i++) {
    if (await evaluate('!!document.getElementById("dlm-native-box")')) break;
    await sleep(200);
  }

  const version = await evaluate('document.documentElement.getAttribute("data-dlm-version")');

  // 面板必须从关着开始
  await evaluate(`(() => { ${S}.getElementById("dlm-panel").hidden = true; return true; })()`);
  await sleep(150);

  // 1) 点球应开面板
  const ball = await centerOf(`${S}.getElementById("dlm-toggle")`);
  await realClick(ball.x, ball.y);
  const opened = await panelOpen();
  log.push(`${c.mode}: 球点击开面板=${opened}`);
  if (!opened) failures.push(`${c.mode}: 点悬浮球没打开面板`);

  // 2) 点「可下载」标签应选中
  let tabOk = false;
  if (opened) {
    const tab = await centerOf(`${S}.querySelector('[data-tab="links"]')`);
    if (tab && tab.w > 0) {
      await realClick(tab.x, tab.y);
      tabOk = (await evaluate(`${S}.querySelector('[data-tab="links"]').getAttribute("aria-selected")`)) === "true";
    }
    log.push(`${c.mode}: 标签切换=${tabOk}`);
    if (!tabOk) failures.push(`${c.mode}: 点「可下载」标签没选中`);
  }

  // 3) 点「×」应关面板
  let foldOk = false;
  if (opened) {
    const fold = await centerOf(`${S}.getElementById("dlm-fold")`);
    if (fold && fold.w > 0) {
      await realClick(fold.x, fold.y);
      foldOk = !(await panelOpen());
    }
    log.push(`${c.mode}: 折叠按钮=${foldOk}`);
    if (!foldOk) failures.push(`${c.mode}: 点「×」没关面板`);
  }

  log.push(`${c.mode}: 扩展版本=${version}`);
}

ws.close();
child.kill();
server.close();
try {
  fs.rmSync(tmp, { recursive: true, force: true });
} catch {}

console.log(log.join("\n"));
if (failures.length) {
  console.error("\n✗ 失败：\n" + failures.map((f) => "  - " + f).join("\n"));
  process.exit(1);
}
console.log("\n✓ 三种页面下，悬浮球与面板按钮都能用真实鼠标操作");
process.exit(0);
