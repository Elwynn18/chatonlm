// Notices livrées dans l'installateur Windows de ChatonLM (NSIS), écrites par
// scripts/package-notices.mjs quand il tourne sous Windows (ou avec
// --windows) : dossier .tools/package/notices/windows/, installé dans
// <dossier de l'app>\licenses par packaging/windows/bundle.conf.json.
//
//   LICENSE.txt, THIRD_PARTY_LICENSES.txt
//   rust-crates.txt            cargo-about, cible x86_64-pc-windows-msvc
//                              (configuration dérivée de packaging/about.toml)
//   npm-packages.txt, speech-models.txt, voice-models.txt
//   onnxruntime-LICENSE.txt, onnxruntime-ThirdPartyNotices.txt
//   windows-components.txt     chargeur WebView2, NSIS, runtime Visual C++
//   WebView2-SDK-LICENSE.txt, NSIS-COPYING.txt
//   ollama\README.txt          l'Ollama intégré pour Windows, ses fichiers
//   ollama\<licences>          de licence (archive officielle), et les
//                              textes des bibliothèques de LLVM et de
//                              mingw-w64 qu'elle contient sans licence
//   sd-server\README.txt       le moteur d'images (sd-server construit par
//   sd-server\<licences>       scripts/build-sd-server.sh) : textes de
//                              packaging/sdcpp/licenses et Apache-2.0 du
//                              chargeur Vulkan de secours
//
// Pas de fichier copyright (format de Debian), pas de textes GPL ni LGPL :
// l'archive Windows d'Ollama n'a pas libgomp (OpenMP de LLVM à la place) et
// l'installateur n'embarque aucune bibliothèque du système.
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import {
  CARGO_ABOUT_VERSION,
  CSS_PACKAGES,
  OLLAMA_DIR,
  OLLAMA_LICENSE_SUMMARY,
  OLLAMA_LICENSES,
  ONNXRUNTIME_DIR,
  ollamaLicenseKindErrors,
  onnxRuntimeErrors,
  parseEnv,
  readSdServer,
  renderNpmPackages,
  renderRustCrates,
  renderSdServerReadme,
  renderSpeechModels,
  renderVoiceModels,
} from "./package-notices.mjs";

const root = path.resolve(import.meta.dirname, "..");
const RULE = "=".repeat(78);

/** Cible Rust de l'installateur Windows. */
export const WINDOWS_TARGET = "x86_64-pc-windows-msvc";

/** Dossier des notices Windows, relatif à la racine. */
export const WINDOWS_NOTICES_DIR = ".tools/package/notices/windows";

/**
 * Licences livrées par Ollama dans lib\ollama de l'archive Windows (0.40.0),
 * exigées : celles de Linux, plus deux propres à Windows.
 */
export const OLLAMA_WINDOWS_LICENSES = {
  ...OLLAMA_LICENSES,
  DLFCN_WIN32_COPYING: "dlfcn-win32 (MIT)",
  "JSON_LICENSE.MIT": "JSON for Modern C++ (MIT)",
};

/**
 * Fichiers de licence de l'archive qui ne sont pas livrés : ils couvrent les
 * bibliothèques de CUDA (cuDNN, CUTLASS, NVTX), retirées de l'Ollama intégré.
 */
export const OLLAMA_WINDOWS_SKIPPED = /^(CUDNN|CUTLASS|NVTX\d*)_/i;

/** Textes de licence ajoutés par ChatonLM à ceux de l'archive d'Ollama. */
export const OLLAMA_WINDOWS_EXTRA_TEXTS = {
  "LLVM-LICENSE.txt": "packaging/windows/licenses/LLVM-LICENSE.txt",
  "winpthreads-COPYING.txt":
    "packaging/windows/licenses/winpthreads-COPYING.txt",
};

