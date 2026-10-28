// B3b-lite 真浏览器验收：页面请求 .m3u8 → 扩展媒体面板出现 → 点击下载 → 宿主直连 dlm --hls。
//
// 用法：node browser/test/verify-media-sniff.mjs [--browser edge.exe]
// 前置：cargo build --bin dlm --bin dlm-native-host && install.ps1

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
const edge = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const chrome = 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe';
const browser = path.resolve(arg('browser', fs.existsSync(edge) ? edge : chrome));
const ext = path.resolve(arg('extension', path.join(root, 'browser', 'extension')));
const dlmExe = path.resolve(arg('dlm', path.join(root, 'target', 'debug', 'dlm.exe')));
const out = path.resolve(arg('out', path.join(root, 'docs', 'evidence', '2026-10-28-media-sniff.png')));
const logPath = path.resolve(arg('log', path.join(root, 'docs', 'evidence', '2026-10-28-media-sniff.log')));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!fs.existsSync(dlmExe)) throw new Error(`missing dlm.exe: ${dlmExe}`);

const pageHtml = '<!doctype html><html><head><meta charset="utf-8"><title>dlm media</title></head>' +
  '<body><h1>dlm media sniff</h1><p>点击按钮请求一个 HLS 播放列表。</p>' +
  '<button id="load-media">加载媒体</button>' +
  '<script>document.getElementById("load-media").onclick = () => fetch("/stream.m3u8").catch(() => {});</script></body></html>';
const playlist = '#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nseg1.ts\n#EXTINF:1,\nseg2.ts\n#EXT-X-ENDLIST\n';
const server = http.createServer((req, res) => {
  if (req.url === '/page.html') {
    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Content-Length': Buffer.byteLength(pageHtml) });
    res.end(pageHtml);
    return;
  }
  if (req.url === '/stream.m3u8') {
    res.writeHead(200, { 'Content-Type': 'application/vnd.apple.mpegurl', 'Content-Length': Buffer.byteLength(playlist) });
    res.end(playlist);
    return;
  }
  if (req.url === '/seg1.ts') {
    res.writeHead(200, { 'Content-Type': 'video/mp2t', 'Content-Length': 3 });
    res.end('AAA');
    return;
  }
  if (req.url === '/seg2.ts') {
    res.writeHead(200, { 'Content-Type': 'video/mp2t', 'Content-Length': 3 });
    res.end('BBB');
    return;
  }
  res.writeHead(404, { 'Content-Length': 0 });
  res.end();
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const pagePort = server.address().port;
const pageUrl = `http://127.0.0.1:${pagePort}/page.html`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'dlm-media-'));
const downloadDir = path.join(tmp, 'dl');
const logDir = path.join(tmp, 'logs');
const hostConfig = path.join(tmp, 'host-config.json');
fs.writeFileSync(hostConfig, JSON.stringify({
  dlm_exe: dlmExe,
  download_dir: downloadDir,
  log_dir: logDir,
  gui_endpoint: path.join(tmp, 'no-gui.json')
}));
const profile = path.join(tmp, 'profile');
fs.mkdirSync(profile, { recursive: true });

async function freePort() {
  const srv = net.createServer();
  await new Promise((resolve) => srv.listen(0, '127.0.0.1', resolve));
  const port = srv.address().port;
  await new Promise((resolve) => srv.close(resolve));
  return port;
}
const debugPort = await freePort();
const child = spawn(browser, [
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
], { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true, env: { ...process.env, DLM_NATIVE_CONFIG: hostConfig } });
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
  if (!target) throw new Error('CDP page target not found');
  const version = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
  log += `browser=${version.Browser}\npage=${pageUrl}\ndlm=${dlmExe}\n`;

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
  const cdp = (method, params = {}) => new Promise((resolve, reject) => {
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
  if (!(await evalText("!!document.querySelector('#dlm-submit')"))) throw new Error('扩展未注入');
  await sleep(1500); // 给 service worker 注册 webRequest 的时间；真实扩展在浏览前就加载
  await evalText("document.querySelector('#load-media').click(); true");
  await sleep(600);

  let mediaText = '';
  for (let i = 0; i < 80; i++) {
    mediaText = (await evalText("document.querySelector('#dlm-media-count')?.textContent || ''")) || '';
    if (mediaText.startsWith('媒体 1') || mediaText.startsWith('媒体 2')) break;
    await sleep(250);
  }
  log += `media_button=${mediaText}\n`;
  if (!mediaText.startsWith('媒体 ')) throw new Error('没有检测到媒体');
  if (mediaText === '媒体 0') throw new Error('媒体计数为 0');

  await evalText("document.querySelector('#dlm-media-count').click(); true");
  const panel = await evalText("(!document.querySelector('#dlm-media-panel')?.hidden)");
  if (!panel) throw new Error('媒体面板没有打开');
  const items = await evalText("Array.from(document.querySelectorAll('#dlm-media-list .url')).map(e=>e.textContent)");
  log += `media_items=${JSON.stringify(items)}\n`;
  if (!items || !items.some((u) => u.includes('/stream.m3u8'))) {
    throw new Error(`媒体面板没有 HLS 条目：${JSON.stringify(items)}`);
  }
  await evalText("Array.from(document.querySelectorAll('#dlm-media-list button')).find((b)=>b.textContent==='下载').click(); true");

  const file = path.join(downloadDir, 'stream.ts');
  let bytes = -1;
  for (let i = 0; i < 120; i++) {
    if (fs.existsSync(file)) {
      bytes = fs.statSync(file).size;
      if (bytes === 6) break;
    }
    await sleep(100);
  }
  log += `file=${file} bytes=${bytes}\n`;
  if (bytes !== 6) throw new Error(`HLS 没有落成 AAABBB：${bytes}`);
  const body = fs.readFileSync(file, 'utf8');
  if (body !== 'AAABBB') throw new Error(`HLS 内容不对：${JSON.stringify(body)}`);

  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
  log += `screenshot=${out}\n`;
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.writeFileSync(logPath, log);
  console.log(log.trim());
  console.log('media sniff ok');
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
  if (!child.killed) spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
  await new Promise((resolve) => server.close(resolve));
  try { fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }); } catch {}
  if (failed) process.exitCode = 1;
}
