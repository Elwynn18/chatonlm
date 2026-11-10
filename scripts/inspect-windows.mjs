// Inspection de l'installateur Windows de ChatonLM (NSIS) construit par
// scripts/package-windows.sh : en-tête NSIS, manifeste sans élévation,
// fichiers extraits par 7-Zip (`7z x`, dans .tools/package/inspect-windows),
// fichiers attendus, notices, aucun fichier CUDA, ROCm ni HIP (le pack CUDA
// est un téléchargement facultatif de l'app) ni de l'ancien Ollama intégré,
// crochets NSIS qui retirent celui-ci d'une installation 0.9.x, ONNX Runtime
// identique à celui de scripts/fetch-onnxruntime.sh, moteur d'images
// identique à celui de scripts/build-sd-server.sh (CRT statique : aucune DLL
// de Visual C++ ni d'OpenMP importée), moteur de texte identique à celui de
// scripts/build-llama-server.sh (mêmes contrôles, pour chacune de ses DLL),
// et surtout imports PE de
// chaque exécutable et de chaque DLL : une DLL demandée doit être livrée à
// côté, ou appartenir à Windows. C'est ce contrôle qui remplace la « machine
// neuve » : le runner de la CI a le redistribuable Visual C++, WebView2 et
// Visual Studio, il ne verrait jamais une DLL oubliée.
//
// Marche sous Linux comme sur le runner Windows (7z dans le PATH ; aucun
// autre outil : les en-têtes PE sont lus ici). Rapport lisible sur la sortie
// standard ; code 1 si un contrôle échoue.
//
// Usage : node scripts/inspect-windows.mjs [options] <ChatonLM_x.y.z_x64-setup.exe>
//   --runtimes <dossier>   copies de référence (défaut : .tools/runtimes ;
//                          onnxruntime, sd-server, llama-server)
//   --report <fichier>     copie du rapport
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  EXCLUDED_ENGINES,
  formatSize,
  testOnlyErrors,
} from "./inspect-packages.mjs";
import { parseEnv } from "./package-notices.mjs";

const root = path.resolve(import.meta.dirname, "..");

/** Exécutable principal (nom du paquet Cargo, quel que soit productName). */
export const MAIN_BINARY = "chatonlm.exe";

/** Désinstalleur écrit par NSIS (7-Zip peut le restituer). */
export const UNINSTALLER = "uninstall.exe";

/** Fichiers attendus dans le dossier d'installation (chemins avec « / »). */
export const REQUIRED_FILES = [
  MAIN_BINARY,
  // ONNX Runtime officiel, chargé à l'exécution depuis le dossier de l'exe
  // (dossier des ressources de Tauri sous Windows).
  "onnxruntime.dll",
  // Runtime Visual C++ (bundle.windows.bundleVCRuntime) : exigé par
  // l'exécutable (CRT dynamique) et par onnxruntime.dll.
  "vcruntime140.dll",
  "vcruntime140_1.dll",
  "msvcp140.dll",
  "msvcp140_1.dll",
  // Moteur d'images (scripts/build-sd-server.sh) : le serveur, à CRT
  // statique, et le chargeur Vulkan de secours, que l'app met en fin de PATH
  // du serveur (image_gen/windows.rs).
  "sd-server/sd-server.exe",
  "sd-server/fallback/vulkan-1.dll",
  "sd-server/VERSION",
  // Moteur de texte (scripts/build-llama-server.sh) : le lanceur et ses DLL,
  // à CRT statique, les moteurs de calcul (variantes du processeur sans AVX
  // et avec AVX2, Vulkan), le chargeur Vulkan de secours, la version et les
  // empreintes.
  "llama-server/llama-server.exe",
  "llama-server/llama-server-impl.dll",
  "llama-server/llama.dll",
  "llama-server/ggml-base.dll",
  "llama-server/ggml-vulkan.dll",
  "llama-server/ggml-cpu-x64.dll",
  "llama-server/ggml-cpu-haswell.dll",
  "llama-server/vcruntime140.dll",
  "llama-server/vcruntime140_1.dll",
  "llama-server/msvcp140.dll",
  "llama-server/fallback/vulkan-1.dll",
  "llama-server/VERSION",
  "llama-server/SHA256SUMS",
  // Notices (scripts/package-notices-windows.mjs).
  "licenses/LICENSE.txt",
  "licenses/THIRD_PARTY_LICENSES.txt",
  "licenses/rust-crates.txt",
  "licenses/npm-packages.txt",
  "licenses/speech-models.txt",
  "licenses/voice-models.txt",
  "licenses/onnxruntime-LICENSE.txt",
  "licenses/onnxruntime-ThirdPartyNotices.txt",
  "licenses/windows-components.txt",
  "licenses/WebView2-SDK-LICENSE.txt",
  "licenses/NSIS-COPYING.txt",
  "licenses/sd-server/README.txt",
  "licenses/llama-server/README.txt",
  "licenses/llama-server/llama.cpp-LICENSE.txt",
];

