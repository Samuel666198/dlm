# dlm Native Messaging 宿主（B1/B2/B10/B3b）

宿主名 `com.dlm.host`；Chrome/Edge 扩展 ID 固定为 `fkebhmogekekphpcggaclfgpfmofhdjk`（扩展 manifest 里的 `key` 决定）。
B1 只做 `{"op":"ping"}` → `{"op":"pong",...}`，不接下载。

## 安装（HKCU，用户级）

```powershell
cargo build --bin dlm-native-host
powershell -ExecutionPolicy Bypass -File browser/native-host/install.ps1
```

`install.ps1` 会：

1. 在 `%LOCALAPPDATA%\dlm\native-host\com.dlm.host.json` 生成 host manifest（`path` 指向本仓库的 `target\debug\dlm-native-host.exe`）；
2. 写 `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.dlm.host` 与
   `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.dlm.host` 的默认值。

卸载：

```powershell
powershell -ExecutionPolicy Bypass -File browser/native-host/uninstall.ps1
```

## 直接测宿主（不走浏览器）

```bash
node browser/test/native-ping.mjs
```

它 spawn `target/debug/dlm-native-host.exe`，写两帧（ping / 未知 op），断言 pong 与 error。

## 真浏览器往返

```bash
node browser/test/verify-native-ping.mjs
```

默认用 Chrome；也可：

```bash
node browser/test/verify-native-ping.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
```

脚本用 `--load-extension` 启动一个临时 profile 的 headed 浏览器，打开本地页面；扩展的 content script 把
`#dlm-native-status` 写进页面，脚本用 CDP 读它，成功时截图到 `docs/evidence/2026-10-22-b1-native-ping.png`。

⚠ Chrome 对 `--load-extension` 有版本/策略限制；脚本带了
`--disable-features=DisableLoadExtensionCommandLineSwitch`，若仍不加载，换 Edge。

## B2：浏览器直接投递

- 右键 `link`/`image`/`video`/`audio` → `Download with dlm`；或页面右下角 `dlm ↓` 把当前页交给 dlm。
- 扩展读 `chrome.cookies.getAll(url)` 并连同 `url/referer` 发给宿主；宿主 spawn 一个 dlm，
  Cookie 走 `--cookie-stdin`（不进命令行/任务日志）。
- 宿主配置（可选）：`%LOCALAPPDATA%\dlm\native-host\config.json`，字段 `dlm_exe`、`download_dir`、`log_dir`；
  测试用 `DLM_NATIVE_CONFIG` 覆盖。默认 dlm.exe 与宿主同目录、下载到 `%USERPROFILE%\Downloads`、
  日志到 `%LOCALAPPDATA%\dlm\native-host\jobs`。
- 验收：`node browser/test/verify-native-download.mjs`（Edge 155 实测：dlm 请求带 `sid=42`/Referer/UA 空，
  落盘文件等于页面；截图/日志在 `docs/evidence/2026-10-22-b2-browser-download.*`）。
- 一任务一进程是 B2 直连的形态；无 Cookie 的公开直链在 `dlm-gui` 开着时由 B10 交给队列。
  页面挂钩/DNR/代理不在 B2；媒体/文件嗅探见 B3b。

## B10：无 Cookie 投递进队列

- `dlm-gui` 在 `%LOCALAPPDATA%\dlm\gui\endpoint.json` 写 `{port,token,pid,started_at}`
  （`--endpoint` 可覆盖）；宿主读该文件，把无 Cookie 的下载 POST 到 `/api/add`。
- 回执带 `mode`：`queue` 显示 `dlm queue: <id>`；`direct` 显示 `dlm job: ...`。
- 带 Cookie ⇒ 仍走 B2 直连（队列不存 Cookie）；端点缺失/连不上 ⇒ 回退直连；
  队列明确拒绝 ⇒ 报错不重复；已发出但超时 ⇒ 报"队列响应超时，未重复下载"。
- 验收：`node browser/test/verify-queue-download.mjs`（Edge 156 实测：队列落盘 65536 B，
  杀 GUI 后回退直连再落盘 65536 B；证据在 `docs/evidence/2026-10-27-quota-queue-b10.*`）。
- 端点文件含 token，和"能直接 spawn dlm"同一信任边界；不自动拉起 GUI。

## B3b：媒体与文件嗅探

