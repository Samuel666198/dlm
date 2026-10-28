param([string]$Version = '')

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

if (-not $Version) {
    $line = Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $line) { throw 'Could not read version from Cargo.toml' }
    $Version = $line.Matches[0].Groups[1].Value
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

foreach ($name in @('dlm.exe', 'dlm-gui.exe', 'dlm-native-host.exe')) {
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

Write-Output "version=$Version"
Write-Output "stage=$Stage"
Write-Output "zip=$Zip"
Write-Output "bytes=$size"
Write-Output "sha256=$($hash.Hash.ToLower())"
