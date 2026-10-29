# 页内下载组件：确认、进度、停止（2026-10-29）

## 目标

把原来“右下角一行状态 + 列表”的页内 UI 换成可确认、可看进度、可停止的组件：

- 折叠胶囊：进行中数量 + 总进度；
- 展开面板：进行中（进度条/速度/剩余时间/停止）、最近（完成/失败/已停止/已入队）、可下载；
- 点页面里的下载链接先弹确认卡片，确认后才开始下载；
- 宿主把 dlm 的进度转发给扩展，停止时杀进程并保留断点。

组件用 shadow DOM 隔离站点 CSS，所有用户数据只用 `textContent` 写入。

## 宿主动议

- dlm 不再用 `--quiet`；stdout 改为管道：
  - 进度行 `45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s ...` 解析成
    `{op:"progress",job,tab,name,percent,done,total,speed,eta}`，每 400 ms 最多一发；
  - 其余行写回任务日志（日志里不再有 `\r` 进度噪音）。
- `accepted`/`progress`/`cancel`/`done` 都带 `tab`，background 据此只发给发起页。
- `done` 增加 `name` 与 `canceled`；`cancel` 用 `taskkill /PID` 杀进程，断点保留。
- dlm 仍以 `CREATE_NO_WINDOW` 启动，没有终端窗口。

## 扩展行为

- content 只在本标签页的宿主回过 pong 时接管链接；点击后先显示确认卡片，
  `取消` 不发任何请求，`下载` 才发 `page-download`。
- 面板里的“下载”按钮是明确动作，直接投递，不再二次确认。
- `job-accepted` 建进行中项；`job-progress` 更新进度条；`job-done` 移入“最近”；
  `job-cancel` 失败时在行内显示原因。
- 同一页多个任务按 job 分开显示；完成/失败最多保留 3 条。

## 站点脚本下载（0.2.6）

ModelScope 文件列表每行的“下载”是一个 `<span>`。点击后站点会：

```js
const a = document.createElement("a");
a.href = ".../resolve/master/transformer/config.json?view=false";
a.download = "transformer/config.json";
a.click();   // 节点不挂在 DOM 上
```

游离节点的 `.click()` 不经过页面 DOM，content script 的捕获监听看不到，浏览器就直接下了那个小文件。
修复：新增 `page-hook.js`（`world:"MAIN"`，`document_start`）钩住 `HTMLAnchorElement.prototype.click`；
命中下载属性/文件后缀时改发 `window.postMessage`，content 收到后弹同一个确认卡片。
宿主不在线时用带 `data-dlm-native="1"` 的临时锚点回退浏览器下载，避免钩子自己循环。

真机复现（Tabbit，未点确认）：

- `config.json` 行 → 确认卡片 `config.json` / `www.modelscope.cn · 文件`；`Downloads` 无文件；
- 大文件行 → 确认卡片 `diffusion_pytorch_model-00001-of-00002.safetensors`；同样无文件、无新任务日志。

## Service Worker 缓存

Tabbit 1.18（Chromium 154）里，只改 background 文件、重启浏览器，content 已更新而
Service Worker 仍跑旧代码；之后试过 `background-sw.js` + `importScripts("...?v=<version>")`，
在 Tabbit 里 SW 直接没启动（storage 仍是旧 `buildVersion`，宿主进程不存在）。
最终改成**带版本号的 SW 文件名**：`background-0.2.6.js`，manifest 直接指向它；
换实现时换文件名。验证标记：`data-dlm-version` 与 storage `buildVersion`。

## 验收

- Edge 156（`browser/test/verify-media-sniff.mjs`）：
  - 面板 `可下载 4`；面板 HLS/文件下载通过；
  - 点 `/auto.bin` 先出现确认，确认前没有文件，确认后落盘 4 B；
  - 站点脚本按钮创建游离 `<a download="/site.bin">` 并 `.click()`：同样先确认，确认后落盘 4 B；
  - `最近` 显示 `✓ auto.bin 完成`；
  - 慢速 32 MiB 时进度条 `aria-valuenow=1`，点停止后 `✕ slow.bin 已停止（断点保留）`；
  - 普通链接 `defaultPrevented=false`。
- Tabbit 1.18 真机：
  - 确认卡片显示 `tabbit-file.bin`，确认后落盘 4 B，`最近` 显示完成；
  - 慢速下载进行中 `204.8 KB / 32.0 MB`，`dlm` 进程 `MainWindowHandle=0`；
  - 点停止后 `已停止（断点保留）`，`dlm` 进程退出；
  - ModelScope 行内“下载”（游离 `<a download>`）：`config.json` 与大文件行都只弹确认，未落盘。

## 边界

- 进度来自人类可读行，精度到 0.1 MiB / 0.1 MiB/s；不是逐 worker 的真值。
- 停止 = 杀进程；断点只包含已确认字节。被硬杀时未确认的字节不算进度（见
  [`2026-10-28-native-host-window.md`](2026-10-28-native-host-window.md) 的 9.35 GB 半截文件）。
- 站点脚本下载只覆盖 `<a download>` 的 `.click()`；`form.submit()`、`window.location` 或 iframe
  触发的下载仍走浏览器。
- 组件只在 content script 能注入的页面出现（`chrome://`、扩展页等不出现）。

## 四闸

`--lib` **311/0**（新增 `progress_lines_are_parsed...` 与 `cancel_unknown_job_is_refused`）·
`--bins` **4/0** · `integration` **117/0** · `clippy --all-targets -- -D warnings` **0**。
