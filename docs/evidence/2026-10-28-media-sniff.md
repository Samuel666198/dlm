# B3b-lite 浏览器媒体与文件嗅探证据（2026-10-28，2026-10-29 更新 UI/进度）

**四闸**：`--lib` **311/0**（3.01 s）· `--bins` **4/0** · `integration` **117/0**（64.56 s）·
`clippy --all-targets -- -D warnings` **0**。

## 实现

- `background-0.2.6.js`（Service Worker 文件名带版本号，manifest 指过去；见
  [`2026-10-29-inpage-widget.md`](2026-10-29-inpage-widget.md) 的缓存说明）：
  - `webRequest.onBeforeRequest` 只读嗅探 `.m3u8`/媒体后缀（每 tab 去重、最多 40 条，
    `storage.session` 跨 service worker 保存，`tabs.onRemoved/onUpdated` 清理）。
  - `webRequest.onHeadersReceived`（`responseHeaders`）认 `Content-Disposition: attachment`
    与 `application/octet-stream`/zip/pdf/msdownload/7z/rar/msi/apk 等二进制响应。
  - `page-links` 消息把页面收集到的文件链接并入同一份列表。
  - `page-download` 回 `{ok}`；把宿主的 `accepted/progress/cancel/done` 按 `tab` 转发给页面。
- `content.js`：页面右下角的 shadow DOM 组件（折叠胶囊 + “进行中 / 最近 / 可下载”面板）；
  每 1.5 s 扫一次 `a[href]` 的常见文件后缀，与后台嗅探结果合并（最多 40 条）。
  - 左键点已识别的下载/媒体链接：**先弹确认卡片**，确认后才发 `page-download`；
    Ctrl/Shift/Alt、`target="_blank"`、普通链接不接管；宿主未 pong 时不接管。
  - `page-hook.js`（MAIN world，`document_start`）钩住站点创建的游离 `<a download>` 的 `.click()`，
    经 `postMessage` 交给 content，同样先确认；宿主不在线时回退浏览器下载。
  - 进行中显示进度条、速度、剩余时间与“停止”；停止走宿主 `cancel`（杀进程、保留断点）。
  - 页面链接投递失败时回退浏览器默认行为；面板里的“下载”是明确动作，不再二次确认。
- `src/host.rs`：
  - spawn dlm 用 `CREATE_NO_WINDOW`（Windows），不再弹终端窗口；
  - 去掉 `--quiet`，读 dlm stdout：进度行转 `op:"progress"`（400 ms 节流），其余写回任务日志；
  - 子进程退出回 `{op:"done",job,tab,name,ok,code,canceled,log}`；
  - `cancel` 用 `taskkill /PID` 杀进程并标记 `canceled`。
- Manifest：扩展版本 **0.2.6**（确认卡片 + 进度/停止 + 站点脚本下载钩子 + 版本化 SW 文件名；
  权限沿用 `webRequest`）。

## 真浏览器验收（Edge 156）

`browser/test/verify-media-sniff.mjs`：

- 页面放 HLS 请求按钮、面板用 `/file.bin` 链接、`/auto.bin` 确认链接、`/slow.bin` 慢速链接；
- 面板 `可下载 4`；点 HLS 条目落盘 `stream.ts` **6 B** = `AAABBB`，点文件条目落盘 `file.bin` **4 B** = `FILE`；
- 点 `/auto.bin`：确认卡片出现；**确认前 `auto.bin` 不存在**；点“下载”后落盘 **4 B** = `AUTO`，页面 URL 不变；
- 站点脚本按钮创建游离 `<a download="/site.bin">` 并 `.click()`：同样先弹确认，确认后才落盘 **4 B** = `SITE`；
- 完成状态：`#dlm-finished` 出现 `✓ auto.bin 完成`；
- 慢速 32 MiB：`#dlm-active .bar` 的 `aria-valuenow` 到 **1**（已有进度）；
  点“停止”后显示 `✕ slow.bin 已停止（断点保留）`；
- 对照臂：普通页面链接 `defaultPrevented=false`（不被接管）。

```
media_button=可下载 4
media_items=[".../file.bin",".../auto.bin",".../slow.bin",".../stream.m3u8"]
plain_file=...\dl\file.bin bytes=4
host_ready=true
auto_click_file=...\dl\auto.bin bytes=4 stayed=true
done_text=✓auto.bin完成✓file.bin完成✓stream.m3u8完成
site_script_file=...\dl\site.bin bytes=4
progress=1
stopped_text=✕slow.bin已停止（断点保留）✓auto.bin完成✓file.bin完成
normal_link_prevented=false
```

截图：`docs/evidence/2026-10-28-media-sniff.png`；日志：`2026-10-28-media-sniff.log`。

## 真浏览器验收（Tabbit 1.18.21 / Chromium 154）

- 扩展从 `%LOCALAPPDATA%\dlm\extension`（0.2.6）加载；组件 `可下载 2`。
- 点下载链接 → 确认卡片（文件名 `tabbit-file.bin`）→ 点“下载” → 落盘 **4 B** = `FILE`，
  `最近` 显示 `✓ tabbit-file.bin 完成`。
- ModelScope 模型文件页的**行内“下载”**（`<span>` 触发站点动态创建游离 `<a download>`）：
  点 `config.json` 行 → 确认卡片显示 `config.json`、`www.modelscope.cn · 文件`，`Downloads` 没有落盘；
  点大文件行 → 确认卡片显示 `diffusion_pytorch_model-00001-of-00002.safetensors`，同样没有落盘。
- 慢速 32 MiB：进行中显示 `204.8 KB / 32.0 MB`、进度条；此时 `dlm` 进程
  `MainWindowHandle=0`（没有终端窗口）。
- 点“停止” → `✕ slow.bin 已停止（断点保留）`，`dlm` 进程退出。
- 截图：`docs/evidence/2026-10-28-tabbit-file-panel.png`；
  加载与快捷方式细节：`2026-10-28-tabbit-extension.md`。

## 边界

- 只支持 HLS（`.m3u8`）和媒体/文件直链；DASH（`.mpd`）不列、不做音视频合并。
- 页面链接靠后缀启发式；无后缀的下载按钮靠响应头识别，识别不到的不会出现在面板。
- 左键确认只覆盖**已识别的链接**；JS 按钮/表单触发的下载仍走浏览器（不做 `chrome.downloads` 全局接管）。
- 进度来自 dlm 的人类可读进度行（400 ms 节流）；停止是杀进程，断点里只包含已确认字节。
- HLS 不可续传，失败保留已完成的半截文件（沿用 B3a）。
- 文件链接仍走 B10：无 Cookie + GUI 开着 ⇒ 队列；带 Cookie/GUI 不可达 ⇒ 直连。
