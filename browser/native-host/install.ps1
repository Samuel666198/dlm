$ErrorActionPreference = 'Stop'
$extId = 'fkebhmogekekphpcggaclfgpfmofhdjk'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$exe = Join-Path $root 'target\debug\dlm-native-host.exe'
if (-not (Test-Path $exe)) {
    throw "Build the host first: cargo build --bin dlm-native-host (missing $exe)"
}

$dest = Join-Path $env:LOCALAPPDATA 'dlm\native-host'
New-Item -ItemType Directory -Force -Path $dest | Out-Null
$manifest = Join-Path $dest 'com.dlm.host.json'
$obj = [ordered]@{
    name = 'com.dlm.host'
    description = 'dlm native messaging host (B1 ping/pong)'
    path = $exe
    type = 'stdio'
    allowed_origins = @("chrome-extension://$extId/")
}
$json = $obj | ConvertTo-Json -Depth 3
[System.IO.File]::WriteAllText($manifest, $json, (New-Object System.Text.UTF8Encoding($false)))

foreach ($browser in @('Google\Chrome', 'Microsoft\Edge')) {
    $parent = "HKCU:\Software\$browser\NativeMessagingHosts"
    $key = Join-Path $parent 'com.dlm.host'
    New-Item -Path $parent -Force | Out-Null
    New-Item -Path $key -Force | Out-Null
    Set-Item -Path $key -Value $manifest
}

Write-Output "manifest=$manifest"
Write-Output "host=$exe"
Write-Output "extension_id=$extId"
