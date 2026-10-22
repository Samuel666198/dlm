// B1 真浏览器往返：启动临时 profile 的 Chrome/Edge + 未打包扩展，打开本地页面，
// 读扩展 content script 写下的 #dlm-native-status，成功时截图。
//
// 用法：
//   node browser/test/verify-native-ping.mjs
//   node browser/test/verify-native-ping.mjs --browser "C:\...\msedge.exe"
// 前置：cargo build --bin dlm-native-host && powershell -File browser/native-host/install.ps1

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
const browser = path.resolve(arg('browser', fs.existsSync(chrome) ? chrome : edge));
const ext = path.resolve(arg('extension', path.join(root, 'browser', 'extension')));
const out = path.resolve(arg('out', path.join(root, 'docs', 'evidence', '2026-10-22-b1-native-ping.png')));
const logPath = path.resolve(arg('log', path.join(root, 'docs', 'evidence', '2026-10-22-b1-native-ping.log')));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function freePort() {
  const srv = net.createServer();
  await new Promise((resolve) => srv.listen(0, '127.0.0.1', resolve));
  const port = srv.address().port;
  await new Promise((resolve) => srv.close(resolve));
  return port;
}

const html = `<!doctype html><html><head><meta charset="utf-8"><title>dlm B1</title></head><body><h1>dlm B1 native ping</h1><p>status is rendered by the extension.</p></body></html>`;
const server = http.createServer((_req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
  res.end(html);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const pagePort = server.address().port;
const pageUrl = `http://127.0.0.1:${pagePort}/`;

const debugPort = await freePort();
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'dlm-b1-'));
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
  { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true }
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
  if (!target) throw new Error('CDP page target not found（浏览器可能没起来/--load-extension 被忽略）');

  const version = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
  log += `browser=${version.Browser}\npage=${pageUrl}\n`;

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
  await cdp('Page.enable');
  const evalText = async (expression) => {
    const r = await cdp('Runtime.evaluate', { expression, returnByValue: true });
    return r.result?.value;
  };

  let status = '';
  const isPong = (text) => text.includes('dlm native: pong ');
  for (let i = 0; i < 80; i++) {
    status = (await evalText("document.querySelector('#dlm-native-status')?.textContent || ''")) || '';
    if (isPong(status)) break;
    if (status && i % 8 === 0) console.log('status:', status);
    await sleep(250);
  }
  log += `status=${status}\n`;
  if (!isPong(status)) {
    const body = await evalText('document.body?.innerText || ""');
    log += `body=${body}\n`;
    throw new Error(`status 没有 pong：${status || '(missing #dlm-native-status)'}`);
  }

  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
  log += `screenshot=${out}\n`;
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.writeFileSync(logPath, log);
  console.log(log.trim());
  console.log('native browser ping ok');
} catch (err) {
  failed = err;
  log += `error=${err.message}\n`;
  if (browserStderr) log += `browser_stderr=${browserStderr.slice(-4000)}\n`;
  try {
    fs.mkdirSync(path.dirname(logPath), { recursive: true });
    fs.writeFileSync(logPath, log);
  } catch {}
  console.error(log.trim());
  process.exitCode = 1;
} finally {
  try { ws?.close(); } catch {}
  if (!child.killed) {
    spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
  }
  await new Promise((resolve) => server.close(resolve));
  try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }); } catch {}
  if (failed) process.exitCode = 1;
}
