# 打 `dlm-mcp` 的单独发布包。
#
# ## 为什么要有这个脚本
#
# 0.6.0 和 0.7.0 的 `dlm-mcp-<release>-win-x64.zip` 是**手工**打出来的，没有脚本。
# 后果在 2026-11-18 暴露：0.7.1 / 0.7.2 发布时都忘了带 MCP 包，于是组件页
# 「下载并安装」直接 404 —— 而 `mcp_asset_url` 只认 `dlm-mcp-<release>-win-x64.zip`
# 这一个名字，没有任何退路。
#
# 打包主包的 `build-package.ps1` 不管 MCP（主包里本来就不含它），所以这一份
# 必须单独跑。把它做成脚本，至少不会因为"忘了"而漏。
#
# 用法：
#   powershell -File packaging/build-mcp-package.ps1
#   powershell -File packaging/build-mcp-package.ps1 -Release 0.7.0   # 补旧版
#
# 产出：`target/package/dlm-mcp-<release>-win-x64.zip`
#
# ## 为什么文件名用发行版号（不是内核版本）
#
# `mcp_asset_url` 是由**发行版号**拼直链的：
#   https://github.com/<repo>/releases/download/v<release>/dlm-mcp-<release>-win-x64.zip
# 所以这里的 `<release>` 必须与发布 tag 一致，否则界面拼出来的地址仍然 404。

param(
    # 发行版版本。缺省取 `gui::RELEASE`（与 build-package.ps1 同一套规则）。
    [string]$Release = ''
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

# 发行版号以代码里的常量为准：它同时也是界面拼直链时用的那一个。
$source = Get-Content -Raw (Join-Path $Root 'src\gui\mod.rs')
$m = [regex]::Match($source, 'pub const RELEASE:\s*&str\s*=\s*"([^"]+)"')
if (-not $m.Success) { throw '找不到 gui::RELEASE（src\gui\mod.rs）' }
if (-not $Release) { $Release = $m.Groups[1].Value }

# 只做发布件时允许补旧版，但必须提醒：二进制是**现在**这份源码编译的。
if ($Release -ne $m.Groups[1].Value) {
    Write-Warning "发行版号 $Release 与代码里的 gui::RELEASE $($m.Groups[1].Value) 不一致。"
    Write-Warning '编出来的 exe 会自报当前源码的版本 —— 补旧版请先 checkout 对应 tag 再打。'
}

$Out = Join-Path $Root 'target\package'
$Stage = Join-Path $Out "dlm-mcp-$Release-win-x64"
$Zip = Join-Path $Out "dlm-mcp-$Release-win-x64.zip"

if (Test-Path $Stage) { Remove-Item -Path $Stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Push-Location $Root
try {
    & cargo build --release --bin dlm-mcp
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release --bin dlm-mcp failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}

$exe = Join-Path $Root 'target\release\dlm-mcp.exe'
if (-not (Test-Path $exe)) { throw "Missing release binary: $exe" }
Copy-Item $exe (Join-Path $Stage 'dlm-mcp.exe') -Force

# README 由脚本生成：版本号写死两次迟早不一致（0.7.0 的 README 里就是手抄的）。
$readme = @"
Download Manager MCP 桥接 $Release —— 让 Agent（Codex / Claude 等）往 dlm 里加下载

这是**可选组件**，主包里没有它。它只通过本地 HTTP 跟 Download Manager 说话，
不碰队列文件；不装它完全不影响其余功能。

## 要求

- 已装 Download Manager $Release 或更新版本（界面要支持 Agent 接口 api >= 2）
- Download Manager 正在运行（它写出的端点文件就是本程序的入口）

## 怎么接进来

把这一段加进 MCP 客户端配置（不需要填 token，它会自己找
%LOCALAPPDATA%\dlm\gui\endpoint.json）：

    {
      "mcpServers": {
        "dlm": { "command": "C:\\Tool\\dlm\\bin\\dlm-mcp.exe" }
      }
    }

**`command` 必须是这个 exe 的绝对路径。** MCP 客户端是用
`Command::new(command)` 起进程的，裸文件名（`dlm-mcp.exe`）只在它恰好位于 PATH
上时才找得到 —— 否则客户端起不动它，界面会报
「Host 尚未注册该 stdio Server 的工具，请检查启动日志后重试」。
不知道装在哪就右键这个 exe 看属性；或者干脆用 设置 → Agent 接入 里的
「复制 MCP 配置」，那里填的就是本机绝对路径。

## 四个工具

| 工具 | 干什么 |
|---|---|
| dlm_add | 加一个下载；开着「Agent 下发要确认」时返回票据 id |
| dlm_ticket | 查票据状态：pending_approval / approved / rejected / expired |
| dlm_list | 列队列（名字、状态、进度） |
| dlm_status | 配额与限速现状 |

默认要求人工确认：Agent 只能拿到票据，要你回到窗口点「允许」才真的下载。
这个开关在 设置 → Agent 接入。

## 查版本

    dlm-mcp.exe --version

## 版本不对时

装错版本（界面太旧）会在第一次调用时直接告诉你「请把 Download Manager 升到 $Release」，
不会静默不工作。
"@
# 无 BOM 的 UTF-8：记事本以外的东西（以及某些 MCP 客户端）读带 BOM 的文本会看到乱码。
[System.IO.File]::WriteAllText(
    (Join-Path $Stage 'README.txt'),
    ($readme -replace "`r`n", "`n"),
    (New-Object System.Text.UTF8Encoding($false))
)

if (Test-Path $Zip) { Remove-Item -Path $Zip -Force }
Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath $Zip -CompressionLevel Optimal

$hash = Get-FileHash -Algorithm SHA256 -Path $Zip
$size = (Get-Item $Zip).Length

# 自检：包里必须真有一个能跑起来的 exe，且它自报的版本与文件名一致。
# 这一步是这次事故的直接教训 —— 名字对了、包里是空的，比 404 更难发现。
$check = Join-Path $env:TEMP "dlm-mcp-verify-$PID"
if (Test-Path $check) { Remove-Item $check -Recurse -Force }
Expand-Archive -Path $Zip -DestinationPath $check -Force
$exeInZip = Get-ChildItem $check -Recurse -Filter 'dlm-mcp.exe' | Select-Object -First 1
if (-not $exeInZip) { throw "包里没有 dlm-mcp.exe：$Zip" }
$reported = (& $exeInZip.FullName --version 2>&1 | Select-Object -First 1)
Remove-Item $check -Recurse -Force

Write-Output "release=$Release"
Write-Output "stage=$Stage"
Write-Output "zip=$Zip"
Write-Output "bytes=$size"
Write-Output "sha256=$($hash.Hash.ToLower())"
Write-Output "probe=$reported"

if ($reported -notmatch [regex]::Escape($Release)) {
    throw "包里的 exe 自报版本与 $Release 不一致：$reported"
}
