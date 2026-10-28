// B10 真浏览器验收：无 Cookie 的页面投递在 dlm-gui 开着时进队列，GUI 关掉后回退 B2 直连。
//
// 用法：node browser/test/verify-queue-download.mjs [--browser edge.exe]
// 前置：cargo build --bin dlm --bin dlm-gui --bin dlm-native-host && install.ps1

import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const argv = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
};
const chrome = 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe';
const edge = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const browser = path.resolve(arg('browser', fs.existsSync(edge) ? edge : chrome));
const ext = path.resolve(arg('extension', path.join(root, 'browser', 'extension')));
const dlmExe = path.resolve(arg('dlm', path.join(root, 'target', 'debug', 'dlm.exe')));
const guiExe = path.resolve(arg('gui', path.join(root, 'target', 'debug', 'dlm-gui.exe')));
const out = path.resolve(arg('out', path.join(root, 'docs', 'evidence', '2026-10-27-quota-queue-b10.png')));
const queueOutPng = out.replace(/\.png$/, '-queue.png');
const logPath = path.resolve(arg('log', path.join(root, 'docs', 'evidence', '2026-10-27-quota-queue-b10-browser.log')));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
for (const p of [dlmExe, guiExe]) if (!fs.existsSync(p)) throw new Error(`missing ${p}`);

// 页面本身就是要下载的文件（页面按钮投递的是 location.href）；服务成 HTML，扩展才能注入。
const prefix = '<!doctype html><html><head><meta charset="utf-8"><title>dlm B10</title></head>' +
  '<body><h1>dlm B10 queue test</h1><p>Click dlm.</p>';
const suffix = '</body></html>';
const pageHtml = prefix + 'x'.repeat(65536 - Buffer.byteLength(prefix) - Buffer.byteLength(suffix)) + suffix;
if (Buffer.byteLength(pageHtml) !== 65536) throw new Error('page payload must be exactly 65536 bytes');
const server = http.createServer((req, res) => {
  if (req.url === '/file.bin') {
    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Content-Length': Buffer.byteLength(pageHtml) });
    res.end(pageHtml);
    return;
  }
  res.writeHead(404, { 'Content-Length': 0 });
  res.end();
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const pagePort = server.address().port;
const pageUrl = `http://127.0.0.1:${pagePort}/file.bin`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'dlm-b10-'));
const queue = path.join(tmp, 'queue.json');
const queueOut = path.join(tmp, 'queue-out');
const guiLogs = path.join(tmp, 'gui-logs');
const endpoint = path.join(tmp, 'endpoint.json');
const directDir = path.join(tmp, 'direct');
const hostLogs = path.join(tmp, 'host-logs');
const hostConfig = path.join(tmp, 'host-config.json');
fs.writeFileSync(
  hostConfig,
  JSON.stringify({
    dlm_exe: dlmExe,
    download_dir: directDir,
    log_dir: hostLogs,
    gui_endpoint: endpoint,
  })
);

async function freePort() {
  const srv = net.createServer();
  await new Promise((resolve) => srv.listen(0, '127.0.0.1', resolve));
  const port = srv.address().port;
  await new Promise((resolve) => srv.close(resolve));
  return port;
}
const guiPort = await freePort();
const token = 'b10-token';
const guiBase = `http://127.0.0.1:${guiPort}`;
const gui = spawn(
  guiExe,
  [
    '--port', String(guiPort),
    '--token', token,
    '--queue', queue,
    '--out-dir', queueOut,
    '--log-dir', guiLogs,
    '--endpoint', endpoint,
    '--dlm', dlmExe,
    '--concurrency', '1',
  ],
  { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true }
);
let guiStderr = '';
gui.stderr.on('data', (d) => (guiStderr += d));
const guiDeadline = Date.now() + 5000;
while (Date.now() < guiDeadline) {
  try {
    const r = await fetch(`${guiBase}/api/tasks?t=${token}`);
    if (r.ok) break;
  } catch {}
  await sleep(100);
}

