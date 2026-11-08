// Notices du moteur de texte (llama-server de llama.cpp), communes aux paquets
// Linux (scripts/package-notices.mjs : licenses/llama-server/ et paragraphes
// du fichier copyright) et à l'installateur Windows
// (scripts/package-notices-windows.mjs : licenses\llama-server\).
//
// llama-server est construit par scripts/build-llama-server.sh depuis les
// sources de la version épinglée dans packaging/llama.env (correctifs
// éventuels de packaging/llama/*.patch), dans .tools/runtimes/llama-server
// (llama-server, ses bibliothèques, fallback/, VERSION, SHA256SUMS). Textes de
// licence des parties compilées : packaging/llama/licenses/ ; chargeur Vulkan
// de secours : le même que celui de sd-server (épingles VULKAN_* de
// packaging/sdcpp.env).
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const REPOSITORY = "https://github.com/Elwynn18/chatonlm";
const RULE = "=".repeat(78);

/** Dossier construit par scripts/build-llama-server.sh. */
export const LLAMA_SERVER_DIR = ".tools/runtimes/llama-server";

/** Textes de licence des parties de llama-server, relevés dans ses sources. */
export const LLAMA_SERVER_LICENSE_DIR = "packaging/llama/licenses";

/**
 * Textes de LLAMA_SERVER_LICENSE_DIR, exigés : nom du fichier → ce qu'il
 * couvre. Un fichier de plus est livré aussi, sans description.
 */
export const LLAMA_SERVER_LICENSES = {
  "llama.cpp-LICENSE.txt":
    "llama.cpp, and ggml with its CPU and Vulkan backends and shaders (MIT)",
  "cpp-httplib-LICENSE.txt": "cpp-httplib (MIT)",
  "nlohmann-json-LICENSE.txt": "JSON for Modern C++ (MIT)",
  "stb-LICENSE.txt": "stb_image (MIT or public domain)",
  "miniaudio-LICENSE.txt": "miniaudio (public domain or MIT No Attribution)",
  "subprocess-LICENSE.txt": "subprocess.h (the Unlicense)",
  "hash-LICENSES.txt":
    "xxHash (BSD-2-Clause), SHA-1 and SHA-256 (public domain), rotate-bits (MIT)",
  "Vulkan-Headers-LICENSE.txt":
    "Vulkan headers compiled into the Vulkan backend (Apache-2.0 or MIT)",
  "ported-code.txt":
    "code ported from YaRN, llamafile, ggllm.cpp, KoboldCpp, Pillow, " +
    "Arm Optimized Routines and whisper.cpp, and base64.hpp " +
    "(MIT, MIT-CMU, Apache-2.0, public domain)",
  "Vulkan-Loader-components.txt":
    "Vulkan loader of fallback/ (Apache-2.0) and the code it includes",
};

/** Textes de packaging/licenses livrés avec ceux de llama-server. */
export const LLAMA_SERVER_EXTRA_TEXTS = {
  "Apache-2.0.txt": "packaging/licenses/Apache-2.0.txt",
};

/** Licences du code de llama-server, en une phrase (README, copyright). */
export const LLAMA_SERVER_LICENSE_SUMMARY =
  "MIT, MIT No Attribution, MIT-CMU, BSD-2-Clause and Apache-2.0 licenses, the Unlicense, and public-domain code";

/**
 * Correctifs de ChatonLM appliqués aux sources (packaging/llama/*.patch),
 * résumés dans la notice : un correctif sans résumé arrête
 * package-notices.mjs. Aucun pour l'instant.
 */
export const LLAMA_SERVER_PATCHES = {};

/** Clés de packaging/llama.env citées par les notices. */
const LLAMA_SERVER_ENV_KEYS = [
  "LLAMA_VERSION",
  "LLAMA_BUILD",
  "LLAMA_REPO",
  "LLAMA_COMMIT",
  "LLAMA_GGML_VERSION",
];

/** Lignes VULKAN_* de packaging/sdcpp.env, comme les relève le script. */
export function vulkanPins(sdEnvText) {
  const lines = sdEnvText.split("\n").filter((line) => /^VULKAN_/.test(line));
  return `${lines.join("\n")}\n`;
}

