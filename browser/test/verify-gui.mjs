// C2/C3 真浏览器验收：启动 dlm-gui，打开本地 UI 页面，填 URL/文件名，点添加，
// 等任务 done，截图。下载目标是一个本地 64 KiB 文件。
//
// 用法：node browser/test/verify-gui.mjs
// 前置：cargo build --bin dlm --bin dlm-gui

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
const dlmGui = path.resolve(arg('gui', path.join(root, 'target', 'debug', 'dlm-gui.exe')));
const dlmExe = path.resolve(arg('dlm', path.join(root, 'target', 'debug', 'dlm.exe')));
const out = path.resolve(arg('out', path.join(root, 'docs', 'evidence', '2026-10-22-c3-gui.png')));
const logPath = path.resolve(arg('log', path.join(root, 'docs', 'evidence', '2026-10-22-c3-gui.log')));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!fs.existsSync(dlmGui)) throw new Error(`missing dlm-gui.exe: ${dlmGui}`);
if (!fs.existsSync(dlmExe)) throw new Error(`missing dlm.exe: ${dlmExe}`);

async function freePort() {
  const srv = net.createServer();
  await new Promise((resolve) => srv.listen(0, '127.0.0.1', resolve));
  const port = srv.address().port;
  await new Promise((resolve) => srv.close(resolve));
  return port;
}

const payload = Buffer.alloc(65536, 7);
const fileServer = http.createServer((_req, res) => {
  res.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Length': payload.length });
  res.end(payload);
});
await new Promise((resolve) => fileServer.listen(0, '127.0.0.1', resolve));
const filePort = fileServer.address().port;
const fileUrl = `http://127.0.0.1:${filePort}/gui.bin`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'dlm-c3-'));
const queue = path.join(tmp, 'queue.json');
const downloadDir = path.join(tmp, 'dl');
const logDir = path.join(tmp, 'logs');
const endpoint = path.join(tmp, 'endpoint.json');
const profile = path.join(tmp, 'profile');
fs.mkdirSync(profile, { recursive: true });
const guiPort = await freePort();
const token = 'gui-test-token';
const guiUrl = `http://127.0.0.1:${guiPort}/?t=${token}`;

