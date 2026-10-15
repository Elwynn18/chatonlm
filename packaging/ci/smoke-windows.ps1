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
# 2. Lancement, adresse d'Ollama fermée (OLLAMA_HOST, adresse explicite :
#    jamais l'Ollama intégré) : l'app vit encore après -Wait secondes, un
#    processus msedgewebview2.exe est son enfant, pas de « panicked ».
# 3. Lancement sans OLLAMA_HOST sur une machine sans Ollama : l'app démarre
#    elle-même l'Ollama intégré de son installation. <données>\logs\ollama.log
#    annonce « Listening on 127.0.0.1:<port> », ce port répond à /api/version
#    avec la version du fichier VERSION installé, le serveur est
#    <installation>\ollama\ollama.exe lancé par l'app (« serve »), son dossier
#    des modèles est sous les données de l'app. L'app est ensuite arrêtée DE
#    FORCE (pas de SIGTERM sous Windows) : il ne doit rester aucun processus
#    de l'installation 15 s plus tard.
# 4. L'Ollama intégré lancé seul (« ollama serve » sur 127.0.0.1:18434) :
#    version, moteur de calcul décrit (« inference compute »), arrêt.
#    Puis le moteur d'images (sd-server) : version du fichier VERSION, et un
#    modèle placé dans un dossier accentué est bien OUVERT (page de code
#    UTF-8, packaging/sdcpp/0002-…) : illisible, il est refusé avec le code 1
#    après « using gguf format ».
# 5. Réinstallation silencieuse par-dessus une app et un Ollama intégré en
#    marche : les crochets de packaging/windows/hooks.nsh doivent les arrêter
#    (par leur chemin), l'installateur réussir, et rien ne rester en marche.
# 6. Désinstallation silencieuse : programme, clé et raccourcis disparus,
#    données de l'utilisateur GARDÉES, aucun processus restant.
#
# Dossiers de données temporaires dont le nom porte une espace et un accent,
# exprès (-PlainPaths pour s'en passer). Journaux copiés dans -LogDir.
param(
  [Parameter(Mandatory = $true)][string]$Installer,
  [string]$LogDir = "out/smoke",
  [int]$Wait = 20,
  [int]$BuiltinWait = 180,
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

# Attend le port annoncé par l'Ollama intégré dans son journal.
function Wait-BuiltinPort([string]$Log, [System.Diagnostics.Process]$App, [int]$Seconds) {
  for ($i = 0; $i -lt $Seconds * 2; $i++) {
    if (Test-Path -LiteralPath $Log) {
      $found = Select-String -LiteralPath $Log -Pattern 'Listening on 127\.0\.0\.1:(\d+)' -AllMatches -ErrorAction SilentlyContinue
      if ($found) {
        return [int]$found[-1].Matches[-1].Groups[1].Value
      }
    }
    if ($App.HasExited) { break }
    Start-Sleep -Milliseconds 500
  }
  return 0
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

# L'intégré ne démarre seul que si aucun Ollama n'est installé ni ne répond.
$builtinPossible = $true
if (Test-Port 11434) {
  Fail "Un serveur répond déjà sur 127.0.0.1:11434 : essai de l'Ollama intégré impossible ici."
  $builtinPossible = $false
}
$other = Get-Command ollama -ErrorAction SilentlyContinue
if ($other) {
  Fail "Commande ollama installée ($($other.Source)) : essai de l'Ollama intégré impossible ici."
  $builtinPossible = $false
}
if (Test-Path "$env:LOCALAPPDATA\Programs\Ollama") {
  Fail "Ollama est installé sur cette machine : essai de l'Ollama intégré impossible ici."
  $builtinPossible = $false
}

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
    "ollama\ollama.exe", "ollama\VERSION", "ollama\lib\ollama\llama-server.exe",
    "sd-server\sd-server.exe", "sd-server\VERSION", "sd-server\fallback\vulkan-1.dll",
    "licenses\LICENSE.txt", "licenses\THIRD_PARTY_LICENSES.txt", "licenses\ollama\README.txt",
    "licenses\sd-server\README.txt")) {
  if (-not (Test-Path -LiteralPath (Join-Path $install $file))) { Fail "Fichier installé absent : $file" }
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
$shippedVersion = ""
$versionFile = Join-Path $install "ollama\VERSION"
if (Test-Path -LiteralPath $versionFile) { $shippedVersion = (Get-Content -LiteralPath $versionFile -Raw).Trim() }

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
  $builtins = @(Get-ProcessesUnder (Join-Path $install "ollama"))
  if ($builtins.Count -gt 0) { Fail "Ollama intégré lancé malgré une adresse explicite : $(Format-Processes $builtins)" }
}
if (Test-Path -LiteralPath $data) {
  Write-Host "Données : $((Get-ChildItem -LiteralPath $data -Name) -join ' ')"
} else {
  Fail "Dossier de données non créé ($data) : CHATONLM_DATA_DIR ignoré ?"
}
Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
$left = Wait-NoProcessUnder $install 15
if ($left.Count -gt 0) { Fail "Processus restants après l'arrêt de l'app : $(Format-Processes $left)"; Stop-ProcessesUnder $install }
foreach ($suffix in @("out", "err")) {
  $file = "$log.$suffix.log"
  if ((Test-Path -LiteralPath $file) -and (Select-String -LiteralPath $file -Pattern "panicked" -Quiet)) { Fail "Panique dans le journal ($suffix)." }
  Save-Log $file "chatonlm-ferme.$suffix.log"
}
Show-Log "journal de ChatonLM (erreur standard)" "$log.err.log"
Remove-Item Env:OLLAMA_HOST -ErrorAction SilentlyContinue