const profile = path.join(tmp, 'profile');
fs.mkdirSync(profile, { recursive: true });
const debugPort = await freePort();
const child = spawn(
  browser,
  [
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${debugPort}`,
    '--remote-allow-origins=*',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-search-engine-choice-screen',
    '--disable-features=DisableLoadExtensionCommandLineSwitch',
    `--disable-extensions-except=${ext}`,
    `--load-extension=${ext}`,
    '--window-size=1000,700',
    pageUrl,
  ],
  { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true, env: { ...process.env, DLM_NATIVE_CONFIG: hostConfig } }
);
let browserStderr = '';
child.stderr.on('data', (d) => (browserStderr += d));
child.stdout.on('data', (d) => (browserStderr += d));

let ws;
let log = '';
let failed = null;
try {
  const deadline = Date.now() + 20000;
  let target = null;
  while (Date.now() < deadline) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
      target = list.find((x) => x.type === 'page' && x.url.startsWith(pageUrl));
      if (target) break;
    } catch {}
    await sleep(200);
  }
  if (!target) throw new Error('CDP page target not found（浏览器没起来/--load-extension 被忽略）');
  const version = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
  log += `browser=${version.Browser}\npage=${pageUrl}\nqueue_out=${queueOut}\ndirect=${directDir}\n`;

  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  let seq = 0;
  const pending = new Map();
  ws.addEventListener('message', (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(JSON.stringify(msg.error)));
      else resolve(msg.result);
    }
  });
  const cdp = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++seq;
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  await cdp('Runtime.enable');
  const evalText = async (expression) => {
    const r = await cdp('Runtime.evaluate', { expression, returnByValue: true });
    return r.result?.value;
  };
  for (let i = 0; i < 80; i++) {
    if (await evalText("!!document.querySelector('#dlm-submit')")) break;
    await sleep(250);
  }
  if (!(await evalText("!!document.querySelector('#dlm-submit')"))) {
    throw new Error('扩展未注入 #dlm-submit');
  }

  // 第一发：GUI 开着、页面无 Cookie ⇒ 队列。
  await evalText("document.querySelector('#dlm-submit').click(); true");
  let status = '';
  for (let i = 0; i < 80; i++) {
    status = (await evalText("document.querySelector('#dlm-native-status')?.textContent || ''")) || '';
    if (status.startsWith('dlm queue: ')) break;
    await sleep(250);
  }
  log += `queue_status=${status}\n`;
  if (!status.startsWith('dlm queue: ')) throw new Error(`投递没有进队列：${status}`);

  let queuedOk = false;
  let queueText = '';
  for (let i = 0; i < 120; i++) {
    const q = await (await fetch(`${guiBase}/api/tasks?t=${token}`)).json();
    if (q.length && q[0].status === 'done') {
      queuedOk = true;
      queueText = JSON.stringify(q);
      break;
    }
    await sleep(100);
  }
  const queuedFile = path.join(queueOut, 'file.bin');
  const queuedBytes = fs.existsSync(queuedFile) ? fs.statSync(queuedFile).size : -1;
  log += `queue_task=${queueText}\nqueue_file=${queuedFile} bytes=${queuedBytes}\n`;
  if (!queuedOk || queuedBytes !== 65536) throw new Error(`队列没有下完：${queueText}`);
  const qshot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.mkdirSync(path.dirname(queueOutPng), { recursive: true });
  fs.writeFileSync(queueOutPng, Buffer.from(qshot.data, 'base64'));
  log += `queue_screenshot=${queueOutPng}\n`;

  // 第二发：杀掉 GUI（端点文件留着），无 Cookie ⇒ 回退直连。
  gui.kill();
  await sleep(800);
  await evalText("document.querySelector('#dlm-submit').click(); true");
  const directFile = path.join(directDir, 'file.bin');
  let directOk = false;
  let directStatus = '';
  for (let i = 0; i < 120; i++) {
    directStatus = (await evalText("document.querySelector('#dlm-native-status')?.textContent || ''")) || '';
    if (fs.existsSync(directFile) && fs.statSync(directFile).size === 65536) {
      directOk = true;
      break;
    }
    await sleep(100);
  }
  log += `direct_status=${directStatus}\ndirect_file=${directFile} ok=${directOk}\n`;
  if (!directOk) throw new Error(`GUI 关掉后没有回退直连：${directStatus}`);

  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
  log += `screenshot=${out}\n`;
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.writeFileSync(logPath, log);
  console.log(log.trim());
  console.log('B10 browser queue/direct ok');
} catch (err) {
  failed = err;
  log += `error=${err.message}\n`;
  if (browserStderr) log += `browser_stderr=${browserStderr.slice(-4000)}\n`;
  if (guiStderr) log += `gui_stderr=${guiStderr.slice(-2000)}\n`;
  try {
    fs.mkdirSync(path.dirname(logPath), { recursive: true });
    fs.writeFileSync(logPath, log);
  } catch {}
  console.error(log.trim());
  process.exitCode = 1;
} finally {
  try { ws?.close(); } catch {}
  if (!child.killed) spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
  if (!gui.killed) spawnSync('taskkill', ['/PID', String(gui.pid), '/T', '/F'], { stdio: 'ignore' });
  await new Promise((resolve) => server.close(resolve));
  try { fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }); } catch {}
  if (failed) process.exitCode = 1;
}
