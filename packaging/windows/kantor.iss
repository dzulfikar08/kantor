; Kantor for Windows: a per-user installer (no administrator rights needed).
; Built by scripts/package-windows.ps1 with Inno Setup 6:
;   iscc /DAppVersion=0.3.1 /DArch=x64 /DSourceDir=dist\Kantor /DOutputDir=dist /DOutputName=Kantor-Setup-x64 packaging\windows\kantor.iss
;
; Installs to %LOCALAPPDATA%\Programs\Kantor: kantor.exe (the app), kantor.com
; (the command line, see crates/kantor_console), licences. Adds a Start menu
; shortcut, an optional desktop icon, an uninstall entry under Settings > Apps,
; and "Open with" entries for .xlsx/.xlsm/.xls/.xlt/.ods/.csv, .docx/.rtf/.doc/.dot and .pptx/.ppt/.pps/.pot that
; don't take over the user's default apps.
;
; The in-app updater (crates/kantor_update) downloads a newer installer and runs
;   Kantor-Setup-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL /SP- /WAITPID=<pid> [/RELAUNCH=1]
; /WAITPID makes setup wait for that Kantor to exit; /RELAUNCH=1 starts Kantor
; again afterwards.

#ifndef AppVersion
  #error Pass /DAppVersion=X.Y.Z
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\Kantor"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#ifndef OutputName
  #define OutputName "Kantor-Setup-" + Arch
#endif