/**
 * Dossiers de l'ancien Ollama intégré (versions 0.9.x) : plus rien n'y est
 * livré ; les crochets NSIS les retirent d'une installation mise à jour.
 */
export const RETIRED_DIRS = ["ollama/", "licenses/ollama/"];

/**
 * Contrôles des crochets NSIS (`hooks` : texte de packaging/windows/hooks.nsh)
 * pour l'ancien Ollama intégré : CHATONLM_CLEAN_OLD_OLLAMA appelée avant la
 * copie (mise à jour) et avant la désinstallation, après l'arrêt de l'app ;
 * l'ancien dossier reconnu par ollama.exe ET VERSION ; ses processus arrêtés
 * par leur chemin exact sous $INSTDIR\ollama\ ; ses fichiers retirés
 * (ollama.exe, VERSION, lib\ollama\) sans jamais « RMDir /r » sur
 * $INSTDIR\ollama ni sur $INSTDIR lui-même (un fichier de l'utilisateur
 * reste) ; jamais d'arrêt par nom (taskkill), jamais rien hors de $INSTDIR
 * (dossier .ollama du profil, données de l'app). Rend des erreurs.
 */
export function oldOllamaHookErrors(hooks) {
  const code = hooks
    .split("\n")
    .filter((line) => !line.trim().startsWith(";"))
    .join("\n");
  const macro = (name) =>
    code.split(new RegExp(`!macro ${name}\\b`))[1]?.split("!macroend")[0] ??
    null;
  const errors = [];
  for (const hook of ["NSIS_HOOK_PREINSTALL", "NSIS_HOOK_PREUNINSTALL"]) {
    const body = macro(hook);
    const stop = body?.indexOf("!insertmacro CHATONLM_STOP_ALL") ?? -1;
    const clean = body?.indexOf("!insertmacro CHATONLM_CLEAN_OLD_OLLAMA") ?? -1;
    if (clean < 0 || stop < 0 || clean < stop) {
      errors.push(
        `${hook} : CHATONLM_CLEAN_OLD_OLLAMA attendue après CHATONLM_STOP_ALL`,
      );
    }
  }
  const recognize = macro("CHATONLM_OLD_OLLAMA") ?? "";
  for (const file of ["ollama.exe", "VERSION"]) {
    if (!recognize.includes(`\${FileExists} "$INSTDIR\\ollama\\${file}"`)) {
      errors.push(`CHATONLM_OLD_OLLAMA : ${file} non vérifié`);
    }
  }
  const stopOld = macro("CHATONLM_STOP_OLD_OLLAMA") ?? "";
  for (const exe of [
    "ollama.exe",
    "lib\\ollama\\llama-server.exe",
    "lib\\ollama\\llama-quantize.exe",
  ]) {
    if (!stopOld.includes(`CHATONLM_STOP_FILE "$INSTDIR\\ollama\\${exe}"`)) {
      errors.push(
        `CHATONLM_STOP_OLD_OLLAMA : $INSTDIR\\ollama\\${exe} non arrêté`,
      );
    }
  }
  const remove = macro("CHATONLM_REMOVE_OLD_OLLAMA") ?? "";
  for (const line of [
    'Delete "$INSTDIR\\ollama\\ollama.exe"',
    'RMDir /r "$INSTDIR\\ollama\\lib\\ollama"',
    'Delete "$INSTDIR\\ollama\\VERSION"',
    'RMDir "$INSTDIR\\ollama"',
  ]) {
    if (!remove.includes(line)) {
      errors.push(`CHATONLM_REMOVE_OLD_OLLAMA : « ${line} » absent`);
    }
  }
  for (const [pattern, why] of [
    [
      /RMDir\s+\/r\s+"\$INSTDIR(\\ollama)?\\?"/i,
      "RMDir /r sur tout un dossier",
    ],
    [/taskkill/i, "arrêt par nom (taskkill)"],
    [
      /\$PROFILE|\$APPDATA|\$LOCALAPPDATA|\.ollama\b/i,
      "chemin hors du dossier d'installation",
    ],
  ]) {
    const match = pattern.exec(code);
    if (match)
      errors.push(`packaging/windows/hooks.nsh : ${why} : ${match[0]}`);
  }
  return errors;
}

/** Dossier du moteur de texte dans l'installation. */
export const LLAMA_SERVER_DIR = "llama-server/";

/**
 * Motifs des fichiers que les crochets NSIS (packaging/windows/hooks.nsh)
 * retirent de llama-server\ avant une mise à jour : ceux de la macro
 * CHATONLM_LLAMA_FILES, plus VERSION (la marque, retirée en dernier). Rend
 * des expressions rationnelles sur le chemin relatif au dossier du moteur
 * (« / »), insensibles à la casse comme Windows.
 */
