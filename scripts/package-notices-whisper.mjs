// Notices du moteur de transcription (whisper-server de whisper.cpp),
// communes aux paquets Linux (scripts/package-notices.mjs :
// licenses/whisper-server/ et paragraphe du fichier copyright) et à
// l'installateur Windows (scripts/package-notices-windows.mjs :
// licenses\whisper-server\).
//
// whisper-server est construit par scripts/build-whisper-server.sh depuis les
// sources de la version épinglée dans packaging/whispercpp.env (correctifs de
// packaging/whispercpp/*.patch), dans .tools/runtimes/whisper-server
// (whisper-server, ses bibliothèques, VERSION, SHA256SUMS ; pas de fallback/ :
// l'app lui donne le chargeur Vulkan de secours de llama-server). Textes de
// licence des parties compilées : packaging/whispercpp/licenses/.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import {
  llamaServerFingerprint,
  versionPattern,
  vulkanPins,
} from "./package-notices-llama.mjs";

const REPOSITORY = "https://github.com/Elwynn18/chatonlm";
const RULE = "=".repeat(78);

/** Dossier construit par scripts/build-whisper-server.sh. */
export const WHISPER_SERVER_DIR = ".tools/runtimes/whisper-server";

/** Textes de licence des parties de whisper-server, relevés dans ses sources. */
export const WHISPER_SERVER_LICENSE_DIR = "packaging/whispercpp/licenses";

/**
 * Textes de WHISPER_SERVER_LICENSE_DIR, exigés : nom du fichier → ce qu'il
 * couvre. Un fichier de plus est livré aussi, sans description.
 */
export const WHISPER_SERVER_LICENSES = {
  "whisper.cpp-LICENSE.txt":
    "whisper.cpp, and ggml with its CPU and Vulkan backends and shaders (MIT)",
  "cpp-httplib-LICENSE.txt": "cpp-httplib (MIT)",
  "nlohmann-json-LICENSE.txt": "JSON for Modern C++ (MIT)",
  "stb-LICENSE.txt": "stb_vorbis (MIT or public domain)",
  "miniaudio-LICENSE.txt": "miniaudio (public domain or MIT No Attribution)",
  "Vulkan-Headers-LICENSE.txt":
    "Vulkan headers compiled into the Vulkan backend (Apache-2.0 or MIT)",
  "ported-code.txt":
    "code ported from OpenAI Whisper, llama.cpp, YaRN, llamafile and " +
    "Arm Optimized Routines (MIT, Apache-2.0)",
};

/** Textes de packaging/licenses livrés avec ceux de whisper-server. */
export const WHISPER_SERVER_EXTRA_TEXTS = {
  "Apache-2.0.txt": "packaging/licenses/Apache-2.0.txt",
};

/** Licences du code de whisper-server, en une phrase (README, copyright). */
export const WHISPER_SERVER_LICENSE_SUMMARY =
  "MIT, MIT No Attribution and Apache-2.0 licenses, and public-domain code";

/**
 * Correctifs de ChatonLM appliqués aux sources (packaging/whispercpp/*.patch),
 * résumés dans la notice : un correctif sans résumé arrête
 * package-notices.mjs.
 */
export const WHISPER_SERVER_PATCHES = {
  "0001-windows-utf8-code-page.patch":
    "Windows only: whisper-server.exe declares UTF-8 as its code page (an " +
    "application manifest), so that a model path with non-ASCII characters " +
    "(a user folder) reaches whisper.cpp in UTF-8 and opens.",
  "0002-refuse-browser-requests.patch":
    "Requests from a web page (Origin header), for another host name, or to " +
    "/load are refused (403), without CORS headers and with one request per " +
    "connection: only ChatonLM talks to its local whisper-server.",
};

/** Clés de packaging/whispercpp.env citées par les notices. */
const WHISPER_SERVER_ENV_KEYS = [
  "WHISPER_VERSION",
  "WHISPER_REPO",
  "WHISPER_COMMIT",
  "WHISPER_GGML_VERSION",
];

/**
 * Empreinte de la construction, comme scripts/build-whisper-server.sh : 12
 * premiers caractères du SHA-256 de packaging/whispercpp.env, des épingles
 * Vulkan, des correctifs (dans l'ordre de leurs noms) et du script, mis bout
 * à bout (même formule que llama-server).
 */
