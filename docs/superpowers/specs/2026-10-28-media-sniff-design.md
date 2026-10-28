# B3b-lite 浏览器媒体嗅探（设计）

**状态**：已实现（2026-10-28；四闸 307/4/117/clippy 0；Edge 156 实测）
**范围**：Chrome/Edge MV3 扩展检测页面请求里的 HLS/媒体直链，页面面板与 popup 一键交给 dlm；
不做 DASH、DRM、下载队列改造。

## 1. 嗅探

- `chrome.webRequest.onBeforeRequest`（只读、非阻塞）监听 `media`/`xmlhttprequest`/`other`，按 URL
  后缀识别：
  - `.m3u8` ⇒ `hls`（交给 dlm `--hls`，直连，不进队列）；
  - `.mp4/.webm/.mkv/.m4a/.mp3/.ts` ⇒ `file`（可直接进队列/直连）；
  - `.mpd` 暂时不列（DASH 需要合并，未交付）。
- 每个 tab 去重、最多 20 条；存 `chrome.storage.session`（service worker 休眠后仍可恢复），
  `tabs.onRemoved` 清理。
- 状态只存在浏览器扩展里；不发外网请求。

## 2. 宿主

- `download` 消息新增可选 `hls: true`；有它就在 spawn 的 dlm 参数加 `--hls`。
- `hls` 下载**强制直连**（不走 `/api/add`）：队列不支持 `--hls`/不可续传，不能悄悄改成普通下载。
- 带 Cookie 的 HLS 仍走直连，Cookie 照旧走 stdin；不带 Cookie 的 HLS 也不进队列。

## 3. 页面/扩展 UI

- content script 增加“媒体 N”按钮：有嗅探结果时出现；点开列出 URL，逐条“下载”。
- toolbar popup：显示宿主状态 + 当前标签页的媒体列表；点击条目发送同一条 `page-download`。
- 全部用 `textContent` 渲染；不插入页面 HTML；面板不遮挡页面主操作。

## 4. 验收

- host：`hls` 强制直连、参数带 `--hls`；无 Cookie 的 HLS 不碰队列端点。
- 真浏览器：页面请求本地 HLS（两段 `AAABBB`），扩展面板出现媒体条目，点击后
  `dlm --hls` 落盘 `stream.ts`，字节等于两段拼接；截图/日志。
- 四闸；B2/B10/B3a 回归。

## 5. 不做

DASH、DRM、Firefox/Opera 包、自动播放/自动下载、队列 HLS。
