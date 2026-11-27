; EcConn Windows Installer Script (Inno Setup 6)
; Bundles unified EcConn.exe (Host + Receiver) and server.exe (DXGI + Virtual Display)

#define MyAppName "EcConn"
#define MyAppVersion "1.0.0"
#define MyAppPublisher "EcConn"
#define MyAppExeName "EcConn.exe"

[Setup]
AppId={{8F3B9B7A-4E1C-4E8B-9C4D-1A2B3C4D5E6F}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=..\..\dist
OutputBaseFilename=EcConn-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "firewall"; Description: "Open Windows Firewall UDP ports (9000, 9100, 9200, 9201)"; GroupDescription: "Network & Driver:"
Name: "vdd"; Description: "Install Virtual Display Driver (for wireless extended monitor)"; GroupDescription: "Network & Driver:"

[Files]
Source: "..\..\dist\EcConn-Windows-Portable\EcConn.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\EcConn-Windows-Portable\server.exe"; DestDir: "{app}"; Flags: ignoreversion
; Bundle ffmpeg.exe if present in the portable staging directory
Source: "..\..\dist\EcConn-Windows-Portable\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "netsh"; Parameters: "advfirewall firewall add rule name=""EcConn"" dir=in action=allow protocol=UDP localport=9000,9100,9200,9201"; Flags: runhidden waituntilterminated; Tasks: firewall
Filename: "{app}\server.exe"; Parameters: "--install-driver"; Flags: runhidden waituntilterminated; Tasks: vdd
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