const gui = spawn(
  dlmGui,
  [
    '--port', String(guiPort),
    '--token', token,
    '--queue', queue,
    '--out-dir', downloadDir,
    '--log-dir', logDir,
    '--endpoint', endpoint,
    '--dlm', dlmExe,
    '--concurrency', '2',
  ],
  { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true }
);
let guiStderr = '';
gui.stderr.on('data', (d) => (guiStderr += d));

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
    '--window-size=1000,700',
    guiUrl,
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
      target = list.find((x) => x.type === 'page' && x.url.startsWith(`http://127.0.0.1:${guiPort}/`));
      if (target) break;
    } catch {}
    await sleep(200);
  }
  if (!target) throw new Error('CDP page target not found（浏览器/GUI 没起来）');
  const version = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
  log += `browser=${version.Browser}\ngui=${guiUrl}\nfile_url=${fileUrl}\n`;

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
    // `awaitPromise`：返回 Promise 的表达式（本文件里 P4/P2 那两段 async IIFE）
    // 不加这个参数会拿到一个空对象，断言就变成"什么都没验到却过了"。
    const r = await cdp('Runtime.evaluate', {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    return r.result?.value;
  };

  // 页面加载与表单出现
  for (let i = 0; i < 80; i++) {
    if (await evalText("!!document.querySelector('#add button[type=submit]')")) break;
    await sleep(250);
  }
  if (!(await evalText("!!document.querySelector('#add button[type=submit]')"))) {
    throw new Error('GUI 页面没加载出添加表单');
  }

  await evalText(`(() => {
    document.querySelector('#url').value = ${JSON.stringify(fileUrl)};
    document.querySelector('#name').value = 'gui.bin';
    document.querySelector('#add button[type=submit]').click();
    return true;
  })()`);

  let text = '';
  for (let i = 0; i < 100; i++) {
    text = (await evalText("Array.from(document.querySelectorAll('.task')).map(e => e.innerText).join('\\n')")) || '';
    if (text.includes('gui.bin') && text.includes('已完成')) break;
    await sleep(250);
  }
  log += `tasks=${JSON.stringify(text)}\n`;
  if (!(text.includes('gui.bin') && text.includes('已完成'))) {
    throw new Error(`任务没到已完成：${text || '(no task rows)'}`);
  }

  const file = path.join(downloadDir, 'gui.bin');
  const bytes = fs.existsSync(file) ? fs.statSync(file).size : -1;
  log += `file=${file} bytes=${bytes}\n`;
  if (bytes !== 65536) throw new Error(`落盘文件大小不对：${bytes}`);

  // ---- P4：成功行不显示 exit 码；Esc 关弹窗且焦点回到触发按钮 -----------------
  const doneText = await evalText("Array.from(document.querySelectorAll('.task')).map(e => e.innerText).join('\\n')");
  log += `p4_done_text=${JSON.stringify(doneText)}\n`;
  if (doneText.includes('exit 0')) throw new Error(`成功行不该再显示 exit 码：${doneText}`);

  // openModal 用 setTimeout(30) 把焦点移进弹窗，所以要等它跑完再断言。
  const focusBack = await evalText(`(async () => {
    const btn = document.querySelector('#addBtn');
    btn.focus();
    btn.click();
    await new Promise((r) => setTimeout(r, 150));
    const inModal = document.activeElement && document.activeElement.closest('#addModal') !== null;
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await new Promise((r) => setTimeout(r, 80));
    return {
      inModal,
      closed: !document.querySelector('#addModal').classList.contains('open'),
      back: document.activeElement === btn,
    };
  })()`);
  log += `p4_modal=${JSON.stringify(focusBack)}\n`;
  if (!focusBack.inModal) throw new Error('打开弹窗后焦点没有进入弹窗');
  if (!focusBack.closed) throw new Error('Esc 没有关掉弹窗');
  if (!focusBack.back) throw new Error('关闭弹窗后焦点没有回到触发按钮');

  // ---- P2：/api/reveal 只收 task id ------------------------------------------
  const revealUnknown = await evalText(`(async () => {
    const r = await fetch('/api/reveal?t=' + encodeURIComponent(${JSON.stringify(token)}), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: 'no-such-task', mode: 'open' }),
    });
    return await r.json();
  })()`);
  log += `p2_reveal_unknown=${JSON.stringify(revealUnknown)}\n`;
  if (revealUnknown.ok !== false) {
    throw new Error(`未知 id 的 reveal 必须拒绝：${JSON.stringify(revealUnknown)}`);
  }

  const revealDone = await evalText(`(async () => {
    const task = document.querySelector('.task');
    const r = await fetch('/api/reveal?t=' + encodeURIComponent(${JSON.stringify(token)}), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: task.dataset.id, mode: 'select', path: 'C:\\\\Windows\\\\System32\\\\calc.exe' }),
    });
    return await r.json();
  })()`);
  log += `p2_reveal_done=${JSON.stringify(revealDone)}\n`;
  if (revealDone.ok !== true) {
    throw new Error(`已完成任务的 reveal 应当成功：${JSON.stringify(revealDone)}`);
  }
  if (String(revealDone.path || '').toLowerCase().includes('calc.exe')) {
    throw new Error(`客户端交来的 path 被采纳了：${revealDone.path}`);
  }
  if (!String(revealDone.path || '').endsWith('gui.bin')) {
    throw new Error(`落点必须来自队列任务名：${revealDone.path}`);
  }
  if (!(revealDone.args || []).some((a) => String(a).startsWith('/select,'))) {
    throw new Error(`/select 必须是一个 argv：${JSON.stringify(revealDone.args)}`);
  }

  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
  log += `screenshot=${out}\n`;
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.writeFileSync(logPath, log);
  console.log(log.trim());
  console.log('gui browser ok');
} catch (err) {
  failed = err;
  log += `error=${err.message}\n`;
  if (browserStderr) log += `browser_stderr=${browserStderr.slice(-3000)}\n`;
  if (guiStderr) log += `gui_stderr=${guiStderr.slice(-3000)}\n`;
  try {
    fs.mkdirSync(path.dirname(logPath), { recursive: true });
    fs.writeFileSync(logPath, log);
  } catch {}
  console.error(log.trim());
  process.exitCode = 1;
} finally {
  try { ws?.close(); } catch {}
  if (!child.killed) spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
  spawnSync('taskkill', ['/PID', String(gui.pid), '/T', '/F'], { stdio: 'ignore' });
  await new Promise((resolve) => fileServer.close(resolve));
  try { fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }); } catch {}
  if (failed) process.exitCode = 1;
}
