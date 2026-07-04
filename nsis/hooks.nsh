; NSIS installer hooks for PSF Guard (Tauri v2 `bundle.windows.nsis.installerHooks`).
;
; The Windows bundle ships `psf-guard-cli.exe` (a console-subsystem CLI) next to
; the GUI app via `externalBin`. These hooks add the install directory to the
; per-user PATH on install and remove it on uninstall, so `psf-guard-cli` is
; runnable from any terminal.
;
; We edit HKCU\Environment (the *user* PATH) because Tauri's default NSIS
; installMode is per-user: no elevation needed, the user PATH is short (so the
; NSIS_MAX_STRLEN=1024 limit is not a concern in practice), and uninstall cleanly
; reverts only this user's environment. A WM_WININICHANGE broadcast makes newly
; launched shells pick up the change without a logout.
;
; Tauri includes this file at global scope before the sections, so the StrFunc
; function instances and the WinMessages/LogicLib helpers below are available to
; the macros, which Tauri inserts inside the (un)install sections.

!include "StrFunc.nsh"
!include "LogicLib.nsh"
!include "WinMessages.nsh"

; Instantiate the StrFunc helpers we use, in both installer and uninstaller
; flavors (the "Un" variants are required from uninstall sections).
${StrStr}
${UnStrRep}

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  ReadRegStr $0 HKCU "Environment" "Path"
  ; Only append when the install dir isn't already on PATH (idempotent across
  ; reinstalls/upgrades).
  ${StrStr} $1 "$0" "$INSTDIR"
  ${If} $1 == ""
    ${If} $0 == ""
      WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
    ${Else}
      WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR"
    ${EndIf}
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${EndIf}
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Push $0
  Push $1
  ReadRegStr $0 HKCU "Environment" "Path"
  ; Strip our entry in whichever position it sits (middle/leading/trailing/only).
  ${UnStrRep} $1 "$0" ";$INSTDIR" ""
  ${UnStrRep} $1 "$1" "$INSTDIR;" ""
  ${UnStrRep} $1 "$1" "$INSTDIR" ""
  WriteRegExpandStr HKCU "Environment" "Path" "$1"
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  Pop $1
  Pop $0
!macroend
