// 直接对真宿主进程做 Native Messaging 帧往返（不经过浏览器）。
// 用法：node browser/test/native-ping.mjs [host.exe]

import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const exe = process.argv[2] ? path.resolve(process.argv[2]) : path.join(root, 'target', 'debug', 'dlm-native-host.exe');

function frame(obj) {
  const body = Buffer.from(JSON.stringify(obj), 'utf8');
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length, 0);
  return Buffer.concat([head, body]);
}

const child = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
let stderr = '';
child.stderr.on('data', (d) => (stderr += d));

let acc = Buffer.alloc(0);
const waiters = [];
const queued = [];
function pump() {
  while (acc.length >= 4) {
    const len = acc.readUInt32LE(0);
    if (acc.length < 4 + len) break;
    const payload = acc.subarray(4, 4 + len);
    acc = acc.subarray(4 + len);
    const msg = JSON.parse(payload.toString('utf8'));
    const waiter = waiters.shift();
    if (waiter) waiter.resolve(msg);
    else queued.push(msg);
  }
}
child.stdout.on('data', (d) => {
  acc = Buffer.concat([acc, d]);
  pump();
});
function next() {
  if (queued.length) return Promise.resolve(queued.shift());
  return new Promise((resolve, reject) => waiters.push({ resolve, reject }));
}

try {
  child.stdin.write(frame({ op: 'ping' }));
  const pong = await next();
  if (pong.op !== 'pong' || pong.host !== 'com.dlm.host') throw new Error(`bad pong: ${JSON.stringify(pong)}`);

  child.stdin.write(frame({ op: 'wat' }));
  const unknown = await next();
  if (unknown.op !== 'error') throw new Error(`bad unknown reply: ${JSON.stringify(unknown)}`);

  child.stdin.end();
  await new Promise((resolve) => child.once('exit', resolve));
  console.log('native-ping ok:', JSON.stringify(pong), JSON.stringify(unknown));
} catch (err) {
  console.error('native-ping failed:', err.message);
  if (stderr) console.error('host stderr:', stderr);
  child.kill();
  process.exit(1);
}