/**
 * Empreinte de la construction, comme scripts/build-llama-server.sh : 12
 * premiers caractères du SHA-256 de packaging/llama.env, des épingles Vulkan,
 * des correctifs (dans l'ordre de leurs noms) et du script, mis bout à bout.
 */
export function llamaServerFingerprint({ env, vulkan, patches, script }) {
  const hash = createHash("sha256");
  for (const part of [env, vulkan, ...patches, script]) hash.update(part);
  return hash.digest("hex").slice(0, 12);
}

/**
 * Erreurs si le llama-server construit (`builtVersion` : son fichier VERSION,
 * ou null si absent) ne vient pas des sources épinglées (texte de
 * packaging/llama.env) et, si `fingerprint` est donnée, des épingles, des
 * correctifs et du script actuels ; s'il manque un texte de licence
 * (`licenseFiles` : noms des fichiers de LLAMA_SERVER_LICENSE_DIR) ; si un
 * correctif (`patches` : noms des fichiers .patch) n'est pas résumé dans
 * LLAMA_SERVER_PATCHES, ou l'inverse.
 */
export function llamaServerNoticeErrors(
  envText,
  builtVersion,
  { licenseFiles = [], patches = [], fingerprint = null } = {},
) {
  const env = parseEnv(envText);
  const missing = LLAMA_SERVER_ENV_KEYS.filter((key) => !env[key]);
  if (missing.length > 0) {
    return missing.map((key) => `${key} absent de packaging/llama.env.`);
  }
  const pinned = env.LLAMA_VERSION;
  const rebuild = "lancer scripts/build-llama-server.sh.";
  const errors = [];
  if (builtVersion === null) {
    errors.push(`llama-server absent de ${LLAMA_SERVER_DIR} : ${rebuild}`);
  } else if (!versionPattern(pinned).test(builtVersion)) {
    errors.push(
      `llama-server ${builtVersion} dans ${LLAMA_SERVER_DIR} au lieu de ` +
        `${pinned}+chatonlm.<empreinte> : ${rebuild}`,
    );
  } else if (fingerprint && !builtVersion.endsWith(`.${fingerprint}`)) {
    errors.push(
      `llama-server ${builtVersion} dans ${LLAMA_SERVER_DIR} construit avec ` +
        `d'autres épingles, correctifs ou un autre script (empreinte attendue ` +
        `${fingerprint}) : ${rebuild}`,
    );
  }
  for (const file of Object.keys(LLAMA_SERVER_LICENSES)) {
    if (!licenseFiles.includes(file)) {
      errors.push(
        `Licence du moteur de texte absente : ${LLAMA_SERVER_LICENSE_DIR}/${file}`,
      );
    }
  }
  for (const patch of patches) {
    if (!LLAMA_SERVER_PATCHES[patch]) {
      errors.push(
        `Correctif packaging/llama/${patch} sans résumé dans ` +
          "LLAMA_SERVER_PATCHES (scripts/package-notices-llama.mjs).",
      );
    }
  }
  for (const patch of Object.keys(LLAMA_SERVER_PATCHES)) {
    if (!patches.includes(patch)) {
      errors.push(`Correctif résumé mais absent : packaging/llama/${patch}`);
    }
  }
  return errors;
}

