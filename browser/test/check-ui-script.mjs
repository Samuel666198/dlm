// src/ui.html 的静态检查：不启动浏览器，只确认内嵌脚本是**能解析的 JavaScript**。
//
// 为什么需要这一条：`dlm-gui` 把 `ui.html` 用 `include_str!` 原样嵌进二进制，页面上的
// 语法错误不会让 `cargo build` 失败 —— 它会变成一张**空白页**，而 Rust 侧四闸全绿。
// 2026-11-08 就在 P2 里踩过一次（`const done` 与既有同名变量重声明）：页面完全静默，
// 只有真浏览器验收才看得出来。这条检查把那一类错误压到秒级、无需浏览器。
//
// 用法：node browser/test/check-ui-script.mjs
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const uiPath = join(here, "..", "..", "src", "ui.html");
const html = readFileSync(uiPath, "utf8");

const open = html.indexOf("<script>");
const close = html.lastIndexOf("</script>");
if (open < 0 || close < 0 || close < open) {
  console.error("ui.html 里找不到 <script> 块");
  process.exit(1);
}
const source = html.slice(open + "<script>".length, close);

// `new vm.Script` 只编译不执行：既能抓到语法错误，又不会因为缺 DOM 而误报。
try {
  new vm.Script(source, { filename: "ui.html <script>" });
} catch (err) {
  console.error("src/ui.html 的内嵌脚本有语法错误（页面会白屏，Rust 四闸看不出来）：");
  console.error(`  ${err.message}`);
  process.exit(1);
}

console.log(`ui script ok: ${source.length} bytes, parses cleanly`);