/** Textes de licence des composants propres à l'installateur Windows. */
export const WINDOWS_COMPONENT_TEXTS = {
  "WebView2-SDK-LICENSE.txt":
    "packaging/windows/licenses/WebView2-SDK-LICENSE.txt",
  "NSIS-COPYING.txt": "packaging/windows/licenses/NSIS-COPYING.txt",
};

/** Vrai pour un fichier de licence de lib\ollama (GO_LICENSE, X_NOTICE…). */
export function isOllamaLicenseFile(name) {
  return /(_LICENSE(\.txt|\.MIT)?|_NOTICE(\.txt)?|_COPYING(\.txt)?)$/.test(
    name,
  );
}

/**
 * Fichiers de licence à livrer parmi `names` (noms des fichiers de
 * licenses/ ou de lib/ollama) : ceux de l'archive, sans ceux de CUDA.
 */
export function ollamaWindowsLicenseFiles(names) {
  return names
    .filter(
      (name) => isOllamaLicenseFile(name) && !OLLAMA_WINDOWS_SKIPPED.test(name),
    )
    .sort();
}

/**
 * Erreurs si la copie de l'Ollama intégré (`fetchedVersion` : son fichier
 * VERSION, ou null) n'est pas la version épinglée (texte de
 * packaging/ollama.env), ou s'il manque une licence attendue.
 */
export function ollamaWindowsErrors(envText, fetchedVersion, licenseFiles) {
  const pinned = parseEnv(envText).OLLAMA_VERSION;
  if (!pinned) return ["OLLAMA_VERSION absent de packaging/ollama.env."];
  if (fetchedVersion !== pinned) {
    return [
      `Ollama ${fetchedVersion ?? "absent"} dans ${OLLAMA_DIR} au lieu ` +
        `de ${pinned} : lancer scripts/fetch-ollama.sh.`,
    ];
  }
  return Object.keys(OLLAMA_WINDOWS_LICENSES)
    .filter((file) => !licenseFiles.includes(file))
    .map((file) => `Licence d'Ollama absente (archive Windows) : ${file}`);
}

/**
 * Adresse et empreinte de l'archive Windows d'Ollama, d'après
 * packaging/ollama.env (clés dont le nom contient WIN, quel que soit le nom
 * exact choisi) ; à défaut, l'adresse officielle de la version épinglée.
 */
export function ollamaWindowsArchive(env) {
  const find = (suffix) =>
    Object.entries(env).find(
      ([key]) => /WIN/.test(key) && key.endsWith(suffix),
    )?.[1];
  return {
    url:
      find("URL") ??
      `https://github.com/ollama/ollama/releases/download/v${env.OLLAMA_VERSION}/ollama-windows-amd64.zip`,
    sha256: find("SHA256") ?? null,
  };
}

/**
 * Configuration de cargo-about pour Windows : celle de packaging/about.toml
 * (même liste blanche), avec la cible Windows à la place de celle de Linux.
 */
export function windowsAboutConfig(aboutToml) {
  const line = /^targets = \[[^\]]*\]$/m;
  if (!line.test(aboutToml)) {
    throw new Error(
      "packaging/about.toml : ligne « targets = [...] » absente.",
    );
  }
  return aboutToml.replace(line, `targets = ["${WINDOWS_TARGET}"]`);
}

function heading(title) {
  return `${title}\n${"-".repeat(title.length)}`;
}

/** Texte en lignes de 76 caractères au plus (mots entiers). */
function wrap(text, width = 76) {
  const lines = [];
  let line = "";
  for (const word of text.split(/\s+/)) {
    if (line && line.length + 1 + word.length > width) {
      lines.push(line);
      line = word;
    } else {
      line = line ? `${line} ${word}` : word;
    }
  }
  if (line) lines.push(line);
  return lines.join("\n");
}

/** Ligne « nom  description » de la liste des fichiers de licence. */
function licenseRow(file, text) {
  if (!text) return `  ${file}`;
  return file.length < 26
    ? `  ${file.padEnd(28)}${text}`
    : `  ${file}\n  ${"".padEnd(28)}${text}`;
}

