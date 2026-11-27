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

if (failures.length) {
  console.error("extension static check FAILED");
  for (const f of failures) console.error(`- ${f}`);
  process.exit(1);
}
console.log(
  `extension static check ok: version=${manifest.version}, sw=${sw}, suffix-regex 双份一致, direct-reason ${directTokens.length} 个对齐`,
);
