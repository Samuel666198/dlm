// 扩展静态一致性检查：不启动浏览器，只读文件。
// 1) manifest 指向的 background 文件必须存在，且版本号与文件名一致（扩展用带版本号的 SW 文件名来绕缓存）；
// 2) content.js 与 page-hook.js 各有一份文件后缀正则（MAIN world 不能共享模块），两份必须逐字相同。
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

if (failures.length) {
  console.error("extension static check FAILED");
  for (const f of failures) console.error(`- ${f}`);
  process.exit(1);
}
console.log(`extension static check ok: version=${manifest.version}, sw=${sw}, suffix-regex 双份一致`);