/**
 * licenses\ollama\README.txt : l'Ollama intégré pour Windows (`env` :
 * packaging/ollama.env ; `licenseFiles` : fichiers de licence livrés).
 */
export function renderOllamaWindowsReadme({ env, licenseFiles }) {
  const version = env.OLLAMA_VERSION;
  const archive = ollamaWindowsArchive(env);
  const describe = (file) =>
    OLLAMA_WINDOWS_LICENSES[file] ??
    (/^MLX/.test(file) ? "MLX (MIT), whose library is not shipped" : "");
  const rows = [
    ...licenseFiles.map((file) => licenseRow(file, describe(file))),
    licenseRow("LLVM-LICENSE.txt", "LLVM runtime libraries (see below)"),
    licenseRow("winpthreads-COPYING.txt", "winpthreads (see below)"),
  ];
  return `ChatonLM - Built-in Ollama
${RULE}

ChatonLM includes a copy of Ollama ${version}, the official build for Windows
(x86-64), used when no other Ollama server is available or when chosen in
Settings. It is installed in the ollama folder, next to chatonlm.exe, and runs
as a separate program.

Release:      https://github.com/ollama/ollama/releases/tag/v${version}
Archive:      ${archive.url}
${archive.sha256 ? `SHA-256:      ${archive.sha256}\n` : ""}Source code:  https://github.com/ollama/ollama/tree/v${version}
License:      MIT, Copyright (c) Ollama

Only part of the archive is shipped: the server (ollama.exe), the llama.cpp
runner (lib\\ollama\\llama-server.exe, lib\\ollama\\llama-quantize.exe) and the
CPU and Vulkan libraries. The CUDA libraries are left out: graphics cards are
used through Vulkan. The files themselves are not modified.

${heading("Licenses")}
${wrap(
  "The license files below are those of the lib\\ollama folder of the " +
    "archive, shipped unmodified in this folder, plus the texts that apply " +
    "to the runtime libraries described below. Besides Ollama itself, they " +
    `cover code under the ${OLLAMA_LICENSE_SUMMARY}.`,
)}

${rows.join("\n")}

${heading("Runtime libraries")}
The archive also contains runtime libraries made by other projects, for which
it carries no license file:

  lib\\ollama\\libc++.dll, libunwind.dll, libomp.dll
      C++, unwinding and OpenMP runtime libraries of the LLVM Project
      (https://llvm.org), as built by llvm-mingw, under the Apache License
      2.0 with LLVM Exceptions: LLVM-LICENSE.txt in this folder.

  lib\\ollama\\libwinpthread-1.dll
      winpthreads, POSIX threads library of the mingw-w64 project
      (https://www.mingw-w64.org), under the MIT License, with parts under a
      BSD 3-Clause License: winpthreads-COPYING.txt in this folder.

  lib\\ollama\\vulkan\\vcruntime140*.dll, msvcp140*.dll, concrt140.dll,
  vccorlib140.dll
      Microsoft Visual C++ runtime libraries, shipped by Ollama for its
      Vulkan engine: see windows-components.txt in the licenses folder.
`;
}

/**
 * licenses\windows-components.txt : composants propres à la version Windows
 * (chargeur WebView2 lié dans l'exécutable, installateur NSIS, runtime
 * Visual C++ livré à côté de l'exécutable).
 */
