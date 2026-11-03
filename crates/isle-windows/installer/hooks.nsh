; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright (C) 2026 zickrian
;
; Isle's additions to the installer Tauri makes. Removing Isle (not updating it): take its hooks out of Claude
; Code's settings and its line out of the programs that start with Windows, so nothing points at a missing
; isle.exe; and with "Delete the application data" ticked, Isle's own data too (%LOCALAPPDATA%\Isle: settings,
; events, reminders, timer, app usage). Unticked, it stays for a later install.

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\isle.exe" --uninstall'
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$LOCALAPPDATA\Isle"
  ${EndIf}
!macroend
