; Crochets de l'installateur NSIS de ChatonLM (bundle.windows.nsis.installerHooks
; de src-tauri/tauri.windows.conf.json), inclus par le modèle de Tauri après
; utils.nsh et Win\RestartManager.nsh.
;
; Le modèle ne surveille que l'exécutable principal. Avant de copier les
; fichiers (installation, mise à jour) et avant de les supprimer
; (désinstallation), ces crochets arrêtent l'Ollama intégré de CETTE
; installation, désigné par son chemin exact ($INSTDIR\ollama\...), puis
; l'app, puis de nouveau l'intégré (l'app, encore en marche, a pu le relancer
; entre-temps), et de même les moteurs que l'app lance ($INSTDIR\sd-server\,
; $INSTDIR\llama-server\). Jamais d'arrêt par nom (« taskkill /im
; ollama.exe » ou « llama-server.exe ») : il arrêterait l'Ollama ou le
; llama.cpp que l'utilisateur a installé lui-même.
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

; $R1 = 1 si $INSTDIR\llama-server\VERSION porte la marque de nos
; constructions (« <version>+chatonlm.<empreinte> », écrite par
; scripts/build-llama-server.sh), 0 sinon : dossier absent, fichier
; illisible, ou llama.cpp de l'utilisateur. Registres : $R1 à $R5.
!macro CHATONLM_OUR_LLAMA
  StrCpy $R1 0
  ClearErrors
  FileOpen $R2 "$INSTDIR\llama-server\VERSION" r
  ${IfNot} ${Errors}
    FileRead $R2 $R3 200
    FileClose $R2
    StrLen $R4 $R3
    ${For} $R2 0 $R4
      StrCpy $R5 $R3 10 $R2
      ${If} $R5 == "+chatonlm."
        StrCpy $R1 1
        ${ExitFor}
      ${EndIf}
    ${Next}
  ${EndIf}
!macroend

; Ce que scripts/build-llama-server.sh livre dans llama-server\, par motifs
; (le lanceur, les DLL de llama.cpp et de ggml, dont les variantes
; ggml-cpu-*.dll, le runtime Visual C++, le chargeur Vulkan de secours,
; SHA256SUMS), sauf VERSION, la marque, retirée en dernier. Rien d'autre :
; un fichier de l'utilisateur (un modèle .gguf…) reste. L'inspection
; (scripts/inspect-windows.mjs) vérifie que chaque fichier livré y répond.
; `action` : macro appliquée à chaque motif.
!macro CHATONLM_LLAMA_FILES action
  !insertmacro ${action} "$INSTDIR\llama-server\llama-server.exe"
  !insertmacro ${action} "$INSTDIR\llama-server\llama*.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\mtmd*.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\ggml*.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\vcruntime140*.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\msvcp140*.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\concrt140*.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\fallback\vulkan-1.dll"
  !insertmacro ${action} "$INSTDIR\llama-server\SHA256SUMS"
!macroend

!macro CHATONLM_DELETE_PATTERN pattern
  Delete "${pattern}"
!macroend

; $R2 + 1 si un fichier répond encore au motif.
!macro CHATONLM_COUNT_LEFT pattern
  ${If} ${FileExists} "${pattern}"
    IntOp $R2 $R2 + 1
  ${EndIf}
!macroend

; Retire nos fichiers de llama-server\ (d'une version précédente, avant la
; copie de la nouvelle). Un fichier peut être encore tenu un instant après
; l'arrêt du serveur (fin du processus, analyse antivirus) : jusqu'à dix
; nouveaux essais (cinq secondes), seulement s'il en reste. Les dossiers
; ne sont retirés que vides (sans « /r »). Registres : $R2 et $R3.
!macro CHATONLM_REMOVE_LLAMA
  !insertmacro CHATONLM_LLAMA_FILES CHATONLM_DELETE_PATTERN
  ${For} $R3 1 10
    StrCpy $R2 0
    !insertmacro CHATONLM_LLAMA_FILES CHATONLM_COUNT_LEFT
    ${If} $R2 = 0
      ${ExitFor}
    ${EndIf}
    Sleep 500
    !insertmacro CHATONLM_LLAMA_FILES CHATONLM_DELETE_PATTERN
  ${Next}
  Delete "$INSTDIR\llama-server\VERSION"
  RMDir "$INSTDIR\llama-server\fallback"
  RMDir "$INSTDIR\llama-server"
!macroend

; L'Ollama intégré : le serveur, puis les programmes de llama.cpp qu'il lance ;
; puis les moteurs d'images (sd-server) et de texte (llama-server), lancés
; par l'app comme l'intégré.
!macro CHATONLM_STOP_BUILTIN
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\ollama.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-server.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-quantize.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\sd-server\sd-server.exe"
  ; Seulement s'il est le nôtre : un dossier llama-server\ trouvé dans un
  ; dossier d'installation saisi à la main peut être le llama.cpp de
  ; l'utilisateur, et son serveur ne nous regarde pas.
  !insertmacro CHATONLM_OUR_LLAMA
  ${If} $R1 = 1
    !insertmacro CHATONLM_STOP_FILE "$INSTDIR\llama-server\llama-server.exe"
  ${EndIf}
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
    ; Moteur de texte d'une version précédente : ses fichiers sont retirés
    ; avant la copie. Il charge toutes les variantes ggml-cpu-*.dll de son
    ; dossier ; une variante laissée par une autre version (absente de la
    ; nouvelle) serait chargée avec des bibliothèques qui ne sont pas les
    ; siennes. Seulement si le dossier porte notre marque, et jamais de
    ; « RMDir /r » : le reste du dossier n'est pas à nous.
    !insertmacro CHATONLM_OUR_LLAMA
    ${If} $R1 = 1
      !insertmacro CHATONLM_REMOVE_LLAMA
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