/** « <version épinglée>+chatonlm.<empreinte de 12 caractères> ». */
export function versionPattern(pinned) {
  const escaped = pinned.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^${escaped}\\+chatonlm\\.[0-9a-f]{12}$`);
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
 * Moteur de texte vu depuis la racine `dir` du dépôt : packaging/llama.env,
 * VERSION du dossier construit (ou null), textes de licence (nom → contenu,
 * ceux de LLAMA_SERVER_LICENSE_DIR puis LLAMA_SERVER_EXTRA_TEXTS),
 * correctifs, empreinte attendue et erreurs (llamaServerNoticeErrors).
 */
export function readLlamaServer(dir) {
  const at = (rel) => path.join(dir, rel);
  const readOr = (rel) =>
    existsSync(at(rel)) ? readFileSync(at(rel), "utf8") : "";
  const envText = readOr("packaging/llama.env");
  const sdEnvText = readOr("packaging/sdcpp.env");
  const versionFile = at(`${LLAMA_SERVER_DIR}/VERSION`);
  const builtVersion = existsSync(versionFile)
    ? readFileSync(versionFile, "utf8").trim()
    : null;
  const licenseDir = at(LLAMA_SERVER_LICENSE_DIR);
  const licenseFiles = existsSync(licenseDir)
    ? readdirSync(licenseDir).sort()
    : [];
  const patchDir = at("packaging/llama");
  const patches = existsSync(patchDir)
    ? readdirSync(patchDir)
        .filter((f) => f.endsWith(".patch"))
        .sort()
    : [];
  const script = at("scripts/build-llama-server.sh");
  const fingerprint =
    envText && existsSync(script)
      ? llamaServerFingerprint({
          env: readFileSync(at("packaging/llama.env")),
          vulkan: vulkanPins(sdEnvText),
          patches: patches.map((f) => readFileSync(path.join(patchDir, f))),
          script: readFileSync(script),
        })
      : null;
  const texts = {};
  for (const file of licenseFiles) {
    texts[file] = readFileSync(path.join(licenseDir, file), "utf8");
  }
  for (const [file, source] of Object.entries(LLAMA_SERVER_EXTRA_TEXTS)) {
    texts[file] = readFileSync(at(source), "utf8");
  }
  return {
    env: parseEnv(envText),
    builtVersion,
    licenseFiles,
    patches,
    fingerprint,
    texts,
    errors: llamaServerNoticeErrors(envText, builtVersion, {
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
 * Paragraphes DEP-5 du moteur de texte (`version`, `commit` épinglés dans
 * packaging/llama.env ; dans l'AppImage, il est rangé dans usr/libexec) :
 * llama-server sous MIT (Expat), le chargeur Vulkan de secours sous
 * Apache-2.0 (paragraphe License autonome : celui du moteur d'images).
 */
export function llamaServerCopyright({ version, commit, appimage }) {
  const dir = appimage
    ? "usr/libexec/chatonlm/llama-server"
    : "usr/lib/chatonlm/llama-server";
  const indent = (line, i) => (i === 0 ? line : ` ${line}`);
  return [
    `Files: ${dir}/*`,
    "Copyright: 2023-2026 The ggml authors, and others",
    "License: Expat",
    ...wrap(
      `Comment: Text engine of ChatonLM: llama-server of llama.cpp ${version} ` +
        `(https://github.com/ggml-org/llama.cpp, commit ${commit}), built by ` +
        "ChatonLM as described in licenses/llama-server/README.txt. It " +
        `includes code under the ${LLAMA_SERVER_LICENSE_SUMMARY}: see that ` +
        "README and the license files next to it.",
      "",
      75,
    ).map(indent),
    "",
    `Files: ${dir}/fallback/*`,
    "Copyright: The Khronos Group Inc., Valve Corporation, LunarG, Inc.,",
    " and others",
    "License: Apache-2.0",
    ...wrap(
      "Comment: Vulkan loader of the Khronos Group " +
        "(https://github.com/KhronosGroup/Vulkan-Loader), the same as the one " +
        "of the image engine, used only where the system has none. It " +
        "includes cJSON (MIT): see " +
        "licenses/llama-server/Vulkan-Loader-components.txt.",
      "",
      75,
    ).map(indent),
    "",
  ];
}

/**
 * licenses/llama-server/README.txt (Linux) ou licenses\llama-server\README.txt
 * (Windows, `windows`) : le moteur de texte, sa provenance, ce qui en est
 * livré, les correctifs de ChatonLM et les licences (`env` :
 * packaging/llama.env ; `licenseFiles` : textes de LLAMA_SERVER_LICENSE_DIR).
 */
export function renderLlamaServerReadme({
  env,
  licenseFiles,
  windows = false,
}) {
  const version = env.LLAMA_VERSION;
  const repo = env.LLAMA_REPO;
  const sep = windows ? "\\" : "/";
  const exe = windows ? "llama-server.exe" : "llama-server";
  const loader = windows ? "vulkan-1.dll" : "libvulkan.so.1";
  const lib = (name) => (windows ? `${name}.dll` : `lib${name}.so*`);
  const files = [
    ...new Set([...licenseFiles, ...Object.keys(LLAMA_SERVER_EXTRA_TEXTS)]),
  ].sort((a, b) => a.localeCompare(b));
  const describe = (file) =>
    LLAMA_SERVER_LICENSES[file] ??
    (file === "Apache-2.0.txt"
      ? "text of the Apache License, Version 2.0"
      : "");
  const rows = files.flatMap((file) => {
    const text = describe(file);
    return text ? [`  ${file}`, ...wrap(text, "      ", 76)] : [`  ${file}`];
  });
  const patches = Object.entries(LLAMA_SERVER_PATCHES);
  const changes =
    patches.length === 0
      ? wrap(
          "None to the source code: llama-server is built from the source " +
            "code of the pinned version as published. ChatonLM only chooses " +
            "the build options listed above.",
        )
      : patches.flatMap(([file, text]) => [
          `  ${file}`,
          ...wrap(text, "      ", 76),
        ]);
  const where = windows
    ? [
        "It is installed in the llama-server folder, next to chatonlm.exe, and",
        "runs as a separate program.",
      ]
    : [
        "It is installed in /usr/lib/chatonlm/llama-server (in the AppImage:",
        "usr/libexec/chatonlm/llama-server, linked from",
        "usr/lib/chatonlm/llama-server) and runs as a separate program.",
      ];
  const runtime = windows
    ? "\n\n" +
      wrap(
        "llama-server.exe and its DLLs use the Microsoft Visual C++ " +
          "runtime libraries (vcruntime140*.dll, msvcp140*.dll), shipped " +
          "unmodified in this folder next to them (redistributable files of " +
          "Microsoft Visual Studio, see windows-components.txt in the " +
          "licenses folder).",
      ).join("\n")
    : "";
  return `ChatonLM - Built-in text engine (llama-server)
${RULE}

${wrap(
  "ChatonLM runs language models with llama-server, the server of " +
    "llama.cpp. ChatonLM builds it from the source code of the version " +
    "below (script scripts/build-llama-server.sh of the source code of " +
    `ChatonLM, ${REPOSITORY}).`,
).join("\n")}
${where.join("\n")}

Project:      llama.cpp
Repository:   ${repo}
Version:      ${version} (build ${env.LLAMA_BUILD}, ggml ${env.LLAMA_GGML_VERSION})
Commit:       ${env.LLAMA_COMMIT}
Source code:  ${repo}/tree/${env.LLAMA_COMMIT}
License:      MIT, Copyright (c) 2023-2026 The ggml authors

${heading("What is shipped")}
${wrap(
  `${exe}, ${["llama-server-impl", "llama", "llama-common", "mtmd", "ggml", "ggml-base"].map(lib).join(", ")}`,
  "  ",
  76,
).join("\n")}
${wrap(
  "the server and its libraries, built without OpenMP, without OpenSSL " +
    "(no outgoing connection: ChatonLM downloads the models itself) and " +
    "without the web interface of the server",
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
  fallback${sep}${loader}
      the Vulkan loader of the Khronos Group, used only where the system
      has none (see below)
  VERSION, SHA256SUMS
${wrap(
  `version of the build: ${version}+chatonlm.<fingerprint>, the ` +
    "fingerprint covering the pinned version, the Vulkan loader, the " +
    "patches and the build script; checksums of the shipped files",
  "      ",
  76,
).join("\n")}${runtime}

${heading("Changes made by ChatonLM")}
${changes.join("\n")}

${heading("Licenses")}
${wrap(
  "llama.cpp, ggml and the code they include are under the " +
    `${LLAMA_SERVER_LICENSE_SUMMARY}. The files below, in this ` +
    `${windows ? "folder" : "directory"}, give each license text and the ` +
    "parts it covers.",
).join("\n")}

${rows.join("\n")}

${heading("Vulkan loader")}
${wrap(
  `fallback${sep}${loader} is the Vulkan loader of the Khronos Group ` +
    "(https://github.com/KhronosGroup/Vulkan-Loader), built by ChatonLM from " +
    "its source code, unmodified, at the tag pinned in packaging/sdcpp.env " +
    "(the same file as the one of the image engine). ChatonLM points " +
    "llama-server to it only where the system has no Vulkan loader of its " +
    "own. It is under the Apache License, Version 2.0 (Apache-2.0.txt), and " +
    "includes cJSON (MIT)" +
    (windows ? " and code of Kevlin Henney under a permissive license" : "") +
    ": see Vulkan-Loader-components.txt.",
).join("\n")}
`;
}
