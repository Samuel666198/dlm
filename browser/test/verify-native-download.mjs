// B2 真浏览器投递验收：扩展把当前页的 url/referer/cookie 交给宿主，宿主 spawn dlm，
// 页面落盘到临时目录。断言：文件内容等于页面 HTML；服务端收到 Cookie: sid=42 与 Referer。
//
// 用法：node browser/test/verify-native-download.mjs [--browser edge.exe]
// 前置：cargo build --bin dlm --bin dlm-native-host && powershell -File browser/native-host/install.ps1

import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
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
const out = path.resolve(arg('out', path.join(root, 'docs', 'evidence', '2026-10-22-b2-browser-download.png')));
const logPath = path.resolve(arg('log', path.join(root, 'docs', 'evidence', '2026-10-22-b2-browser-download.log')));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!fs.existsSync(dlmExe)) throw new Error(`missing dlm.exe: ${dlmExe}（先 cargo build --bin dlm）`);

const pageHtml =
  '<!doctype html><html><head><meta charset="utf-8"><title>dlm B2</title></head>' +
  '<body><h1>dlm B2 native download</h1><p>Click the dlm button.</p></body></html>';

const requests = [];
const server = http.createServer((req, res) => {
  if (req.url === '/__requests') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(requests));
    return;
  }
  requests.push({
    url: req.url,
    cookie: req.headers.cookie || '',
    referer: req.headers.referer || '',
    ua: req.headers['user-agent'] || '',
  });
  if (req.url === '/b2file.html') {
    res.writeHead(200, {
      'Content-Type': 'text/html; charset=utf-8',
      'Content-Length': Buffer.byteLength(pageHtml),
      'Set-Cookie': 'sid=42; Path=/',
    });
    res.end(pageHtml);
    return;
  }
  res.writeHead(404, { 'Content-Length': 0 });
  res.end();
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const pagePort = server.address().port;
const pageUrl = `http://127.0.0.1:${pagePort}/b2file.html`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'dlm-b2-'));
const downloadDir = path.join(tmp, 'dl');
const logDir = path.join(tmp, 'logs');
const hostConfig = path.join(tmp, 'host-config.json');
fs.writeFileSync(
  hostConfig,
  JSON.stringify({ dlm_exe: dlmExe, download_dir: downloadDir, log_dir: logDir })
);
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
  if (!target) throw new Error('CDP page target not found（浏览器可能没起来/--load-extension 被忽略）');
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

  // 等扩展注入按钮；按钮同时说明 content script/service worker 已加载。
  for (let i = 0; i < 80; i++) {
    const has = await evalText("!!document.querySelector('#dlm-submit')");
    if (has) break;
    await sleep(250);
  }
  if (!(await evalText("!!document.querySelector('#dlm-submit')"))) {
    throw new Error('扩展未注入 #dlm-submit（扩展没加载/权限被拒）');
  }

  await evalText("document.querySelector('#dlm-submit').click(); true");

  const file = path.join(downloadDir, 'b2file.html');
  let fileBody = '';
  let fileOk = false;
  let reqOk = false;
  let status = '';
  for (let i = 0; i < 100; i++) {
    if (fs.existsSync(file)) {
      fileBody = fs.readFileSync(file, 'utf8');
      fileOk = fileBody === pageHtml;
    }
    const reqs = await (await fetch(`http://127.0.0.1:${pagePort}/__requests`)).json();
    // 只认 dlm 自己那一枪：同页、带 sid=42、Referer 指向页面、且 UA 为空（dlm 不伪装 UA）。
    // 不这样收紧的话，浏览器自己的 /favicon.ico 也带 Cookie/Referer，会假绿。
    reqOk = reqs.some(
      (r) =>
        r.url === '/b2file.html' &&
        (r.cookie || '').includes('sid=42') &&
        (r.referer || '').startsWith(pageUrl) &&
        (r.ua || '') === ''
    );
    status = (await evalText("document.querySelector('#dlm-native-status')?.textContent || ''")) || '';
    if (fileOk && reqOk) break;
    await sleep(250);
  }

  log += `status=${status}\n`;
  log += `file=${file} bytes=${fileBody.length} equals_page=${fileOk}\n`;
  log += `file_sha256=${crypto.createHash('sha256').update(fileBody).digest('hex')}\n`;
  const reqs = await (await fetch(`http://127.0.0.1:${pagePort}/__requests`)).json();
  log += `requests=${JSON.stringify(reqs)}\n`;
  if (!fileOk) throw new Error(`落盘文件不匹配：${file}（存在=${fs.existsSync(file)}）`);
  if (!reqOk) throw new Error('服务端没看到带 Cookie: sid=42 且 Referer 指向页面 URL 的 dlm 请求');

  // 把 dlm 的任务日志也带进证据（不含 Cookie）。
  try {
    const jobs = fs.readdirSync(logDir);
    if (jobs.length) {
      const jobText = fs.readFileSync(path.join(logDir, jobs[0]), 'utf8');
      log += `job_log=${jobText.slice(-4000)}\n`;
    }
  } catch {}

  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
  log += `screenshot=${out}\n`;
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.writeFileSync(logPath, log);
  console.log(log.trim());
  console.log('native browser download ok');
} catch (err) {
  failed = err;
  log += `error=${err.message}\n`;
  if (browserStderr) log += `browser_stderr=${browserStderr.slice(-4000)}\n`;
  try {
    const reqs = await (await fetch(`http://127.0.0.1:${pagePort}/__requests`)).json();
    log += `requests=${JSON.stringify(reqs)}\n`;
  } catch {}
  try {
    const jobs = fs.readdirSync(logDir);
    for (const j of jobs) log += `job_log_${j}=${fs.readFileSync(path.join(logDir, j), 'utf8').slice(-4000)}\n`;
  } catch {}
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
