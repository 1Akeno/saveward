!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Saveward"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "SaveVault"
!macroend
