# dlm gates: run every check with one command.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\gates.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\gates.ps1 -SkipIntegration
#
# Exit code 0 = all green; 1 = at least one gate failed.
param(
    [switch]$SkipIntegration
)

$ErrorActionPreference = 'Continue'
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $Root

# 工具链兜底：这台机器的 `cargo` 不在系统 PATH 上（rustup 的 shim 目录被摘了），
# 而 `node` 可能在另一个位置。两个都按"PATH 里有就用，没有就找工具链目录"解析，
# 否则门禁会以"cargo 不是命令"的形式报红，看起来像代码坏了。
function Resolve-Tool {
    param([string]$Name, [string[]]$Fallbacks)
    $found = Get-Command $Name -ErrorAction SilentlyContinue
    if ($found) { return $found.Source }
    foreach ($path in $Fallbacks) {
        if (Test-Path $path) { return $path }
    }
    return $null
}

$Cargo = Resolve-Tool -Name 'cargo' -Fallbacks @(
    "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe",
    "$env:USERPROFILE\.cargo\bin\cargo.exe",
    "C:\Program Files\Rust\bin\cargo.exe"
)
$Node = Resolve-Tool -Name 'node' -Fallbacks @(
    "C:\Program Files\nodejs\node.exe",
    "$env:LOCALAPPDATA\Programs\nodejs\node.exe",
    "C:\Program Files (x86)\nodejs\node.exe"
)
if ($Cargo) {
    # 让 cargo 自己能找到同目录的 rustc。
    $cargoDir = Split-Path -Parent $Cargo
    if (($env:PATH -split ';') -notcontains $cargoDir) { $env:PATH = "$cargoDir;$env:PATH" }
}

$script:Failed = 0
$script:Results = @()

# 更新检查会在启动 10 秒后请求 raw.githubusercontent.com。门禁里不许有真实外网请求：
# 断网的机器会变红，联网的机器会悄悄依赖网络。子进程继承这个变量 ⇒ 一处设置覆盖全部用例。
$env:DLM_NO_UPDATE_CHECK = '1'

# 「打开落点 / 在资源管理器里选中」这类外壳动作，门禁里一律不真的启动：
# 有一批用例会走到 `/api/reveal`，落点是 `target/harness-scratch/<pid>-<tag>-<n>/dl`
# 这种**用例临时目录**；用例结束就删，屏幕上那个资源管理器窗口随即弹
# 「位置不可用 … 不可用」对话框 —— 每跑一次门禁就在桌面上留一个（2026-11-12 实测）。
# 回执里的程序名/参数仍按真实计划断言，只是 `spawned` 说 false。
$env:DLM_NO_SHELL_OPEN = '1'
# 别让 GUI 的「启动失败」消息框弹到用户桌面上（自动化里没人点它）。
$env:DLM_NO_POPUP = '1'
# 单元用例会真的建一个 `App`（整帧渲染用例）。`App::new` 要建托盘图标，而测试线程
# 没有消息泵：图标会留在任务栏上，点了没反应。建托盘这一支在门禁里直接跳过。
$env:DLM_NO_TRAY = '1'

function Invoke-Gate {
    param(
        [string]$Name,
        [string]$Note,
        [scriptblock]$Body
    )

    Write-Host ''
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    $out = & $Body 2>&1 | ForEach-Object { $_ }
    $code = $LASTEXITCODE
    $out | ForEach-Object { Write-Host $_ }

    $passed = $null
    $sum = 0
    $seen = $false
    foreach ($line in $out) {
        if ($line -match 'test result: ok\. (\d+) passed') {
            $sum += [int]$Matches[1]
            $seen = $true
        }
    }
    if ($seen) { $passed = $sum }

    if ($code -eq 0) {
        $detail = if ($null -ne $passed) { "$passed passed" } else { $Note }
        $script:Results += [pscustomobject]@{ Gate = $Name; Result = 'PASS'; Detail = $detail }
    }
    else {
        $script:Failed++
        $script:Results += [pscustomobject]@{ Gate = $Name; Result = 'FAIL'; Detail = "exit $code" }
    }
}

if (-not $Cargo) {
    Write-Host "GATES FAILED: cargo not found (set PATH or install the Rust toolchain)" -ForegroundColor Red
    Pop-Location
    exit 1
}

Invoke-Gate -Name 'fmt' -Note 'cargo fmt --check' -Body { & $Cargo fmt --check }
Invoke-Gate -Name 'lib' -Note 'cargo test --lib' -Body { & $Cargo test --lib }
Invoke-Gate -Name 'bins' -Note 'cargo test --bins' -Body { & $Cargo test --bins }

