# 打 Windows 安装包（Inno Setup）：产出可双击安装的 dlm-<版本>-win-x64-setup.exe。
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1 -Notes "改了什么"
#
# 它做三件事，顺序不能反：
#   1. 复用 build-package.ps1：编译 4 个 exe、组装暂存目录、产 zip 与 latest.json
#      （zip 是给 dlm-updater.exe 用的更新载荷；setup.exe 是给人双击的）
#   2. 用 Inno Setup 的 ISCC 把暂存目录打成安装包
#   3. 打印 setup.exe 的字节数与 sha256 —— 发布时要抄进说明里的就是这两个数
param(
    [string]$Release = '',
    [string]$Notes = ''
)

$ErrorActionPreference = 'Stop'

# cargo 不一定在 PATH 上（这台机器的 rustup shim 被摘了）⇒ 与 gates.ps1 一样兜底。
# 不兜底的话，构建会以「cargo 不是命令」失败，看起来像代码坏了。
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $toolchain = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
    if (Test-Path (Join-Path $toolchain 'cargo.exe')) { $env:PATH = "$toolchain;$env:PATH" }
    else { throw '找不到 cargo：装 rustup，或把工具链 bin 加进 PATH' }
}

$Root = Split-Path -Parent $PSScriptRoot
$Out = Join-Path $Root 'target\package'

# ISCC 的位置：winget 装的是当前用户目录版本，不保证在 PATH 上。
function Resolve-Iscc {
    $found = Get-Command ISCC -ErrorAction SilentlyContinue
    if ($found) { return $found.Source }
    foreach ($p in @(
            (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
            'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
            'C:\Program Files\Inno Setup 6\ISCC.exe')) {
        if (Test-Path $p) { return $p }
    }
    throw '找不到 ISCC.exe。装一次 Inno Setup：winget install --id JRSoftware.InnoSetup'
}

# 1. 先按原流程打包（会顺带校验 gui::RELEASE 与 -Release 一致）
$pkgArgs = @()
if ($Release) { $pkgArgs += @('-Release', $Release) }
if ($Notes) { $pkgArgs += @('-Notes', $Notes) }
# `2>&1` 把 cargo 的 stderr 也接进来；若不临时放宽 EAP，PS 5.1 会把每一行原生命令的
# stderr 当成终止错误（.NativeCommandError），构建明明成功也会中断。
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$pkgOut = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'build-package.ps1') @pkgArgs 2>&1
$pkgExit = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($pkgExit -ne 0) { $pkgOut | ForEach-Object { Write-Output $_ }; throw "build-package.ps1 失败（exit $pkgExit）" }

# 从打包输出里读回版本号与暂存目录（不重新推导一遍，否则两处迟早对不上）
$version = ($pkgOut | Select-String -Pattern '^version=(.+)$').Matches.Groups[1].Value
$releaseVer = ($pkgOut | Select-String -Pattern '^release=(.+)$').Matches.Groups[1].Value
$stage = ($pkgOut | Select-String -Pattern '^stage=(.+)$').Matches.Groups[1].Value
$zipPath = ($pkgOut | Select-String -Pattern '^zip=(.+)$').Matches.Groups[1].Value
if (-not $version -or -not $stage) { $pkgOut | ForEach-Object { Write-Output $_ }; throw '从打包输出里读不到 version/stage' }

$iscc = Resolve-Iscc
Write-Output "iscc=$iscc"
Write-Output "version=$version release=$releaseVer stage=$stage"

# 2. 编译安装包
$ico = Join-Path $Root 'assets\icon\dlm.ico'
& $iscc "/DAppVersion=$releaseVer" "/DSourceDir=$stage" "/DOutDir=$Out" "/DIcoFile=$ico" (Join-Path $PSScriptRoot 'installer.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC 失败（exit $LASTEXITCODE）" }

# 3. 报数
$setup = Join-Path $Out "dlm-$releaseVer-win-x64-setup.exe"
if (-not (Test-Path $setup)) { throw "没有产出 $setup" }
$size = (Get-Item $setup).Length
$hash = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash.ToLower()
Write-Output "setup=$setup"
Write-Output "setup_bytes=$size"
Write-Output "setup_sha256=$hash"
Write-Output "zip=$zipPath"
Write-Output "zip_bytes=$((Get-Item $zipPath).Length)"
Write-Output "zip_sha256=$((Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLower())"
