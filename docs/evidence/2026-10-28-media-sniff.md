# B3b-lite 浏览器媒体与文件嗅探证据（2026-10-28）

**四闸**：`--lib` **308/0**（3.01 s）· `--bins` **4/0** · `integration` **117/0** ·
`clippy --all-targets -- -D warnings` **0**（本轮只改扩展 JS 与文档，Rust 零改动；
308 里的 +1 是 User-Agent 修复那轮的回归针）。

## 实现

- `background.js`：
  - `webRequest.onBeforeRequest` 只读嗅探 `.m3u8`/媒体后缀（每 tab 去重、最多 40 条，
    `storage.session` 跨 service worker 保存，`tabs.onRemoved/onUpdated` 清理）。
  - `webRequest.onHeadersReceived`（`responseHeaders`）认 `Content-Disposition: attachment`
    与 `application/octet-stream`/zip/pdf/msdownload/7z/rar/msi/apk 等二进制响应；
    覆盖 `main_frame`/`sub_frame`/`object`/`xmlhttprequest`/`other`。
  - `page-links` 消息把页面收集到的文件链接并入同一份列表。
- `content.js`：页面右下角“下载 N”按钮 + 面板；每 1.5 s 扫一次 `a[href]` 的常见文件后缀
  （zip/rar/pdf/docx/exe/msi/apk/图片…，本地最多 30 条），与后台嗅探结果合并（最多 40 条）；
  条目“下载”发送 `page-download`，HLS 带 `hls`。面板与按钮文案从“媒体”改为“下载”。
- `content.js` 点击接管：已识别为下载/媒体的 `<a>` 左键点击在捕获阶段 `preventDefault`，
  改发 `page-download`；只在宿主最近回过 pong 时生效；Ctrl/Shift/Alt、`target="_blank"`、
  普通链接不接管；`page-download` 回执 `ok:false` 时回退浏览器默认行为。
- `background.js`：`page-download` 回 `{ok}`，让 content 能区分“宿主没接住”并回退。
- `popup.html` / `popup.js`：扩展图标 popup，显示宿主状态和当前标签页下载列表。
- `src/host.rs`：`download` 接受 `hls:true`，跳过队列，给 dlm 加 `--hls`；HLS 强制直连；
  普通文件不带 Cookie 时按 B10 进队列。**本轮 host.rs 无改动。**
- Manifest：扩展版本 **0.2.2**（点击接管增量；权限沿用 `webRequest`）。

## 真浏览器验收（Edge 156）

`browser/test/verify-media-sniff.mjs`：

- 页面放一个请求 `/stream.m3u8` 的按钮、一个 `<a href="/file.bin">` 面板用链接、
  一个 `<a id="auto-file-link" href="/auto.bin">` 点击接管链接；
- 扩展面板出现 `下载 3`，条目为 `/stream.m3u8`、`/file.bin`、`/auto.bin`；
- 点击 HLS 条目后宿主直连 `dlm --hls`，落盘 `stream.ts` **6 B**，内容 `AAABBB`（两段拼接）；
- 点击文件条目后落盘 `file.bin` **4 B**，内容 `FILE`；
- 左键点击 `/auto.bin`：页面 URL 停在 `page.html`，由 dlm 落盘 `auto.bin` **4 B** = `AUTO`；
- 对照臂：点一条普通页面链接，`defaultPrevented` 为 `false`（不被接管）；
- 截图：`docs/evidence/2026-10-28-media-sniff.png`；日志：`2026-10-28-media-sniff.log`。

```
media_button=下载 3
media_items=[".../file.bin",".../auto.bin",".../stream.m3u8"]
file=...\dl\stream.ts bytes=6
plain_file=...\dl\file.bin bytes=4
host_status=dlm job: ...
auto_click_file=...\dl\auto.bin bytes=4 stayed=true
normal_link_prevented=false
```

## 真浏览器验收（Tabbit 1.18.21 / Chromium 154）

- 扩展从 `%LOCALAPPDATA%\dlm\extension`（0.2.2）加载；页面出现 `下载 2`，
  条目 `stream.m3u8` + `tabbit-file.bin`。
- 点文件条目：`%USERPROFILE%\Downloads\tabbit-file.bin` **4 B** = `FILE`；
- 点 HLS 条目：`%USERPROFILE%\Downloads\stream.ts` **6 B** = `AAABBB`；
- **左键点页面里的下载链接**：页面 URL 不变，dlm 落盘 `tabbit-file.bin` **4 B** = `FILE`；
  本地站点记录到请求头 `ua="dlm/0.1.0"`。
- 截图：`docs/evidence/2026-10-28-tabbit-file-panel.png`；
  加载与快捷方式细节：`2026-10-28-tabbit-extension.md`。

回归：B1/B2/B10 三条真浏览器用例在 B3b-lite 首发那轮已通过；本轮只改扩展 JS，未重复运行，
本轮的端到端证据是上面的 Edge 与 Tabbit 两发。

## 边界

- 只支持 HLS（`.m3u8`）和媒体/文件直链；DASH（`.mpd`）不列、不做音视频合并。
- 页面链接靠后缀启发式；无后缀的下载按钮靠响应头识别，识别不到的不会出现在面板。
- 左键接管只覆盖**已识别的链接**；JS 按钮/表单触发的下载仍走浏览器（不做 `chrome.downloads` 全局接管）。
- 嗅探结果只在浏览器会话里；不自动下载、不自动播放。
- HLS 不可续传，失败保留已完成的半截文件（沿用 B3a）。
- 文件链接仍走 B10：无 Cookie + GUI 开着 ⇒ 队列；带 Cookie/GUI 不可达 ⇒ 直连。
