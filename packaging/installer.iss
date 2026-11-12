; dlm 的 Windows 安装包（Inno Setup 6）。
;
; 由 packaging\build-installer.ps1 调用，不手敲：
;   ISCC.exe /DAppVersion=0.1.1 /DSourceDir=<暂存目录> /DOutDir=<输出目录> installer.iss
;
; ## 为什么是「当前用户安装」
;
; 装到 %LOCALAPPDATA%\dlm\bin，全程不要管理员权限。理由是**自动更新**：
; dlm-updater.exe 直接替换安装目录里的 exe，它不提权。装到 Program Files 的话，
; 每次自更新都会因为权限失败 —— 那是「装得上、更新不了」的半残状态。
; 装到当前用户目录 = 安装不弹 UAC、以后每次更新也不弹。
;
; ## 为什么不用 [Registry] 里的固定路径写 native host 清单
;
; 清单里要写 `dlm-native-host.exe` 的**绝对路径**，而它随安装目录变；
; 所以清单文件在 [Code] 里按实际 {app} 生成，注册表值用 {app} 展开即可。

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\package\dlm-0.0.0-win-x64"
#endif
#ifndef OutDir
  #define OutDir "..\target\package"
#endif
; 安装包自己的图标（Setup.exe 在资源管理器里显示的那个）。
; 与 exe 内嵌的是同一份 .ico，由 build.rs 编进资源段。
#ifndef IcoFile
  #define IcoFile ""
#endif

#define AppName "dlm"
#define AppPublisher "dlm"
#define ExtId "fkebhmogekekphpcggaclfgpfmofhdjk"
#define HostName "com.dlm.host"

[Setup]
; AppId 一旦定下就不能改：它是「同一个软件」的身份证，改了就会在
; 「应用和功能」里出现两条 dlm，且老版本不会被新安装覆盖。
AppId={{A7C4F1D2-3B9E-4C5A-9D31-7C2E8F10B6A4}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\dlm\bin
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; 当前用户安装 ⇒ 不要管理员（见文件头）
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename=dlm-{#AppVersion}-win-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; 正在跑的 dlm-gui / dlm-native-host 会被 Restart Manager 请求关闭，
; 不重启它们（用户自己开，或下面 [Run] 里问一句）
CloseApplications=yes
RestartApplications=no
UninstallDisplayName={#AppName} {#AppVersion}
UninstallDisplayIcon={app}\dlm-gui.exe
#if FileExists(IcoFile)
SetupIconFile={#IcoFile}
#endif

[Languages]
Name: "chinese"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked
Name: "autostart"; Description: "开机自动启动（托盘常驻）"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "{#SourceDir}\dlm.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\dlm-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\dlm-native-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\dlm-updater.exe"; DestDir: "{app}"; Flags: ignoreversion
; 扩展：不上商店，装到这里，用户在 chrome://extensions 里「加载已解压」
Source: "{#SourceDir}\browser\extension\*"; DestDir: "{localappdata}\dlm\extension"; Flags: ignoreversion recursesubdirs
Source: "{#SourceDir}\browser\README.md"; DestDir: "{localappdata}\dlm\extension"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\START-HERE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\dlm-gui.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\dlm-gui.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
; Chrome / Edge 的 native messaging 宿主登记。指向清单文件，清单里写 exe 的绝对路径。
Root: HKCU; Subkey: "Software\Google\Chrome\NativeMessagingHosts\{#HostName}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#HostName}.json"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Edge\NativeMessagingHosts\{#HostName}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#HostName}.json"; Flags: uninsdeletekey
; 开机自启（可选任务）。值与 dlm-gui --autostart-install 写的一致。
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "dlm-gui"; ValueData: """{app}\dlm-gui.exe"" --autostart"; Flags: uninsdeletevalue; Tasks: autostart

[UninstallDelete]
; [Code] 里生成的文件不会被自动跟踪，卸载要显式删
Type: files; Name: "{app}\{#HostName}.json"
Type: filesandordirs; Name: "{localappdata}\dlm\extension"
Type: dirifempty; Name: "{localappdata}\dlm"

[Run]
Filename: "{app}\dlm-gui.exe"; Description: "启动 dlm"; Flags: nowait postinstall skipifsilent
Filename: "{localappdata}\dlm\extension"; Description: "打开扩展目录（在 chrome://extensions 里『加载已解压』选它）"; Flags: postinstall shellexec skipifsilent unchecked

[Code]
// native messaging 清单：Inno 只能写固定文本，而这里要写「安装目录下的 exe」，
// 所以按实际安装路径生成一份。
procedure WriteNativeHostManifest();
var
  ManifestPath, ExePath, Json: String;
begin
  ManifestPath := ExpandConstant('{app}\{#HostName}.json');
  ExePath := ExpandConstant('{app}\dlm-native-host.exe');
  // 反斜杠要转义成 \\（JSON 字符串），否则浏览器读不懂
  StringChangeEx(ExePath, '\', '\\', True);
  Json :=
    '{' + #13#10 +
    '  "name": "{#HostName}",' + #13#10 +
    '  "description": "dlm native messaging host",' + #13#10 +
    '  "path": "' + ExePath + '",' + #13#10 +
    '  "type": "stdio",' + #13#10 +
    '  "allowed_origins": ["chrome-extension://{#ExtId}/"]' + #13#10 +
    '}' + #13#10;
  if not SaveStringToFile(ManifestPath, Json, False) then
    MsgBox('写入 ' + ManifestPath + ' 失败。扩展将无法连接 dlm。', mbError, MB_OK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    WriteNativeHostManifest();
end;

// 装完给一句「下一步」，因为扩展必须由人手点一次「加载已解压」。
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpFinished then
    WizardForm.FinishedLabel.Caption :=
      'dlm 已安装。' + #13#10#13#10 +
      '还差一步：打开 chrome://extensions（或 edge://extensions），开启「开发者模式」，' +
      '点「加载已解压的扩展程序」，选择：' + #13#10 +
      ExpandConstant('{localappdata}\dlm\extension') + #13#10#13#10 +
      '（已经装过扩展的话，只需在那里点一次「重新加载」。）';
end;

// 卸载前提醒：扩展目录会被删，浏览器里那条「加载已解压」的条目要自己移除。
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    if MsgBox('卸载会删掉已加载的扩展文件（' + ExpandConstant('{localappdata}\dlm\extension') + '）。' +
      '浏览器 chrome://extensions 里的那条 dlm 需要你自己移除。继续吗？',
      mbConfirmation, MB_YESNO) = IDNO then
      Abort;
end;
