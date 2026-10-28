# B3b-lite 浏览器媒体嗅探 Implementation Plan

**状态**：已交付（2026-10-28；四闸 307/4/117/clippy 0；Edge 156 实测）。
**Spec**：[`../specs/2026-10-28-media-sniff-design.md`](../specs/2026-10-28-media-sniff-design.md)

## Task 1 扩展参数与宿主

- `browser/extension/manifest.json`：加 `webRequest`、`action.default_popup`。
- `src/host.rs`：`download` 读 `hls`；`hls` 跳过队列，直接 spawn `dlm ... --hls`。
- host 单测：`hls` 直连不碰队列端点；参数带 `--hls`（提取/复用参数构造或本地验收）。

## Task 2 嗅探与展示

- `background.js`：`webRequest` 识别、按 tab 去重/限 20、`storage.session` 持久、`tabs.onRemoved` 清理；
  `media-list` 消息；`page-download` 透传 `hls`。
- `content.js`：媒体按钮 + 面板 + 下载按钮；轮询 media-list。
- `popup.html` / `popup.js`：宿主状态 + 媒体列表 + 下载。
- 全部 `textContent`，不 innerHTML 插值。

## Task 3 验收

- `browser/test/verify-media-sniff.mjs`：本地页面请求 HLS，Edge 扩展面板出现条目；
  点击后宿主直连 `--hls`；落盘 `stream.ts` 字节 `AAABBB`；截图/日志。
- B2/B10 浏览器回归；四闸。
- 文档 README/browser README/STATUS/路线图/能力表/证据；提交。
