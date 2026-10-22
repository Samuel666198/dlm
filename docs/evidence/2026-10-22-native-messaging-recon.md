# Native Messaging 本机取证（B0，2026-10-22）

**范围**：只读取证。读 `idm-recon` 既有文档、IDM 安装目录、注册表、Chrome/Edge 已安装扩展；IDM 启动两次最小化实例量端口，量完停止。**未写 `idm-recon`**；未向 IDM 本地监听发送任何未知请求；未启动浏览器。
**上游**：[`../../../idm-recon/docs/20-browser-integration.md`](../../../idm-recon/docs/20-browser-integration.md)（下称 `20`）与 `50` 能力规格。
**结论先行**：浏览器侧注册/宿主/manifest/扩展都已量到；IDM 本地监听端口是**动态**的，属主是 `IDMIntegrator64.exe`，不是 `1001`。Q-B1 操作码语义仍未取证，但不阻塞自建协议。

## 1. 注册表（只读）

| 浏览器 | 键 | 默认值 |
|---|---|---|
| Chrome（HKCU） | `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.tonec.idm` | `C:\Program Files (x86)\Internet Download Manager\IDMMsgHost.json` |
| Chrome（HKLM） | `HKLM\Software\Google\Chrome\NativeMessagingHosts\com.tonec.idm` | 同上 |
| Edge（HKCU） | `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.tonec.idm` | 同上 |
| Edge（HKLM） | 同上 | **不存在** |
| Firefox（HKCU） | `HKCU\Software\Mozilla\NativeMessagingHosts\com.tonec.idm` | `...\IDMMsgHostMoz.json` |

命令：`powershell.exe -NoProfile -Command` + `(Get-Item $p).GetValue('')`（只读）。`reg query` 在 Git Bash 下受斜杠改写影响，未采用。

## 2. 宿主 manifest（安装目录原文）

`IDMMsgHost.json`：

```json
{
    "name"               : "com.tonec.idm",
    "description"        : "Internet Download Manager extension helper",
    "type"               : "stdio",
    "allowed_origins"    : [ "chrome-extension://ngpampappnmepgilojfohadhhmbhlaek/", "chrome-extension://llbjbkhnmlidjebalopleeepgdfgcpec/" ],
    "path"               : "IDMMsgHost.exe"
}
```

`IDMMsgHostMoz.json`：

```json
{
    "name"               : "com.tonec.idm",
    "description"        : "Internet Download Manager extension helper",
    "type"               : "stdio",
    "allowed_extensions" : [ "mozilla_cc3@internetdownloadmanager.com" ],
    "path"               : "IDMMsgHost.exe"
}
```

