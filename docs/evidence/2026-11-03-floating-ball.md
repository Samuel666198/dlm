# 悬浮球与面板重排（2026-11-03）

## 目标

把页内组件从“右下角固定胶囊”改成可拖动的悬浮球：

- 球可拖到页面任意位置，位置跨页面刷新保留；
- 闲置 5 秒且没有进行中任务时降到 55% 透明；
- 展开面板按球的位置自动换向，窄屏变底部抽屉；
- 面板改成“进行中 / 可下载 / 最近”三个标签页。

## 实现

- 扩展版本 **0.2.7**：`background-0.2.6.js` 改名为 `background-0.2.7.js`，manifest 同步（沿用版本化 SW 文件名）。
- `browser/extension/content.js`
  - 球 48px；`pointerdown/move/up` 拖动，移动超过 4px 才算拖动，低于阈值算点击；
  - 位置存 `chrome.storage.local` 的 `dlmBallPos`（全局一份，像素坐标）；窗口缩放时夹在视口内；
  - 闲置由 `IDLE_AFTER_MS = 5000` 控制；进行中、面板展开、确认卡片存在时不淡出；
  - 面板侧向：球在左半屏 `dir-down` 向下展开，右半屏 `side-left` 向左展开；`<520px` 用底部抽屉；
  - 标签页保留原 id：`#dlm-list`、`#dlm-active`、`#dlm-finished`，原有验收脚本继续可用。
- `browser/README.md` 同步交互说明与验收命令。

## 验收

命令：

```bash
node browser/test/verify-widget-ball.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
node browser/test/verify-media-sniff.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
```

真浏览器 Edge 156（`Edg/156.0.4314.58`）实测，日志在 [`2026-11-03-floating-ball.log`](2026-11-03-floating-ball.log)：

```text
initial={"x":918,"y":549,"w":48,"h":48}
moved={"x":298,"y":129,"w":48,"h":48}
panel_at_top_left=panel dir-down
moved2={"x":922,"y":553,"w":48,"h":48}
panel_at_bottom_right=panel side-left
links_tab=true count=2
panel_closed=true
class_5s=root idle
class_6s=root idle
idle=true opacity=0.55
after_reload={"x":922,"y":553,"w":48,"h":48}
```

- 拖动生效：球从右下 `(918,549)` 拖到左上 `(298,129)`，再拖回右下 `(922,553)`；
- 位置持久化：刷新后仍在 `(922,553)`；
- 闲置淡出：关闭面板 5.5 秒后 `#dlm-root` 带 `idle`，球 `opacity=0.55`；
- 面板方向：左上时 `dir-down`（向下），右下时 `side-left`（向左）；
- 标签页：切到“可下载”后条目数 2，条目可点。

媒体/文件嗅探回归也通过：HLS 合并 `AAABBB`、`file.bin` 4 B、`auto.bin` 自动接管、站点脚本 `/site.bin` 确认后落盘、慢速任务停止保留断点、普通链接不拦截。截图见 [`2026-11-03-floating-ball.png`](2026-11-03-floating-ball.png)。

## 未做 / 边界

- 位置是**全局一份**，不按站点分别记；要按站点记需把 storage key 换成 `dlmBallPos:<host>`。
- 面板没有焦点陷阱；Esc 可收起，球本身可 Tab 聚焦、Enter 展开。
- 闲置淡出只在“面板收起且没有进行中任务”时生效；下载中保持 100%。
- 拖动只挂在球上；面板打开时拖球，面板跟随移动。
