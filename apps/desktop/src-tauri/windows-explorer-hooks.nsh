!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\*\shell\FormatWright" "MUIVerb" "Open in FormatWright"
  WriteRegStr SHCTX "Software\Classes\*\shell\FormatWright" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr SHCTX "Software\Classes\*\shell\FormatWright\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --shell-open "%1"'

  WriteRegStr SHCTX "Software\Classes\Directory\shell\FormatWright" "MUIVerb" "Open in FormatWright"
  WriteRegStr SHCTX "Software\Classes\Directory\shell\FormatWright" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr SHCTX "Software\Classes\Directory\shell\FormatWright\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --shell-open "%1"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey SHCTX "Software\Classes\*\shell\FormatWright"
  DeleteRegKey SHCTX "Software\Classes\Directory\shell\FormatWright"
!macroend