export function llamaHookPatterns(hooks) {
  const code = hooks
    .split("\n")
    .filter((line) => !line.trim().startsWith(";"))
    .join("\n");
  const files = code
    .split("!macro CHATONLM_LLAMA_FILES")[1]
    ?.split("!macroend")[0];
  const quoted = [
    ...(files ?? "").matchAll(/"\$INSTDIR\\llama-server\\([^"]+)"/g),
    ...code.matchAll(/^\s*Delete "\$INSTDIR\\llama-server\\(VERSION)"/gm),
  ].map((match) => match[1]);
  return quoted.map(
    (glob) =>
      new RegExp(
        `^${glob
          .replaceAll("\\", "/")
          .replace(/[.+?^${}()|[\]]/g, "\\$&")
          .replaceAll("*", "[^/]*")}$`,
        "i",
      ),
  );
}

/**
 * DLL qu'aucun fichier du moteur de texte ne doit importer : OpenMP (coupé à
 * la construction), runtimes de GCC, OpenSSL. Son runtime Visual C++ (CRT en
 * DLL, voir scripts/build-llama-server.sh) est livré dans son dossier :
 * importProblems vérifie qu'il y est.
 */
export const LLAMA_SERVER_FORBIDDEN =
  /^(vcomp|libomp|libgomp|libstdc|libgcc|libwinpthread|libssl|libcrypto|libcurl)/;

/** Dossier du moteur d'images dans l'installation, et son serveur. */
export const SD_SERVER_DIR = "sd-server/";
export const SD_SERVER_EXE = "sd-server/sd-server.exe";

/**
 * DLL que sd-server.exe ne doit pas importer : runtime de Visual C++ (CRT
 * statique, aucune DLL livrée à côté de lui), OpenMP (coupé à la
 * construction), runtimes de GCC. Même liste que scripts/build-sd-server.sh.
 */
export const SD_SERVER_FORBIDDEN =
  /^(msvcp|vcruntime|vccorlib|concrt|vcomp|libomp|libgomp|libstdc|libgcc|api-ms-win-crt)/;

/** Mentions exigées dans les notices : fichier → motifs. */
export const NOTICE_MENTIONS = [
  ["licenses/THIRD_PARTY_LICENSES.txt", [/Silero VAD/, /Part 9\. Windows/]],
  [
    "licenses/rust-crates.txt",
    [
      /target x86_64-pc-windows-msvc/,
      /^\s*webview2-com-sys \d/m,
      /^\s*sonora \d/m,
    ],
  ],
  ["licenses/windows-components.txt", [/WebView2/, /NSIS/, /Visual C\+\+/]],
  [
    "licenses/sd-server/README.txt",
    [
      /stable-diffusion\.cpp/,
      /fallback\\vulkan-1\.dll/,
      /Apache License/,
      /Visual C\+\+/,
    ],
  ],
  [
    "licenses/llama-server/README.txt",
    [
      /llama\.cpp/,
      /fallback\\vulkan-1\.dll/,
      /Apache License/,
      /Visual\sC\+\+/,
    ],
  ],
];

/**
 * DLL de Windows qu'un exécutable peut demander sans qu'on les livre (liste
 * de l'audit de scripts/build_windows.ps1 d'Ollama, complétée de celles
 * d'une app Tauri). Les ensembles d'API (api-ms-win-*, ext-ms-*) sont admis
 * par leur préfixe.
 */
export const SYSTEM_DLLS = new Set(
  [
    "advapi32 avrt bcrypt bcryptprimitives cfgmgr32 combase comctl32 comdlg32",
    "coremessaging credui crypt32 cryptbase d2d1 d3d11 d3d12 d3d9 dbghelp",
    "dcomp dnsapi dwmapi dwrite dxcore dxgi fwpuclnt gdi32 gdi32full gdiplus",
    "hid imm32 iphlpapi kernel32 kernelbase mmdevapi mpr msasn1 msimg32",
    "mswsock msvcrt ncrypt netapi32 normaliz ntdll ole32 oleacc oleaut32",
    "opengl32 pdh powrprof profapi propsys psapi rpcrt4 rstrtmgr sechost",
    "secur32 setupapi shcore shell32 shlwapi sspicli ucrtbase uiautomationcore",
    "urlmon user32 userenv usp10 uxtheme version winhttp wininet winmm",
    "winspool.drv wintrust wldap32 ws2_32 wtsapi32 xmllite",
  ]
    .join(" ")
    .split(" ")
    .map((name) => (name.includes(".") ? name : `${name}.dll`)),
);

/**
 * DLL que seul le système de l'utilisateur fournit, et qui peuvent manquer :
 * le chargeur Vulkan vient du pilote graphique (sd-server.exe et
 * llama-server.exe prennent sinon celui de leur dossier fallback).
 */
export const HOST_ONLY_DLLS = new Set(["vulkan-1.dll"]);

/**
 * DLL qui ne sont jamais dans Windows : runtimes Visual C++ et OpenMP,
 * chargeur WebView2, ONNX Runtime, bibliothèques à la façon de mingw
 * (lib*.dll), ggml, CUDA. Demandée sans être livrée : erreur. Toute autre
 * DLL inconnue n'est qu'un avertissement (liste SYSTEM_DLLS à compléter).
 */
export const NEVER_SYSTEM =
  /^(vcruntime|msvcp|concrt|vccorlib|vcomp|ucrtbased|webview2loader|onnxruntime|lib|ggml|cu(da|blas|dnn|fft)|nv)/;

