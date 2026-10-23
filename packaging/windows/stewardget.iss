; StewardGet installer for Windows (Inno Setup 6.4 or newer).
;
; One setup file for x64 and ARM64 PCs. Installs for the current user by
; default (no administrator rights needed); "Install for all users" is offered
; on the first page.
;
; Built by the release workflow:
;   ISCC /DAppVersion=1.2.3 /DBinX64=<dir> [/DBinArm64=<dir>] /DImages=<dir> /DOutputDir=<dir> stewardget.iss
; <dir>s hold stewardget-gui.exe and stewardget.exe; Images holds the wizard bitmaps
; from packaging/windows/wizard_images.py.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinX64
  #error Pass /DBinX64=<folder with the x64 binaries>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
; A new AppId since the rename from Velox (0.7), whose install is removed in RemoveVelox.
AppId={{4B7DDA6E-B432-48FD-96C1-FF2AD0D22700}
AppName=StewardGet
AppVersion={#AppVersion}
AppVerName=StewardGet {#AppVersion}
AppPublisher=StewardGet contributors
AppPublisherURL=https://github.com/anand34577/stewardget
AppSupportURL=https://github.com/anand34577/stewardget/wiki
AppUpdatesURL=https://github.com/anand34577/stewardget/releases
AppCopyright=StewardGet contributors, MIT or Apache-2.0
VersionInfoVersion={#AppVersion}
VersionInfoDescription=StewardGet Setup
DefaultDirName={autopf}\StewardGet
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
#ifdef BinArm64
ArchitecturesAllowed=x64compatible or arm64
ArchitecturesInstallIn64BitMode=x64compatible or arm64
#else
ArchitecturesAllowed=x64compatible and not arm64
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=StewardGet-Setup
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\stewardget-gui.exe
UninstallDisplayName=StewardGet
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
#ifdef Images
WizardImageFile={#Images}\wizard-100.bmp,{#Images}\wizard-200.bmp
WizardSmallImageFile={#Images}\wizard-small-100.bmp,{#Images}\wizard-small-200.bmp
#endif
; Closes a running StewardGet (it saves its progress first) and doesn't restart it.
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Messages]
WelcomeLabel2=This installs StewardGet {#AppVersion}, a fast download manager with no telemetry.%n%nStewardGet closes automatically if it is running; unfinished downloads continue afterwards.

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "startup"; Description: "&Start StewardGet when I sign in (browser downloads reach StewardGet only while it runs)"; GroupDescription: "Options:"
Name: "path"; Description: "Add the &stewardget command to PATH (for the terminal)"; GroupDescription: "Options:"

[Files]
#ifdef BinArm64
Source: "{#BinX64}\stewardget-gui.exe"; DestDir: "{app}"; Check: not IsArm64; Flags: ignoreversion
Source: "{#BinX64}\stewardget.exe"; DestDir: "{app}"; Check: not IsArm64; Flags: ignoreversion
Source: "{#BinArm64}\stewardget-gui.exe"; DestDir: "{app}"; Check: IsArm64; Flags: ignoreversion
Source: "{#BinArm64}\stewardget.exe"; DestDir: "{app}"; Check: IsArm64; Flags: ignoreversion
#else
Source: "{#BinX64}\stewardget-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinX64}\stewardget.exe"; DestDir: "{app}"; Flags: ignoreversion
#endif
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\StewardGet"; Filename: "{app}\stewardget-gui.exe"; AppUserModelID: "dev.stewardget.StewardGet"; Comment: "Download manager"
Name: "{autodesktop}\StewardGet"; Filename: "{app}\stewardget-gui.exe"; Tasks: desktopicon; Comment: "Download manager"

[Registry]
; The same value the app's "Start with your computer" setting writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "StewardGet"; ValueData: """{app}\stewardget-gui.exe"" --background"; Tasks: startup

[Run]
Filename: "{app}\stewardget-gui.exe"; Description: "Open StewardGet"; Flags: nowait postinstall skipifsilent

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ApprovedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run';
  MachineEnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  VeloxUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{8F4E2D1C-6B7A-4C3E-9A51-2D7B0E5F9C14}_is1';

var
  VeloxStarted: Boolean;

function EnvRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKLM else Result := HKCU;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then Result := MachineEnvKey else Result := 'Environment';
end;

function PathHas(Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then Paths := '';
  if PathHas(Paths, Dir) then Exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then Paths := Paths + ';';
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then Exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then Exit;
  Delete(Paths, P, Length(Dir) + 1);
  // Drop the sentinels added above.
  if Copy(Paths, 1, 1) = ';' then Delete(Paths, 1, 1);
  if Copy(Paths, Length(Paths), 1) = ';' then Delete(Paths, Length(Paths), 1);
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('path') then
      AddToPath(ExpandConstant('{app}'))
    else
      RemoveFromPath(ExpandConstant('{app}'));
    // Unticked, the login item is left alone: it may have been turned on in
    // the app, which is also where it is turned off.
    if WizardIsTaskSelected('startup') then
      // A "Disable" from Task Manager would otherwise keep it from starting.
      RegDeleteValue(HKCU, ApprovedKey, 'StewardGet')
    else if VeloxStarted then
      RegWriteStringValue(HKCU, RunKey, 'StewardGet', '"' + ExpandConstant('{app}\stewardget-gui.exe') + '" --background');
  end;
end;

// StewardGet was called Velox before 0.7 and installed under that name. Its
// uninstaller closes it and keeps the data, which StewardGet moves into its own
// folder on first start; whether it started at sign-in is noted first, since
// the uninstaller drops that.
procedure RemoveVelox;
var
  Uninstaller: String;
  Code: Integer;
begin
  VeloxStarted := RegValueExists(HKCU, RunKey, 'Velox');
  if not RegQueryStringValue(HKCU, VeloxUninstallKey, 'UninstallString', Uninstaller) and
     not RegQueryStringValue(HKLM, VeloxUninstallKey, 'UninstallString', Uninstaller) then
    Exit;
  Uninstaller := RemoveQuotes(Uninstaller);
  if FileExists(Uninstaller) then
    Exec(Uninstaller, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

// Asks a running StewardGet to quit and waits for it (it saves its progress first).
// `--quit` works even when its close button only minimises it. Only called
// while StewardGet runs: started on its own, an older version would open its window.
procedure QuitStewardGet(Exe: String);
var
  Code: Integer;
begin
  if FileExists(Exe) and (FindWindowByWindowName('StewardGet') <> 0) then
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  RemoveVelox;
  QuitStewardGet(ExpandConstant('{app}\stewardget-gui.exe'));
  Result := '';
end;

// Asks a running StewardGet to close (it saves progress) before files are removed.
function InitializeUninstall: Boolean;
var
  Wnd: HWND;
  Waited: Integer;
begin
  Result := True;
  QuitStewardGet(ExpandConstant('{app}\stewardget-gui.exe'));
  Wnd := FindWindowByWindowName('StewardGet');
  if Wnd = 0 then Exit;
  PostMessage(Wnd, 16 { WM_CLOSE }, 0, 0);
  Waited := 0;
  while (FindWindowByWindowName('StewardGet') <> 0) and (Waited < 8000) do
  begin
    Sleep(250);
    Waited := Waited + 250;
  end;
  if FindWindowByWindowName('StewardGet') <> 0 then
  begin
    if not UninstallSilent then
      MsgBox('Please close StewardGet, then run the uninstaller again.', mbError, MB_OK);
    Result := False;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RemoveFromPath(ExpandConstant('{app}'));
    RegDeleteValue(HKCU, RunKey, 'StewardGet');
    RegDeleteValue(HKCU, ApprovedKey, 'StewardGet');
  end;
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
  begin
    if MsgBox('Also delete your StewardGet settings, download list and statistics?' + #13#10#13#10 +
              'Your downloaded files are not affected either way.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{userappdata}\stewardget\StewardGet'), True, True, True);
      RemoveDir(ExpandConstant('{userappdata}\stewardget'));
      // The browser extension StewardGet keeps in Velox's old folder, if any.
      DelTree(ExpandConstant('{userappdata}\velox'), True, True, True);
    end;
  end;
end;
