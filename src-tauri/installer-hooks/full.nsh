!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "WordFunc.nsh"
!insertmacro GetFileVersion
!insertmacro VersionCompare

Function QlisaCheckNdiDllVersion
  IfFileExists "$R9" 0 qlisa_ndi_version_done
  StrCpy $R1 ""
  ClearErrors
  ${GetFileVersion} "$R9" $R1
  IfErrors qlisa_ndi_version_done
  ${If} $R1 != ""
    ${VersionCompare} "$R1" "6.3.2.0" $R5
    ${If} $R5 != 2
      StrCpy $R8 "1"
    ${EndIf}
  ${EndIf}
qlisa_ndi_version_done:
FunctionEnd

!macro NSIS_HOOK_POSTINSTALL
  StrCpy $R8 "0"
  ReadEnvStr $R9 "NDI_RUNTIME_DIR_V6"
  ${If} $R9 != ""
    StrCpy $R9 "$R9\Processing.NDI.Lib.x64.dll"
    Call QlisaCheckNdiDllVersion
  ${EndIf}

  StrCpy $R9 "$PROGRAMFILES64\NDI\NDI 6 Runtime\v6\Processing.NDI.Lib.x64.dll"
  Call QlisaCheckNdiDllVersion

  ${If} $R8 == "1"
    DetailPrint "A current NDI 6 Runtime is already installed; keeping it."
  ${Else}
    DetailPrint "Installing the pinned NDI 6 Runtime."
    ExecWait '"$INSTDIR\ndi\NDI-6-Runtime.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /RESTARTEXITCODE=3010 /LOG="$TEMP\Qlisa-NDI-Runtime-install.log"' $R0
    ${If} $R0 == 3010
      SetRebootFlag true
    ${ElseIf} $R0 != 0
      SetErrorLevel 1
      MessageBox MB_OK|MB_ICONSTOP "NDI Runtime installation failed with exit code $R0. See $TEMP\Qlisa-NDI-Runtime-install.log. Qlisa has not been installed completely." /SD IDOK
      Quit
    ${EndIf}
  ${EndIf}
!macroend