/** Taille au-delà de laquelle l'installateur embarque sûrement un moteur de trop. */
export const MAX_INSTALLER_BYTES = 250 * 1024 * 1024;

const NSIS_SIGNATURE = Buffer.from([
  0xef, 0xbe, 0xad, 0xde, 0x4e, 0x75, 0x6c, 0x6c, 0x73, 0x6f, 0x66, 0x74, 0x49,
  0x6e, 0x73, 0x74,
]);

/**
 * En-têtes d'un fichier PE : machine (0x8664 pour x64), DLL importées et
 * importées en différé (noms en minuscules, triés), présence d'une signature
 * Authenticode (répertoire de sécurité non vide). Lève une erreur si le
 * tampon n'est pas un PE.
 */
export function parsePe(buffer) {
  if (buffer.length < 0x40 || buffer.readUInt16LE(0) !== 0x5a4d) {
    throw new Error("PE invalide : en-tête MZ absent");
  }
  const pe = buffer.readUInt32LE(0x3c);
  if (pe + 24 > buffer.length || buffer.readUInt32LE(pe) !== 0x00004550) {
    throw new Error("PE invalide : signature PE absente");
  }
  const machine = buffer.readUInt16LE(pe + 4);
  const sectionCount = buffer.readUInt16LE(pe + 6);
  const optionalSize = buffer.readUInt16LE(pe + 20);
  const optional = pe + 24;
  const magic = buffer.readUInt16LE(optional);
  if (magic !== 0x20b && magic !== 0x10b) {
    throw new Error("PE invalide : en-tête optionnel inconnu");
  }
  const directories = optional + (magic === 0x20b ? 112 : 96);
  const directoryCount = buffer.readUInt32LE(directories - 4);
  const directory = (index) =>
    index < directoryCount
      ? {
          address: buffer.readUInt32LE(directories + index * 8),
          size: buffer.readUInt32LE(directories + index * 8 + 4),
        }
      : { address: 0, size: 0 };
  const sections = [];
  for (let i = 0; i < sectionCount; i += 1) {
    const at = optional + optionalSize + i * 40;
    if (at + 40 > buffer.length) break;
    sections.push({
      virtualSize: buffer.readUInt32LE(at + 8),
      virtualAddress: buffer.readUInt32LE(at + 12),
      rawSize: buffer.readUInt32LE(at + 16),
      rawOffset: buffer.readUInt32LE(at + 20),
    });
  }
  const offsetOf = (rva) => {
    for (const s of sections) {
      const size = Math.max(s.virtualSize, s.rawSize);
      if (rva >= s.virtualAddress && rva < s.virtualAddress + size) {
        return rva - s.virtualAddress + s.rawOffset;
      }
    }
    return null;
  };
  const nameAt = (rva) => {
    const at = offsetOf(rva);
    if (at === null || at >= buffer.length) return null;
    let end = at;
    while (end < buffer.length && buffer[end] !== 0) end += 1;
    return buffer.toString("latin1", at, end).toLowerCase();
  };
  // Table des descripteurs : `size` octets chacun, nom à `nameField`, fin au
  // premier descripteur sans nom.
  const names = (dir, size, nameField) => {
    const found = new Set();
    let at = dir.address ? offsetOf(dir.address) : null;
    while (at !== null && at + size <= buffer.length) {
      const rva = buffer.readUInt32LE(at + nameField);
      if (rva === 0) break;
      const name = nameAt(rva);
      if (name) found.add(name);
      at += size;
    }
    return [...found].sort();
  };
  return {
    machine,
    imports: names(directory(1), 20, 12),
    delayImports: names(directory(13), 32, 4),
    signed: directory(4).size > 0,
  };
}

/**
 * Valeur d'une chaîne de la ressource de version (« ProductVersion »,
 * « ProductName »…), ou null : les ressources sont en UTF-16. Le nom de la
 * clé peut aussi se trouver ailleurs dans le binaire (chaîne d'une API de
 * Windows, suivie d'octets quelconques) : la première occurrence suivie d'un
 * texte lisible est celle de la ressource.
 */
export function versionString(buffer, key) {
  const needle = Buffer.from(`${key}\0`, "utf16le");
  let from = 0;
  for (;;) {
    let at = buffer.indexOf(needle, from);
    if (at < 0) return null;
    from = at + 2;
    at += needle.length;
    // Alignement sur 4 octets : mots nuls avant la valeur.
    while (at + 1 < buffer.length && buffer.readUInt16LE(at) === 0) at += 2;
    let end = at;
    while (end + 1 < buffer.length && buffer.readUInt16LE(end) !== 0) end += 2;
    const value = buffer.toString("utf16le", at, end);
    if (/^[\x20-\x7e\u00a0-\u024f]{1,200}$/.test(value)) return value;
  }
}

/** Vrai si le tampon est un installateur NSIS (PE avec l'en-tête de NSIS). */
export function isNsisInstaller(buffer) {
  try {
    parsePe(buffer);
  } catch {
    return false;
  }
  return buffer.indexOf(NSIS_SIGNATURE) > 0;
}

