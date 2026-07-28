#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\mezon.exe"
#endif
#ifndef SetupIcon
  #define SetupIcon "..\..\crates\mezon-app\resources\windows\app.ico"
#endif

[Setup]
AppId={{FEE083EA-AAF9-4A10-8988-E2D59DC5D2DA}
AppName=Mezon
AppVersion={#MyAppVersion}
AppPublisher=Mezon
DefaultDirName={localappdata}\Programs\Mezon
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
OutputBaseFilename=mezon-setup-{#MyAppVersion}
SetupIconFile={#SetupIcon}
UninstallDisplayIcon={app}\mezon.exe
Compression=lzma
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Mezon"; Filename: "{app}\mezon.exe"
Name: "{userdesktop}\Mezon"; Filename: "{app}\mezon.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\mezon.exe"; Description: "{cm:LaunchProgram,Mezon}"; Flags: nowait postinstall skipifsilent