export function renderWindowsComponents() {
  return `ChatonLM - Components of the Windows version
${RULE}

${heading("WebView2")}
ChatonLM shows its interface with Microsoft Edge WebView2. The loader of the
WebView2 SDK (Microsoft.Web.WebView2) is linked into chatonlm.exe. It is
under a BSD 3-Clause License, Copyright (C) Microsoft Corporation:
WebView2-SDK-LICENSE.txt in this folder.

The WebView2 Runtime itself is not part of ChatonLM. Windows 11 includes it;
where it is missing, the installer downloads its installer from Microsoft
(https://go.microsoft.com/fwlink/p/?LinkId=2124703), under Microsoft's terms.

${heading("Installer")}
The installer and uninstall.exe are made with NSIS, the Nullsoft Scriptable
Install System (https://nsis.sourceforge.io), under the zlib/libpng license;
its LZMA compression module is under the Common Public License 1.0, with a
special exception for the installers it produces: NSIS-COPYING.txt in this
folder. They also contain the installer template and the nsis_tauri_utils
plug-in of the Tauri project (https://github.com/tauri-apps), under the MIT
License or the Apache License 2.0.

${heading("Microsoft Visual C++ runtime")}
The files vcruntime140*.dll, msvcp140*.dll, concrt140.dll and
vccorlib140.dll next to chatonlm.exe are the Microsoft Visual C++ runtime
libraries, Copyright (c) Microsoft Corporation. ChatonLM, ONNX Runtime and
the Vulkan engine of the built-in Ollama need them (Ollama ships its own
copies in ollama\\lib\\ollama\\vulkan). They are not open-source software:
they are redistributable files of Microsoft Visual Studio, shipped
unmodified under the Microsoft Software License Terms of Visual Studio.
sd-server\\sd-server.exe, the image engine, contains the static version of
these libraries, under the same terms.
`;
}

function read(file) {
  return readFileSync(path.join(root, file), "utf8");
}

function write(dir, file, content) {
  mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
  writeFileSync(path.join(dir, file), content);
}

function fail(message) {
  console.error(`package-notices (Windows) : ${message}`);
  process.exit(1);
}