/**
 * Niveau d'exécution demandé par le manifeste de l'installateur
 * (« asInvoker » : sans élévation), ou null s'il n'en déclare pas.
 */
export function executionLevel(buffer) {
  const text = buffer.toString("latin1", 0, Math.min(buffer.length, 8 << 20));
  return (
    /requestedExecutionLevel\s+level="([A-Za-z]+)"/.exec(text)?.[1] ?? null
  );
}

/** Dossiers où Windows trouvera les DLL demandées par `file`. */
function searchDirs(file) {
  const dir = path.posix.dirname(file);
  const dirs = [dir === "." ? "" : dir];
  // Moteur d'images : dossier de sd-server.exe (celui du processus), puis
  // fallback, ajouté en fin de PATH par l'app.
  if (file.startsWith(SD_SERVER_DIR))
    dirs.push("sd-server", "sd-server/fallback");
  // Moteur de texte : de même (ses moteurs de calcul, chargés par ggml,
  // trouvent ggml-base.dll dans le dossier du processus).
  if (file.startsWith(LLAMA_SERVER_DIR))
    dirs.push("llama-server", "llama-server/fallback");
  return [...new Set(dirs)];
}

/**
 * Imports non résolus : `files` (chemins en minuscules → chemin réel),
 * `pe(chemin)` rend les en-têtes d'un exe ou d'une DLL. Rend
 * { errors, warnings }.
 */
export function importProblems({ files, pe }) {
  const errors = [];
  const warnings = [];
  const present = new Set([...files.keys()]);
  for (const [lower, file] of [...files].sort()) {
    if (!/\.(exe|dll)$/.test(lower)) continue;
    // Le désinstalleur est le programme de NSIS (32 bits), pas un fichier
    // de ChatonLM.
    if (lower === UNINSTALLER) continue;
    let headers;
    try {
      headers = pe(file);
    } catch (error) {
      errors.push(`${file} : ${error.message}`);
      continue;
    }
    if (headers.machine !== 0x8664) {
      errors.push(
        `${file} : machine 0x${headers.machine.toString(16)} au lieu de x64 (0x8664)`,
      );
    }
    for (const name of headers.imports) {
      if (/^(api|ext)-ms-/.test(name) || SYSTEM_DLLS.has(name)) continue;
      const found = searchDirs(lower).some((dir) =>
        present.has(dir ? `${dir}/${name}` : name),
      );
      if (found) continue;
      if (HOST_ONLY_DLLS.has(name)) continue;
      if (NEVER_SYSTEM.test(name)) {
        errors.push(`${file} : DLL demandée et non livrée à côté : ${name}`);
      } else {
        warnings.push(
          `${file} : DLL non répertoriée (${name}) : de Windows ? sinon la livrer, puis compléter SYSTEM_DLLS`,
        );
      }
    }
  }
  return { errors, warnings };
}

/**
 * Contrôles du contenu de l'installateur. `files` : chemins relatifs au
 * dossier d'installation (« / ») → taille ; `read(chemin)` : texte ;
 * `hash(chemin)` : SHA-256 ; `pe(chemin)` : en-têtes PE ; `reference` :
 * { onnxruntime: SHA-256 | null, sdServer: Map(chemin relatif → SHA-256)
 *   | null, sdServerPinned, llamaServer: Map | null, llamaServerPinned }
 * (copies vérifiées de .tools/runtimes, versions de packaging/sdcpp.env et
 * packaging/llama.env) ; `version` de l'app ;
 * `bytes(chemin)` : contenu brut (exécutable sans le code des essais). Rend
 * { errors, warnings }.
 */