# --- 3. Lancement avec l'Ollama intégré ------------------------------------
Section "Lancement avec l'Ollama intégré"
if ($builtinPossible) {
  $data = Join-Path $work "$dataName 2"
  $env:CHATONLM_DATA_DIR = $data
  $log = Join-Path $work "chatonlm-2"
  $ollamaLog = Join-Path $data "logs\ollama.log"
  $process = Start-Logged $app @() $log
  $port = Wait-BuiltinPort $ollamaLog $process $BuiltinWait
  if ($port -eq 0) {
    Fail "L'Ollama intégré n'écoute pas (rien dans $ollamaLog après $BuiltinWait s)."
  } else {
    Pass "Ollama intégré lancé par l'app : 127.0.0.1:$port."
    $expected = Join-Path $install "ollama\ollama.exe"
    $servers = @(Get-ProcessesUnder (Join-Path $install "ollama") | Where-Object {
        $_.Name -eq "ollama.exe" -and $_.CommandLine -match '\bserve\b'
      })
    if ($servers.Count -eq 0) {
      Fail "Aucun processus « ollama serve » de l'installation."
    } else {
      $server = $servers[0]
      if ($server.ExecutablePath -ieq $expected) { Pass "Binaire de l'installation : $($server.ExecutablePath) (pid $($server.ProcessId))." } else { Fail "Binaire inattendu : $($server.ExecutablePath) (attendu : $expected)." }
      if ($server.ParentProcessId -eq $process.Id) { Pass "Lancé par l'app (pid $($process.Id))." } else { Fail "Parent du serveur : $($server.ParentProcessId), et non l'app ($($process.Id))." }
    }
    try {
      $answer = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/version" -TimeoutSec 10
      if ($shippedVersion -and $answer.version -eq $shippedVersion) { Pass "/api/version : $shippedVersion." } else { Fail "/api/version : « $($answer.version) » au lieu de la version livrée « $shippedVersion »." }
    } catch {
      Fail "/api/version ne répond pas : $($_.Exception.Message)"
    }
    # Dossier des modèles : sous les données de l'app (ligne « server config »
    # du journal d'Ollama ; le dossier de l'essai porte un jeton unique).
    $token = Split-Path -Leaf $work
    $config = Select-String -LiteralPath $ollamaLog -Pattern 'msg="server config"' -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $config) {
      Write-Host "⚠ Ligne « server config » absente du journal : dossier des modèles non vérifié."
    } elseif ($config.Line -match "OLLAMA_MODELS:[^\]]*$token") {
      Pass "Dossier des modèles sous les données de l'app."
    } else {
      Fail "OLLAMA_MODELS hors des données de l'app : $($config.Line)"
    }
  }
  if ($process.HasExited) { Fail "ChatonLM s'est arrêté pendant l'essai (code $($process.ExitCode))." }

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
    Save-Log $file "chatonlm-integre.$suffix.log"
  }
  Save-Log $ollamaLog "ollama-integre.log"
  Show-Log "journal de l'Ollama intégré" $ollamaLog
  Show-Log "journal de ChatonLM (erreur standard)" "$log.err.log"
} else {
  Write-Host "(essai sauté : voir plus haut)"
}

