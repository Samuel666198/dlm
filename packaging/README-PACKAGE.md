# dlm Windows x64 便携包

这个包包含：

- `dlm.exe`：下载内核 + CLI（分段、续传、HLS、校验、完成后动作）
- `dlm-gui.exe`：本地 Web UI + 任务队列（只监听 `127.0.0.1`）
- `dlm-native-host.exe`：浏览器 Native Messaging 宿主
- `browser/extension/`：Chrome/Edge MV3 扩展（右键/页面按钮/媒体嗅探）
- `install.ps1` / `uninstall.ps1`：用户级安装/卸载（HKCU，不需要管理员）

## 安装

1. 解压 zip 到任意目录。
2. 在该目录打开 PowerShell：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

脚本会把三个 exe 复制到 `%LOCALAPPDATA%\dlm\bin`，扩展复制到
`%LOCALAPPDATA%\dlm\extension`，写入 Chrome/Edge 的 Native Messaging 注册表，
并创建开始菜单快捷方式。

3. 打开 `chrome://extensions` 或 `edge://extensions`，开启“开发者模式”，
   “加载已解压的扩展程序”选择 `%LOCALAPPDATA%\dlm\extension`。
4. 从开始菜单启动 dlm，或在 PowerShell 运行：

```powershell
%LOCALAPPDATA%\dlm\bin\dlm-gui.exe --open
```

## 使用

- 浏览器右键链接/图片/音视频：Download with dlm
- 页面右下角 `dlm`：投递当前页
- 页面右下角 `媒体 N`：查看嗅探到的 HLS/媒体直链，逐条下载
- 本地队列：`dlm-gui` 页面添加 URL，可设配额、分类目录、完成后动作

## 数据位置

| 内容 | 位置 |
|---|---|
| 队列/端点/日志 | `%LOCALAPPDATA%\dlm` |
| 浏览器宿主任务日志 | `%LOCALAPPDATA%\dlm\native-host\jobs` |
| 下载文件 | 默认 `%USERPROFILE%\Downloads`，可用 `--out-dir` 修改 |

## 卸载

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
```

删除注册、开始菜单快捷方式、已安装的 exe/扩展；**保留**队列/日志/下载文件。

## 说明

- 这是 Windows x64 MSVC release 构建；不包含 Electron/Node 运行时。
- 扩展需要手动“加载已解压”，Chrome/Edge 不允许普通用户静默 sideload。
- HLS 直链由扩展嗅探后走 `dlm --hls`；DASH/DRM 暂不支持。
- 原生 GUI 目前是 Web UI 的 app 模式窗口（`--open`），不是原生控件。
