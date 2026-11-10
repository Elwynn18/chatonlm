# Essai de fumée de la CI Windows (.github/workflows/packages-windows.yml),
# jamais sur un bureau réel : installe l'installateur NSIS de ChatonLM en
# silence, lance l'app installée, puis la désinstalle. PowerShell 7.
#
#   packaging/ci/smoke-windows.ps1 -Installer out\ChatonLM_x.y.z_x64-setup.exe
#
# Étapes (chaque contrôle affiche « ✓ » ou « ✗ » ; code 1 s'il y a un « ✗ ») :
# 1. Installation silencieuse (/S), sans élévation : fichiers attendus dans
#    %LOCALAPPDATA%\ChatonLM, clé de désinstallation sous HKCU, raccourci du
#    menu Démarrer.
# 2. Lancement, adresse d'Ollama fermée (OLLAMA_HOST, par prudence : jamais
#    un serveur de la machine) : l'app vit encore après -Wait secondes, un
#    processus msedgewebview2.exe est son enfant, pas de « panicked » ; l'app
#    arrêtée DE FORCE (pas de SIGTERM sous Windows), il ne doit rester aucun
#    processus de l'installation 15 s plus tard.
# 3. Le moteur d'images (sd-server) : version du fichier VERSION, et un
#    modèle placé dans un dossier accentué est bien OUVERT (page de code
#    UTF-8, packaging/sdcpp/0002-…) : illisible, il est refusé avec le code 1
#    après « using gguf format ».
#    Puis le moteur de texte (llama-server) : fichiers conformes à SHA256SUMS,
#    version du fichier VERSION, modèle illisible d'un dossier accentué ouvert
#    puis refusé (code 1, « failed to load model »), et un vrai petit modèle
#    (stories260K, empreinte épinglée) sur le processeur : /health, 401 sans
#    la clé d'API, complétion avec elle.
# 4. Réinstallation silencieuse par-dessus une app et un llama-server en
#    marche, et par-dessus un ancien Ollama intégré (dossier ollama\ des
#    versions 0.9.x, reconstitué, son runner en marche) : les crochets de
#    packaging/windows/hooks.nsh doivent les arrêter (par leur chemin),
#    retirer nos fichiers du dossier llama-server (une variante
#    ggml-cpu-*.dll d'une autre version n'y reste pas, un fichier de
#    l'utilisateur si), retirer l'ancien Ollama intégré et ses notices (un
#    fichier de l'utilisateur posé dans ollama\ reste), l'installateur
#    réussir, et rien ne rester en marche.
# 5. Désinstallation silencieuse : programme, clé et raccourcis disparus,
#    données de l'utilisateur GARDÉES, aucun processus restant.
#
# Dossiers de données temporaires dont le nom porte une espace et un accent,
# exprès (-PlainPaths pour s'en passer). Journaux copiés dans -LogDir.
param(
  [Parameter(Mandatory = $true)][string]$Installer,
  [string]$LogDir = "out/smoke",
  [int]$Wait = 20,
  [switch]$PlainPaths
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$ProductName = "ChatonLM"
$MainBinary = "chatonlm.exe"
$UninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"

$script:Failed = $false
function Pass([string]$Message) { Write-Host "✓ $Message" }
function Fail([string]$Message) {
  Write-Host "✗ $Message"
  $script:Failed = $true
}
function Section([string]$Title) { Write-Host ""; Write-Host "== $Title ==" }

# Processus dont l'exécutable est sous `$Dir` (ceux de l'installation).
function Get-ProcessesUnder([string]$Dir) {
  $prefix = $Dir.TrimEnd('\') + '\'
  @(Get-CimInstance Win32_Process | Where-Object {
      $_.ExecutablePath -and
      $_.ExecutablePath.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
    })
}

function Format-Processes($Processes) {
  (@($Processes) | Where-Object { $_ } | ForEach-Object { "$($_.ProcessId) $($_.ExecutablePath)" }) -join " ; "
}

# Attend qu'il ne reste aucun processus sous `$Dir` ; rend ceux qui restent.
function Wait-NoProcessUnder([string]$Dir, [int]$Seconds) {
  $left = @()
  for ($i = 0; $i -lt $Seconds * 2; $i++) {
    $left = @(Get-ProcessesUnder $Dir)
    if ($left.Count -eq 0) { break }
    Start-Sleep -Milliseconds 500
  }
  return , $left
}

function Stop-ProcessesUnder([string]$Dir) {
  foreach ($process in @(Get-ProcessesUnder $Dir)) {
    Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
  }
}

function Test-Port([int]$Port) {
  $client = [System.Net.Sockets.TcpClient]::new()
  try {
    $task = $client.ConnectAsync("127.0.0.1", $Port)
    return ($task.Wait(1500) -and $client.Connected)
  } catch {
    return $false
  } finally {
    $client.Dispose()
  }
}

# Lance un programme avec sa sortie dans des fichiers ; rend le processus.
function Start-Logged([string]$FilePath, [string[]]$Arguments, [string]$LogBase) {
  $parameters = @{
    FilePath               = $FilePath
    PassThru               = $true
    RedirectStandardOutput = "$LogBase.out.log"
    RedirectStandardError  = "$LogBase.err.log"
  }
  if ($Arguments -and $Arguments.Count -gt 0) { $parameters.ArgumentList = $Arguments }
  $process = Start-Process @parameters
  # Sans cette lecture, ExitCode reste vide une fois le processus terminé.
  $null = $process.Handle
  return $process
}

# Lance l'installateur ou le désinstalleur en silence ; rend son code de
# sortie, ou $null après `$Seconds` secondes.
function Invoke-Silent([string]$FilePath, [string[]]$Arguments, [int]$Seconds) {
  $process = Start-Process -FilePath $FilePath -ArgumentList $Arguments -PassThru
  $null = $process.Handle
  if (-not $process.WaitForExit($Seconds * 1000)) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    return $null
  }
  return $process.ExitCode
}

function Show-Log([string]$Title, [string]$Path, [int]$Tail = 200) {
  Write-Host "--- $Title ---"
  if (Test-Path -LiteralPath $Path) {
    Get-Content -LiteralPath $Path -Tail $Tail -ErrorAction SilentlyContinue | ForEach-Object { Write-Host $_ }
  } else {
    Write-Host "(absent)"
  }
}

function Save-Log([string]$Path, [string]$Name) {
  if (Test-Path -LiteralPath $Path) {
    Copy-Item -LiteralPath $Path -Destination (Join-Path $LogDir $Name) -Force -ErrorAction SilentlyContinue
  }
}

# L'app installée ne doit rien devoir à l'environnement de la CI : ONNX
# Runtime de .tools (posé pour cargo test), réglages d'Ollama.
foreach ($name in @("ORT_DYLIB_PATH", "OLLAMA_HOST", "OLLAMA_MODELS", "CHATONLM_DATA_DIR")) {
  Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}

$Installer = (Resolve-Path -LiteralPath $Installer).Path
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$LogDir = (Resolve-Path -LiteralPath $LogDir).Path
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("chatonlm-fumee-" + [System.Guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null
$dataName = if ($PlainPaths) { "donnees" } else { "données fumée" }

# --- Machine ---------------------------------------------------------------
Section "Machine"
$os = Get-CimInstance Win32_OperatingSystem
Write-Host "Windows : $($os.Caption) $($os.Version)"
$webview2 = $null
foreach ($key in @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}")) {
  $value = (Get-ItemProperty -Path $key -Name pv -ErrorAction SilentlyContinue).pv
  if ($value) { $webview2 = $value; break }
}
Write-Host "WebView2 avant l'installation : $(if ($webview2) { $webview2 } else { "absent (l'installateur le téléchargera)" })"
$redist = Test-Path "$env:SystemRoot\System32\vcruntime140.dll"
Write-Host "Redistribuable Visual C++ du système : $(if ($redist) { "présent (son absence n'est pas essayée ici : voir scripts/inspect-windows.mjs)" } else { "absent" })"
$sound = @(Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue).Count
Write-Host "Cartes son : $sound"
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Write-Host "Compte administrateur : $admin (l'installation par utilisateur n'en a pas besoin)"

# --- 1. Installation -------------------------------------------------------
Section "Installation silencieuse"
$code = Invoke-Silent $Installer @("/S") 900
if ($null -eq $code) {
  Fail "L'installateur ne s'est pas terminé en 15 min."
} elseif ($code -ne 0) {
  Fail "Installateur : code de sortie $code."
} else {
  Pass "Installateur terminé (code 0)."
}

$install = Join-Path $env:LOCALAPPDATA $ProductName
$registered = Get-ItemProperty -Path $UninstallKey -ErrorAction SilentlyContinue
if ($registered) {
  Pass "Clé de désinstallation : $($registered.DisplayName) $($registered.DisplayVersion)."
  if ($registered.InstallLocation) { $install = $registered.InstallLocation.Trim('"') }
} else {
  Fail "Clé de désinstallation absente ($UninstallKey)."
}
Write-Host "Dossier d'installation : $install"
$app = Join-Path $install $MainBinary
foreach ($file in @(
    $MainBinary, "uninstall.exe", "onnxruntime.dll", "vcruntime140.dll", "msvcp140.dll",
    "sd-server\sd-server.exe", "sd-server\VERSION", "sd-server\fallback\vulkan-1.dll",
    "llama-server\llama-server.exe", "llama-server\VERSION", "llama-server\SHA256SUMS",
    "llama-server\fallback\vulkan-1.dll",
    "licenses\LICENSE.txt", "licenses\THIRD_PARTY_LICENSES.txt",
    "licenses\sd-server\README.txt", "licenses\llama-server\README.txt")) {
  if (-not (Test-Path -LiteralPath (Join-Path $install $file))) { Fail "Fichier installé absent : $file" }
}
# Plus d'Ollama intégré depuis le moteur llama.cpp.
foreach ($dir in @("ollama", "licenses\ollama")) {
  if (Test-Path -LiteralPath (Join-Path $install $dir)) { Fail "Dossier de l'ancien Ollama intégré installé : $dir" }
}
if (Test-Path -LiteralPath $app) { Pass "Fichiers installés." }
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$ProductName.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath("Desktop")) "$ProductName.lnk"
if (Test-Path -LiteralPath $startMenu) { Pass "Raccourci du menu Démarrer." } else { Fail "Raccourci du menu Démarrer absent ($startMenu)." }
$webview2After = $null
foreach ($key in @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}")) {
  $value = (Get-ItemProperty -Path $key -Name pv -ErrorAction SilentlyContinue).pv
  if ($value) { $webview2After = $value; break }
}
if ($webview2After) { Pass "WebView2 : $webview2After." } else { Fail "WebView2 absent après l'installation." }

if (-not (Test-Path -LiteralPath $app)) {
  Write-Host "✗ $MainBinary n'est pas installé : essais de lancement impossibles."
  exit 1
}

# --- 2. Lancement, Ollama fermé --------------------------------------------
Section "Lancement (adresse d'Ollama fermée)"
$data = Join-Path $work "$dataName 1"
$env:CHATONLM_DATA_DIR = $data
$env:OLLAMA_HOST = "http://127.0.0.1:9"
$log = Join-Path $work "chatonlm-1"
$process = Start-Logged $app @() $log
Start-Sleep -Seconds $Wait
if ($process.HasExited) {
  Fail "ChatonLM s'est arrêté avant la fin de l'essai (code $($process.ExitCode))."
} else {
  Pass "ChatonLM vit encore après $Wait s."
  $views = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($process.Id) AND Name='msedgewebview2.exe'")
  if ($views.Count -gt 0) { Pass "msedgewebview2.exe : $($views.Count) enfant(s) de l'app." } else { Fail "Aucun processus msedgewebview2.exe lancé par l'app." }
}
if (Test-Path -LiteralPath $data) {
  Write-Host "Données : $((Get-ChildItem -LiteralPath $data -Name) -join ' ')"
} else {
  Fail "Dossier de données non créé ($data) : CHATONLM_DATA_DIR ignoré ?"
}
# Arrêt de force : rien de l'installation ne doit survivre à l'app.
Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
$left = Wait-NoProcessUnder $install 15
if ($left.Count -gt 0) {
  Fail "Processus encore actifs 15 s après l'arrêt forcé de l'app : $(Format-Processes $left)"
  Stop-ProcessesUnder $install
} else {
  Pass "Aucun processus de l'installation après l'arrêt forcé de l'app."
}
foreach ($suffix in @("out", "err")) {
  $file = "$log.$suffix.log"
  if ((Test-Path -LiteralPath $file) -and (Select-String -LiteralPath $file -Pattern "panicked" -Quiet)) { Fail "Panique dans le journal ($suffix)." }
  Save-Log $file "chatonlm-ferme.$suffix.log"
}
Show-Log "journal de ChatonLM (erreur standard)" "$log.err.log"
Remove-Item Env:OLLAMA_HOST -ErrorAction SilentlyContinue

# --- 3. Le moteur d'images seul --------------------------------------------
Section "Moteur d'images (sd-server)"
$sd = Join-Path $install "sd-server\sd-server.exe"
$sdVersionFile = Join-Path $install "sd-server\VERSION"
if ((Test-Path -LiteralPath $sd) -and (Test-Path -LiteralPath $sdVersionFile)) {
  $sdExpected = (Get-Content -LiteralPath $sdVersionFile -Raw).Trim().Split("+")[0]
  # Comme l'app (image_gen/windows.rs) : le chargeur de secours en fin de PATH,
  # System32 passe avant s'il a le sien.
  $systemLoader = Test-Path -LiteralPath (Join-Path $env:WINDIR "System32\vulkan-1.dll")
  Write-Host "vulkan-1.dll dans System32 : $systemLoader"
  $env:PATH = "$env:PATH;$(Join-Path $install 'sd-server\fallback')"
  $sdReported = (& $sd --version 2>&1 | Select-Object -First 1) -join ""
  $sdDevices = (& $sd --list-devices 2>&1) -join "; "
  if ($sdDevices -match "(^|; )CPU") { Pass "sd-server --list-devices : $sdDevices" } else { Fail "sd-server --list-devices sans processeur : $sdDevices" }
  if ($sdReported -like "*$sdExpected*") {
    Pass "sd-server --version : $sdReported"
  } else {
    Fail "sd-server --version : « $sdReported » (attendu : $sdExpected)."
  }
  $accented = Join-Path $work "Thé chaton"
  New-Item -ItemType Directory -Force -Path $accented | Out-Null
  $badModel = Join-Path $accented "modèle illisible.gguf"
  [IO.File]::WriteAllBytes($badModel, [byte[]](0x47, 0x47, 0x55, 0x46, 3, 0, 0, 0) + [Text.Encoding]::ASCII.GetBytes("not a model"))
  $sdLog = Join-Path $work "sd-server-accent"
  $sdProcess = Start-Logged $sd @("--listen-port", "18437", "--diffusion-model", "`"$badModel`"") $sdLog
  if (-not $sdProcess.WaitForExit(60000)) {
    Stop-Process -Id $sdProcess.Id -Force -ErrorAction SilentlyContinue
    Fail "sd-server ne s'est pas arrêté sur un modèle illisible en 60 s."
  } else {
    $sdOutput = (Get-Content -LiteralPath "$sdLog.out.log", "$sdLog.err.log" -Raw -ErrorAction SilentlyContinue) -join "`n"
    if ($sdProcess.ExitCode -ne 1) { Fail "sd-server : code de sortie $($sdProcess.ExitCode) sur un modèle illisible (attendu : 1)." }
    if ($sdOutput -match "using gguf format") {
      Pass "Modèle d'un dossier accentué ouvert (page de code UTF-8)."
    } else {
      Fail "Modèle d'un dossier accentué non ouvert par sd-server."
    }
    if ($sdOutput -notmatch "new_sd_ctx_t failed") { Fail "Erreur du modèle illisible absente de la sortie de sd-server." }
    Save-Log "$sdLog.out.log" "sd-server-accent.out.log"
    Save-Log "$sdLog.err.log" "sd-server-accent.err.log"
  }
} else {
  Fail "sd-server non installé : essai du moteur d'images impossible."
}

# --- 3 bis. Le moteur de texte seul ----------------------------------------
Section "Moteur de texte (llama-server)"
$llama = Join-Path $install "llama-server\llama-server.exe"
$llamaDir = Join-Path $install "llama-server"
$llamaVersionFile = Join-Path $llamaDir "VERSION"
# Modèle d'essai : tinyllamas de Karpathy (MIT) en GGUF, révision figée ; même
# fichier que packaging/ci/smoke-llama-server.sh.
$llamaModelUrl = "https://huggingface.co/ggml-org/models-moved/resolve/499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf"
$llamaModelSha256 = "270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d"
$llamaModel = $null
if ((Test-Path -LiteralPath $llama) -and (Test-Path -LiteralPath $llamaVersionFile)) {
  # Fichiers installés identiques à ceux de SHA256SUMS.
  $sums = Get-Content -LiteralPath (Join-Path $llamaDir "SHA256SUMS")
  $bad = @()
  foreach ($line in $sums) {
    if ($line -notmatch '^([0-9a-f]{64}) [ *](.+)$') { continue }
    $file = Join-Path $llamaDir ($Matches[2] -replace '/', '\')
    $hash = if (Test-Path -LiteralPath $file) { (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLower() } else { "" }
    if ($hash -ne $Matches[1]) { $bad += $Matches[2] }
  }
  if ($sums.Count -gt 0 -and $bad.Count -eq 0) { Pass "llama-server : $($sums.Count) fichiers conformes à SHA256SUMS." } else { Fail "llama-server : fichiers différents de SHA256SUMS : $($bad -join ', ')" }
  $llamaExpected = "version: " + (Get-Content -LiteralPath $llamaVersionFile -Raw).Trim().Split("+")[0].TrimStart("v") + " (build"
  # Comme l'app : le chargeur de secours en fin de PATH.
  $env:PATH = "$env:PATH;$(Join-Path $llamaDir 'fallback')"
  $llamaReported = (& $llama --version 2>&1 | Select-Object -First 1) -join ""
  if ($llamaReported -like "$llamaExpected*") { Pass "llama-server --version : $llamaReported" } else { Fail "llama-server --version : « $llamaReported » (attendu : $llamaExpected…)." }
  $llamaDevices = (& $llama --list-devices 2>&1) -join "; "
  if ($llamaDevices -match "^Available devices:") { Pass "llama-server --list-devices : $llamaDevices" } else { Fail "llama-server --list-devices illisible : $llamaDevices" }
  # Modèle illisible dans un dossier accentué : ouvert (page de code UTF-8
  # du manifeste de llama.cpp), puis refusé avec le code 1.
  $accented = Join-Path $work "Thé chaton"
  New-Item -ItemType Directory -Force -Path $accented | Out-Null
  $badModel = Join-Path $accented "modèle illisible.gguf"
  [IO.File]::WriteAllBytes($badModel, [byte[]](0x47, 0x47, 0x55, 0x46, 3, 0, 0, 0) + [Text.Encoding]::ASCII.GetBytes("not a model"))
  $llamaLog = Join-Path $work "llama-server-accent"
  $llamaProcess = Start-Logged $llama @("--host", "127.0.0.1", "--port", "18438", "-m", "`"$badModel`"", "-c", "256", "--no-webui", "--offline") $llamaLog
  if (-not $llamaProcess.WaitForExit(60000)) {
    Stop-Process -Id $llamaProcess.Id -Force -ErrorAction SilentlyContinue
    Fail "llama-server ne s'est pas arrêté sur un modèle illisible en 60 s."
  } else {
    $llamaOutput = (Get-Content -LiteralPath "$llamaLog.out.log", "$llamaLog.err.log" -Raw -ErrorAction SilentlyContinue) -join "`n"
    if ($llamaProcess.ExitCode -ne 1) { Fail "llama-server : code de sortie $($llamaProcess.ExitCode) sur un modèle illisible (attendu : 1)." }
    if ($llamaOutput -match "gguf_init_from_reader") { Pass "llama-server : modèle d'un dossier accentué ouvert." } else { Fail "llama-server : modèle d'un dossier accentué non ouvert." }
    if ($llamaOutput -notmatch "failed to load model") { Fail "Erreur du modèle illisible absente de la sortie de llama-server." }
    Save-Log "$llamaLog.err.log" "llama-server-accent.err.log"
  }
  # Vrai petit modèle, sur le processeur.
  $llamaModel = Join-Path $work "stories260K.gguf"
  Invoke-WebRequest -Uri $llamaModelUrl -OutFile $llamaModel -UseBasicParsing
  if ((Get-FileHash -LiteralPath $llamaModel -Algorithm SHA256).Hash.ToLower() -ne $llamaModelSha256) {
    Fail "Empreinte inattendue du modèle d'essai."
    $llamaModel = $null
  } else {
    $key = "essai-" + [System.Guid]::NewGuid().ToString("N")
    $llamaLog = Join-Path $work "llama-server-run"
    $llamaProcess = Start-Logged $llama @("--host", "127.0.0.1", "--port", "18438", "--api-key", $key, "-m", "`"$llamaModel`"", "-c", "256", "-np", "1", "-ngl", "0", "--no-webui", "--offline") $llamaLog
    $ready = $false
    for ($i = 0; $i -lt 240 -and -not $llamaProcess.HasExited; $i++) {
      try {
        if ((Invoke-WebRequest -Uri "http://127.0.0.1:18438/health" -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200) { $ready = $true; break }
      } catch { }
      Start-Sleep -Milliseconds 500
    }
    if (-not $ready) {
      # Code de sortie en hexadécimal : 0xC000001D instruction illégale,
      # 0xC0000005 accès interdit, 0xC0000409 arrêt rapide (tas abîmé).
      $state = if ($llamaProcess.HasExited) { "sorti, code 0x{0:X8}" -f $llamaProcess.ExitCode } else { "toujours en marche" }
      Fail "llama-server : /health ne répond pas ($state)."
      Show-Log "llama-server (erreurs)" "$llamaLog.err.log" 60
      Show-Log "llama-server (sortie)" "$llamaLog.out.log" 60
      Save-Log "$llamaLog.out.log" "llama-server-run.out.log"
      # Diagnostic : même lancement, journal détaillé, un seul fil.
      Stop-Process -Id $llamaProcess.Id -Force -ErrorAction SilentlyContinue
      $diagLog = Join-Path $work "llama-server-diag"
      $diag = Start-Logged $llama @("--host", "127.0.0.1", "--port", "18440", "--api-key", $key, "-m", "`"$llamaModel`"", "-c", "256", "-np", "1", "-ngl", "0", "-t", "1", "-lv", "4", "--no-webui", "--offline") $diagLog
      $null = $diag.WaitForExit(60000)
      $diagState = if ($diag.HasExited) { "sorti, code 0x{0:X8}" -f $diag.ExitCode } else { "toujours en marche" }
      Write-Host "Diagnostic de llama-server (-t 1 -lv 4) : $diagState"
      Show-Log "llama-server diagnostic (erreurs)" "$diagLog.err.log" 80
      Stop-Process -Id $diag.Id -Force -ErrorAction SilentlyContinue
      Save-Log "$diagLog.err.log" "llama-server-diag.err.log"
    } else {
      $denied = 0
      try {
        Invoke-WebRequest -Uri "http://127.0.0.1:18438/completion" -Method Post -ContentType "application/json" -Body '{"prompt":"Once","n_predict":4}' -UseBasicParsing | Out-Null
      } catch { $denied = [int]$_.Exception.Response.StatusCode }
      if ($denied -eq 401) { Pass "llama-server : requête sans clé d'API refusée (401)." } else { Fail "llama-server : requête sans clé d'API : code $denied (attendu : 401)." }
      try {
        $answer = Invoke-RestMethod -Uri "http://127.0.0.1:18438/completion" -Method Post -ContentType "application/json" -Headers @{ Authorization = "Bearer $key" } -Body '{"prompt":"Once upon a time","n_predict":8,"temperature":0}'
        if ($answer.content) { Pass "llama-server : complétion sur le processeur : « $($answer.content) »." } else { Fail "llama-server : complétion vide." }
      } catch {
        Fail "llama-server : complétion en échec : $($_.Exception.Message)"
      }
    }
    Stop-Process -Id $llamaProcess.Id -Force -ErrorAction SilentlyContinue
    $null = $llamaProcess.WaitForExit(10000)
    Save-Log "$llamaLog.err.log" "llama-server-run.err.log"
  }
} else {
  Fail "llama-server non installé : essai du moteur de texte impossible."
}

# --- 4. Réinstallation par-dessus une app en marche ------------------------
Section "Réinstallation par-dessus une app en marche"
$data = Join-Path $work "$dataName 3"
$env:CHATONLM_DATA_DIR = $data
$log = Join-Path $work "chatonlm-3"
$process = Start-Logged $app @() $log
Start-Sleep -Seconds $Wait
# Un llama-server en marche (lancé comme par l'app) et une variante laissée
# par une autre version : les crochets arrêtent l'un et retirent l'autre ; un
# fichier qui n'est pas à nous (un modèle de l'utilisateur) reste.
$stale = Join-Path $install "llama-server\ggml-cpu-ancienne.dll"
$foreign = Join-Path $install "llama-server\modele-utilisateur.gguf"
$foreignSet = $false
if (Test-Path -LiteralPath $llama) {
  Set-Content -LiteralPath $stale -Value "ancienne variante"
  Set-Content -LiteralPath $foreign -Value "fichier de l'utilisateur"
  $foreignSet = $true
  if ($llamaModel) {
    $null = Start-Logged $llama @("--host", "127.0.0.1", "--port", "18439", "-m", "`"$llamaModel`"", "-c", "256", "-ngl", "0", "--no-webui", "--offline") (Join-Path $work "llama-server-reinstall")
  }
}
# Ancien Ollama intégré d'une 0.9.x, reconstitué : ollama.exe (un fichier
# quelconque : seul son chemin compte), VERSION, lib\ollama\ (copie de
# llama-server, dont le serveur est lancé de là comme l'était son runner),
# notices licenses\ollama\ ; plus un fichier de l'utilisateur, qui doit rester.
$oldOllama = Join-Path $install "ollama"
$oldRunnerDir = Join-Path $oldOllama "lib\ollama"
$oldLicenses = Join-Path $install "licenses\ollama"
$oldForeign = Join-Path $oldOllama "notes-utilisateur.txt"
$oldSet = $false
if (Test-Path -LiteralPath $llama) {
  New-Item -ItemType Directory -Force -Path $oldRunnerDir, $oldLicenses | Out-Null
  Copy-Item -Path (Join-Path $llamaDir "*") -Destination $oldRunnerDir -Recurse -Force
  Copy-Item -LiteralPath $llama -Destination (Join-Path $oldOllama "ollama.exe") -Force
  Set-Content -LiteralPath (Join-Path $oldOllama "VERSION") -Value "0.40.15"
  Set-Content -LiteralPath (Join-Path $oldLicenses "README.txt") -Value "ancienne notice"
  Set-Content -LiteralPath $oldForeign -Value "fichier de l'utilisateur"
  $oldSet = $true
  if ($llamaModel) {
    $null = Start-Logged (Join-Path $oldRunnerDir "llama-server.exe") @("--host", "127.0.0.1", "--port", "18441", "-m", "`"$llamaModel`"", "-c", "256", "-ngl", "0", "--no-webui", "--offline") (Join-Path $work "ancien-runner")
  }
}
Write-Host "Processus de l'installation : $(Format-Processes (Get-ProcessesUnder $install))"
$code = Invoke-Silent $Installer @("/S") 900
if ($null -eq $code) {
  Fail "La réinstallation ne s'est pas terminée en 15 min."
} elseif ($code -ne 0) {
  Fail "Réinstallation : code de sortie $code."
} else {
  Pass "Réinstallation terminée (code 0)."
}
$left = Wait-NoProcessUnder $install 5
if ($left.Count -gt 0) {
  Fail "Processus de l'installation encore en marche après la réinstallation : $(Format-Processes $left)"
  Stop-ProcessesUnder $install
} else {
  Pass "App et moteurs arrêtés par l'installateur."
}
foreach ($file in @($MainBinary, "onnxruntime.dll", "llama-server\llama-server.exe", "llama-server\VERSION")) {
  if (-not (Test-Path -LiteralPath (Join-Path $install $file))) { Fail "Fichier absent après la réinstallation : $file" }
}
if (Test-Path -LiteralPath $stale) { Fail "Variante d'une autre version restée dans llama-server après la réinstallation." } else { Pass "Nos fichiers de llama-server retirés avant la copie (aucune variante d'une autre version)." }
if (Test-Path -LiteralPath $foreign) {
  Pass "Fichier de l'utilisateur gardé dans llama-server (seuls nos fichiers sont retirés)."
  # Retiré ici : la désinstallation (§ 5) doit vider le dossier d'installation.
  Remove-Item -LiteralPath $foreign -Force -ErrorAction SilentlyContinue
} elseif ($foreignSet) {
  Fail "Fichier de l'utilisateur effacé de llama-server par la réinstallation."
}
if ($oldSet) {
  $oldLeft = @(@("ollama\ollama.exe", "ollama\VERSION", "ollama\lib", "licenses\ollama") | Where-Object { Test-Path -LiteralPath (Join-Path $install $_) })
  if ($oldLeft.Count -eq 0) { Pass "Ancien Ollama intégré et ses notices retirés par la réinstallation." } else { Fail "Ancien Ollama intégré resté après la réinstallation : $($oldLeft -join ', ')" }
  if (Test-Path -LiteralPath $oldForeign) {
    Pass "Fichier de l'utilisateur gardé dans ollama\ (seuls nos fichiers sont retirés)."
    # Retirés ici : la désinstallation (§ 5) doit vider le dossier d'installation.
    Remove-Item -LiteralPath $oldForeign -Force -ErrorAction SilentlyContinue
    # Dossier vide seulement (jamais de question de Remove-Item sur un dossier plein).
    try { [System.IO.Directory]::Delete($oldOllama) } catch { Fail "Dossier ollama\ non vide après la réinstallation : $($_.Exception.Message)" }
  } else {
    Fail "Fichier de l'utilisateur effacé de ollama\ par la réinstallation."
  }
}
Save-Log "$log.err.log" "chatonlm-reinstallation.err.log"

# --- 5. Désinstallation ----------------------------------------------------
Section "Désinstallation silencieuse"
$uninstaller = Join-Path $install "uninstall.exe"
# Données par défaut de l'app (créées si un essai n'a pas suivi
# CHATONLM_DATA_DIR, ou par WebView2) : gardées elles aussi.
$defaultData = Join-Path $env:LOCALAPPDATA "com.chatonlm.chatonlm"
$hadDefaultData = Test-Path -LiteralPath $defaultData
Write-Host "Données par défaut ($defaultData) avant la désinstallation : $hadDefaultData"
if (Test-Path -LiteralPath $uninstaller) {
  # Le désinstalleur de NSIS se recopie ailleurs et rend la main aussitôt :
  # on attend la disparition de l'exécutable.
  $null = Invoke-Silent $uninstaller @("/S") 300
  for ($i = 0; $i -lt 240; $i++) {
    if (-not (Test-Path -LiteralPath $app)) { break }
    Start-Sleep -Milliseconds 500
  }
  Start-Sleep -Seconds 3
  if (Test-Path -LiteralPath $app) { Fail "$MainBinary encore là 2 min après la désinstallation." } else { Pass "Programme supprimé." }
  if (Test-Path -LiteralPath $install) {
    # Le désinstalleur peut finir après la disparition de l'exécutable (nouveaux
    # essais du crochet sur uninstall.exe) : on lui laisse 15 s.
    $rest = @()
    for ($i = 0; $i -lt 30; $i++) {
      $rest = @(Get-ChildItem -LiteralPath $install -Recurse -File -ErrorAction SilentlyContinue)
      if ($rest.Count -eq 0) { break }
      Start-Sleep -Milliseconds 500
    }
    if ($rest.Count -gt 0) {
      Fail "Fichiers restants dans $install : $(($rest | Select-Object -First 10 | ForEach-Object { $_.FullName.Substring($install.Length + 1) }) -join ', ')"
      # Diagnostic : fichier encore tenu, ou supprimable après coup ?
      Write-Host "Processus de désinstallation : $(Format-Processes @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^(Un|Au)_|uninstall' }))"
      foreach ($file in ($rest | Select-Object -First 3)) {
        try {
          Remove-Item -LiteralPath $file.FullName -Force -ErrorAction Stop
          Write-Host "  $($file.Name) : supprimable après coup (tenu seulement pendant la désinstallation)."
        } catch {
          Write-Host "  $($file.Name) : encore tenu ($($_.Exception.Message))"
        }
      }
    } elseif ($i -gt 0) {
      Write-Host "Dossier d'installation vidé $($i * 0.5) s après la disparition de l'exécutable."
    }
  }
  if (Get-ItemProperty -Path $UninstallKey -ErrorAction SilentlyContinue) { Fail "Clé de désinstallation encore présente." } else { Pass "Clé de désinstallation supprimée." }
  foreach ($shortcut in @($startMenu, $desktop)) {
    if (Test-Path -LiteralPath $shortcut) { Fail "Raccourci encore présent : $shortcut" }
  }
  if (Test-Path -LiteralPath $data) { Pass "Données de l'utilisateur gardées ($data)." } else { Fail "Données de l'utilisateur supprimées par la désinstallation silencieuse ($data)." }
  if ($hadDefaultData -and -not (Test-Path -LiteralPath $defaultData)) { Fail "Données par défaut supprimées par la désinstallation silencieuse ($defaultData)." }
  $left = Wait-NoProcessUnder $install 5
  if ($left.Count -gt 0) { Fail "Processus restants après la désinstallation : $(Format-Processes $left)"; Stop-ProcessesUnder $install }
} else {
  Fail "uninstall.exe absent ($uninstaller)."
}

Remove-Item Env:CHATONLM_DATA_DIR -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue

Write-Host ""
if ($script:Failed) {
  Write-Host "✗ Essai de ChatonLM en échec."
  exit 1
}
Write-Host "✓ Essai de ChatonLM réussi."
exit 0
