; Inno Setup script. Build: iscc /DAppVersion=0.1.0 packaging\windows\intelli.iss   (input: dist\windows\, output: dist\installer\)
; NOT RUN YET: written on Linux, never executed on Windows.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B0A3A2C4-7E41-4F0B-9B6E-3D5C8E2F7A11}
AppName=Intelli
AppVersion={#AppVersion}
AppPublisher=Rahul Parimal
AppPublisherURL=https://github.com/rahulparimal/Intelligent-Browser-fast
DefaultDirName={autopf}\Intelli
DefaultGroupName=Intelli
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\..\dist\installer
OutputBaseFilename=Intelli-Rust-{#AppVersion}-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\intelli-search.exe
ArchitecturesInstallIn64BitMode=x64compatible
#ifexist "..\build\icon.ico"
SetupIconFile=..\build\icon.ico
#endif

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "addtopath"; Description: "Add the &intelli-rust command to PATH (for terminal users)"; GroupDescription: "Command line:"; Flags: unchecked

[Files]
Source: "..\..\dist\windows\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Intelli Search"; Filename: "{app}\intelli-search.exe"
Name: "{group}\Uninstall Intelli"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Intelli Search"; Filename: "{app}\intelli-search.exe"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "{code:EnvKey}"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\intelli-search.exe"; Description: "Open Intelli Search now"; Flags: nowait postinstall skipifsilent

[Code]
function EnvKey(Param: String): String;
begin
  if IsAdminInstallMode then Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else Result := 'Environment';
end;

function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKA, EnvKey(''), 'Path', OrigPath) then begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;
