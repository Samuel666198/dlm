// 悬浮球真浏览器验收：拖动、位置记忆、闲置淡出、面板方向与标签页。
//
// 用法：node browser/test/verify-widget-ball.mjs [--browser edge.exe]
// 前置：无需宿主；只要扩展能加载。页面里放一个文件链接，扩展会把它列为“可下载”。
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const argv = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
};
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const chrome = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const browser = path.resolve(arg("browser", fs.existsSync(edge) ? edge : chrome));
const ext = path.resolve(arg("extension", path.join(root, "browser", "extension")));
const out = path.resolve(arg("out", path.join(root, "docs", "evidence", "2026-11-03-floating-ball.png")));
const logPath = path.resolve(arg("log", path.join(root, "docs", "evidence", "2026-11-03-floating-ball.log")));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const pageHtml = '<!doctype html><html><head><meta charset="utf-8"><title>dlm ball</title></head>' +
  '<body style="font-family:Segoe UI,sans-serif;padding:24px">' +
  '<h1>dlm 悬浮球验收</h1><p>下面两个链接会被扩展识别为可下载。</p>' +
  '<a id="file-link" href="/file.bin">file.bin</a> · <a id="file-link2" href="/other.bin">other.bin</a>' +
  '</body></html>';
const server = http.createServer((req, res) => {
  if (req.url === "/page.html") {
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Content-Length": Buffer.byteLength(pageHtml) });
    res.end(pageHtml);
    return;
  }
  if (req.url === "/file.bin" || req.url === "/other.bin") {
    res.writeHead(200, { "Content-Type": "application/octet-stream", "Content-Length": 4 });
    res.end("FILE");
    return;
  }
  res.writeHead(404, { "Content-Length": 0 });
  res.end();
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const pageUrl = `http://127.0.0.1:${server.address().port}/page.html`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dlm-ball-"));
const profile = path.join(tmp, "profile");
fs.mkdirSync(profile, { recursive: true });

async function freePort() {
  const srv = net.createServer();
  await new Promise((resolve) => srv.listen(0, "127.0.0.1", resolve));
  const port = srv.address().port;
  await new Promise((resolve) => srv.close(resolve));
  return port;
}
const debugPort = await freePort();
const child = spawn(browser, [
  `--user-data-dir=${profile}`,
  `--remote-debugging-port=${debugPort}`,
  "--remote-allow-origins=*",
  "--no-first-run",
  "--no-default-browser-check",
  "--disable-search-engine-choice-screen",
  "--disable-features=DisableLoadExtensionCommandLineSwitch",
  `--disable-extensions-except=${ext}`,
  `--load-extension=${ext}`,
  "--window-size=1000,700",
  pageUrl,
], { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });

let ws;
let log = "";
let failed = null;
try {
  const deadline = Date.now() + 20000;
  let target = null;
  while (Date.now() < deadline) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
      target = list.find((x) => x.type === "page" && x.url.startsWith(pageUrl));
      if (target) break;
    } catch {}
    await sleep(200);
  }
  if (!target) throw new Error("CDP page target not found");
  const version = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
  log += `browser=${version.Browser}\npage=${pageUrl}\n`;

  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  let seq = 0;
  const pending = new Map();
  ws.addEventListener("message", (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(JSON.stringify(msg.error)));
      else resolve(msg.result);
    }
  });
  const cdp = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
  const evalText = async (expression) => {
    const r = await cdp("Runtime.evaluate", { expression, returnByValue: true });
    return r.result?.value;
  };
  const evalJson = async (expression) => {
    const r = await cdp("Runtime.evaluate", { expression, returnByValue: true });
    return r.result?.value;
  };
  const helper = "window.__dlm=(sel)=>document.getElementById('dlm-native-box')?.shadowRoot?.querySelector(sel)||null; true";
  const waitInjected = async () => {
    await evalText(helper);
    for (let i = 0; i < 60; i++) {
      if (await evalText("!!window.__dlm('#dlm-toggle')")) return;
      await sleep(250);
    }
    throw new Error("扩展未注入");
  };
  const rectOf = async (expr) =>
    evalJson(`(() => { const r=${expr}; return { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) }; })()`);
  const dragBall = async (dx, dy) => {
    const p = await evalJson(`(() => { const r=window.__dlm('#dlm-toggle').getBoundingClientRect(); return { x: r.left + r.width/2, y: r.top + r.height/2 }; })()`);
    await cdp("Input.dispatchMouseEvent", { type: "mousePressed", x: p.x, y: p.y, button: "left", buttons: 1, clickCount: 1 });
    for (let i = 1; i <= 6; i++) {
      await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: p.x + (dx * i) / 6, y: p.y + (dy * i) / 6, button: "left", buttons: 1 });
      await sleep(20);
    }
    await cdp("Input.dispatchMouseEvent", { type: "mouseReleased", x: p.x + dx, y: p.y + dy, button: "left", buttons: 0, clickCount: 1 });
    await sleep(350);
  };

  await waitInjected();
  await sleep(2200);
  const meta = (await evalText("window.__dlm('#dlm-pill-meta')?.textContent || ''")) || "";
  log += `ball_meta=${meta}\n`;
  if (!meta.startsWith("可下载")) throw new Error(`悬浮球没有显示可下载数量：${meta}`);
  const initial = await rectOf("document.getElementById('dlm-native-box').getBoundingClientRect()");
  log += `initial=${JSON.stringify(initial)}\n`;
  if (initial.x < 700 || initial.y < 400) throw new Error(`初始位置不在右下角：${JSON.stringify(initial)}`);

  await dragBall(-620, -420);
  const moved = await rectOf("document.getElementById('dlm-native-box').getBoundingClientRect()");
  log += `moved=${JSON.stringify(moved)}\n`;
  if (Math.abs(moved.x - initial.x) < 100 || Math.abs(moved.y - initial.y) < 100) {
    throw new Error(`拖动没有生效：${JSON.stringify({ initial, moved })}`);
  }

  await evalText("window.__dlm('#dlm-toggle').click(); true");
  await sleep(200);
  let panel = await evalText("!window.__dlm('#dlm-panel')?.hidden");
  if (!panel) throw new Error("点击悬浮球没有打开面板");
  const dirTopLeft = await evalText("window.__dlm('#dlm-panel').className");
  log += `panel_at_top_left=${dirTopLeft}\n`;
  if (!dirTopLeft.includes("dir-down")) throw new Error("球在左上时面板应在下方展开");
  await evalText("window.__dlm('#dlm-fold').click(); true");
  await sleep(100);

  await dragBall(700, 500);
  const moved2 = await rectOf("document.getElementById('dlm-native-box').getBoundingClientRect()");
  log += `moved2=${JSON.stringify(moved2)}\n`;
  if (moved2.x <= moved.x || moved2.y <= moved.y) throw new Error(`第二次拖动没有生效：${JSON.stringify({ moved, moved2 })}`);
  await evalText("window.__dlm('#dlm-toggle').click(); true");
  await sleep(200);
  const dirBottomRight = await evalText("window.__dlm('#dlm-panel').className");
  log += `panel_at_bottom_right=${dirBottomRight}\n`;
  if (!dirBottomRight.includes("side-left")) throw new Error("球在右下时面板应向左展开");

  await evalText("window.__dlm('.tab[data-tab=links]').click(); true");
  await sleep(150);
  const linksVisible = await evalText("!window.__dlm('#dlm-pane-links')?.hidden");
  const linkCount = await evalText("window.__dlm('#dlm-list')?.children.length || 0");
  log += `links_tab=${linksVisible} count=${linkCount}\n`;
  if (!linksVisible || linkCount < 2) throw new Error("可下载标签页没有显示条目");

  const shot = await cdp("Page.captureScreenshot", { format: "png" });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, "base64"));
  log += `screenshot=${out}\n`;

  await evalText("window.__dlm('#dlm-fold').click(); true");
  await sleep(200);
  const panelClosed = await evalText("window.__dlm('#dlm-panel')?.hidden");
  log += `panel_closed=${panelClosed}\n`;
  if (!panelClosed) throw new Error("点击收起后面板没有关闭");
  for (let i = 0; i < 6; i++) {
    await sleep(1000);
    log += `class_${i + 1}s=` + (await evalText("window.__dlm('#dlm-root')?.className || ''")) + "\n";
  }
  const idleClass = await evalText("window.__dlm('#dlm-root')?.classList.contains('idle')");
  const opacity = await evalText("getComputedStyle(window.__dlm('#dlm-toggle')).opacity");
  log += `idle=${idleClass} opacity=${opacity}\n`;
  if (!idleClass) throw new Error("闲置 5 秒后没有进入 idle");
  if (Number(opacity) > 0.7) throw new Error(`闲置透明度没有下降：${opacity}`);

  await cdp("Page.reload", { ignoreCache: true });
  await sleep(2500);
  await waitInjected();
  await sleep(1200);
  const afterReload = await rectOf("document.getElementById('dlm-native-box').getBoundingClientRect()");
  log += `after_reload=${JSON.stringify(afterReload)}\n`;
  if (Math.abs(afterReload.x - moved2.x) > 4 || Math.abs(afterReload.y - moved2.y) > 4) {
    throw new Error(`位置没有持久化：${JSON.stringify({ moved2, afterReload })}`);
  }

  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.writeFileSync(logPath, log);
  console.log(log.trim());
  console.log("widget ball ok");
} catch (err) {
  failed = err;
  log += `error=${err.message}\n`;
  try {
    fs.mkdirSync(path.dirname(logPath), { recursive: true });
    fs.writeFileSync(logPath, log);
  } catch {}
  console.error(log.trim());
  process.exitCode = 1;
} finally {
  try { ws?.close(); } catch {}
  if (!child.killed) spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  await new Promise((resolve) => server.close(resolve));
  try { fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }); } catch {}
  if (failed) process.exitCode = 1;
}
