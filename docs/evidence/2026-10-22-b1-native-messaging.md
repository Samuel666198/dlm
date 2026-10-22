# B1 Native Messaging 最小闭环（2026-10-22）

**范围**：`com.dlm.host` 宿主 + 一个 MV3 扩展 + HKCU 注册；只做 ping/pong，不接下载。
**结论**：宿主直接帧往返通过；**Edge 155 真浏览器往返通过**（页面显示 `dlm native: pong com.dlm.host v0.1.0`，
截图 `2026-10-22-b1-native-ping.png`）；Chrome 155 的 `--load-extension` 被忽略，手动加载未测。

## 1. 宿主

- 二进制：`target\debug\dlm-native-host.exe`（`src/bin/dlm-native-host.rs` 薄壳；协议在 `src/native.rs`）。
- 帧：4 字节本机序长度 + UTF-8 JSON；单帧上限 **1 MiB**；stdout 只出帧，诊断走 stderr。
- 协议：
  - `{"op":"ping"}` → `{"op":"pong","host":"com.dlm.host","version":"0.1.0"}`
  - `{"op":"hello"}` → `{"op":"hello",...}`
  - 未知 op → `{"op":"error","message":"unknown op"}`；坏 JSON → `{"op":"error","message":"invalid json"}`
- 直接测：`node browser/test/native-ping.mjs` ⇒ `native-ping ok: {"host":"com.dlm.host","op":"pong","version":"0.1.0"} {"message":"unknown op","op":"error"}`

## 2. 扩展

- `browser/extension/`，MV3，固定 `key`；扩展 ID 由 key 算出并用 IDM 现役 Chrome key 校准过算法：
  **`fkebhmogekekphpcggaclfgpfmofhdjk`**。
- 权限只 `nativeMessaging` + `storage`；`background.service_worker` 建 `connectNative("com.dlm.host")` 长连接，
  `onDisconnect` 1500 ms 重连；`content.js` 把 pong/错误写到页面右下角 `#dlm-native-status`。
- 未打包扩展加载时 service worker 可能没被事件拉起 ⇒ `content.js` 发 `{op:"wake"}` 主动唤醒；
  这是第一次 Edge 只显示 `waiting for pong` 的根因，修完后拿到 pong。
- 验收判据只认 `dlm native: pong ` 前缀，不认 `waiting for pong` 里的子串。

## 3. 注册（HKCU，用户级）

`browser/native-host/install.ps1` 生成 `%LOCALAPPDATA%\dlm\native-host\com.dlm.host.json`：

```json
{
    "name":  "com.dlm.host",
    "description":  "dlm native messaging host (B1 ping/pong)",
    "path":  "E:\\WorkSpace\\projects\\tool\\dlm\\target\\debug\\dlm-native-host.exe",
    "type":  "stdio",
    "allowed_origins":  [ "chrome-extension://fkebhmogekekphpcggaclfgpfmofhdjk/" ]
}
```

并写：

- `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.dlm.host`
- `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.dlm.host`

两个默认值都指向上面这份 manifest。卸载脚本 `browser/native-host/uninstall.ps1` 会删两个键与 `%LOCALAPPDATA%\dlm\native-host`。
⚠ PS5.1 对 UTF-8/LF 的 `.ps1` 解析不稳：脚本保持 **ASCII + CRLF**。

## 4. 真浏览器往返

命令：

```bash
node browser/test/verify-native-ping.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
```

脚本用临时 profile 启动 headed Edge，`--load-extension` 加载扩展，打开本地页面；CDP 读
`#dlm-native-status`，成功后 `Page.captureScreenshot`。

实测日志 `2026-10-22-b1-native-ping.log`：

```text
browser=Edg/155.0.4283.62
page=http://127.0.0.1:57932/
status=dlm native: pong com.dlm.host v0.1.0
screenshot=E:\WorkSpace\projects\tool\dlm\docs\evidence\2026-10-22-b1-native-ping.png
```

截图右下角绿标：`dlm native: pong com.dlm.host v0.1.0`。

**Chrome 155**：同一个脚本启动 Chrome 155（`Chrome/155.0.8059.85`）时页面没有
`#dlm-native-status`，扩展未加载；`--load-extension`（含
`--disable-features=DisableLoadExtensionCommandLineSwitch`）被忽略。**手动加载未测**；这不影响宿主/扩展本身，
只是 CLI 测试通道被 Chrome 版本限制。真 Chrome 验收可走 `chrome://extensions` → 开发者模式 → 加载已解压的扩展。

## 5. 安全边界

- 宿主不监听端口、不接下载、不读页面；只认 ping/hello。
- 扩展权限只有 `nativeMessaging` + `storage`；content script 只显示状态，不挂钩页面请求。
- 注册写 HKCU 用户级；安装/卸载脚本都在 `browser/native-host/`，宿主 manifest 在 `%LOCALAPPDATA%`。
- B2 才引入动态端口 + 令牌 + 任务投递；B1 不预埋。

## 6. 未做 / 下一步

- Chrome 手动加载、Firefox/Opera 扩展：未测。
- 动态端口/令牌、下载投递、页面挂钩：B2。
- B2 的宿主/监听可以是同一个进程；B1 先只走 Native Messaging stdio。