export const whisperServerFingerprint = llamaServerFingerprint;

/**
 * Erreurs si le whisper-server construit (`builtVersion` : son fichier
 * VERSION, ou null si absent) ne vient pas des sources épinglées (texte de
 * packaging/whispercpp.env) et, si `fingerprint` est donnée, des épingles,
 * des correctifs et du script actuels ; s'il manque un texte de licence
 * (`licenseFiles`) ; si un correctif (`patches` : noms des fichiers .patch)
 * n'est pas résumé dans WHISPER_SERVER_PATCHES, ou l'inverse.
 */
export function whisperServerNoticeErrors(
  envText,
  builtVersion,
  { licenseFiles = [], patches = [], fingerprint = null } = {},
) {
  const env = parseEnv(envText);
  const missing = WHISPER_SERVER_ENV_KEYS.filter((key) => !env[key]);
  if (missing.length > 0) {
    return missing.map((key) => `${key} absent de packaging/whispercpp.env.`);
  }
  const pinned = env.WHISPER_VERSION;
  const rebuild = "lancer scripts/build-whisper-server.sh.";
  const errors = [];
  if (builtVersion === null) {
    errors.push(`whisper-server absent de ${WHISPER_SERVER_DIR} : ${rebuild}`);
  } else if (!versionPattern(pinned).test(builtVersion)) {
    errors.push(
      `whisper-server ${builtVersion} dans ${WHISPER_SERVER_DIR} au lieu de ` +
        `${pinned}+chatonlm.<empreinte> : ${rebuild}`,
    );
  } else if (fingerprint && !builtVersion.endsWith(`.${fingerprint}`)) {
    errors.push(
      `whisper-server ${builtVersion} dans ${WHISPER_SERVER_DIR} construit ` +
        `avec d'autres épingles, correctifs ou un autre script (empreinte ` +
        `attendue ${fingerprint}) : ${rebuild}`,
    );
  }
  for (const file of Object.keys(WHISPER_SERVER_LICENSES)) {
    if (!licenseFiles.includes(file)) {
      errors.push(
        `Licence du moteur de transcription absente : ${WHISPER_SERVER_LICENSE_DIR}/${file}`,
      );
    }
  }
  for (const patch of patches) {
    if (!WHISPER_SERVER_PATCHES[patch]) {
      errors.push(
        `Correctif packaging/whispercpp/${patch} sans résumé dans ` +
          "WHISPER_SERVER_PATCHES (scripts/package-notices-whisper.mjs).",
      );
    }
  }
  for (const patch of Object.keys(WHISPER_SERVER_PATCHES)) {
    if (!patches.includes(patch)) {
      errors.push(
        `Correctif résumé mais absent : packaging/whispercpp/${patch}`,
      );
    }
  }
  return errors;
}

/** Valeurs KEY=valeur d'un fichier .env (comme parseEnv de package-notices.mjs). */
function parseEnv(text) {
  return Object.fromEntries(
    text
      .split("\n")
      .map((line) => /^([A-Z0-9_]+)=(.*)$/.exec(line.trim()))
      .filter(Boolean)
      .map((m) => [m[1], m[2]]),
  );
}

/**
 * Moteur de transcription vu depuis la racine `dir` du dépôt :
 * packaging/whispercpp.env, VERSION du dossier construit (ou null), textes
 * de licence (nom → contenu, ceux de WHISPER_SERVER_LICENSE_DIR puis
 * WHISPER_SERVER_EXTRA_TEXTS), correctifs, empreinte attendue et erreurs
 * (whisperServerNoticeErrors).
 */
