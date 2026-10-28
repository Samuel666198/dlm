# Tabbit 扩展安装记录（2026-10-28）

**浏览器**：Tabbit Browser 1.18.21.0（Chromium 154）
**扩展路径**：`%LOCALAPPDATA%\dlm\extension`（由包内 `install.ps1` 复制）
**宿主**：`%LOCALAPPDATA%\dlm\bin\dlm-native-host.exe`，HKCU Chrome/Edge 已注册 `com.dlm.host`

## 为什么不用“加载已解压”

- Tabbit 的 `chrome://extensions` 在 Playwright/CDP 里被拦截（`Navigation URL is not allowed`）。
- Tabbit 没有 `Extensions.loadUnpacked` CDP 命令（实测 `'Extensions.loadUnpacked' wasn't found`）。

## 实际方案

给 Tabbit 的四个快捷方式加启动参数：

```text
--load-extension="C:\Users\ROG\AppData\Local\dlm\extension" --disable-features=DisableLoadExtensionCommandLineSwitch
```

修改的快捷方式：

- `%USERPROFILE%\Desktop\Tabbit Browser.lnk`
- `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Tabbit Browser.lnk`
- `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\Tabbit Browser.lnk`
- `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\Tabbit Browser.lnk`

原快捷方式备份在 `%LOCALAPPDATA%\dlm\shortcut-backup\`。启动参数对每次通过这些快捷方式启动生效。

## 验证

- 重启 Tabbit 后打开测试页：
  - `#dlm-submit` 存在；
  - `#dlm-native-status` 显示扩展状态（本轮显示上次任务 `dlm job: …`；宿主连通性以点下载成功为准）。
- 本地页面请求 `/stream.m3u8` 且带一个 `/tabbit-file.bin` 文件链接：
  - 页面出现 `下载 2`；
  - 扩展面板列出 `stream.m3u8`（HLS）与 `tabbit-file.bin`（文件）。
- 点文件条目：`%USERPROFILE%\Downloads\tabbit-file.bin` **4 B** = `FILE`；
- 点 HLS 条目：`%USERPROFILE%\Downloads\stream.ts` **6 B** = `AAABBB`；
- 截图：`docs/evidence/2026-10-28-tabbit-file-panel.png`。

**注意**：扩展文件更新后必须完全退出 Tabbit（`Stop-Process -Name 'Tabbit Browser'`）再从未改过的快捷方式启动；
仅关闭窗口或再次点快捷方式不会重新加载已解压扩展，面板仍是旧代码。

**Service Worker 缓存**：只改 `background.js`、重启浏览器后，content script 会更新而 Service Worker
可能仍跑旧缓存（实测 done 分支不执行）。扩展 0.2.4 起入口固定为 `background-sw.js`，内部按版本
`importScripts("background-impl.js?v=<version>")`；升版本即换 URL，强制重取实现。
排查标记：页面 `document.documentElement` 的 `data-dlm-version`，storage 的 `buildVersion`。

## 注意

- 重启 Tabbit 时它没有自动恢复原标签页；已手动恢复 4 个 URL（本机 `samueldemac-mini.local` 已 DNS 不解析，未能恢复）。
- 如果 Tabbit 更新或重建快捷方式，启动参数可能丢失；届时重新加回上面的参数即可。
- 通过其它入口启动（例如自动更新器直接启动 exe、非上述快捷方式）不会带 `--load-extension`，扩展不会加载；先完全退出 Tabbit，再从未被改过的快捷方式启动。
- 要撤销：把 `%LOCALAPPDATA%\dlm\shortcut-backup\` 里的对应 `.lnk` 复制回原位置。
