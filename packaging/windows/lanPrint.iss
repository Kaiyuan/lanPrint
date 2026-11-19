; Inno Setup script for lanPrint (Windows 7+)
; Build: iscc /DMyAppVersion=2.0.0 packaging\windows\lanPrint.iss
#ifndef MyAppVersion
  #define MyAppVersion "2.0.0"
#endif
#ifndef MySource
  #define MySource "..\..\target\release\lanPrint.exe"
#endif

[Setup]
AppId={{8B6C7F42-2F54-4E95-9E21-4D2B5E8A6F11}
AppName=lanPrint
AppVersion={#MyAppVersion}
AppPublisher=Kaiyuan
AppPublisherURL=https://github.com/Kaiyuan/lanPrint
AppSupportURL=https://github.com/Kaiyuan/lanPrint/issues
AppUpdatesURL=https://github.com/Kaiyuan/lanPrint/releases
DefaultDirName={autopf}\lanPrint
DefaultGroupName=lanPrint
DisableProgramGroupPage=yes
OutputBaseFilename=lanPrint-{#MyAppVersion}-setup
OutputDir=..\..\dist
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=6.1sp1
; Admin rights are required to install into Program Files and to add the
; inbound firewall rule. Users can still pick a per-user install in the dialog.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
UninstallDisplayIcon={app}\lanPrint.exe
SetupIconFile=..\..\crates\desktop\ui\assets\lanprint-icon.ico
LicenseFile=..\..\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "Start lanPrint with Windows"; GroupDescription: "Startup:"

[Files]
Source: "{#MySource}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README_EN.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\lanPrint"; Filename: "{app}\lanPrint.exe"
Name: "{group}\Uninstall lanPrint"; Filename: "{uninstallexe}"
Name: "{autodesktop}\lanPrint"; Filename: "{app}\lanPrint.exe"; Tasks: desktopicon

[Registry]
; HKA resolves to HKLM in the default all-users install and to HKCU when the
; user chooses a per-user install.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "lanPrint"; ValueData: """{app}\lanPrint.exe"" --tray"; Flags: uninsdeletevalue; Tasks: autostart

[Run]
; Allow inbound connections from clients and mDNS discovery. The rule is
; tied to lanPrint.exe so it keeps working after updates.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""lanPrint"""; Flags: runhidden; StatusMsg: "Updating the Windows Firewall rule..."
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""lanPrint"" dir=in action=allow program=""{app}\lanPrint.exe"" enable=yes profile=any"; Flags: runhidden; StatusMsg: "Adding the Windows Firewall rule..."
Filename: "{app}\lanPrint.exe"; Description: "{cm:LaunchProgram,lanPrint}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""lanPrint"""; Flags: runhidden; RunOnceId: "RemoveFirewallRule"
Filename: "{cmd}"; Parameters: "/C taskkill /IM lanPrint.exe /F >NUL 2>&1"; Flags: runhidden; RunOnceId: "StopLanPrint"