export function contentProblems({
  files,
  read,
  bytes,
  hash,
  pe,
  reference,
  version,
  hooks = readFileSync(path.join(root, "packaging/windows/hooks.nsh"), "utf8"),
}) {
  const errors = [];
  const warnings = [];
  const lower = new Map([...files.keys()].map((f) => [f.toLowerCase(), f]));

  for (const file of REQUIRED_FILES) {
    if (!lower.has(file.toLowerCase())) errors.push(`Fichier absent : ${file}`);
  }
  // Aucun fichier CUDA, ROCm ni HIP, où qu'il soit (pack CUDA téléchargé à
  // part par l'app), ni de l'ancien Ollama intégré.
  for (const file of [...files.keys()].sort()) {
    if (EXCLUDED_ENGINES.test(file)) {
      errors.push(
        `Fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : ${file}`,
      );
    }
    const lowerFile = file.toLowerCase();
    if (RETIRED_DIRS.some((dir) => lowerFile.startsWith(dir))) {
      errors.push(`Fichier de l'ancien Ollama intégré livré : ${file}`);
    }
  }
  errors.push(...oldOllamaHookErrors(hooks));
  if (reference.onnxruntime && files.has("onnxruntime.dll")) {
    if (hash("onnxruntime.dll") !== reference.onnxruntime) {
      errors.push("onnxruntime.dll : différent de la copie vérifiée");
    }
  } else if (!reference.onnxruntime) {
    warnings.push(
      "Copie de référence d'ONNX Runtime absente (.tools/runtimes/onnxruntime) : fichier non comparé",
    );
  }

  // Moteur d'images : exactement les fichiers du dossier construit.
  if (reference.sdServer) {
    for (const [rel, sum] of reference.sdServer) {
      const file = `${SD_SERVER_DIR}${rel}`;
      if (!files.has(file)) {
        errors.push(
          `Moteur d'images : fichier absent de l'installateur : ${file}`,
        );
      } else if (hash(file) !== sum) {
        errors.push(`Moteur d'images : fichier modifié : ${file}`);
      }
    }
    for (const file of files.keys()) {
      if (
        file.startsWith(SD_SERVER_DIR) &&
        !reference.sdServer.has(file.slice(SD_SERVER_DIR.length))
      ) {
        errors.push(`Moteur d'images : fichier inattendu : ${file}`);
      }
    }
  } else {
    warnings.push(
      "Copie de référence du moteur d'images absente (.tools/runtimes/sd-server) : fichiers non comparés",
    );
  }
  const sdVersion = `${SD_SERVER_DIR}VERSION`;
  if (reference.sdServerPinned && files.has(sdVersion)) {
    const shipped = read(sdVersion).trim();
    const pinned = reference.sdServerPinned.replace(
      /[.*+?^${}()|[\]\\]/g,
      "\\$&",
    );
    if (!new RegExp(`^${pinned}\\+chatonlm\\.[0-9a-f]{12}$`).test(shipped)) {
      errors.push(
        `${sdVersion} : « ${shipped} » au lieu de ${reference.sdServerPinned}+chatonlm.<empreinte> (packaging/sdcpp.env)`,
      );
    }
  }
  if (files.has(SD_SERVER_EXE)) {
    const headers = pe(SD_SERVER_EXE);
    const crt = [...headers.imports, ...(headers.delayImports ?? [])].filter(
      (name) => SD_SERVER_FORBIDDEN.test(name),
    );
    if (crt.length > 0) {
      errors.push(
        `${SD_SERVER_EXE} : DLL du runtime Visual C++ ou d'OpenMP importée (CRT statique attendu) : ${[...new Set(crt)].join(", ")}`,
      );
    }
  }

  // Moteur de texte : exactement les fichiers du dossier construit
  // (SHA256SUMS compris), sans OpenMP ni runtime de GCC.
  if (reference.llamaServer) {
    for (const [rel, sum] of reference.llamaServer) {
      const file = `${LLAMA_SERVER_DIR}${rel}`;
      if (!files.has(file)) {
        errors.push(
          `Moteur de texte : fichier absent de l'installateur : ${file}`,
        );
      } else if (hash(file) !== sum) {
        errors.push(`Moteur de texte : fichier modifié : ${file}`);
      }
    }
    for (const file of files.keys()) {
      if (
        file.startsWith(LLAMA_SERVER_DIR) &&
        !reference.llamaServer.has(file.slice(LLAMA_SERVER_DIR.length))
      ) {
        errors.push(`Moteur de texte : fichier inattendu : ${file}`);
      }
    }
  } else {
    warnings.push(
      "Copie de référence du moteur de texte absente (.tools/runtimes/llama-server) : fichiers non comparés",
    );
  }
  // Chaque fichier livré est retiré par les crochets NSIS avant une mise à
  // jour (sinon une variante d'une autre version resterait, et serait
  // chargée) ; ils ne retirent que des fichiers à nous, par motifs.
  const removed = llamaHookPatterns(hooks);
  if (removed.length === 0) {
    errors.push(
      "packaging/windows/hooks.nsh : motifs de CHATONLM_LLAMA_FILES illisibles",
    );
  } else {
    for (const file of [...files.keys()].sort()) {
      if (!file.startsWith(LLAMA_SERVER_DIR)) continue;
      const rel = file.slice(LLAMA_SERVER_DIR.length);
      if (!removed.some((pattern) => pattern.test(rel))) {
        errors.push(
          `Moteur de texte : fichier que les crochets NSIS ne retirent pas avant une mise à jour : ${file}`,
        );
      }
    }
  }
  const llamaVersion = `${LLAMA_SERVER_DIR}VERSION`;
  if (reference.llamaServerPinned && files.has(llamaVersion)) {
    const shipped = read(llamaVersion).trim();
    const pinned = reference.llamaServerPinned.replace(
      /[.*+?^${}()|[\]\\]/g,
      "\\$&",
    );
    if (!new RegExp(`^${pinned}\\+chatonlm\\.[0-9a-f]{12}$`).test(shipped)) {
      errors.push(
        `${llamaVersion} : « ${shipped} » au lieu de ${reference.llamaServerPinned}+chatonlm.<empreinte> (packaging/llama.env)`,
      );
    }
  }
  for (const file of [...files.keys()].sort()) {
    if (
      !file.startsWith(LLAMA_SERVER_DIR) ||
      file.startsWith(`${LLAMA_SERVER_DIR}fallback/`) ||
      !/\.(exe|dll)$/i.test(file)
    ) {
      continue;
    }
    const headers = pe(file);
    const banned = [...headers.imports, ...(headers.delayImports ?? [])].filter(
      (name) => LLAMA_SERVER_FORBIDDEN.test(name),
    );
    if (banned.length > 0) {
      errors.push(
        `${file} : DLL d'OpenMP, de GCC ou d'OpenSSL importée : ${[...new Set(banned)].join(", ")}`,
      );
    }
  }

  for (const [file, mentions] of NOTICE_MENTIONS) {
    if (!files.has(file)) continue;
    const text = read(file);
    for (const mention of mentions) {
      if (!mention.test(text)) {
        errors.push(`${file} : mention absente (${mention.source})`);
      }
    }
  }

  if (files.has(MAIN_BINARY)) {
    errors.push(...testOnlyErrors(bytes(MAIN_BINARY), MAIN_BINARY));
  }

  const imports = importProblems({ files: lower, pe });
  errors.push(...imports.errors);
  warnings.push(...imports.warnings);

  if (files.has(MAIN_BINARY)) {
    const product = pe(MAIN_BINARY).productVersion;
    if (product === null || product === undefined) {
      warnings.push(`${MAIN_BINARY} : version de la ressource illisible`);
    } else if (product !== version) {
      errors.push(
        `${MAIN_BINARY} : version ${product} dans ses ressources au lieu de ${version}`,
      );
    }
  }
  return { errors, warnings };
}

