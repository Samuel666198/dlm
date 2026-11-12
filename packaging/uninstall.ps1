$ErrorActionPreference = 'Continue'

foreach ($browser in @('Google\Chrome', 'Microsoft\Edge')) {
    $key = "HKCU:\Software\$browser\NativeMessagingHosts\com.dlm.host"
    if (Test-Path $key) { Remove-Item -Path $key -Recurse -Force }
}

# 开机自启（P3）：按值名删，不碰这个键下别人的值。
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Test-Path $RunKey) {
    $existing = Get-ItemProperty -Path $RunKey -Name 'dlm-gui' -ErrorAction SilentlyContinue
    if ($existing) {
        Remove-ItemProperty -Path $RunKey -Name 'dlm-gui' -Force -ErrorAction SilentlyContinue
        Write-Output 'Removed autostart entry (HKCU Run\dlm-gui).'
    }
}

$Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\dlm.lnk'
if (Test-Path $Shortcut) { Remove-Item -Path $Shortcut -Force }

$Bin = Join-Path $env:LOCALAPPDATA 'dlm\bin'
$Ext = Join-Path $env:LOCALAPPDATA 'dlm\extension'
$HostDir = Join-Path $env:LOCALAPPDATA 'dlm\native-host'
foreach ($path in @($Bin, $Ext, $HostDir)) {
    if (Test-Path $path) {
        try { Remove-Item -Path $path -Recurse -Force } catch { Write-Warning "Could not remove $path" }
    }
}

Write-Output 'Removed dlm native messaging registration, Start Menu shortcut, installed binaries and extension.'
Write-Output 'Kept queue, logs and download data under %LOCALAPPDATA%\dlm and your download folder.'
