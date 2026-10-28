# dlm Native Messaging 宿主（B1/B2/B10）

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
  页面挂钩/DNR/代理/媒体嗅探不在 B2。

## B10：无 Cookie 投递进队列

- `dlm-gui` 在 `%LOCALAPPDATA%\dlm\gui\endpoint.json` 写 `{port,token,pid,started_at}`
  （`--endpoint` 可覆盖）；宿主读该文件，把无 Cookie 的下载 POST 到 `/api/add`。
- 回执带 `mode`：`queue` 显示 `dlm queue: <id>`；`direct` 显示 `dlm job: ...`。
- 带 Cookie ⇒ 仍走 B2 直连（队列不存 Cookie）；端点缺失/连不上 ⇒ 回退直连；
  队列明确拒绝 ⇒ 报错不重复；已发出但超时 ⇒ 报"队列响应超时，未重复下载"。
- 验收：`node browser/test/verify-queue-download.mjs`（Edge 156 实测：队列落盘 65536 B，
  杀 GUI 后回退直连再落盘 65536 B；证据在 `docs/evidence/2026-10-27-quota-queue-b10.*`）。
- 端点文件含 token，和"能直接 spawn dlm"同一信任边界；不自动拉起 GUI。

## 手动加载

`chrome://extensions` → 开发者模式 → 加载已解压的扩展 → 选 `browser/extension`。扩展动作徽标应为 `OK`，
任意页面右下角显示 `dlm native: pong com.dlm.host v...`。

## 安全边界

- 宿主只读 stdin、写 stdout 帧；诊断走 stderr。B1 只认 `ping`/`hello`；B2 增加 `download`
  （只接 `http/https`，spawn `dlm`，Cookie 走 `--cookie-stdin`）；B10 对无 Cookie 的下载先尝试
  队列 `/api/add`。不执行任意命令；宿主自身没有本地监听（队列端口属于 `dlm-gui`）。
- B2 扩展权限：`nativeMessaging` + `storage` + `contextMenus` + `cookies`，`host_permissions: <all_urls>`；
  content script 显示状态并提供页面按钮。
- 页面挂钩/DNR/代理/媒体嗅探不在 B2/B10。
