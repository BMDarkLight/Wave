; Wave - Copyright (C) 2025 BMDarkLight
; Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
; See the LICENSE file in the project root for the full license text.
; https://github.com/BMDarkLight/Wave
;
; Puts the `wave` command on PATH at install time and takes it off again at
; uninstall time.
;
; Only $INSTDIR\bin goes on PATH - never the install root. That directory holds
; wave.cmd, a shim around the GUI-subsystem wave.exe; see wave.cmd for why the
; indirection is needed and why the two files must not share a directory.
;
; The PATH edit itself is delegated to wave-path.ps1 because NSIS strings are
; capped at 1024 characters, and a PATH longer than that would be silently
; truncated and written back, destroying the rest of it.

!include LogicLib.nsh

; Run wave-path.ps1. ACTION is "add" or "remove".
!macro WaveUpdatePath ACTION
  Push $R7
  Push $R8

  ; A per-machine install edits the system PATH so every account gets the
  ; command; a per-user install stays inside HKCU and needs no elevation.
  StrCpy $R7 "User"
  ${If} $MultiUser.InstallMode == "AllUsers"
    StrCpy $R7 "Machine"
  ${EndIf}

  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\bin\wave-path.ps1" -Action ${ACTION} -Directory "$INSTDIR\bin" -Scope $R7'
  Pop $R8

  ; PATH is a convenience: the app and its Start Menu entry work regardless, so
  ; a failure here is reported but never aborts the install or uninstall.
  ${If} $R8 != 0
    DetailPrint "Wave: could not update PATH automatically (${ACTION})."
    DetailPrint "Wave: add $INSTDIR\bin to PATH by hand to use the 'wave' command."
  ${EndIf}

  Pop $R8
  Pop $R7
!macroend

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Wave: registering the 'wave' command on PATH..."
  !insertmacro WaveUpdatePath "add"
!macroend

; PREUNINSTALL, not POSTUNINSTALL: wave-path.ps1 lives under $INSTDIR and is
; already deleted by the time the post-uninstall hook runs.
!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Wave: removing the 'wave' command from PATH..."
  !insertmacro WaveUpdatePath "remove"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
