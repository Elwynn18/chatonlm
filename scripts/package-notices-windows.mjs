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
//   sd-server\README.txt       le moteur d'images (sd-server construit par
//   sd-server\<licences>       scripts/build-sd-server.sh) : textes de
//                              packaging/sdcpp/licenses et Apache-2.0 du
//                              chargeur Vulkan de secours
//   llama-server\README.txt    le moteur de texte (llama-server construit
//   llama-server\<licences>    par scripts/build-llama-server.sh) : textes
//                              de packaging/llama/licenses et Apache-2.0
//
// Pas de fichier copyright (format de Debian), pas de textes GPL ni LGPL :
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
  readLlamaServer,
  renderLlamaServerReadme,
} from "./package-notices-llama.mjs";
import {
  CARGO_ABOUT_VERSION,
  CSS_PACKAGES,
  ONNXRUNTIME_DIR,
  onnxRuntimeErrors,
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

/** Textes de licence des composants propres à l'installateur Windows. */
export const WINDOWS_COMPONENT_TEXTS = {
  "WebView2-SDK-LICENSE.txt":
    "packaging/windows/licenses/WebView2-SDK-LICENSE.txt",
  "NSIS-COPYING.txt": "packaging/windows/licenses/NSIS-COPYING.txt",
};

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
libraries, Copyright (c) Microsoft Corporation. ChatonLM and ONNX Runtime
need them. They are not open-source software: they are redistributable files
of Microsoft Visual Studio, shipped unmodified under the Microsoft Software
License Terms of Visual Studio.
llama-server\\, the text engine, has its own copies of them (a program
started by ChatonLM does not look next to chatonlm.exe);
sd-server\\sd-server.exe, the image engine, contains the static version of
these libraries. All under the same terms.
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
  const sdServer = readSdServer(root);
  errors.push(...sdServer.errors);
  const llamaServer = readLlamaServer(root);
  errors.push(...llamaServer.errors);
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
  };
  for (const [file, source] of Object.entries(WINDOWS_COMPONENT_TEXTS)) {
    files[file] = read(source);
  }
  files["sd-server/README.txt"] = renderSdServerReadme({
    env: sdServer.env,
    licenseFiles: sdServer.licenseFiles,
    windows: true,
  });
  for (const [file, text] of Object.entries(sdServer.texts)) {
    files[`sd-server/${file}`] = text;
  }
  files["llama-server/README.txt"] = renderLlamaServerReadme({
    env: llamaServer.env,
    licenseFiles: llamaServer.licenseFiles,
    windows: true,
  });
  for (const [file, text] of Object.entries(llamaServer.texts)) {
    files[`llama-server/${file}`] = text;
  }

  rmSync(out, { recursive: true, force: true });
  for (const [file, content] of Object.entries(files)) {
    write(out, file, content);
  }
  console.log(
    `Notices Windows écrites dans ${WINDOWS_NOTICES_DIR} ` +
      `(${about.crates.length} crates, ` +
      `${Object.keys(sdServer.texts).length} du moteur d'images, ` +
      `${Object.keys(llamaServer.texts).length} du moteur de texte).`,
  );
}
