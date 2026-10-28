# B3b-lite 浏览器媒体嗅探证据（2026-10-28）

**四闸**：`--lib` **307/0**（3.01 s）· `--bins` **4/0** · `integration` **117/0**（65.07 s）·
`clippy --all-targets -- -D warnings` **0**。

## 实现

- `background.js`：`webRequest.onBeforeRequest` 只读嗅探 `.m3u8`/媒体后缀（每 tab 去重、最多 20 条，
  `storage.session` 跨 service worker 保存，`tabs.onRemoved/onUpdated` 清理）。
- `content.js`：页面右下角“媒体 N”按钮 + 面板；条目“下载”发送 `page-download` 并带 `hls`。
- `popup.html` / `popup.js`：扩展图标 popup，显示宿主状态和当前标签页媒体列表。
- `src/host.rs`：`download` 接受 `hls:true`，跳过队列，给 dlm 加 `--hls`；HLS 强制直连。
- Manifest：新增 `webRequest`、`action.default_popup`；扩展版本 0.2.0。

## 真浏览器验收

`browser/test/verify-media-sniff.mjs`，Edge 156：

- 页面点击“加载媒体”请求 `/stream.m3u8`；
- 扩展面板出现 `媒体 1`，条目为 `http://127.0.0.1:<port>/stream.m3u8`；
- 点击“下载”后宿主直连 `dlm --hls`，落盘 `stream.ts` **6 B**，内容 `AAABBB`（两段拼接）；
- 截图：`docs/evidence/2026-10-28-media-sniff.png`；日志：`2026-10-28-media-sniff.log`。

回归：B1 ping、B2 直接投递、B10 queue/direct 三条真浏览器用例均通过。

## 边界

- 只支持 HLS（`.m3u8`）和媒体直链；DASH（`.mpd`）不列、不做音视频合并。
- 只做 Chrome/Edge MV3；Firefox/Opera 包未做。
- 嗅探结果只在浏览器会话里；不自动下载、不自动播放。
- HLS 不可续传，失败保留已完成的半截文件（沿用 B3a）。
