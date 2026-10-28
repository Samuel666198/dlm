$ErrorActionPreference = 'Stop'

$ExtId = 'fkebhmogekekphpcggaclfgpfmofhdjk'
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$Bin = Join-Path $env:LOCALAPPDATA 'dlm\bin'
$Ext = Join-Path $env:LOCALAPPDATA 'dlm\extension'
$HostDir = Join-Path $env:LOCALAPPDATA 'dlm\native-host'
$Manifest = Join-Path $HostDir 'com.dlm.host.json'
$HostExe = Join-Path $Bin 'dlm-native-host.exe'

foreach ($name in @('dlm.exe', 'dlm-gui.exe', 'dlm-native-host.exe')) {
    $src = Join-Path $Root $name
    if (-not (Test-Path $src)) { throw "Package is incomplete: missing $name" }
}
if (-not (Test-Path (Join-Path $Root 'browser\extension\manifest.json'))) {
    throw 'Package is incomplete: missing browser\extension'
}

New-Item -ItemType Directory -Force -Path $Bin | Out-Null
New-Item -ItemType Directory -Force -Path $Ext | Out-Null
New-Item -ItemType Directory -Force -Path $HostDir | Out-Null

foreach ($name in @('dlm.exe', 'dlm-gui.exe', 'dlm-native-host.exe')) {
    try {
        Copy-Item (Join-Path $Root $name) (Join-Path $Bin $name) -Force
    } catch {
        throw "Could not copy $name. Close dlm-gui / dlm / dlm-native-host and run install.ps1 again. $($_.Exception.Message)"
    }
}
Copy-Item (Join-Path $Root 'browser\extension\*') $Ext -Recurse -Force

$obj = [ordered]@{
    name = 'com.dlm.host'
    description = 'dlm native messaging host'
    path = $HostExe
    type = 'stdio'
    allowed_origins = @("chrome-extension://$ExtId/")
}
$json = $obj | ConvertTo-Json -Depth 3
[System.IO.File]::WriteAllText($Manifest, $json, (New-Object System.Text.UTF8Encoding($false)))

foreach ($browser in @('Google\Chrome', 'Microsoft\Edge')) {
    $parent = "HKCU:\Software\$browser\NativeMessagingHosts"
    $key = Join-Path $parent 'com.dlm.host'
    New-Item -Path $parent -Force | Out-Null
    New-Item -Path $key -Force | Out-Null
    Set-Item -Path $key -Value $Manifest
}

$Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\dlm.lnk'
try {
    $ws = New-Object -ComObject WScript.Shell
    $lnk = $ws.CreateShortcut($Shortcut)
    $lnk.TargetPath = Join-Path $Bin 'dlm-gui.exe'
    $lnk.Arguments = '--open'
    $lnk.WorkingDirectory = $Bin
    $lnk.Description = 'dlm download manager'
    $lnk.Save()
} catch {
    Write-Warning "Could not create Start Menu shortcut: $($_.Exception.Message)"
}

Write-Output "manifest=$Manifest"
Write-Output "host=$HostExe"
Write-Output "extension=$Ext"
Write-Output "extension_id=$ExtId"
Write-Output ''
Write-Output 'Next steps:'
Write-Output '1. Open chrome://extensions (or edge://extensions), enable Developer mode.'
Write-Output "2. Load unpacked: $Ext"
Write-Output '3. Start dlm from the Start Menu, or run:'
Write-Output "   $Bin\dlm-gui.exe --open"
