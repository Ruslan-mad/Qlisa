!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Removing unused full-installer payload files from the application directory."
  Delete "$INSTDIR\full-runtime\full-package.json"
  Delete "$INSTDIR\full-runtime\ffmpeg.exe"
  Delete "$INSTDIR\full-runtime\ffprobe.exe"
  Delete "$INSTDIR\full-runtime\libmpv-2.dll"
  Delete "$INSTDIR\full-runtime\vulkan-1.dll"
  RMDir "$INSTDIR\full-runtime"
  Delete "$INSTDIR\ndi\NDI-6-Runtime.exe"
  RMDir "$INSTDIR\ndi"
!macroend
