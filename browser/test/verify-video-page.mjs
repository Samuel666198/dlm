// `isVideoPage` 的行为检查：把 background 里那一段抠出来，在 vm 里跑。
//
// 为什么不启动浏览器：这条逻辑只依赖字符串，起一个 Chrome 只会让结论更慢更脆。
// 与 verify-context-target.mjs 同一个套路。
//
// 用法：node browser/test/verify-video-page.mjs
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const ext = join(here, "..", "extension");
const manifest = JSON.parse(readFileSync(join(ext, "manifest.json"), "utf8"));
const bg = readFileSync(join(ext, manifest.background.service_worker), "utf8");

// 抠出三样东西：站点表、直链扩展名表、函数本体。
const hosts = /const VIDEO_PAGE_HOSTS = \[([\s\S]*?)\];/.exec(bg);
const exts = /const DIRECT_VIDEO_EXT = \[([\s\S]*?)\];/.exec(bg);
const fn = /function isVideoPage\(url\) \{([\s\S]*?)\n\}/.exec(bg);
if (!hosts || !exts || !fn) {
  console.error("verify-video-page FAILED：从 background 抠不出 isVideoPage 这一段");
  process.exit(1);
}

const sandbox = {};
vm.createContext(sandbox);
vm.runInContext(
  `const VIDEO_PAGE_HOSTS = [${hosts[1]}];
   const DIRECT_VIDEO_EXT = [${exts[1]}];
   function isVideoPage(url) {${fn[1]}
   }
   globalThis.isVideoPage = isVideoPage;`,
  sandbox,
);

const failures = [];
const expect = (url, want) => {
  const got = sandbox.isVideoPage(url);
  if (got !== want) {
    failures.push(`${JSON.stringify(url)} 期望 ${want}，实际 ${got}`);
  }
};

// 视频页：这些要打"视频"标记，否则桌面端会去下网页
expect("https://www.bilibili.com/video/BV1atse6EEfx", true);
expect("https://www.bilibili.com/video/BV1atse6EEfx?spm_id_from=333.999", true);
expect("https://b23.tv/abcd", true);
expect("https://youtu.be/xyz", true);
expect("https://www.youtube.com/watch?v=abc", true);
expect("https://www.douyin.com/video/1234", true);
expect("HTTPS://WWW.BILIBILI.COM/VIDEO/BV1", true); // 大小写不该影响判断

// 直链视频文件：交给普通下载（分段 + 续传），不打视频标记
expect("https://cdn.example.com/a.mp4", false);
expect("https://cdn.example.com/a.mp4?token=1", false);
expect("https://cdn.example.com/a.mkv", false);

// 普通网页 / 空值 / 非 http
expect("https://example.com/file.zip", false);
expect("", false);
expect(null, false);
expect("D:\\a.mp4", false);

if (failures.length) {
  console.error("verify-video-page FAILED");
  for (const f of failures) console.error("-", f);
  process.exit(1);
}
console.log("verify-video-page ok：视频页识别 14 条判据全对");