- Chrome 与 Edge 共用同一份 Chrome 形态 manifest；`allowed_origins` 同时列 Chrome/Edge 两个扩展 ID。
- `path` 是**相对路径**，相对 manifest 所在目录（`C:\Program Files (x86)\Internet Download Manager\`）。
- `type: "stdio"` ⇒ 浏览器用 stdio 与宿主通信；具体的 4 字节长度帧是浏览器契约，B0 未在 IDM 宿主上主动发消息验证。

## 3. 二进制版本

| 文件 | Product | ProductVersion | FileVersion | Size |
|---|---|---|---|---|
| `IDMMsgHost.exe` | Internet Download Manager | 6.38.20.0 | 6.38.20.9 | 39,168 B |
| `IDMan.exe` | Internet Download Manager (IDM) | 6.42.42.2 | 6.42.42.2 | 6,036,992 B |
| `IDMan.exe.BAK` | Internet Download Manager (IDM) | 6.42.42.2 | 6.42.42.2 | 6,047,568 B |
| `IDMIntegrator64.exe` | Internet Download Manager | **6.41.23.1** | 6.41.23.4 | 27,864 B |

宿主版本比主程序旧；`IDMIntegrator64.exe` 是本地监听的属主（见 §5）。

## 4. 已安装扩展（现役，不是安装目录里的 CRX）

| 浏览器 | 扩展目录 | 版本 | manifest | 关键事实 |
|---|---|---|---|---|
| Chrome | `...\Chrome\User Data\Default\Extensions\ngpampappnmepgilojfohadhhmbhlaek\6.42.64_0` | **6.42.64** | **MV3**，service_worker | `nativeMessaging` 权限；`connectNative("com.tonec.idm")`；1500 ms 退避；`postMessage` 操作码 14–19 |
| Edge | `...\Edge\User Data\Default\Extensions\llbjbkhnmlidjebalopleeepgdfgcpec\6.43.1_0` | **6.43.1** | **MV2**，persistent background | 同一 `connectNative`；操作码 14–18（该版本未见 19） |

- 安装目录里的 `IDMGCExt.crx` 是 6.42.42、`IDMEdgeExt.crx` 是 6.42.18.3 —— **比浏览器里现役的旧**；B0 以已安装目录为准。
- Chrome manifest 的 `web_accessible_resources` 含 `document.js`、`captured.js`、`captured.html`；Edge MV2 只列 `captured.html`（与 `20` 的结论一致）。
- Firefox 扩展在 `%APPDATA%\Mozilla\Firefox\Profiles` 下没找到；安装目录有 `idmmzcc3.xpi`（6.42.22）。Firefox 宿主 manifest 的 `allowed_extensions` 已量到，但现役安装未验证。

## 5. 本地监听（关键新事实）

两次启动 `IDMan.exe`（最小化），5 秒后量 `Get-NetTCPConnection -State Listen`，再停止本次启动的进程：

| 启动 | 监听 | 属主 PID / 进程 | 备注 |
|---|---|---|---|
| 1 | `127.0.0.1:51122` | 41376 / `IDMIntegrator64.exe` | `IDMan.exe` PID 7928；`IDMMsgHost.exe` 未启动 |
| 2 | `127.0.0.1:50627` | 20752 / `IDMIntegrator64.exe` | `IDMan.exe` PID 26676；`IDMMsgHost.exe` 未启动 |

- **端口是动态的**（两次不同），不是固定 `1001`；`1001` 只出现在扩展常量 `La=["127.0.0.1:1001","0.1.0.1:1001"]` 里，是候选/回退，不是本机实测监听。
- 监听属主是 `IDMIntegrator64.exe`，不是 `IDMan.exe`；`IDMMsgHost.exe` 只有浏览器 `connectNative` 时才启动。
- **推断（未取证）**：动态端口由 native 宿主演讲给扩展（扩展不能凭常量知道 51122/50627）。具体是哪条消息、哪个字段，仍属 Q-B1。
- 本轮**没有**向监听发送任何 HTTP 请求；`POST /open/1` 的帧语义仍未取证。

## 6. 对 B1 的约束

1. **自建协议不要复刻 IDM 的 opcode 14–19/字段号**；B1 用可读 JSON（如 `{"op":"ping"}`）。
2. 本地监听用**动态回环端口 + 随机令牌**，端口/令牌通过我们自己的 Native Messaging 通道交给扩展；不要硬编码 1001。
3. 宿主 manifest 按浏览器分：Chrome/Edge 用 `allowed_origins`，Firefox 用 `allowed_extensions`；`type: "stdio"`，`path` 稳定。
4. 宿主与监听可以是同一进程，也可以像 IDM 那样拆分（`IDMMsgHost` + `IDMIntegrator64`）；B1 先做单进程最小闭环。
5. 版本/来源校验：浏览器侧由 manifest 的 allowed origins 管；宿主侧是否再校验调用方，本机未取证。B1 的宿主至少校验扩展 ID + 令牌。
6. Chrome 侧先做 MV3（现役 6.42.64 是 MV3）；Edge 也支持 MV3，B1 不必跟 IDM 的 Edge MV2 分叉。

## 7. 未取证 / 不阻塞

- **Q-B1**：操作码 14–19 与字段号 6/17/31/50/133/134 的语义 —— 仍未取证。只影响"兼容 IDM 私有协议"，不影响自建协议。
- **Q-B2 的协议帧**：端口已量到（动态、属主 integrator），但 `POST /open/1` 的报文/认证未取证；自建监听不需要它。
- 宿主侧 origin 校验、Native Messaging 4 字节帧的活体行为：B1 用我们自己的 host + 扩展实测。
- Firefox 现役扩展安装：未找到；B1 先 Chrome/Edge。

## 8. 安全与可复现

- IDM 启动两次，均为 `Start-Process -WindowStyle Minimized`；量完按本次启动时间筛选 PID，`Stop-Process -Force` 停止本轮的 `IDMan`/`IDMIntegrator64`；当前 `tasklist` 两者均为空。
- 未改注册表、未写安装目录、未写 `idm-recon`、未启动浏览器、未向监听发请求。
- 复现命令（只读）：
  - 注册表：`(Get-Item 'HKCU:\...\com.tonec.idm').GetValue('')`
  - 版本：`(Get-Item '...\IDMMsgHost.exe').VersionInfo`
  - 端口：启动 `IDMan.exe` 后 `Get-NetTCPConnection -State Listen | Where OwningProcess -in $pids`
  - 扩展：读 `...\Extensions\<id>\<ver>\manifest.json` 与 `background.js`
