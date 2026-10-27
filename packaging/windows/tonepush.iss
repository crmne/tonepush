; No longer shipped: releases after 0.10.0 ship only the Windows Installer
; package (tonepush.wxs), which takes over a copy this script installed.
; test-installer.ps1 still builds it to make such a copy to take over.
;
; Windows installer built from a release build with Inno Setup 6.3 or newer:
;
;   iscc /DVersion=0.9.0 /DArch=x86_64 /DBinaries=...\target\x86_64-pc-windows-msvc\release ^
;        /DOutputDir=dist packaging\windows\tonepush.iss
;
; Arch matches the Rust target: x86_64 or aarch64. Binaries is the folder
; holding tonepush-gui.exe (the editor) and tonepush.exe (the command line).
; Installation uses the current user's Programs folder and does not need
; administrator rights. Updates close a running copy before replacing it.

#ifndef Version
  #error Version must be defined on the ISCC command line
#endif
#ifndef Arch
  #error Arch must be defined on the ISCC command line (x86_64 or aarch64)
#endif
#ifndef NumericVersion
  #define NumericVersion Version
#endif
#ifndef Binaries
  #error Binaries must be defined on the ISCC command line
#endif
#ifndef OutputDir
  #error OutputDir must be defined on the ISCC command line
#endif
#if Arch == "aarch64"
  #define InnoArch "arm64"
#else
  #define InnoArch "x64compatible"
#endif

#define AppName "TonePush"
#define AppExeName "tonepush-gui.exe"

[Setup]
; Never change: this is how Windows tells an update from a new program.
AppId={{E7F9B874-71BE-4DE7-8323-BB09AFBD702F}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher=Carmine Paolino
AppCopyright=© 2026 Carmine Paolino
AppPublisherURL=https://tonepush.rocks
AppSupportURL=https://github.com/crmne/tonepush/issues
AppUpdatesURL=https://github.com/crmne/tonepush/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed={#InnoArch}
ArchitecturesInstallIn64BitMode={#InnoArch}
MinVersion=10.0
LicenseFile=..\..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=tonepush-v{#Version}-{#Arch}-pc-windows-msvc-setup
SetupIconFile=tonepush.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\{#AppExeName}
VersionInfoVersion={#NumericVersion}.0

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#Binaries}\tonepush-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Binaries}\tonepush.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; Tells TonePush's updater that this copy came from the installer, so an
; update downloads and runs the next installer instead of an archive.
Source: "tonepush-installer.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[UninstallDelete]
; The updater's working folders (fastframe-update), left by the last update.
Type: filesandordirs; Name: "{app}\.tonepush-update-*"

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
// Ends the copies of Exe running from this install. A portable copy can
// have the same name, so it is never closed by name alone.
procedure CloseInstalled(Exe: String);
var
  Locator, Services, Processes, Process, Path: Variant;
  Index: Integer;
  Target: String;
begin
  Target := ExpandConstant('{app}\') + Exe;
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Services := Locator.ConnectServer('', 'root\CIMV2');
    Processes := Services.ExecQuery('SELECT * FROM Win32_Process WHERE Name = ''' + Exe + '''');
    for Index := 0 to Processes.Count - 1 do begin
      Process := Processes.ItemIndex(Index);
      Path := Process.ExecutablePath;
      if not VarIsNull(Path) then
        if CompareText(Path, Target) = 0 then
          Log('Closing installed ' + Exe + ': result ' + IntToStr(Process.Terminate(0)));
    end;
  except
    Log('Could not close installed ' + Exe + ': ' + GetExceptionMessage);
  end;
end;

// Whether Exe in this install can be removed. Windows denies write access
// to a running executable; opening it writes nothing and also catches other
// locks on this exact copy.
function Removable(Exe: String): Boolean;
var
  Path: String;
  Probe: TFileStream;
begin
  Path := ExpandConstant('{app}\') + Exe;
  Result := True;
  if not FileExists(Path) then
    Exit;
  try
    Probe := TFileStream.Create(Path, fmOpenReadWrite or fmShareExclusive);
    Probe.Free;
  except
    Log('Cannot open ' + Path + ' for uninstall: ' + GetExceptionMessage);
    Result := False;
  end;
end;

// CloseApplications covers Setup, not Uninstall. Check the editor and the
// command-line tool before removing anything, including the uninstaller
// needed to try again.
function InitializeUninstall: Boolean;
begin
  Result := False;
  while not (Removable('{#AppExeName}') and Removable('tonepush.exe')) do begin
    if UninstallSilent then
      Exit;
    case SuppressibleTaskDialogMsgBox('TonePush cannot be removed yet.',
      'TonePush or its tonepush command is in use or inaccessible. Quit it and retry, or close it here to continue.' + #13#10#13#10 +
      'Close TonePush ends it immediately. Unsaved changes are lost, and a transfer to your pedal is cut off.',
      mbError, MB_YESNOCANCEL, ['Retry', 'Close TonePush'], 0, IDCANCEL) of
      IDYES: ;
      IDNO: begin
        CloseInstalled('{#AppExeName}');
        CloseInstalled('tonepush.exe');
        Sleep(200);
      end;
    else
      Exit;
    end;
  end;
  Result := True;
end;