[Setup]
; Never change AppId: upgrades and the uninstall entry are keyed on it.
AppId={{6E4B1C52-8F0A-4C1E-9D7B-4B3A2F6C9E11}
AppName=Kantor
AppVersion={#AppVersion}
AppVerName=Kantor {#AppVersion}
AppPublisher=Dzulfikar Ats Tsauri
AppPublisherURL=https://kantor.paparanai.id
AppSupportURL=https://github.com/dzulfikar08/kantor/issues
AppUpdatesURL=https://kantor.paparanai.id
AppCopyright=Copyright (C) Dzulfikar Ats Tsauri. AGPL-3.0-or-later.
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Kantor
DefaultGroupName=Kantor
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
; Per user, no UAC prompt: {autopf} is %LOCALAPPDATA%\Programs.
PrivilegesRequired=lowest
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile=kantor.ico
UninstallDisplayIcon={app}\kantor.exe
UninstallDisplayName=Kantor
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; Shell notifications for the file-type entries.
ChangesAssociations=yes
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no
#ifdef SignTool
SignTool={#SignTool}
SignedUninstaller=yes
#endif

[Languages]
; English only: Inno Setup ships no Indonesian translation.
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
AddToPath=Add Kantor to PATH (use "kantor" in a terminal)
LaunchKantor=Start Kantor

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "{cm:AddToPath}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\kantor.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\kantor.com"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\*.txt"; DestDir: "{app}"; Excludes: "kantor-portable.txt"; Flags: ignoreversion
Source: "{#SourceDir}\*.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[InstallDelete]
; The portable marker must never sit next to an installed copy.
Type: files; Name: "{app}\kantor-portable.txt"

[UninstallDelete]
; What the in-app updater leaves while it works.
Type: filesandordirs; Name: "{app}\.kantor-update"
Type: files; Name: "{app}\.kantor.exe.kantor-old"

[Icons]
Name: "{autoprograms}\Kantor"; Filename: "{app}\kantor.exe"; AppUserModelID: "Kantor"
Name: "{autodesktop}\Kantor"; Filename: "{app}\kantor.exe"; Tasks: desktopicon

[Registry]
; One ProgID per kind of document; the extensions list it under
; OpenWithProgids, so Kantor shows in "Open with" without becoming the default.
Root: HKA; Subkey: "Software\Classes\Kantor.Sheet"; ValueType: string; ValueName: ""; ValueData: "Kantor spreadsheet"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Kantor.Sheet\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"",0"
Root: HKA; Subkey: "Software\Classes\Kantor.Sheet\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Kantor.Document"; ValueType: string; ValueName: ""; ValueData: "Kantor document"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Kantor.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"",0"
Root: HKA; Subkey: "Software\Classes\Kantor.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Kantor.Presentation"; ValueType: string; ValueName: ""; ValueData: "Kantor presentation"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Kantor.Presentation\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"",0"
Root: HKA; Subkey: "Software\Classes\Kantor.Presentation\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"" ""%1"""

Root: HKA; Subkey: "Software\Classes\.xlsx\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Sheet"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.xlsm\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Sheet"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.xls\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Sheet"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.xlt\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Sheet"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ods\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Sheet"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.csv\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Sheet"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.docx\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.rtf\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.doc\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dot\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pptx\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Presentation"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ppt\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Presentation"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pps\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Presentation"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pot\OpenWithProgids"; ValueType: string; ValueName: "Kantor.Presentation"; ValueData: ""; Flags: uninsdeletevalue

; "Open with" > "Choose another app" lists kantor.exe for these types.
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Kantor"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\kantor.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".xlsx"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".xlsm"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".xls"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".xlt"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".ods"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".csv"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".docx"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".rtf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".doc"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".dot"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".pptx"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".ppt"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".pps"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\kantor.exe\SupportedTypes"; ValueType: string; ValueName: ".pot"; ValueData: ""

; Settings > Apps > Default apps can offer Kantor for these types.
Root: HKA; Subkey: "Software\Kantor\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Kantor"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Kantor\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Spreadsheets, documents and presentations"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".xlsx"; ValueData: "Kantor.Sheet"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".xlsm"; ValueData: "Kantor.Sheet"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".xls"; ValueData: "Kantor.Sheet"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".xlt"; ValueData: "Kantor.Sheet"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".ods"; ValueData: "Kantor.Sheet"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".csv"; ValueData: "Kantor.Sheet"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".docx"; ValueData: "Kantor.Document"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".rtf"; ValueData: "Kantor.Document"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".doc"; ValueData: "Kantor.Document"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".dot"; ValueData: "Kantor.Document"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pptx"; ValueData: "Kantor.Presentation"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".ppt"; ValueData: "Kantor.Presentation"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pps"; ValueData: "Kantor.Presentation"
Root: HKA; Subkey: "Software\Kantor\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pot"; ValueData: "Kantor.Presentation"
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Kantor"; ValueData: "Software\Kantor\Capabilities"; Flags: uninsdeletevalue

[Run]
; Interactive install: a "Start Kantor" box on the last page.
Filename: "{app}\kantor.exe"; Description: "{cm:LaunchKantor}"; Flags: nowait postinstall skipifsilent
; Silent update from the app with /RELAUNCH=1.
Filename: "{app}\kantor.exe"; Flags: nowait; Check: RelaunchAfterUpdate

[Code]
const
  SYNCHRONIZE = $00100000;

function OpenProcess(dwDesiredAccess: DWORD; bInheritHandle: BOOL; dwProcessId: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(hHandle: THandle; dwMilliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(hObject: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

function RelaunchAfterUpdate(): Boolean;
begin
  Result := ExpandConstant('{param:RELAUNCH|0}') = '1';
end;

{ /WAITPID=N: the Kantor that started this update is still quitting; give it
  up to 30 s to exit before files are replaced. }
function InitializeSetup(): Boolean;
var
  Pid: Integer;
  H: THandle;
begin
  Pid := StrToIntDef(ExpandConstant('{param:WAITPID|0}'), 0);
  if Pid > 0 then
  begin
    H := OpenProcess(SYNCHRONIZE, False, Pid);
    if H <> 0 then
    begin
      WaitForSingleObject(H, 30000);
      CloseHandle(H);
    end;
  end;
  Result := True;
end;

{ PATH (task "addtopath"): the user's own Path in HKCU\Environment. }
const
  EnvKey = 'Environment';

function PathHas(Paths, Dir: string): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(Dir: string);
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths) then
    Paths := '';
  if PathHas(Paths, Dir) then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: string);
var
  Paths: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths) then
    exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then
    exit;
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
