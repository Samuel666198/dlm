param(
    [string]$Version = '',
    # 发行版版本：用户感知的「软件版本」。缺省跟内核版本一致。
    [string]$Release = '',
    [string]$Notes = '',
    # 低于这个内核版本的用户不能自动更新（只能整包重装）。缺省 = 本包的内核版本。
    [string]$MinCore = '',
    # 发布地址。缺省按 GitHub Release 约定推到 tag v<Release> 下的本包文件名。
    [string]$Url = ''
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

if (-not $Version) {
    $line = Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $line) { throw 'Could not read version from Cargo.toml' }
    $Version = $line.Matches[0].Groups[1].Value
}
if (-not $Release) { $Release = $Version }
if (-not $MinCore) { $MinCore = $Version }

# 发行版版本必须与代码里的 `gui::RELEASE` 一致。
#
# 为什么卡这一条：从源码装的用户没有 `installed.json`，本机版本就等于 `gui::RELEASE`。
# 如果清单发的是 0.1.1 而代码里还写着 0.1.0，那些人每次启动都会被告知「有新版 0.1.1」，
# 点了更新装完还是 0.1.1 —— 界面在说谎，而且是永远说下去的那种。
$source = Get-Content -Raw (Join-Path $Root 'src\gui\mod.rs')
$m = [regex]::Match($source, 'pub const RELEASE:\s*&str\s*=\s*"([^"]+)"')
if (-not $m.Success) { throw '找不到 gui::RELEASE（src\gui\mod.rs）' }
if ($m.Groups[1].Value -ne $Release) {
    throw "发行版版本不一致：-Release $Release，而 src\gui\mod.rs 里 gui::RELEASE = $($m.Groups[1].Value)。先把常量改掉再打包。"
}
if (-not $Url) {
    $Url = "https://github.com/Samuel666198/dlm/releases/download/v$Release/dlm-$Version-win-x64.zip"
}

$Out = Join-Path $Root 'target\package'
$Stage = Join-Path $Out "dlm-$Version-win-x64"
$Zip = Join-Path $Out "dlm-$Version-win-x64.zip"

if (Test-Path $Stage) { Remove-Item -Path $Stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Push-Location $Root
try {
    & cargo build --release --bins
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

foreach ($name in @('dlm.exe', 'dlm-gui.exe', 'dlm-native-host.exe', 'dlm-updater.exe')) {
    $src = Join-Path $Root "target\release\$name"
    if (-not (Test-Path $src)) { throw "Missing release binary: $src" }
    Copy-Item $src (Join-Path $Stage $name) -Force
}

$Browser = Join-Path $Stage 'browser'
New-Item -ItemType Directory -Force -Path $Browser | Out-Null
Copy-Item (Join-Path $Root 'browser\extension') (Join-Path $Browser 'extension') -Recurse -Force
Copy-Item (Join-Path $Root 'browser\README.md') (Join-Path $Browser 'README.md') -Force

foreach ($name in @('install.ps1', 'uninstall.ps1', 'START-HERE.txt', 'README-PACKAGE.md')) {
    Copy-Item (Join-Path $PSScriptRoot $name) (Join-Path $Stage $name) -Force
}
Copy-Item (Join-Path $Root 'README.md') (Join-Path $Stage 'README.md') -Force

if (Test-Path $Zip) { Remove-Item -Path $Zip -Force }
Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath $Zip -CompressionLevel Optimal
$hash = Get-FileHash -Algorithm SHA256 -Path $Zip
$size = (Get-Item $Zip).Length

# 更新清单跟包一起产出：sha256 与字节数必须来自**刚打出来的这个 zip**，
# 手抄一遍迟早抄错，抄错就等于「下载完校验失败」。
# 清单本身不进 zip（它是给外面的更新器读的），落在 $Out 里等发布时拷到 main。
$manifestPath = Join-Path $Root 'browser\extension\manifest.json'
$extVersion = (Get-Content -Raw $manifestPath | ConvertFrom-Json).version
$publishedAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$latest = [ordered]@{
    release      = $Release
    core         = $Version
    extension    = $extVersion
    url          = $Url
    sha256       = $hash.Hash.ToLower()
    size         = $size
    notes        = $Notes
    min_core     = $MinCore
    published_at = $publishedAt
}
$latestPath = Join-Path $Out 'latest.json'
$latest | ConvertTo-Json -Depth 4 | Set-Content -Path $latestPath -Encoding UTF8

Write-Output "version=$Version"
Write-Output "release=$Release"
Write-Output "extension=$extVersion"
Write-Output "stage=$Stage"
Write-Output "zip=$Zip"
Write-Output "bytes=$size"
Write-Output "sha256=$($hash.Hash.ToLower())"
Write-Output "latest=$latestPath"
