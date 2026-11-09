; Inno Setup script. scripts\package.ps1 passes the defines and runs ISCC.
#ifndef AppVersion
  #error Run scripts\package.ps1. It passes AppVersion, StageDir, OutputDir and IconFile.
#endif

[Setup]
AppId={{B5A4C8E2-6F1D-4E0B-9C57-2D3A7E91F4B8}
AppName=Cadence
AppVersion={#AppVersion}
AppPublisher=jesubohr
AppPublisherURL=https://github.com/jesubohr/Cadence
AppSupportURL=https://github.com/jesubohr/Cadence/issues
DefaultDirName={autopf}\Cadence
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
AppMutex=Local\CadenceSingleInstance
OutputDir={#OutputDir}
OutputBaseFilename=Cadence-{#AppVersion}-win64-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\Cadence.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: tools; Description: "Download yt-dlp, FFmpeg and Deno (Cadence needs them to download media)"
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Cadence"; Filename: "{app}\Cadence.exe"
Name: "{autoprograms}\Install tools"; Filename: "{app}\Install-Tools.cmd"; WorkingDir: "{app}"
Name: "{autodesktop}\Cadence"; Filename: "{app}\Cadence.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Cadence.exe"; Description: "Start Cadence"; Flags: nowait postinstall skipifsilent

[Code]
// The tools go to %LOCALAPPDATA%\Cadence\tools, where Cadence looks for them.
// The script skips tools that are already installed at the pinned version.
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('tools') then
  begin
    WizardForm.StatusLabel.Caption := 'Downloading yt-dlp, FFmpeg and Deno...';
    if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -ExecutionPolicy Bypass -File "' + ExpandConstant('{app}\scripts\setup-deps.ps1') + '"',
      '', SW_SHOW, ewWaitUntilTerminated, ResultCode) or (ResultCode <> 0) then
      SuppressibleMsgBox(
        'Cadence is installed, but the tools were not downloaded. ' +
        'Connect to the internet, then run "Install tools" from the Start menu.',
        mbError, MB_OK, IDOK);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    if SuppressibleMsgBox(
      'Also delete the Cadence settings, queue and downloaded tools?' + #13#10 +
      'Your downloaded music and videos stay where they are.',
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
      DelTree(ExpandConstant('{localappdata}\Cadence'), True, True, True);
end;
