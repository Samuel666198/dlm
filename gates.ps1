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
    # UI 脚本语法：`include_str!` 嵌进二进制的页面写错了，Rust 四闸看不出来，只会白屏。
    Invoke-Gate -Name 'ui-script' -Note 'check-ui-script.mjs' -Body { & $Node browser/test/check-ui-script.mjs }
}
else {
    $script:Results += [pscustomobject]@{ Gate = 'extension'; Result = 'SKIP'; Detail = 'node not found' }
    $script:Results += [pscustomobject]@{ Gate = 'ui-script'; Result = 'SKIP'; Detail = 'node not found' }
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