export function readWhisperServer(dir) {
  const at = (rel) => path.join(dir, rel);
  const readOr = (rel) =>
    existsSync(at(rel)) ? readFileSync(at(rel), "utf8") : "";
  const envText = readOr("packaging/whispercpp.env");
  const sdEnvText = readOr("packaging/sdcpp.env");
  const versionFile = at(`${WHISPER_SERVER_DIR}/VERSION`);
  const builtVersion = existsSync(versionFile)
    ? readFileSync(versionFile, "utf8").trim()
    : null;
  const licenseDir = at(WHISPER_SERVER_LICENSE_DIR);
  const licenseFiles = existsSync(licenseDir)
    ? readdirSync(licenseDir).sort()
    : [];
  const patchDir = at("packaging/whispercpp");
  const patches = existsSync(patchDir)
    ? readdirSync(patchDir)
        .filter((f) => f.endsWith(".patch"))
        .sort()
    : [];
  const script = at("scripts/build-whisper-server.sh");
  const fingerprint =
    envText && existsSync(script)
      ? whisperServerFingerprint({
          env: readFileSync(at("packaging/whispercpp.env")),
          vulkan: vulkanPins(sdEnvText),
          patches: patches.map((f) => readFileSync(path.join(patchDir, f))),
          script: readFileSync(script),
        })
      : null;
  const texts = {};
  for (const file of licenseFiles) {
    texts[file] = readFileSync(path.join(licenseDir, file), "utf8");
  }
  for (const [file, source] of Object.entries(WHISPER_SERVER_EXTRA_TEXTS)) {
    texts[file] = readFileSync(at(source), "utf8");
  }
  return {
    env: parseEnv(envText),
    builtVersion,
    licenseFiles,
    patches,
    fingerprint,
    texts,
    errors: whisperServerNoticeErrors(envText, builtVersion, {
      licenseFiles,
      patches,
      fingerprint,
    }),
  };
}

/**
 * Texte en lignes de 76 caractères au plus (mots entiers), chacune précédée
 * de `prefix`.
 */
function wrap(text, prefix = "", width = 76) {
  const lines = [];
  let line = "";
  for (const word of text.split(/\s+/)) {
    if (line && prefix.length + line.length + 1 + word.length > width) {
      lines.push(prefix + line);
      line = word;
    } else {
      line = line ? `${line} ${word}` : word;
    }
  }
  if (line) lines.push(prefix + line);
  return lines;
}

function heading(title) {
  return `${title}\n${"-".repeat(title.length)}`;
}

/**
 * Paragraphe DEP-5 du moteur de transcription (`version`, `commit` épinglés
 * dans packaging/whispercpp.env ; dans l'AppImage, il est rangé dans
 * usr/libexec) : whisper-server sous MIT (Expat). Pas de chargeur Vulkan à
 * lui : celui de llama-server/fallback/ a son propre paragraphe.
 */
export function whisperServerCopyright({ version, commit, appimage }) {
  const dir = appimage
    ? "usr/libexec/chatonlm/whisper-server"
    : "usr/lib/chatonlm/whisper-server";
  const indent = (line, i) => (i === 0 ? line : ` ${line}`);
  return [
    `Files: ${dir}/*`,
    "Copyright: 2023-2026 The ggml authors, and others",
    "License: Expat",
    ...wrap(
      `Comment: Transcription engine of ChatonLM: whisper-server of ` +
        `whisper.cpp ${version} (https://github.com/ggml-org/whisper.cpp, ` +
        `commit ${commit}), built by ChatonLM as described in ` +
        "licenses/whisper-server/README.txt. It includes code under the " +
        `${WHISPER_SERVER_LICENSE_SUMMARY}: see that README and the license ` +
        "files next to it.",
      "",
      75,
    ).map(indent),
    "",
  ];
}

/**
 * licenses/whisper-server/README.txt (Linux) ou
 * licenses\whisper-server\README.txt (Windows, `windows`) : le moteur de
 * transcription, sa provenance, ce qui en est livré, les correctifs de
 * ChatonLM et les licences (`env` : packaging/whispercpp.env ;
 * `licenseFiles` : textes de WHISPER_SERVER_LICENSE_DIR).
 */