# --- 4. L'Ollama intégré seul ----------------------------------------------
Section "Ollama intégré seul"
$bin = Join-Path $install "ollama\ollama.exe"
$alone = Join-Path $work "ollama-seul"
New-Item -ItemType Directory -Force -Path "$alone\home", "$alone\models" | Out-Null
$saved = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME; OLLAMA_MODELS = $env:OLLAMA_MODELS; OLLAMA_HOST = $env:OLLAMA_HOST }
$env:USERPROFILE = "$alone\home"
$env:HOME = "$alone\home"
$env:OLLAMA_MODELS = "$alone\models"
$env:OLLAMA_HOST = "127.0.0.1:18434"
$serve = Start-Logged $bin @("serve") (Join-Path $work "ollama-seul")
foreach ($name in $saved.Keys) {
  if ($null -eq $saved[$name]) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue } else { Set-Item "Env:$name" $saved[$name] }
}
$version = ""
for ($i = 0; $i -lt 60; $i++) {
  try {
    $version = (Invoke-RestMethod -Uri "http://127.0.0.1:18434/api/version" -TimeoutSec 2).version
  } catch {
    $version = ""
  }
  if ($version -or $serve.HasExited) { break }
  Start-Sleep -Milliseconds 500
}
if ($shippedVersion -and $version -eq $shippedVersion) { Pass "Ollama intégré $shippedVersion : répond sur 127.0.0.1:18434." } else { Fail "Ollama intégré seul : réponse « $version » au lieu de la version $shippedVersion." }
$serveLog = Join-Path $work "ollama-seul.err.log"
$compute = $false
for ($i = 0; $i -lt 20; $i++) {
  if ((Test-Path -LiteralPath $serveLog) -and (Select-String -LiteralPath $serveLog -Pattern 'msg="inference compute"' -Quiet)) { $compute = $true; break }
  Start-Sleep -Milliseconds 500
}
if ($compute) { Pass "Moteur de calcul décrit." } else { Fail "Aucun moteur de calcul décrit." }
if (-not $serve.HasExited) { & taskkill.exe /F /T /PID $serve.Id | Out-Null }
$left = Wait-NoProcessUnder $install 10
if ($left.Count -gt 0) { Fail "Processus encore actifs après l'arrêt de l'Ollama intégré : $(Format-Processes $left)"; Stop-ProcessesUnder $install }
Save-Log $serveLog "ollama-seul.err.log"
Show-Log "journal de l'Ollama intégré seul" $serveLog

# --- 4 bis. Le moteur d'images seul -----------------------------------------
Section "Moteur d'images (sd-server)"
$sd = Join-Path $install "sd-server\sd-server.exe"
$sdVersionFile = Join-Path $install "sd-server\VERSION"
if ((Test-Path -LiteralPath $sd) -and (Test-Path -LiteralPath $sdVersionFile)) {
  $sdExpected = (Get-Content -LiteralPath $sdVersionFile -Raw).Trim().Split("+")[0]
  $sdReported = (& $sd --version 2>&1 | Select-Object -First 1) -join ""
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

# --- 5. Réinstallation par-dessus une app en marche ------------------------
Section "Réinstallation par-dessus une app en marche"
$data = Join-Path $work "$dataName 3"
$env:CHATONLM_DATA_DIR = $data
$log = Join-Path $work "chatonlm-3"
$process = Start-Logged $app @() $log
if ($builtinPossible) {
  $port = Wait-BuiltinPort (Join-Path $data "logs\ollama.log") $process $BuiltinWait
  Write-Host "Avant la réinstallation : app pid $($process.Id), Ollama intégré $(if ($port) { "sur le port $port" } else { "non démarré" })."
} else {
  Start-Sleep -Seconds $Wait
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
  Pass "App et Ollama intégré arrêtés par l'installateur."
}
foreach ($file in @($MainBinary, "ollama\ollama.exe", "ollama\lib\ollama\llama-server.exe", "onnxruntime.dll")) {
  if (-not (Test-Path -LiteralPath (Join-Path $install $file))) { Fail "Fichier absent après la réinstallation : $file" }
}
Save-Log "$log.err.log" "chatonlm-reinstallation.err.log"

# --- 6. Désinstallation ----------------------------------------------------
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