/** Contrôles du fichier de l'installateur lui-même. */
export function installerProblems({ buffer, size }) {
  const errors = [];
  const warnings = [];
  if (!isNsisInstaller(buffer)) {
    errors.push(
      "Ce fichier n'est pas un installateur NSIS (PE + en-tête NSIS).",
    );
    return { errors, warnings };
  }
  const level = executionLevel(buffer);
  if (level === null) {
    warnings.push("Manifeste de l'installateur : niveau d'exécution illisible");
  } else if (level !== "asInvoker") {
    errors.push(
      `Manifeste de l'installateur : niveau « ${level} » au lieu de asInvoker (installation sans administrateur)`,
    );
  }
  if (size > MAX_INSTALLER_BYTES) {
    errors.push(
      `Installateur de ${formatSize(size)} : plus de ${formatSize(MAX_INSTALLER_BYTES)} (moteur CUDA embarqué ?)`,
    );
  }
  return { errors, warnings };
}

/** Fichiers de `dir` (récursif) : chemins relatifs avec « / ». */
export function walk(dir, base = dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) found.push(...walk(full, base));
    else if (entry.isFile()) {
      found.push(path.relative(base, full).split(path.sep).join("/"));
    }
  }
  return found.sort();
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

/**
 * Dossier d'installation dans l'extraction de 7-Zip : celui qui contient
 * l'exécutable principal (7-Zip range les fichiers de $INSTDIR à la racine,
 * les greffons dans $PLUGINSDIR).
 */
export function installRoot(extracted) {
  const main = walk(extracted)
    .filter((f) => path.posix.basename(f).toLowerCase() === MAIN_BINARY)
    .sort((a, b) => a.length - b.length)[0];
  return main === undefined
    ? null
    : path.join(extracted, path.posix.dirname(main));
}

/** Copies de référence de .tools/runtimes (ou null si absentes). */
function readReference(runtimes) {
  const ort = path.join(runtimes, "onnxruntime", "onnxruntime.dll");
  // Moteur d'images : tous les fichiers du dossier construit, sauf
  // SHA256SUMS (non livré).
  const sdDir = path.join(runtimes, "sd-server");
  let sdServer = null;
  if (existsSync(path.join(sdDir, "sd-server.exe"))) {
    sdServer = new Map(
      walk(sdDir)
        .filter((rel) => rel !== "SHA256SUMS")
        .map((rel) => [rel, sha256(path.join(sdDir, rel))]),
    );
  }
  // Moteur de texte : tous les fichiers du dossier construit, SHA256SUMS
  // compris (livré).
  const llamaDir = path.join(runtimes, "llama-server");
  let llamaServer = null;
  if (existsSync(path.join(llamaDir, "llama-server.exe"))) {
    llamaServer = new Map(
      walk(llamaDir).map((rel) => [rel, sha256(path.join(llamaDir, rel))]),
    );
  }
  const env = (file) =>
    parseEnv(readFileSync(path.join(root, "packaging", file), "utf8"));
  return {
    onnxruntime: existsSync(ort) ? sha256(ort) : null,
    sdServer,
    sdServerPinned: env("sdcpp.env").SDCPP_VERSION ?? null,
    llamaServer,
    llamaServerPinned: env("llama.env").LLAMA_VERSION ?? null,
  };
}

function extract(installer, dest) {
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  const result = spawnSync("7z", ["x", "-y", "-bd", `-o${dest}`, installer], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `7z x a échoué (${result.error?.message ?? `code ${result.status}`}) : ${(result.stderr || result.stdout || "").trim().split("\n").slice(-5).join(" | ")}`,
    );
  }
}

