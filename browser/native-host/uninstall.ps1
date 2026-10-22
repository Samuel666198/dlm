$ErrorActionPreference = 'Continue'
foreach ($browser in @('Google\Chrome', 'Microsoft\Edge')) {
    $key = "HKCU:\Software\$browser\NativeMessagingHosts\com.dlm.host"
    if (Test-Path $key) { Remove-Item -Path $key -Recurse -Force }
}
$dest = Join-Path $env:LOCALAPPDATA 'dlm\native-host'
if (Test-Path $dest) { Remove-Item -Path $dest -Recurse -Force }
Write-Output 'removed com.dlm.host registration and %LOCALAPPDATA%\dlm\native-host'
