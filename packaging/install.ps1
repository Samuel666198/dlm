$ErrorActionPreference = 'Stop'

# -Autostart: 可选，默认关。打开后把 dlm-gui --autostart 写进 HKCU 的 Run 项，
# 开机就有一个队列在后台跑（不弹浏览器窗口）。不传这个开关 = 不写任何自启项。
param([switch]$Autostart)

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
    # 不传 `--open`：原生窗口就是界面。传了会额外用 Edge/Chrome 开一个 app 模式窗口，
    # 而那个窗口现在只剩一张「dlm 现在是原生窗口」的说明页 —— 双击图标弹两个窗口，
    # 第一个测试版本不该是这个体验。
    $lnk.Arguments = ''
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
if ($Autostart) {
    # 与 `dlm-gui --autostart-install` 写的是同一个值名（dlm-gui）；
    # 路径含空格时整体加引号，否则 Windows 会把路径拆成两段。
    $GuiExe = Join-Path $Bin 'dlm-gui.exe'
    $Cmd = if ($GuiExe -like '* *') { '"{0}" --autostart' -f $GuiExe } else { '{0} --autostart' -f $GuiExe }
    $RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    New-Item -Path $RunKey -Force | Out-Null
    New-ItemProperty -Path $RunKey -Name 'dlm-gui' -Value $Cmd -PropertyType String -Force | Out-Null
    Write-Output "autostart=$Cmd"
}
else {
    Write-Output 'autostart=off (pass -Autostart to enable)'
}
Write-Output ''
Write-Output 'Next steps:'
Write-Output '1. Open chrome://extensions (or edge://extensions), enable Developer mode.'
Write-Output "2. Load unpacked: $Ext"
Write-Output '3. Start dlm from the Start Menu, or run:'
Write-Output "   $Bin\dlm-gui.exe"
Write-Output '   (the app is the native window; closing it keeps dlm running in the tray)'
