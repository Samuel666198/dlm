# B3b-lite 浏览器媒体与文件嗅探证据（2026-10-28）

**四闸**：`--lib` **307/0**（3.01 s）· `--bins` **4/0** · `integration` **117/0**（64.65 s）·
`clippy --all-targets -- -D warnings` **0**（本轮只改扩展 JS 与文档，Rust 零改动）。

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
- `popup.html` / `popup.js`：扩展图标 popup，显示宿主状态和当前标签页下载列表。
- `src/host.rs`：`download` 接受 `hls:true`，跳过队列，给 dlm 加 `--hls`；HLS 强制直连；
  普通文件不带 Cookie 时按 B10 进队列。**本轮 host.rs 无改动。**
- Manifest：扩展版本 **0.2.1**（文件支持增量；权限沿用 `webRequest`）。

## 真浏览器验收（Edge 156）

`browser/test/verify-media-sniff.mjs`：

- 页面放一个请求 `/stream.m3u8` 的按钮和一个 `<a href="/file.bin">` 文件链接；
- 扩展面板出现 `下载 2`，条目为 `/stream.m3u8` 与 `/file.bin`；
- 点击 HLS 条目后宿主直连 `dlm --hls`，落盘 `stream.ts` **6 B**，内容 `AAABBB`（两段拼接）；
- 点击文件条目后落盘 `file.bin` **4 B**，内容 `FILE`；
- 截图：`docs/evidence/2026-10-28-media-sniff.png`；日志：`2026-10-28-media-sniff.log`。

```
media_button=下载 2
media_items=["http://127.0.0.1:<port>/file.bin","http://127.0.0.1:<port>/stream.m3u8"]
file=...\dl\stream.ts bytes=6
plain_file=...\dl\file.bin bytes=4
```

## 真浏览器验收（Tabbit 1.18.21 / Chromium 154）

- 扩展从 `%LOCALAPPDATA%\dlm\extension`（0.2.1）加载；页面出现 `下载 2`，
  条目 `stream.m3u8` + `tabbit-file.bin`。
- 点文件条目：`%USERPROFILE%\Downloads\tabbit-file.bin` **4 B** = `FILE`；
- 点 HLS 条目：`%USERPROFILE%\Downloads\stream.ts` **6 B** = `AAABBB`；
- 截图：`docs/evidence/2026-10-28-tabbit-file-panel.png`；
  加载与快捷方式细节：`2026-10-28-tabbit-extension.md`。

回归：B1 ping、B2 直接投递、B10 queue/direct 三条真浏览器用例均通过。

## 边界

- 只支持 HLS（`.m3u8`）和媒体/文件直链；DASH（`.mpd`）不列、不做音视频合并。
- 页面链接靠后缀启发式；无后缀的下载按钮靠响应头识别，识别不到的不会出现在面板。
- 嗅探结果只在浏览器会话里；不自动下载、不自动播放。
- HLS 不可续传，失败保留已完成的半截文件（沿用 B3a）。
- 文件链接仍走 B10：无 Cookie + GUI 开着 ⇒ 队列；带 Cookie/GUI 不可达 ⇒ 直连。
