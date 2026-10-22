# B2 浏览器直接投递（2026-10-22）

**范围**：右键/页面按钮把 `url/referer/cookie` 交给宿主；宿主 spawn dlm，Cookie 走 `--cookie-stdin`。
**结论**：**Edge 155 真浏览器通过**；dlm 请求带 `Cookie: sid=42`、`Referer: <page>`、UA 空；落盘 156 B 等于页面。
Chrome 155 的 `--load-extension` 仍被忽略（B1 已记录），本发用 Edge 验收。

## 1. 投递链

```
页面按钮/右键 → extension background（chrome.cookies.getAll(url)）
  → Native Messaging `{"op":"download",url,referer,cookie}`
  → dlm-native-host（stdio 长连接）
  → spawn dlm --url ... --out-dir ... --quiet --referer ... --cookie-stdin
  → 宿主回 {"op":"accepted","job":...}；dlm stdout/stderr 进 jobs/<id>.log
```

## 2. 宿主

- `src/host.rs`：`HostConfig` + `Host::handle`；`download` 只接受 `http://`/`https://`；每次下载
  spawn 一个 dlm（**一任务一进程，不排队**——C1 换队列时协议不变）。
- 配置：`DLM_NATIVE_CONFIG` → `%LOCALAPPDATA%\dlm\native-host\config.json` → 默认
  （dlm.exe 与宿主同目录、下载到 `%USERPROFILE%\Downloads`、日志到
  `%LOCALAPPDATA%\dlm\native-host\jobs`）。字段 `dlm_exe`／`download_dir`／`log_dir`。
- **Cookie 走 `--cookie-stdin`**：宿主写 child stdin，不写命令行、不写 job log；`--cookie` 与
  `--cookie-stdin` 互斥。断点文件仍按既有设计存 `task.cookies`（B2 不改这条边界）。
- 拒绝/启动失败回 `{"op":"error","message":...}`；不回显 URL/Cookie。

## 3. 扩展

- 权限新增 `contextMenus` + `cookies` + `host_permissions:["<all_urls>"]`。
- 右键 `link/image/video/audio` → `Download with dlm`；页面右下角 `dlm ↓` 按钮走同一个
  `page-download`。
- `chrome.cookies.getAll({url})` 拼 Cookie 头；只对 http/https 页面投递。
- accepted/error 写 `chrome.storage`，页面状态显示 `dlm job: <id>` 或错误。

## 4. 实测（Edge 155）

命令：`node browser/test/verify-native-download.mjs`

日志 `2026-10-22-b2-browser-download.log` 的关键行：

```text
browser=Edg/155.0.4283.62
page=http://127.0.0.1:60337/b2file.html
dlm=E:\WorkSpace\projects\tool\dlm\target\debug\dlm.exe
status=dlm job: 1792659129-1
file=...\dl\b2file.html bytes=156 equals_page=true
file_sha256=a0ab1379171f0d92d6af08bd5b36a7c9b963e1465e2d07cc63385ebdc1caf25d
requests=[
 {"url":"/b2file.html","cookie":"","referer":"","ua":"...Edg/155..."},
 {"url":"/favicon.ico","cookie":"sid=42","referer":"<page>","ua":"...Edg/155..."},
 {"url":"/b2file.html","cookie":"sid=42","referer":"<page>","ua":""}
]
job_log=下载已完成 ... 全长 156 B，落盘 156 B，归属已定 156 B ... 用时 11 ms
```

- 第三条就是 **dlm 自己那一枪**：同页、`Cookie: sid=42`、`Referer: <page>`、**UA 空**（dlm 不伪装 UA）。
- 截图 `2026-10-22-b2-browser-download.png`：页面右下角 `dlm job: 1792659129-1` + `dlm ↓`。
- job log 里没有 Cookie；只有 dlm 的摘要。

### 验收判据里的假绿

第一版 `reqOk` 只查"任一请求带 Cookie/Referer"，而浏览器自己的 `/favicon.ico` 也带
`Cookie: sid=42` + Referer ⇒ 即使 dlm 没带 Cookie 也可能绿。收紧为
`url === "/b2file.html" && cookie.includes("sid=42") && referer.startsWith(pageUrl) && ua === ""`
后才算通过。

## 5. 安全边界与未做

- Cookie 不进命令行/进程列表（`--cookie-stdin`），不进 job log；断点文件仍存 `task.cookies`（既有设计）。
- 没有本地 HTTP 监听/令牌（B10）、页面挂钩/DNR/代理读取（B3）、媒体嗅探（B3）、队列（C1）。
- 一任务一进程是 B2 临时形态；C1 队列会替换入口，协议不变。
- Chrome 手动加载扩展未测；B2 的真浏览器验收走 Edge 155。

## 6. 复现

```bash
cargo build --bin dlm --bin dlm-native-host
powershell -ExecutionPolicy Bypass -File browser/native-host/install.ps1
node browser/test/verify-native-download.mjs
```
