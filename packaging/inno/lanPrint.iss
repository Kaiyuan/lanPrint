#define MyAppName "lanPrint"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "lanPrint"
#define MyAppExeName "lanPrint.exe"

[Setup]
AppId={{8D03C2C2-2ECA-4549-B9F3-6E68F8CD8A37}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64
OutputDir=..\..\dist
OutputBaseFilename=lanPrint-setup-win

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimp"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Files]
Source: "..\..\dist\lanPrint.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\web\*"; DestDir: "{app}\web"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\lanPrint"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall lanPrint"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Run lanPrint"; Flags: nowait postinstall skipifsilent
