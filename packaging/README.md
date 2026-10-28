# 打包（Windows x64）

从仓库根目录运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File packaging/build-package.ps1
```

脚本会：

1. `cargo build --release --bins`；
2. 在 `target/package/dlm-<version>-win-x64/` 组装 release 三件套、`browser/extension`、
   `install.ps1`、`uninstall.ps1`、`START-HERE.txt`、`README-PACKAGE.md`、主 README；
3. 生成 `target/package/dlm-<version>-win-x64.zip`，打印字节数与 sha256。

版本号从 `Cargo.toml` 的 `version` 读取，也可 `-Version 0.1.0` 覆盖。

## 安装器行为

`install.ps1`（用户级，不需要管理员）：

- 复制三个 exe 到 `%LOCALAPPDATA%\dlm\bin`；
- 复制扩展到 `%LOCALAPPDATA%\dlm\extension`；
- 写 `%LOCALAPPDATA%\dlm\native-host\com.dlm.host.json`；
- 注册 HKCU Chrome/Edge 的 Native Messaging 宿主；
- 创建开始菜单快捷方式（`dlm-gui.exe --open`）。

扩展仍需在 `chrome://extensions` / `edge://extensions` 开发者模式里手动“加载已解压”。
`uninstall.ps1` 删除注册、快捷方式、已安装 exe/扩展，不删队列、日志和下载文件。

## 注意

- 打包产物在 `target/` 下，按仓库约定不入库。
- 不要用仓库里的 `browser/native-host/install.ps1`（它是开发树安装器，指向 `target/debug`）；
  包内使用 `packaging/install.ps1`。
- 原生 GUI 目前是 Web UI 的浏览器 app 模式窗口；不是原生控件。