if ($SkipIntegration) {
    $script:Results += [pscustomobject]@{ Gate = 'integration'; Result = 'SKIP'; Detail = '-SkipIntegration' }
}
else {
    Invoke-Gate -Name 'integration' -Note 'cargo test --test integration' -Body { & $Cargo test --test integration }
}

Invoke-Gate -Name 'clippy' -Note 'cargo clippy --all-targets -D warnings' -Body { & $Cargo clippy --all-targets -- -D warnings }

if ($Node) {

    Invoke-Gate -Name 'extension' -Note 'check-extension-static.mjs' -Body { & $Node browser/test/check-extension-static.mjs }

    # 扩展 content script 的存活守卫（2026-11-18）。
    # 钉住"扩展重载后旧脚本不许刷 `Extension context invalidated`"这件事 ——
    # 它是纯逻辑，用 vm 沙箱跑，不启动浏览器。
    Invoke-Gate -Name 'ext-guard' -Note 'verify-content-guard.mjs' -Body { & $Node browser/test/verify-content-guard.mjs }

    # 浏览器下载接管的**判定逻辑**（2026-11-27）。
    # 这些规则决定“要不要抢浏览器自己那份下载”，判错的代价是两边都没下成，
    # 而且保守这件事不会自己保持 —— 后来的人很容易为了覆盖更多去放宽某一条件。
    # takeover.js 本身无 chrome 依赖、导出 module.exports，所以直接 require 进来逐个分支打。
    Invoke-Gate -Name 'takeover' -Note 'verify-takeover.mjs' -Body { & $Node browser/test/verify-takeover.mjs }
Invoke-Gate -Name 'ctx-target' -Note 'verify-context-target.mjs' -Body { & $Node browser/test/verify-context-target.mjs }
Invoke-Gate -Name 'video-page' -Note 'verify-video-page.mjs' -Body { & $Node browser/test/verify-video-page.mjs }
Invoke-Gate -Name 'sw-download' -Note 'verify-sw-download.mjs' -Body { & $Node browser/test/verify-sw-download.mjs }
}
else {
    $script:Results += [pscustomobject]@{ Gate = 'extension'; Result = 'SKIP'; Detail = 'node not found' }
    $script:Results += [pscustomobject]@{ Gate = 'ext-guard'; Result = 'SKIP'; Detail = 'node not found' }
    $script:Results += [pscustomobject]@{ Gate = 'takeover'; Result = 'SKIP'; Detail = 'node not found' }
}

# PowerShell 5.1 decodes a .ps1 WITHOUT a UTF-8 BOM using the local code page.
# Non-ASCII text then turns into mojibake, and in the wrong combination the tokenizer
# mis-parses and reports a bogus syntax error in a script nobody edited.
# 2026-11-18: this cost two failed release runs (the packaging scripts would not start).
#
# The rule enforced here: any .ps1 in this repo that contains non-ASCII bytes
# (comments included -- comments alone can trigger it when the file also has LF
# endings) MUST start with a UTF-8 BOM. With a BOM both CRLF and LF are fine,
# so line endings are deliberately NOT part of this check.
Invoke-Gate -Name 'ps-enc' -Note 'PowerShell scripts with non-ASCII keep a UTF-8 BOM' -Body {
    $bad = @()
    $files = @()
    $files += Get-ChildItem -Path $Root -Filter '*.ps1' -File
    $files += Get-ChildItem -Path (Join-Path $Root 'packaging') -Filter '*.ps1' -File -ErrorAction SilentlyContinue
    foreach ($f in $files) {
        $bytes = [System.IO.File]::ReadAllBytes($f.FullName)
        $hasBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191)
        $text = [System.Text.Encoding]::UTF8.GetString($bytes)
        $nonAscii = ($text.ToCharArray() | Where-Object { [int]$_ -gt 127 }).Count -gt 0
        if ($nonAscii -and -not $hasBom) { $bad += $f.Name }
    }
    if ($bad.Count -gt 0) { throw ("missing UTF-8 BOM: " + ($bad -join ', ')) }
}

Write-Host ''
Write-Host '=== summary ===' -ForegroundColor Cyan
$script:Results | Format-Table -AutoSize | Out-String | Write-Host

if ($script:Failed -gt 0) {
    Write-Host "GATES FAILED: $script:Failed" -ForegroundColor Red
    Pop-Location
    exit 1
}
Write-Host 'GATES PASSED' -ForegroundColor Green
Pop-Location
exit 0