export function renderWhisperServerReadme({
  env,
  licenseFiles,
  windows = false,
}) {
  const version = env.WHISPER_VERSION;
  const repo = env.WHISPER_REPO;
  const sep = windows ? "\\" : "/";
  const exe = windows ? "whisper-server.exe" : "whisper-server";
  const loader = windows ? "vulkan-1.dll" : "libvulkan.so.1";
  const lib = (name) => (windows ? `${name}.dll` : `lib${name}.so*`);
  const files = [
    ...new Set([...licenseFiles, ...Object.keys(WHISPER_SERVER_EXTRA_TEXTS)]),
  ].sort((a, b) => a.localeCompare(b));
  const describe = (file) =>
    WHISPER_SERVER_LICENSES[file] ??
    (file === "Apache-2.0.txt"
      ? "text of the Apache License, Version 2.0"
      : "");
  const rows = files.flatMap((file) => {
    const text = describe(file);
    return text ? [`  ${file}`, ...wrap(text, "      ", 76)] : [`  ${file}`];
  });
  const patches = Object.entries(WHISPER_SERVER_PATCHES);
  const changes =
    patches.length === 0
      ? wrap(
          "None to the source code: whisper-server is built from the source " +
            "code of the pinned version as published. ChatonLM only chooses " +
            "the build options listed above.",
        )
      : [
          ...wrap(
            "The patches below, in packaging/whispercpp of the source code " +
              "of ChatonLM, are applied to the source code before the build:",
          ),
          ...patches.flatMap(([file, text]) => [
            `  ${file}`,
            ...wrap(text, "      ", 76),
          ]),
        ];
  const where = windows
    ? [
        "It is installed in the whisper-server folder, next to chatonlm.exe,",
        "and runs as a separate program.",
      ]
    : [
        "It is installed in /usr/lib/chatonlm/whisper-server (in the AppImage:",
        "usr/libexec/chatonlm/whisper-server, linked from",
        "usr/lib/chatonlm/whisper-server) and runs as a separate program.",
      ];
  const runtime = windows
    ? "\n\n" +
      wrap(
        "whisper-server.exe and its DLLs use the Microsoft Visual C++ " +
          "runtime libraries (vcruntime140*.dll, msvcp140*.dll), shipped " +
          "unmodified in this folder next to them (redistributable files of " +
          "Microsoft Visual Studio, see windows-components.txt in the " +
          "licenses folder).",
      ).join("\n")
    : "";
  return `ChatonLM - Built-in transcription engine (whisper-server)
${RULE}

${wrap(
  "ChatonLM transcribes speech with whisper-server, the server of " +
    "whisper.cpp, on the graphics card when there is one. ChatonLM builds " +
    "it from the source code of the version below (script " +
    "scripts/build-whisper-server.sh of the source code of ChatonLM, " +
    `${REPOSITORY}).`,
).join("\n")}
${where.join("\n")}

Project:      whisper.cpp
Repository:   ${repo}
Version:      ${version} (ggml ${env.WHISPER_GGML_VERSION})
Commit:       ${env.WHISPER_COMMIT}
Source code:  ${repo}/tree/${env.WHISPER_COMMIT}
License:      MIT, Copyright (c) 2023-2026 The ggml authors

${heading("What is shipped")}
${wrap(`${exe}, ${["whisper", "ggml", "ggml-base"].map(lib).join(", ")}`, "  ", 76).join("\n")}
${wrap(
  "the server and its libraries, built without OpenMP, without curl (no " +
    "outgoing connection: ChatonLM downloads the models itself), without " +
    "ffmpeg and without SDL",
  "      ",
  76,
).join("\n")}
  ${lib("ggml-cpu-*")}
${wrap(
  "the CPU backend, one build per instruction set (x64, SSE4.2, AVX, AVX2, " +
    "AVX-512…): the best one for the processor is chosen at startup",
  "      ",
  76,
).join("\n")}
  ${lib("ggml-vulkan")}
${wrap(
  "the Vulkan backend (graphics cards through Vulkan; without one, the " +
    "server computes on the processor)",
  "      ",
  76,
).join("\n")}
  VERSION, SHA256SUMS
${wrap(
  `version of the build: ${version}+chatonlm.<fingerprint>, the ` +
    "fingerprint covering the pinned version, the Vulkan build tools, the " +
    "patches and the build script; checksums of the shipped files",
  "      ",
  76,
).join("\n")}${runtime}

${heading("Changes made by ChatonLM")}
${changes.join("\n")}

${heading("Licenses")}
${wrap(
  "whisper.cpp, ggml and the code they include are under the " +
    `${WHISPER_SERVER_LICENSE_SUMMARY}. The files below, in this ` +
    `${windows ? "folder" : "directory"}, give each license text and the ` +
    "parts it covers.",
).join("\n")}

${rows.join("\n")}

${heading("Vulkan loader")}
${wrap(
  "whisper-server has no Vulkan loader of its own. Where the system has " +
    "none, ChatonLM points it to the one shipped with the text engine, " +
    `llama-server${sep}fallback${sep}${loader} (the Vulkan loader of the ` +
    "Khronos Group, under the Apache License, Version 2.0: see " +
    `the licenses of the text engine, llama-server${sep}README.txt).`,
).join("\n")}
`;
}
