// Inspection de l'installateur Windows de ChatonLM (NSIS) construit par
// scripts/package-windows.sh : en-tête NSIS, manifeste sans élévation,
// fichiers extraits par 7-Zip (`7z x`, dans .tools/package/inspect-windows),
// fichiers attendus, notices, Ollama intégré identique à la copie vérifiée de
// scripts/fetch-ollama.sh et sans moteur CUDA, ROCm ni MLX, ONNX Runtime
// identique à celui de scripts/fetch-onnxruntime.sh, moteur d'images
// identique à celui de scripts/build-sd-server.sh (CRT statique : aucune DLL
// de Visual C++ ni d'OpenMP importée), et surtout imports PE de
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
//                          ollama, onnxruntime, sd-server)
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
import { EXCLUDED_ENGINES, formatSize } from "./inspect-packages.mjs";
import { parseEnv } from "./package-notices.mjs";
import { isOllamaLicenseFile } from "./package-notices-windows.mjs";

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
  // Ollama intégré, arborescence de l'archive officielle.
  "ollama/ollama.exe",
  "ollama/VERSION",
  "ollama/lib/ollama/llama-server.exe",
  "ollama/lib/ollama/ggml-base.dll",
  "ollama/lib/ollama/vulkan/ggml-vulkan.dll",
  // Moteur d'images (scripts/build-sd-server.sh) : le serveur, à CRT
  // statique, et le chargeur Vulkan de secours, que l'app met en fin de PATH
  // du serveur (image_gen/windows.rs).
  "sd-server/sd-server.exe",
  "sd-server/fallback/vulkan-1.dll",
  "sd-server/VERSION",
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
  "licenses/ollama/README.txt",
  "licenses/ollama/GO_LICENSE",
  "licenses/ollama/LLAMA_CPP_LICENSE",
  "licenses/ollama/LLVM-LICENSE.txt",
  "licenses/ollama/winpthreads-COPYING.txt",
  "licenses/sd-server/README.txt",
];

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
  ["licenses/ollama/README.txt", [/official build for Windows/, /libomp\.dll/]],
  [
    "licenses/sd-server/README.txt",
    [
      /stable-diffusion\.cpp/,
      /fallback\\vulkan-1\.dll/,
      /Apache License/,
      /Visual C\+\+/,
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
 * le chargeur Vulkan vient du pilote graphique (le moteur Vulkan d'Ollama
 * n'est chargé que s'il est là ; sd-server.exe prend sinon celui de
 * sd-server/fallback).
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
  // Bibliothèques des moteurs d'Ollama (vulkan/…) : le serveur ajoute
  // lib/ollama à leur recherche.
  if (file.startsWith("ollama/lib/ollama/")) dirs.push("ollama/lib/ollama");
  // Moteur d'images : dossier de sd-server.exe (celui du processus), puis
  // fallback, ajouté en fin de PATH par l'app.
  if (file.startsWith(SD_SERVER_DIR))
    dirs.push("sd-server", "sd-server/fallback");
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
 * { ollama: Map(chemin relatif → SHA-256), onnxruntime: SHA-256 | null,
 *   ollamaVersion, sdServer: Map(chemin relatif → SHA-256) | null,
 *   sdServerPinned } (copies vérifiées de .tools/runtimes, versions de
 * packaging/ollama.env et packaging/sdcpp.env) ; `version` de l'app. Rend
 * { errors, warnings }.
 */
export function contentProblems({ files, read, hash, pe, reference, version }) {
  const errors = [];
  const warnings = [];
  const lower = new Map([...files.keys()].map((f) => [f.toLowerCase(), f]));

  for (const file of REQUIRED_FILES) {
    if (!lower.has(file.toLowerCase())) errors.push(`Fichier absent : ${file}`);
  }
  for (const file of files.keys()) {
    // Un fichier de licence resté dans lib/ollama (MLX_LICENSE…) n'est pas
    // un moteur.
    if (
      file.startsWith("ollama/") &&
      !isOllamaLicenseFile(path.posix.basename(file)) &&
      EXCLUDED_ENGINES.test(file)
    ) {
      errors.push(
        `Moteur exclu de l'Ollama intégré (CUDA, ROCm, MLX) : ${file}`,
      );
    }
  }

  // Ollama intégré : exactement les fichiers de la copie vérifiée.
  if (reference.ollama) {
    for (const [rel, sum] of reference.ollama) {
      const file = `ollama/${rel}`;
      if (!files.has(file)) {
        errors.push(
          `Ollama intégré : fichier absent de l'installateur : ${file}`,
        );
      } else if (hash(file) !== sum) {
        errors.push(`Ollama intégré : fichier modifié : ${file}`);
      }
    }
    for (const file of files.keys()) {
      if (
        file.startsWith("ollama/") &&
        !reference.ollama.has(file.slice("ollama/".length))
      ) {
        errors.push(`Ollama intégré : fichier inattendu : ${file}`);
      }
    }
  } else {
    warnings.push(
      "Copie de référence de l'Ollama intégré absente (.tools/runtimes/ollama) : fichiers non comparés",
    );
  }
  if (reference.ollamaVersion && files.has("ollama/VERSION")) {
    const shipped = read("ollama/VERSION").trim();
    if (shipped !== reference.ollamaVersion) {
      errors.push(
        `ollama/VERSION : « ${shipped} » au lieu de ${reference.ollamaVersion} (packaging/ollama.env)`,
      );
    }
  }
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

  for (const [file, mentions] of NOTICE_MENTIONS) {
    if (!files.has(file)) continue;
    const text = read(file);
    for (const mention of mentions) {
      if (!mention.test(text)) {
        errors.push(`${file} : mention absente (${mention.source})`);
      }
    }
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
  const ollamaDir = path.join(runtimes, "ollama");
  let ollama = null;
  if (existsSync(path.join(ollamaDir, "ollama.exe"))) {
    ollama = new Map();
    const shipped = ["ollama.exe", "VERSION"].filter((f) =>
      existsSync(path.join(ollamaDir, f)),
    );
    const lib = existsSync(path.join(ollamaDir, "lib"))
      ? walk(path.join(ollamaDir, "lib")).map((f) => `lib/${f}`)
      : [];
    for (const rel of [...shipped, ...lib]) {
      ollama.set(rel, sha256(path.join(ollamaDir, rel)));
    }
  }
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
  const env = (file) =>
    parseEnv(readFileSync(path.join(root, "packaging", file), "utf8"));
  return {
    ollama,
    onnxruntime: existsSync(ort) ? sha256(ort) : null,
    ollamaVersion: env("ollama.env").OLLAMA_VERSION ?? null,
    sdServer,
    sdServerPinned: env("sdcpp.env").SDCPP_VERSION ?? null,
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
    `    ollama/ : ${formatSize(group("ollama/"))}`,
    `    sd-server/ : ${formatSize(group(SD_SERVER_DIR))}`,
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
