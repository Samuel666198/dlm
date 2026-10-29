// B3b-lite 真浏览器验收：页面请求 .m3u8 + 文件链接 → 扩展“下载”面板出现 → 分别点击 → 宿主落盘。
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
  '<a id="file-link" href="/file.bin">下载文件</a>' +
  '<a id="auto-file-link" href="/auto.bin">自动接管</a>' +
  '<a id="slow-file-link" href="/slow.bin">慢速文件</a>' +
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
  if (req.url === '/file.bin') {
    res.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Length': 4 });
    res.end('FILE');
    return;
  }
  if (req.url === '/auto.bin') {
    res.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Length': 4 });
    res.end('AUTO');
    return;
  }
  if (req.url === '/slow.bin') {
    const SIZE = 32 * 1024 * 1024;
    const range = req.headers.range;
    let start = 0, end = SIZE - 1, status = 200;
    const m = range && /^bytes=(\d*)-(\d*)$/.exec(range);
    if (m) {
      if (m[1]) start = Number(m[1]);
      if (m[2]) end = Number(m[2]);
      status = 206;
    }
    const len = end - start + 1;
    const headers = { 'Content-Type': 'application/octet-stream', 'Content-Length': String(len), 'Accept-Ranges': 'bytes' };
    if (status === 206) headers['Content-Range'] = `bytes ${start}-${end}/${SIZE}`;
    res.writeHead(status, headers);
    let pos = start;
    const chunk = 256 * 1024;
    const timer = setInterval(() => {
      if (pos > end) { clearInterval(timer); res.end(); return; }
      const n = Math.min(chunk, end - pos + 1);
      res.write(Buffer.alloc(n, 0x41));
      pos += n;
    }, 400);
    req.on('close', () => clearInterval(timer));
    return;
  }
  if (req.url === '/normal-page') {
    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    res.end('<!doctype html><title>normal</title>ok');
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
  await cdp('Page.enable');
  const evalText = async (expression) => {
    const r = await cdp('Runtime.evaluate', { expression, returnByValue: true });
    return r.result?.value;
  };

  const helper = "window.__dlm=(sel)=>document.getElementById('dlm-native-box')?.shadowRoot?.querySelector(sel)||null; true";
  await evalText(helper);
  let injected = false;
  for (let i = 0; i < 40; i++) {
    injected = !!(await evalText("!!window.__dlm('#dlm-submit')"));
    if (injected) break;
    await sleep(250);
  }
  if (!injected) {
    // 扩展在页面开始加载之后才完成注册时，content script 不会补注入；重载一次。
    await cdp('Page.reload', { ignoreCache: true });
    await sleep(2500);
    await evalText(helper);
    for (let i = 0; i < 40; i++) {
      injected = !!(await evalText("!!window.__dlm('#dlm-submit')"));
      if (injected) break;
      await sleep(250);
    }
  }
  if (!injected) throw new Error('扩展未注入');
  await sleep(2500); // 等宿主 pong + 页面链接扫描
  await evalText("document.querySelector('#load-media').click(); true");
  await sleep(600);

  let mediaText = '';
  for (let i = 0; i < 80; i++) {
    mediaText = (await evalText("window.__dlm('#dlm-pill-meta')?.textContent || ''")) || '';
    if (mediaText === '可下载 3') break;
    await sleep(250);
  }
  log += `media_button=${mediaText}\n`;
  if (!mediaText.startsWith('可下载')) throw new Error('没有检测到下载条目');

  await evalText("window.__dlm('#dlm-toggle').click(); true");
  const panel = await evalText("!window.__dlm('#dlm-panel')?.hidden");
  if (!panel) throw new Error('下载面板没有打开');
  const items = await evalText("Array.from(window.__dlm('#dlm-list').querySelectorAll('.url')).map(e=>e.title)");
  log += `media_items=${JSON.stringify(items)}\n`;
  if (!items || !items.some((u) => u.includes('/stream.m3u8'))) {
    throw new Error(`下载面板没有 HLS 条目：${JSON.stringify(items)}`);
  }
  if (!items.some((u) => u.includes('/file.bin'))) {
    throw new Error(`下载面板没有文件链接：${JSON.stringify(items)}`);
  }
  const clickedHls = await evalText(`(() => {
    const rows = [...window.__dlm('#dlm-list').children];
    const row = rows.find((r) => (r.querySelector('.url')?.title || '').includes('/stream.m3u8'));
    if (!row) return false;
    row.querySelector('button')?.click();
    return true;
  })()`);
  if (!clickedHls) throw new Error('下载面板里没有可点的 HLS 条目');

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

  const clickedFile = await evalText(`(() => {
    const rows = [...window.__dlm('#dlm-list').children];
    const row = rows.find((r) => (r.querySelector('.url')?.title || '').includes('/file.bin'));
    if (!row) return false;
    row.querySelector('button')?.click();
    return true;
  })()`);
  if (!clickedFile) throw new Error('下载面板里没有可点的 file.bin');
  const filePath = path.join(downloadDir, 'file.bin');
  let fileBytes = -1;
  for (let i = 0; i < 120; i++) {
    if (fs.existsSync(filePath)) {
      fileBytes = fs.statSync(filePath).size;
      if (fileBytes === 4) break;
    }
    await sleep(100);
  }
  log += `plain_file=${filePath} bytes=${fileBytes}\n`;
  if (fileBytes !== 4) throw new Error(`普通文件没有落成 FILE：${fileBytes}`);

  // 宿主就绪后，点击下载链接先弹确认，不直接下载。
  let hostReady = false;
  for (let i = 0; i < 40; i++) {
    hostReady = !!(await evalText("window.__dlm('#dlm-host-note')?.hidden"));
    if (hostReady) break;
    await sleep(250);
  }
  log += `host_ready=${hostReady}\n`;
  if (!hostReady) throw new Error('宿主未就绪');

  await evalText("document.getElementById('auto-file-link').click(); true");
  let confirmShown = false;
  for (let i = 0; i < 40; i++) {
    confirmShown = !!(await evalText("!window.__dlm('#dlm-confirm')?.hidden"));
    if (confirmShown) break;
    await sleep(100);
  }
  if (!confirmShown) throw new Error('点击下载链接没有弹确认');
  const autoPath = path.join(downloadDir, 'auto.bin');
  if (fs.existsSync(autoPath)) throw new Error('确认前就开始了下载');
  await evalText("window.__dlm('#dlm-confirm-yes').click(); true");
  let autoBytes = -1;
  for (let i = 0; i < 120; i++) {
    if (fs.existsSync(autoPath)) {
      autoBytes = fs.statSync(autoPath).size;
      if (autoBytes === 4) break;
    }
    await sleep(100);
  }
  const stayed = (await evalText("location.pathname")) === '/page.html';
  log += `auto_click_file=${autoPath} bytes=${autoBytes} stayed=${stayed}\n`;
  if (autoBytes !== 4) throw new Error(`确认后没有交给 dlm：${autoBytes}`);
  if (!stayed) throw new Error('确认后页面跳走了');
  if (fs.readFileSync(autoPath, 'utf8') !== 'AUTO') throw new Error('自动接管内容不对');

  // 宿主在子进程退出后要回 done，页面必须看得到完成/失败。
  let doneText = '';
  for (let i = 0; i < 100; i++) {
    doneText = (await evalText("window.__dlm('#dlm-finished')?.textContent || ''")) || '';
    if (doneText.includes('完成')) break;
    await sleep(100);
  }
  log += `done_text=${doneText}\n`;
  if (!doneText.includes('完成')) throw new Error(`没有收到完成反馈：${doneText}`);

  // 慢速文件：要能看到进度条，点“停止”后显示断点保留。
  await evalText("document.getElementById('slow-file-link').click(); true");
  let slowConfirm = false;
  for (let i = 0; i < 40; i++) {
    slowConfirm = !!(await evalText("!window.__dlm('#dlm-confirm')?.hidden"));
    if (slowConfirm) break;
    await sleep(100);
  }
  if (!slowConfirm) throw new Error('慢速文件没有弹确认');
  await evalText("window.__dlm('#dlm-confirm-yes').click(); true");
  let progress = -1;
  for (let i = 0; i < 120; i++) {
    progress = Number(await evalText("window.__dlm('#dlm-active .bar')?.getAttribute('aria-valuenow') || '-1'"));
    if (progress > 0) break;
    await sleep(100);
  }
  log += `progress=${progress}\n`;
  if (!(progress > 0)) throw new Error('没有收到进度');
  await evalText("window.__dlm('#dlm-active button.ghost')?.click(); true");
  let stoppedText = '';
  for (let i = 0; i < 60; i++) {
    stoppedText = (await evalText("window.__dlm('#dlm-finished')?.textContent || ''")) || '';
    if (stoppedText.includes('已停止')) break;
    await sleep(100);
  }
  log += `stopped_text=${stoppedText}\n`;
  if (!stoppedText.includes('已停止')) throw new Error(`停止后没有反馈：${stoppedText}`);

  // 普通页面链接不许被接管（对照臂：defaultPrevented 必须为 false）。
  const normalPrevented = await evalText(`(() => {
    let prevented = null;
    const a = document.createElement('a');
    a.href = '/normal-page';
    document.body.appendChild(a);
    a.addEventListener('click', (e) => { prevented = e.defaultPrevented; e.preventDefault(); }, { once: true });
    a.click();
    a.remove();
    return prevented;
  })()`);
  log += `normal_link_prevented=${normalPrevented}\n`;
  if (normalPrevented) throw new Error('普通链接被误拦截');

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
