; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright (C) 2026 zickrian
;
; Isle's additions to the installer Tauri makes: before removing Isle, take its hooks out of Claude Code's
; settings and its line out of the programs that start with Windows, so nothing points at a missing isle.exe.

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\isle.exe" --uninstall'
!macroend
