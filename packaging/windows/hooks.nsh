; Crochets de l'installateur NSIS de ChatonLM (bundle.windows.nsis.installerHooks
; de src-tauri/tauri.windows.conf.json), inclus par le modèle de Tauri après
; utils.nsh et Win\RestartManager.nsh.
;
; Le modèle ne surveille que l'exécutable principal. Avant de copier les
; fichiers (installation, mise à jour) et avant de les supprimer
; (désinstallation), ces crochets arrêtent l'app, puis les moteurs qu'elle
; lance ($INSTDIR\sd-server\, $INSTDIR\llama-server\), désignés par leur
; chemin exact. Jamais d'arrêt par nom (« taskkill /im llama-server.exe » ou
; « ollama.exe ») : il arrêterait le llama.cpp ou l'Ollama que l'utilisateur a
; installé lui-même.
;
; Ancien Ollama intégré : les versions 0.9.x livraient un Ollama dans
; $INSTDIR\ollama\, que les paquets ne contiennent plus. Une mise à jour (ou
; une désinstallation) depuis l'une d'elles arrête ses processus, désignés
; par leur chemin exact sous $INSTDIR\ollama\, puis retire ses fichiers
; (CHATONLM_CLEAN_OLD_OLLAMA) et ses notices (licenses\ollama\, écrites par
; nous) : rien d'autre, ni les données de l'utilisateur (dossier des données
; de l'app, dossier .ollama de son profil), ni un fichier qu'il aurait posé
; dans ce dossier.
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

; $R1 = 1 si $INSTDIR\ollama\ est l'Ollama intégré d'une version 0.9.x
; (ollama.exe et le fichier VERSION que ces versions y installaient,
; absent de l'Ollama officiel), 0 sinon.
!macro CHATONLM_OLD_OLLAMA
  StrCpy $R1 0
  ${If} ${FileExists} "$INSTDIR\ollama\ollama.exe"
  ${AndIf} ${FileExists} "$INSTDIR\ollama\VERSION"
    StrCpy $R1 1
  ${EndIf}
!macroend

; Ancien Ollama intégré : le serveur, puis les programmes de llama.cpp qu'il
; lançait, par leur chemin exact.
!macro CHATONLM_STOP_OLD_OLLAMA
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\ollama.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-server.exe"
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-quantize.exe"
!macroend

; Retire les fichiers de l'ancien Ollama intégré : ollama.exe, VERSION et
; lib\ollama\ (son runner et ses bibliothèques, tous livrés par nous), avec
; jusqu'à dix nouveaux essais (cinq secondes) pour un fichier encore tenu un
; instant ; puis lib\ et ollama\, seulement vides (sans « /r »). VERSION,
; la marque, n'est retiré qu'avec ollama.exe : un reste encore tenu sera
; reconnu et retiré à la désinstallation. Registre : $R3.
!macro CHATONLM_REMOVE_OLD_OLLAMA
  ${For} $R3 0 10
    ${If} $R3 > 0
      ${IfNot} ${FileExists} "$INSTDIR\ollama\ollama.exe"
      ${AndIfNot} ${FileExists} "$INSTDIR\ollama\lib\ollama\*.*"
        ${ExitFor}
      ${EndIf}
      Sleep 500
    ${EndIf}
    Delete "$INSTDIR\ollama\ollama.exe"
    RMDir /r "$INSTDIR\ollama\lib\ollama"
  ${Next}
  ${IfNot} ${FileExists} "$INSTDIR\ollama\ollama.exe"
    Delete "$INSTDIR\ollama\VERSION"
  ${EndIf}
  RMDir "$INSTDIR\ollama\lib"
  RMDir "$INSTDIR\ollama"
!macroend

; Ancien Ollama intégré (mise à jour depuis une 0.9.x, ou reste qu'une mise à
; jour n'a pas pu retirer) : ses fichiers s'il est le nôtre, et ses notices
; (licenses\ollama\, dossier écrit par nous seuls). Ni la nouvelle version ni
; son désinstalleur ne les connaissent : restés là, ils empêcheraient aussi
; la désinstallation de retirer le dossier d'installation. Registres : $R1,
; $R3. À appeler après CHATONLM_STOP_ALL.
!macro CHATONLM_CLEAN_OLD_OLLAMA
  !insertmacro CHATONLM_OLD_OLLAMA
  ${If} $R1 = 1
    !insertmacro CHATONLM_REMOVE_OLD_OLLAMA
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\licenses\ollama\*.*"
    RMDir /r "$INSTDIR\licenses\ollama"
  ${EndIf}
!macroend

; Les moteurs d'images (sd-server) et de texte (llama-server), lancés par
; l'app, et l'ancien Ollama intégré s'il est encore là.
!macro CHATONLM_STOP_ENGINES
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\sd-server\sd-server.exe"
  ; Seulement s'il est le nôtre : un dossier llama-server\ trouvé dans un
  ; dossier d'installation saisi à la main peut être le llama.cpp de
  ; l'utilisateur, et son serveur ne nous regarde pas.
  !insertmacro CHATONLM_OUR_LLAMA
  ${If} $R1 = 1
    !insertmacro CHATONLM_STOP_FILE "$INSTDIR\llama-server\llama-server.exe"
  ${EndIf}
  !insertmacro CHATONLM_OLD_OLLAMA
  ${If} $R1 = 1
    !insertmacro CHATONLM_STOP_OLD_OLLAMA
  ${EndIf}
!macroend

; L'app d'abord : sa fin ferme le Job Object de ses moteurs, qu'elle ne peut
; alors plus relancer ; puis les moteurs par leur chemin, pour un reste hors
; Job.
!macro CHATONLM_STOP_ALL
  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro CHATONLM_STOP_ENGINES
!macroend

; Seulement dans une installation de ChatonLM déjà en place (son exécutable
; est là) : un dossier d'installation saisi à la main peut contenir un dossier
; « ollama » ou « llama-server » qui n'est pas le nôtre (%LOCALAPPDATA%\Programs
; contient l'Ollama officiel) ; rien n'y est arrêté ni effacé.
!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro CHATONLM_STOP_ALL
    !insertmacro CHATONLM_CLEAN_OLD_OLLAMA
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
    !insertmacro CHATONLM_CLEAN_OLD_OLLAMA
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