- `webRequest`（只读）监听当前标签页的 `.m3u8`、媒体直链，以及普通文件：
  - `onBeforeRequest` 认 `.m3u8` 与媒体后缀；
  - `onHeadersReceived` 认 `Content-Disposition: attachment` 和
    `application/octet-stream`/zip/pdf/msdownload/7z/rar/msi/apk 等二进制响应；
  - 页面 `content.js` 每 1.5 s 扫一次 `a[href]` 的常见文件后缀（zip/rar/pdf/docx/exe/msi/apk/图片…），
    通过 `page-links` 并入同一份列表。
- 每 tab 去重、最多 40 条，`storage.session` 跨 service worker 保存。
- 页面右下角是一个 shadow DOM 小组件：折叠时显示进行中数量与总进度；展开后分“进行中 / 最近 / 可下载”。
  进行中项有进度条、速度、剩余时间与“停止”（杀进程、保留断点）；最近显示完成/失败/已停止/已入队。
- 点条目“下载”发送 `page-download`，HLS 带 `hls:true`。页面里已识别的下载/媒体链接，左键点击先弹
  确认卡片，确认后才交给 dlm；仅当宿主最近回过 pong 时生效；Ctrl/Shift/Alt、`target="_blank"`、
  普通链接不接管；投递失败时页面链接回退浏览器默认行为。
- `page-hook.js` 跑在 MAIN world（`document_start`），钩住站点脚本创建的**游离 `<a download>`** 的
  `.click()`（这种点击不经过页面 DOM，content script 看不到）。它用 `postMessage` 把 URL 交给 content，
  同样先弹确认；宿主不在线时用带 `data-dlm-native="1"` 的临时锚点回退浏览器下载。
- 宿主 spawn dlm 时用 `CREATE_NO_WINDOW`，不再弹空白终端窗口（关窗口会顺手杀掉下载）。
- 宿主把 dlm 的进度行解析成 `op:"progress"`（400 ms 节流），子进程退出后回 `op:"done"`；
  页面与 popup 的完成/失败状态都来自这里。
- 工具栏 popup 显示宿主状态与当前标签页的下载列表。
- Service Worker 用**带版本号的文件名**（当前 `background-0.2.6.js`，manifest 里指过去）：
  未打包扩展的 SW 会被浏览器缓存，只改内容、重启浏览器仍可能跑旧代码；换文件名才能强制重取。
  排查标记：页面 `document.documentElement` 的 `data-dlm-version`、storage 的 `buildVersion`。
- 宿主 `hls:true` 强制直连 `dlm --hls`，不走队列（队列不接不可续传的 HLS）；
  普通文件不带 Cookie 时按 B10 进队列（GUI 开着），带 Cookie 或 GUI 不可达时直连。
- 验收：

```bash
node browser/test/verify-media-sniff.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
```

证据：`docs/evidence/2026-10-28-media-sniff.*`（本地页面请求 HLS + 文件链接 → 面板 `下载 3` → 点击 →
`stream.ts` = `AAABBB`、`file.bin` = `FILE`；左键点 `/auto.bin` 由 dlm 落盘且页面不跳走；
普通链接不被拦截；Tabbit 实测见 `2026-10-28-tabbit-extension.md`）。
DASH（`.mpd`）暂不列；不做音视频合并。

## 手动加载

`chrome://extensions` → 开发者模式 → 加载已解压的扩展 → 选 `browser/extension`。扩展动作徽标应为 `OK`，
任意页面右下角显示 `dlm native: pong com.dlm.host v...`。

## 安全边界

- 宿主只读 stdin、写 stdout 帧；诊断走 stderr。B1 只认 `ping`/`hello`；B2 增加 `download`
  （只接 `http/https`，spawn `dlm`，Cookie 走 `--cookie-stdin`）；B10 对无 Cookie 的下载先尝试
  队列 `/api/add`。不执行任意命令；宿主自身没有本地监听（队列端口属于 `dlm-gui`）。
- B2/B3b 扩展权限：`nativeMessaging` + `storage` + `contextMenus` + `cookies` + `webRequest`，
  `host_permissions: <all_urls>`；content script 显示状态、页面按钮和媒体面板。
- 页面挂钩/DNR/代理不在 B2/B10；媒体/文件嗅探只到 HLS/直链/文件链接，DASH/DRM 未做。

## Tabbit Browser（本机 1.18）

Tabbit 是 Chromium 154，支持该扩展。自动化无法打开 `chrome://extensions`，本机用快捷方式参数
`--load-extension=%LOCALAPPDATA%\dlm\extension` 加载；记录与回退方式见
`docs/evidence/2026-10-28-tabbit-extension.md`。
