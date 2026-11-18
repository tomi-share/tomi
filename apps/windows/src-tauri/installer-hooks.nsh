; NSIS hooks of the Tomi installer (bundle > windows > nsis > installerHooks).
; Syntax: Tauri NSIS bundler, macros NSIS_HOOK_PREINSTALL / POSTINSTALL / PREUNINSTALL /
; POSTUNINSTALL, inserted by the generated installer.nsi (installMode currentUser, so
; $APPDATA is the current user's).

; The "Send to > Tomi" shortcut is written by the app itself at launch (src/sendto.rs), not by the
; installer, so the generated uninstaller does not know it. It goes with the app, but not during
; an update ($UpdateMode = 1: the uninstaller runs first, the new version keeps the shortcut).
; The app does not rewrite the shortcut when the same build is installed again (the settings that
; remember it survive an uninstall), so a fresh install (not an update) creates it here. It is
; only created when missing; the app rewrites it itself when the executable changes.
!macro NSIS_HOOK_POSTINSTALL
  ${If} $UpdateMode <> 1
    ${IfNot} ${FileExists} "$APPDATA\Microsoft\Windows\SendTo\Tomi.lnk"
      CreateShortCut "$APPDATA\Microsoft\Windows\SendTo\Tomi.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    Delete "$APPDATA\Microsoft\Windows\SendTo\Tomi.lnk"
  ${EndIf}
!macroend