/** Rapport lisible. */
export function formatReport({
  installer,
  size,
  signed,
  files,
  pes,
  problems,
}) {
  const total = [...files.values()].reduce((sum, s) => sum + s, 0);
  const group = (prefix) =>
    [...files]
      .filter(([f]) =>
        prefix === "" ? !f.includes("/") : f.startsWith(prefix),
      )
      .reduce((sum, [, s]) => sum + s, 0);
  const lines = [
    `Installateur : ${installer}`,
    `  taille : ${formatSize(size)} (${size} octets)`,
    `  signature Authenticode : ${signed ? "présente" : "absente (attendu : pas de signature aujourd'hui)"}`,
    `  installé : ${files.size} fichiers, ${formatSize(total)}`,
    `    racine (exe, DLL) : ${formatSize(group(""))}`,
    `    sd-server/ : ${formatSize(group(SD_SERVER_DIR))}`,
    `    llama-server/ : ${formatSize(group(LLAMA_SERVER_DIR))}`,
    `    licenses/ : ${formatSize(group("licenses/"))}`,
    "",
    "Exécutables et DLL (imports hors Windows, signature) :",
  ];
  for (const [file, headers] of pes) {
    const own = headers.imports.filter(
      (name) => !/^(api|ext)-ms-/.test(name) && !SYSTEM_DLLS.has(name),
    );
    lines.push(
      `  ${file}${headers.signed ? " [signé]" : ""} : ${own.join(", ") || "Windows seulement"}` +
        (headers.delayImports.length > 0
          ? ` ; en différé : ${headers.delayImports.join(", ")}`
          : ""),
    );
  }
  lines.push("");
  for (const warning of problems.warnings) lines.push(`⚠ ${warning}`);
  for (const error of problems.errors) lines.push(`✗ ${error}`);
  lines.push(
    problems.errors.length === 0
      ? "✓ Installateur conforme."
      : `✗ ${problems.errors.length} contrôle(s) en échec.`,
  );
  return `${lines.join("\n")}\n`;
}

function main() {
  const args = process.argv.slice(2);
  let runtimes = path.join(root, ".tools/runtimes");
  let report = null;
  const rest = [];
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--runtimes") runtimes = path.resolve(args[(i += 1)]);
    else if (args[i] === "--report") report = args[(i += 1)];
    else rest.push(args[i]);
  }
  if (rest.length !== 1) {
    console.error(
      "Usage : node scripts/inspect-windows.mjs [--runtimes <dossier>] [--report <fichier>] <setup.exe>",
    );
    process.exit(2);
  }
  const installer = path.resolve(rest[0]);
  const buffer = readFileSync(installer);
  const size = statSync(installer).size;
  const problems = installerProblems({ buffer, size });
  const version = JSON.parse(
    readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8"),
  ).version;

  let files = new Map();
  const pes = new Map();
  if (problems.errors.length === 0) {
    const dest = path.join(root, ".tools/package/inspect-windows");
    let installed = null;
    try {
      extract(installer, dest);
      installed = installRoot(dest);
      if (installed === null) {
        problems.errors.push(`${MAIN_BINARY} introuvable dans l'installateur.`);
      }
    } catch (error) {
      problems.errors.push(error.message);
    }
    if (installed !== null) {
      files = new Map(
        walk(installed)
          // Greffons et fichiers temporaires de l'installateur lui-même.
          .filter((f) => !f.startsWith("$"))
          .map((f) => [f, statSync(path.join(installed, f)).size]),
      );
      const headers = (file) => {
        if (!pes.has(file)) {
          const data = readFileSync(path.join(installed, file));
          pes.set(file, {
            ...parsePe(data),
            productVersion:
              file === MAIN_BINARY
                ? versionString(data, "ProductVersion")
                : null,
          });
        }
        return pes.get(file);
      };
      const content = contentProblems({
        files,
        read: (file) => readFileSync(path.join(installed, file), "utf8"),
        bytes: (file) => readFileSync(path.join(installed, file)),
        hash: (file) => sha256(path.join(installed, file)),
        pe: headers,
        reference: readReference(runtimes),
        version,
      });
      problems.errors.push(...content.errors);
      problems.warnings.push(...content.warnings);
    }
  }
  let signed = false;
  try {
    signed = parsePe(buffer).signed;
  } catch {
    // Déjà signalé par installerProblems.
  }
  const text = formatReport({
    installer: path.basename(installer),
    size,
    signed,
    files,
    pes: [...pes].sort(([a], [b]) => a.localeCompare(b)),
    problems,
  });
  process.stdout.write(text);
  if (report) {
    mkdirSync(path.dirname(path.resolve(report)), { recursive: true });
    writeFileSync(report, text);
  }
  process.exit(problems.errors.length === 0 ? 0 : 1);
}

if (
  process.argv[1] &&
  import.meta.url.toLowerCase() ===
    pathToFileURL(process.argv[1]).href.toLowerCase()
) {
  main();
}
