# dlm Native Messaging 宿主（B1）

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

## 手动加载

`chrome://extensions` → 开发者模式 → 加载已解压的扩展 → 选 `browser/extension`。扩展动作徽标应为 `OK`，
任意页面右下角显示 `dlm native: pong com.dlm.host v...`。

## 安全边界

- 宿主只读 stdin、写 stdout 帧；诊断走 stderr；只认 ping/hello，其余回 `error`。
- 没有本地监听、没有下载投递、没有页面挂钩；这些是 B2。
- 扩展权限只有 `nativeMessaging` + `storage`；content script 只显示状态。
