; Instalador de SWEPR (Inno Setup 6).
;
; El mismo instalador sirve para instalar y para actualizar: como el AppId no cambia nunca,
; ejecutar un setup más nuevo reemplaza la versión instalada en la misma carpeta, conserva
; la configuración (%LOCALAPPDATA%\swepr) y vuelve a abrir SWEPR si estaba abierto.
;
; Normalmente lo compila prepare-release.ps1, que pasa la versión de Cargo.toml:
;   ISCC.exe /DAppVersion=1.1.0 /DSourceExe=..\target\release\swepr.exe /DOutputDir=..\releases\v1.1.0 installer\swepr.iss
;
; Actualización desatendida (sin preguntas): SWEPR-vX.Y.Z-setup.exe /VERYSILENT

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\swepr.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\releases"
#endif

; VersionInfoVersion solo admite números: "1.2.0-rc1" pasa a "1.2.0".
#if Pos("-", AppVersion) > 0
  #define NumericVersion Copy(AppVersion, 1, Pos("-", AppVersion) - 1)
#else
  #define NumericVersion AppVersion
#endif

#define AppName "SWEPR"
#define AppExeName "swepr.exe"
; Deben coincidir con src\main.rs y src\app.rs.
#define AppMutex "Local\SWEPR_SingleInstance_Mutex"
#define MainWindowClass "SWEPR_MainWindowClass"

[Setup]
; ¡No cambiar nunca! Es lo que identifica la instalación para poder actualizarla.
AppId={{0F10D2C6-40C5-44A8-A96F-F64B6EC96CDB}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppName}
AppComments=Wallpaper selector for Windows
VersionInfoVersion={#NumericVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
; Instalación por usuario: no pide administrador, ni al instalar ni al actualizar.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
; La carpeta solo se pregunta la primera vez; las actualizaciones usan la existente.
DisableDirPage=auto
UsePreviousAppDir=yes
UsePreviousTasks=yes
OutputDir={#OutputDir}
OutputBaseFilename=SWEPR-v{#AppVersion}-setup
SetupIconFile=..\resources\swepr.ico
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; SWEPR se cierra desde [Code] (de forma ordenada, quitando el icono de la bandeja).
CloseApplications=no
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[CustomMessages]
english.LaunchApp=Start SWEPR
spanish.LaunchApp=Iniciar SWEPR
english.CloseFailed=SWEPR is still running. Close it from the tray icon and try again.
spanish.CloseFailed=SWEPR sigue abierto. Ciérralo desde el icono de la bandeja e inténtalo de nuevo.
english.DeleteSettings=Do you also want to delete your SWEPR settings, favorites, and log?
spanish.DeleteSettings=¿Quieres borrar también la configuración, las favoritas y el registro de SWEPR?

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\languages\*.json"; DestDir: "{app}\languages"; Flags: ignoreversion
Source: "..\languages\README.md"; DestDir: "{app}\languages"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Registry]
; SWEPR gestiona "Iniciar con Windows" por sí mismo; al desinstalar se quita la entrada.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "SWEPR"; Flags: uninsdeletevalue dontcreatekey

[Run]
; Si SWEPR estaba abierto antes de actualizar, se vuelve a abrir solo (también en modo silencioso).
Filename: "{app}\{#AppExeName}"; Flags: nowait; Check: WasRunning
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent; Check: not WasRunning

[Code]
const
  WM_CLOSE = $0010;

var
  SweprWasRunning: Boolean;

function WasRunning: Boolean;
begin
  Result := SweprWasRunning;
end;

function IsSweprRunning: Boolean;
begin
  Result := CheckForMutexes('{#AppMutex}');
end;

{ Pide a SWEPR que se cierre (WM_CLOSE a su ventana oculta) y espera hasta 5 s.
  Si no responde, se termina el proceso. Devuelve si ya no está en ejecución. }
function CloseSwepr: Boolean;
var
  Wnd: HWND;
  Tries: Integer;
  ResultCode: Integer;
begin
  Wnd := FindWindowByClassName('{#MainWindowClass}');
  if Wnd <> 0 then
    PostMessage(Wnd, WM_CLOSE, 0, 0);
  Tries := 0;
  while IsSweprRunning and (Tries < 50) do
  begin
    Sleep(100);
    Tries := Tries + 1;
  end;
  if IsSweprRunning then
  begin
    Log('SWEPR no se cerró a tiempo; se termina el proceso.');
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExeName}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Sleep(500);
  end;
  Result := not IsSweprRunning;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  SweprWasRunning := IsSweprRunning;
  if SweprWasRunning then
  begin
    Log('SWEPR está en ejecución: cerrándolo antes de actualizar.');
    if not CloseSwepr then
      Result := CustomMessage('CloseFailed');
  end;
end;

function InitializeUninstall: Boolean;
begin
  Result := True;
  if IsSweprRunning and not CloseSwepr then
  begin
    MsgBox(CustomMessage('CloseFailed'), mbError, MB_OK);
    Result := False;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { Los datos del usuario solo se borran si lo pide (en modo silencioso se conservan). }
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    if MsgBox(CustomMessage('DeleteSettings'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{localappdata}\swepr'), True, True, True);
end;