/** Version de cargo-about installée, ou null. */
function installedCargoAbout() {
  try {
    const out = execFileSync("cargo", ["about", "--version"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return /cargo-about (\S+)/.exec(out)?.[1] ?? null;
  } catch {
    return null;
  }
}

/** Licence d'un paquet npm de node_modules, au format de license.json. */
function nodePackage(name) {
  const dir = path.join(root, "node_modules", name);
  const pkg = JSON.parse(readFileSync(path.join(dir, "package.json"), "utf8"));
  const file = readdirSync(dir).find((f) =>
    /^(licen[cs]e|copying)(\.|$)/i.test(f),
  );
  return {
    name,
    version: pkg.version,
    identifier: typeof pkg.license === "string" ? pkg.license : "",
    text: file ? readFileSync(path.join(dir, file), "utf8") : "",
  };
}

/**
 * Dossier des fichiers de licence de l'Ollama intégré : licenses/ s'il
 * existe (rangés à part par scripts/fetch-ollama.sh, comme sous Linux),
 * sinon lib/ollama (arborescence de l'archive).
 */
function ollamaLicenseDir() {
  const apart = path.join(root, OLLAMA_DIR, "licenses");
  if (existsSync(apart) && readdirSync(apart).length > 0) return apart;
  return path.join(root, OLLAMA_DIR, "lib", "ollama");
}

export function main() {
  const offline = process.argv.includes("--offline");
  const out = path.join(root, WINDOWS_NOTICES_DIR);

  const ortVersionFile = path.join(root, ONNXRUNTIME_DIR, "VERSION");
  const errors = onnxRuntimeErrors(
    read("packaging/onnxruntime.env"),
    existsSync(ortVersionFile)
      ? readFileSync(ortVersionFile, "utf8").trim()
      : null,
  );
  const licenseDir = ollamaLicenseDir();
  const licenseFiles = existsSync(licenseDir)
    ? ollamaWindowsLicenseFiles(readdirSync(licenseDir))
    : [];
  const ollamaVersionFile = path.join(root, OLLAMA_DIR, "VERSION");
  errors.push(
    ...ollamaWindowsErrors(
      read("packaging/ollama.env"),
      existsSync(ollamaVersionFile)
        ? readFileSync(ollamaVersionFile, "utf8").trim()
        : null,
      licenseFiles,
    ),
    ...ollamaLicenseKindErrors(
      new Map(
        licenseFiles.map((file) => [
          file,
          readFileSync(path.join(licenseDir, file), "utf8"),
        ]),
      ),
    ),
  );
  const sdServer = readSdServer(root);
  errors.push(...sdServer.errors);
  if (errors.length > 0) fail(errors.join("\n"));
  const installed = installedCargoAbout();
  if (installed !== CARGO_ABOUT_VERSION) {
    fail(
      `cargo-about ${CARGO_ABOUT_VERSION} attendu (${installed ?? "absent"}) : ` +
        `cargo install cargo-about --version ${CARGO_ABOUT_VERSION} --locked --features cli`,
    );
  }
  if (!existsSync(path.join(root, "dist/.vite/license.json"))) {
    fail("dist/.vite/license.json introuvable : lancer d'abord vite build.");
  }

  // Configuration de cargo-about dérivée de celle de Linux, hors du dépôt.
  const aboutConfig = path.join(root, ".tools/package/about-windows.toml");
  mkdirSync(path.dirname(aboutConfig), { recursive: true });
  writeFileSync(aboutConfig, windowsAboutConfig(read("packaging/about.toml")));
  const about = JSON.parse(
    execFileSync(
      "cargo",
      [
        "about",
        "generate",
        "--format",
        "json",
        "--config",
        aboutConfig,
        "--manifest-path",
        "src-tauri/Cargo.toml",
        "--locked",
        "--fail",
        ...(offline ? ["--offline"] : []),
      ],
      { cwd: root, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
    ),
  );

  const files = {
    "LICENSE.txt": read("LICENSE"),
    "THIRD_PARTY_LICENSES.txt": read("THIRD_PARTY_LICENSES"),
    "rust-crates.txt": renderRustCrates(about, {
      cargoAboutVersion: CARGO_ABOUT_VERSION,
      target: WINDOWS_TARGET,
    }),
    "npm-packages.txt": renderNpmPackages(
      JSON.parse(read("dist/.vite/license.json")),
      {
        css: CSS_PACKAGES.map(nodePackage),
        apacheText: read("packaging/licenses/Apache-2.0.txt"),
      },
    ),
    "speech-models.txt": renderSpeechModels(),
    "voice-models.txt": renderVoiceModels(),
    "onnxruntime-LICENSE.txt": read(`${ONNXRUNTIME_DIR}/LICENSE`),
    "onnxruntime-ThirdPartyNotices.txt": read(
      `${ONNXRUNTIME_DIR}/ThirdPartyNotices.txt`,
    ),
    "windows-components.txt": renderWindowsComponents(),
    "ollama/README.txt": renderOllamaWindowsReadme({
      env: parseEnv(read("packaging/ollama.env")),
      licenseFiles,
    }),
  };
  for (const [file, source] of Object.entries(WINDOWS_COMPONENT_TEXTS)) {
    files[file] = read(source);
  }
  for (const file of licenseFiles) {
    files[`ollama/${file}`] = readFileSync(path.join(licenseDir, file), "utf8");
  }
  for (const [file, source] of Object.entries(OLLAMA_WINDOWS_EXTRA_TEXTS)) {
    files[`ollama/${file}`] = read(source);
  }
  files["sd-server/README.txt"] = renderSdServerReadme({
    env: sdServer.env,
    licenseFiles: sdServer.licenseFiles,
    windows: true,
  });
  for (const [file, text] of Object.entries(sdServer.texts)) {
    files[`sd-server/${file}`] = text;
  }

  rmSync(out, { recursive: true, force: true });
  for (const [file, content] of Object.entries(files)) {
    write(out, file, content);
  }
  console.log(
    `Notices Windows écrites dans ${WINDOWS_NOTICES_DIR} ` +
      `(${about.crates.length} crates, ${licenseFiles.length} licences d'Ollama, ` +
      `${Object.keys(sdServer.texts).length} du moteur d'images).`,
  );
}
