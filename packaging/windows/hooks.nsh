; Crochets de l'installateur NSIS de ChatonLM (bundle.windows.nsis.installerHooks
; de src-tauri/tauri.windows.conf.json), inclus par le modèle de Tauri après
; utils.nsh et Win\RestartManager.nsh.
;
; Le modèle ne surveille que l'exécutable principal. Avant de copier les
; fichiers (installation, mise à jour) et avant de les supprimer
; (désinstallation), ces crochets arrêtent l'Ollama intégré de CETTE
; installation, désigné par son chemin exact ($INSTDIR\ollama\...), puis
; l'app, puis de nouveau l'intégré (l'app, encore en marche, a pu le relancer
; entre-temps). Jamais d'arrêt par nom (« taskkill /im ollama.exe ») : il
; arrêterait l'Ollama que l'utilisateur a installé lui-même.
;
; Arrêt par le Restart Manager de Windows, avec les mêmes appels que la macro
; CheckIfAppIsRunning du modèle (utils.nsh), mais sans question ni étiquette :
; la macro du modèle pose des étiquettes nommées d'après __LINE__, qu'une
; seconde insertion dans la même section pourrait redéfinir. Comme
; l'installateur officiel d'Ollama (ollama.iss, TaskKill avant chaque copie),
; l'arrêt ne demande rien.
;
; Les données de l'utilisateur ne sont pas touchées : la désinstallation ne
; les efface que si la case du modèle est cochée.

; Arrête les processus dont l'exécutable est exactement `executablePath`.
; Registres utilisés : $R0 (session), $0 à $3, comme la macro du modèle qui
; suit ces crochets.
!macro CHATONLM_STOP_FILE executablePath
  !insertmacro RestartManager_StartSession $R0
  ${If} $R0 != ""
    !insertmacro RestartManager_RegisterFile $R0 "${executablePath}"
    ${If} $0 = 0
      System::Call 'RSTRTMGR::RmGetList(p R0, *i .r1, *i .r2, p 0, *i .r3) i .r0'
      ; ERROR_MORE_DATA : au moins un processus utilise ce fichier.
      ${If} $0 = ${ERROR_MORE_DATA}
        System::Call 'RSTRTMGR::RmShutdown(p R0, i ${RmForceShutdown}, p 0) i .r0'
      ${EndIf}
    ${EndIf}
    !insertmacro RestartManager_EndSession $R0
  ${EndIf}
!macroend

; L'Ollama intégré : le serveur, puis les programmes de llama.cpp qu'il lance ;
; et le moteur d'images (sd-server), lancé par l'app comme l'intégré.
!macro CHATONLM_STOP_BUILTIN
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\ollama.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-server.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-quantize.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\sd-server\sd-server.exe"
!macroend

; L'app d'abord : sa fin ferme le Job Object de l'intégré, que son
; superviseur ne peut alors plus relancer ; puis l'intégré par son chemin,
; pour un reste hors Job.
!macro CHATONLM_STOP_ALL
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_STOP_BUILTIN
!macroend

; Seulement dans une installation de ChatonLM déjà en place (son exécutable
; est là) : un dossier d'installation saisi à la main peut contenir un dossier
; « ollama » qui n'est pas le nôtre (%LOCALAPPDATA%\Programs contient l'Ollama
; officiel) ; rien n'y est arrêté ni effacé.
!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro CHATONLM_STOP_ALL
    ; Bibliothèques d'une version précédente de l'Ollama intégré : retirées
    ; avant la copie (comme [InstallDelete] {app}\lib\ollama de l'officiel).
    ${If} ${FileExists} "$INSTDIR\ollama\ollama.exe"
      RMDir /r "$INSTDIR\ollama\lib\ollama"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro CHATONLM_STOP_ALL
  ${EndIf}
!macroend

; Le modèle supprime uninstall.exe et l'exécutable de l'app une seule fois,
; sans nouvel essai : si l'un est tenu à cet instant (analyse antivirus d'un
; fichier neuf, fin du premier processus du désinstalleur), il reste dans le
; dossier d'installation, que le modèle ne peut alors plus retirer (vu sur le
; runner de la CI, pour l'un puis pour l'autre).
; Nouvel essai après une demi-seconde, seulement si le fichier est encore là.
!macro CHATONLM_DELETE_AGAIN filePath
  ${If} ${FileExists} "${filePath}"
    Sleep 500
    Delete "${filePath}"
  ${EndIf}
!macroend

; Après la désinstallation : jusqu'à dix essais (cinq secondes) pour chacun,
; puis le dossier d'installation, retiré seulement s'il est vide (sans « /r »).
!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  !insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
!macroend
