// Notices livrées dans les paquets Linux, écrites avant l'empaquetage par
// build.beforeBundleCommand (src-tauri/tauri.conf.json), dans le dossier que
// bundle.linux.{deb,appimage}.files installe en /usr/share/doc/chatonlm :
//
//   .tools/package/notices/deb/        .deb
//   .tools/package/notices/appimage/   AppImage : le même contenu, plus les
//                                      textes LGPL/GPL et l'inventaire des
//                                      bibliothèques du système embarquées
//
// Contenu : copyright (format DEP-5), LICENSE, THIRD_PARTY_LICENSES, et
// licenses/ : crates Rust (cargo-about, version épinglée ci-dessous, sortie
// JSON mise en forme ici), paquets npm du bundle (dist/.vite/license.json),
// notices d'ONNX Runtime (bibliothèque livrée à part, tirées de l'archive
// officielle vérifiée par scripts/fetch-onnxruntime.sh), licences du moteur
// d'images (licenses/sd-server/ : notice, textes des parties
// compilées dans sd-server, de packaging/sdcpp/licenses, et Apache-2.0 du
// chargeur Vulkan de secours), licences du moteur de texte
// (licenses/llama-server/ : notice, textes des parties compilées dans
// llama-server, de packaging/llama/licenses, et Apache-2.0 ; voir
// scripts/package-notices-llama.mjs), attribution des modèles de dictée
// téléchargés à l'usage.
//
// L'inventaire de l'AppImage vient de scripts/appimage-libs.sh (Ubuntu, CI),
// rangé dans .tools/package/appimage-libs/ ; sans lui (construction locale),
// l'AppImage porte une notice qui le dit.
//
// Sous Windows (ou avec --windows), ce sont les notices de l'installateur
// NSIS qui sont écrites, dans .tools/package/notices/windows/ : voir
// scripts/package-notices-windows.mjs. Rien ne change pour les paquets Linux.
//
// Usage : node scripts/package-notices.mjs [--offline] [--windows]
//   --offline : cargo-about sans réseau (licences des seules sources locales).
//   --windows : notices de l'installateur Windows (défaut sous Windows).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  llamaServerCopyright,
  readLlamaServer,
  renderLlamaServerReadme,
} from "./package-notices-llama.mjs";

const root = path.resolve(import.meta.dirname, "..");

/** Version de cargo-about attendue (installée par cargo install --locked). */
export const CARGO_ABOUT_VERSION = "0.9.2";

/**
 * ONNX Runtime : build officiel de Microsoft chargé à l'exécution (ort,
 * fonctionnalité load-dynamic), version épinglée dans
 * packaging/onnxruntime.env et extraite par scripts/fetch-onnxruntime.sh dans
 * .tools/runtimes/onnxruntime avec sa LICENSE et ses ThirdPartyNotices.txt.
 */
export const ONNXRUNTIME_DIR = ".tools/runtimes/onnxruntime";

/**
 * Moteur d'images : sd-server de stable-diffusion.cpp, construit par
 * scripts/build-sd-server.sh depuis les sources épinglées dans
 * packaging/sdcpp.env, avec les correctifs de packaging/sdcpp/*.patch, dans
 * .tools/runtimes/sd-server (sd-server, fallback/, VERSION, SHA256SUMS).
 */
export const SD_SERVER_DIR = ".tools/runtimes/sd-server";

/** Textes de licence des parties de sd-server, relevés dans ses sources. */
export const SD_SERVER_LICENSE_DIR = "packaging/sdcpp/licenses";

/**
 * Textes de SD_SERVER_LICENSE_DIR, exigés : nom du fichier → ce qu'il
 * couvre. Un fichier de plus est livré aussi, sans description.
 */
export const SD_SERVER_LICENSES = {
  "stable-diffusion.cpp-LICENSE.txt": "stable-diffusion.cpp (MIT)",
  "ggml-LICENSE.txt": "ggml and its Vulkan shaders (MIT)",
  "cpp-httplib-LICENSE.txt": "cpp-httplib (MIT)",
  "nlohmann-json-LICENSE.txt": "JSON for Modern C++ (MIT)",
  "stb-LICENSE.txt":
    "stb_image, stb_image_write and stb_image_resize (MIT or public domain)",
  "miniz-LICENSE.txt": "miniz (MIT, and public domain)",
  "zip-LICENSE.txt": "zip library (public domain; MIT since 2024)",
  "darts-clone-LICENSE.txt": "Darts-clone (BSD-3-Clause)",
  "oniguruma-COPYING.txt": "Oniguruma (BSD-2-Clause)",
  "utf8proc-LICENSE.txt":
    "utf8proc (MIT) and its Unicode data (Unicode license)",
  "Vulkan-Headers-LICENSE.txt":
    "Vulkan headers compiled into the Vulkan backend (Apache-2.0 or MIT)",
  "ported-code.txt":
    "code ported from PyTorch, SentencePiece, llama.cpp, k-diffusion, " +
    "NumCpp, YaRN and SD 3.5 (BSD-3-Clause, Apache-2.0, MIT)",
  "tokenizer-data.txt": "tokenizer data of CLIP, Qwen2, Mistral, T5 and UMT5",
  "Vulkan-Loader-components.txt":
    "Vulkan loader of fallback/ (Apache-2.0) and the code it includes",
};

/** Textes de packaging/licenses livrés avec ceux de sd-server. */
export const SD_SERVER_EXTRA_TEXTS = {
  "Apache-2.0.txt": "packaging/licenses/Apache-2.0.txt",
};

/** Licences du code de sd-server, en une phrase (README, copyright). */
export const SD_SERVER_LICENSE_SUMMARY =
  "MIT, BSD-2-Clause, BSD-3-Clause and Apache-2.0 licenses, the Unicode data license, and public-domain code";

/**
 * Correctifs de ChatonLM appliqués aux sources (packaging/sdcpp/*.patch),
 * résumés dans la notice : un correctif sans résumé arrête
 * package-notices.mjs.
 */
export const SD_SERVER_PATCHES = {
  "0001-refuse-browser-requests.patch":
    "the server refuses any request that carries an Origin header (made by " +
    "a web page) or names another host than its own address (DNS " +
    "rebinding), and serves one request per connection: only ChatonLM " +
    "talks to it.",
  "0002-windows-utf8-code-page.patch":
    "Windows only: a manifest makes UTF-8 the code page of the program, so " +
    "that model paths with non-ASCII characters open.",
  "0003-no-philox-port.patch":
    "the random number generator imitating PyTorch's CUDA generator, ported " +
    "from AUTOMATIC1111's stable-diffusion-webui (AGPL-3.0), is removed; the " +
    "MT19937 generator (PyTorch's CPU generator, the same on every system) " +
    "is used instead.",
  "0004-no-gemma-tokenizer-data.patch":
    "the tokenizer data of Gemma 3 (Gemma Terms of Use, not a permissive " +
    "license) is left out; a model that needs it is reported as a loading " +
    "failure, not a crash.",
  "0005-own-prompt-attention-parser.patch":
    "the parser of the prompt attention syntax ((text), (text:1.2), [text], " +
    "BREAK), a port of AUTOMATIC1111's prompt_parser.py (AGPL-3.0), is " +
    "replaced by an implementation written for ChatonLM.",
};

/** Clés de packaging/sdcpp.env citées par les notices. */
const SD_SERVER_ENV_KEYS = [
  "SDCPP_VERSION",
  "SDCPP_REPO",
  "SDCPP_COMMIT",
  "SDCPP_GGML_COMMIT",
];

/**
 * Empreinte de la construction, comme scripts/build-sd-server.sh : 12
 * premiers caractères du SHA-256 de packaging/sdcpp.env, des correctifs
 * (dans l'ordre de leurs noms) et du script, mis bout à bout.
 */
export function sdServerFingerprint({ env, patches, script }) {
  const hash = createHash("sha256");
  for (const part of [env, ...patches, script]) hash.update(part);
  return hash.digest("hex").slice(0, 12);
}

/**
 * Erreurs si le sd-server construit (`builtVersion` : son fichier VERSION,
 * ou null si absent) ne vient pas des sources épinglées (texte de
 * packaging/sdcpp.env) et, si `fingerprint` est donnée, des correctifs et du
 * script actuels ; s'il manque un texte de licence (`licenseFiles` : noms des
 * fichiers de SD_SERVER_LICENSE_DIR) ; si un correctif (`patches` : noms des
 * fichiers .patch) n'est pas résumé dans SD_SERVER_PATCHES, ou l'inverse.
 */
export function sdServerNoticeErrors(
  envText,
  builtVersion,
  { licenseFiles = [], patches = [], fingerprint = null } = {},
) {
  const env = parseEnv(envText);
  const missing = SD_SERVER_ENV_KEYS.filter((key) => !env[key]);
  if (missing.length > 0) {
    return missing.map((key) => `${key} absent de packaging/sdcpp.env.`);
  }
  const pinned = env.SDCPP_VERSION;
  const rebuild = "lancer scripts/build-sd-server.sh.";
  const errors = [];
  const escaped = pinned.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (builtVersion === null) {
    errors.push(`sd-server absent de ${SD_SERVER_DIR} : ${rebuild}`);
  } else if (
    !new RegExp(`^${escaped}\\+chatonlm\\.[0-9a-f]{12}$`).test(builtVersion)
  ) {
    errors.push(
      `sd-server ${builtVersion} dans ${SD_SERVER_DIR} au lieu de ` +
        `${pinned}+chatonlm.<empreinte> : ${rebuild}`,
    );
  } else if (fingerprint && !builtVersion.endsWith(`.${fingerprint}`)) {
    errors.push(
      `sd-server ${builtVersion} dans ${SD_SERVER_DIR} construit avec ` +
        `d'autres correctifs ou un autre script (empreinte attendue ` +
        `${fingerprint}) : ${rebuild}`,
    );
  }
  for (const file of Object.keys(SD_SERVER_LICENSES)) {
    if (!licenseFiles.includes(file)) {
      errors.push(
        `Licence du moteur d'images absente : ${SD_SERVER_LICENSE_DIR}/${file}`,
      );
    }
  }
  for (const patch of patches) {
    if (!SD_SERVER_PATCHES[patch]) {
      errors.push(
        `Correctif packaging/sdcpp/${patch} sans résumé dans ` +
          "SD_SERVER_PATCHES (scripts/package-notices.mjs).",
      );
    }
  }
  for (const patch of Object.keys(SD_SERVER_PATCHES)) {
    if (!patches.includes(patch)) {
      errors.push(`Correctif résumé mais absent : packaging/sdcpp/${patch}`);
    }
  }
  return errors;
}

/** Paquets npm qui n'entrent que par le CSS (comme check-npm-licenses.mjs). */
export const CSS_PACKAGES = ["tailwindcss", "@tailwindcss/typography", "katex"];

/** Textes de licence de packaging/licenses/ propres à l'AppImage. */
export const APPIMAGE_LICENSE_TEXTS = [
  "LGPL-2.1.txt",
  "LGPL-3.0.txt",
  "GPL-2.0.txt",
  "GPL-3.0.txt",
];

const REPOSITORY = "https://github.com/Elwynn18/chatonlm";
const RULE = "=".repeat(78);

/**
 * Erreurs si la copie d'ONNX Runtime (`VERSION` de .tools/runtimes/onnxruntime,
 * ou null si absente) n'est pas la version épinglée (texte de
 * packaging/onnxruntime.env).
 */
export function onnxRuntimeErrors(envText, fetchedVersion) {
  const pinned = parseEnv(envText).ORT_VERSION;
  if (!pinned) return ["ORT_VERSION absent de packaging/onnxruntime.env."];
  if (fetchedVersion === pinned) return [];
  return [
    `ONNX Runtime ${fetchedVersion ?? "absent"} dans ${ONNXRUNTIME_DIR} au lieu ` +
      `de ${pinned} : lancer scripts/fetch-onnxruntime.sh.`,
  ];
}

/** Licence MIT du dépôt, indentée pour un paragraphe DEP-5. */
function dep5Text(text) {
  return text
    .trim()
    .split("\n")
    .map((line) => (line.trim() === "" ? " ." : ` ${line}`))
    .join("\n");
}

/**
 * Fichier copyright du paquet (format DEP-5 de Debian). `license` est le
 * texte de LICENSE ; `appimage` ajoute le renvoi aux bibliothèques du système
 * embarquées.
 */
export function debCopyright({
  license,
  appimage = false,
  sdServer = null,
  llamaServer = null,
}) {
  const libraries = appimage
    ? [
        " The AppImage also bundles system libraries, some of them under the GNU",
        " LGPL: see appimage-libraries/README.txt.",
      ]
    : [];
  return [
    "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/",
    "Upstream-Name: ChatonLM",
    `Upstream-Contact: ${REPOSITORY}/issues`,
    `Source: ${REPOSITORY}`,
    "Comment: ChatonLM is not affiliated with, endorsed or sponsored by",
    ' Ollama. "Ollama" is a trademark of its owner and is used only to',
    " describe compatibility.",
    "",
    "Files: *",
    "Copyright: 2026 Elwynn18 and ChatonLM contributors",
    "License: Expat",
    "Comment: Portions Copyright (c) Ollama (https://github.com/ollama/ollama),",
    " also under the MIT (Expat) License: see THIRD_PARTY_LICENSES, part 1.",
    " .",
    " ChatonLM includes third-party code (Rust crates, npm packages, whisper.cpp,",
    " ONNX Runtime) and a model built into its executable (Silero VAD, MIT).",
    " The packages also hold programs that ChatonLM runs separately:",
    " the text engine (llama-server of llama.cpp) and the",
    " image engine (sd-server of stable-diffusion.cpp). Their notices are in",
    " THIRD_PARTY_LICENSES and in the licenses/ directory next to this file.",
    ...libraries,
    "",
    ...(llamaServer ? llamaServerCopyright({ ...llamaServer, appimage }) : []),
    ...(sdServer ? sdServerCopyright({ ...sdServer, appimage }) : []),
    ...(sdServer || llamaServer ? apacheLicense({ appimage }) : []),
    "License: Expat",
    dep5Text(license),
    "",
  ].join("\n");
}

/**
 * Paragraphes DEP-5 du moteur d'images (`version` et `commit` épinglés dans
 * packaging/sdcpp.env ; dans l'AppImage, il est rangé dans usr/libexec) :
 * sd-server sous MIT (Expat), le chargeur Vulkan de secours sous Apache-2.0.
 */
function sdServerCopyright({ version, commit, appimage }) {
  const dir = appimage
    ? "usr/libexec/chatonlm/sd-server"
    : "usr/lib/chatonlm/sd-server";
  return [
    `Files: ${dir}/*`,
    "Copyright: 2023 leejet, the ggml authors, and others",
    "License: Expat",
    ...wrap(
      `Comment: Image engine of ChatonLM: sd-server of stable-diffusion.cpp ` +
        `${version} (https://github.com/leejet/stable-diffusion.cpp, commit ` +
        `${commit}), built by ChatonLM with the changes described in ` +
        "licenses/sd-server/README.txt. It includes code under the " +
        `${SD_SERVER_LICENSE_SUMMARY}, and tokenizer data: see that README ` +
        "and the license files next to it.",
      "",
      75,
    ).map((line, i) => (i === 0 ? line : ` ${line}`)),
    "",
    `Files: ${dir}/fallback/*`,
    "Copyright: The Khronos Group Inc., Valve Corporation, LunarG, Inc.,",
    " and others",
    "License: Apache-2.0",
    ...wrap(
      "Comment: Vulkan loader of the Khronos Group " +
        "(https://github.com/KhronosGroup/Vulkan-Loader), built from its " +
        "source code at the tag pinned by ChatonLM, used only where the system has " +
        "none. It includes cJSON (MIT): see " +
        "licenses/sd-server/Vulkan-Loader-components.txt.",
      "",
      75,
    ).map((line, i) => (i === 0 ? line : ` ${line}`)),
    "",
  ];
}

/**
 * Paragraphe License autonome de l'Apache-2.0 (chargeur Vulkan) : avis
 * de la licence, et renvoi au texte complet (dans le .deb, aussi à
 * /usr/share/common-licenses).
 */
function apacheLicense({ appimage }) {
  return [
    "License: Apache-2.0",
    ' Licensed under the Apache License, Version 2.0 (the "License"); you may',
    " not use this file except in compliance with the License. You may obtain",
    " a copy of the License at",
    " .",
    " http://www.apache.org/licenses/LICENSE-2.0",
    " .",
    " Unless required by applicable law or agreed to in writing, software",
    ' distributed under the License is distributed on an "AS IS" BASIS,',
    " WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.",
    " See the License for the specific language governing permissions and",
    " limitations under the License.",
    " .",
    ...(appimage
      ? []
      : [
          " On Debian systems, the complete text of the Apache License, Version",
          " 2.0 can be found in /usr/share/common-licenses/Apache-2.0.",
        ]),
    " The text of the license is in licenses/sd-server/Apache-2.0.txt.",
    "",
  ];
}

/**
 * Texte en lignes de 76 caractères au plus (mots entiers), chacune précédée
 * de `prefix` (une espace pour la suite d'un champ DEP-5).
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

/** Section d'un texte de notice : titre souligné. */
function heading(title) {
  return `${title}\n${"-".repeat(title.length)}`;
}

/**
 * licenses/rust-crates.txt, depuis la sortie JSON de cargo-about :
 * { overview: [{id, count}], licenses: [{name, id, text, used_by: [{crate}]}],
 *   crates: [{package, license}] }. `target` : cible de cargo-about (celle
 *   de Linux par défaut ; celle de Windows pour l'installateur NSIS).
 */
export function renderRustCrates(
  about,
  { cargoAboutVersion, target = "x86_64-unknown-linux-gnu" },
) {
  const crates = [...about.crates]
    .map((c) => c.package)
    .filter((pkg) => pkg.name !== "chatonlm")
    .sort(
      (a, b) =>
        a.name.localeCompare(b.name) || a.version.localeCompare(b.version),
    );
  const mpl = crates.filter((pkg) =>
    about.crates.some(
      (c) => c.package.id === pkg.id && /\bMPL-2\.0\b/.test(c.license ?? ""),
    ),
  );
  const lines = [
    "ChatonLM - Rust crates compiled into ChatonLM",
    RULE,
    "",
    `Generated by cargo-about ${cargoAboutVersion} for the chatonlm package`,
    `(target ${target}, without development dependencies).`,
    `${crates.length} crates. Where a crate offers a choice of licenses, the`,
    "first license of this list is used: MIT, Apache-2.0, then the others.",
    "",
    heading("Licenses"),
    ...about.overview.map((o) => `  ${o.id}: ${o.count}`),
    "",
  ];
  if (mpl.length > 0) {
    lines.push(
      heading("Crates under the Mozilla Public License 2.0"),
      "These crates are used unmodified. Their source code is available from",
      "their repository and from crates.io:",
      "",
    );
    for (const pkg of mpl) {
      lines.push(
        `  ${pkg.name} ${pkg.version}`,
        `    ${pkg.repository ?? "(no repository given)"}`,
        `    https://crates.io/api/v1/crates/${pkg.name}/${pkg.version}/download`,
      );
    }
    lines.push("");
  }
  lines.push(heading("Crates"));
  for (const pkg of crates) {
    const license = about.crates.find((c) => c.package.id === pkg.id)?.license;
    lines.push(
      `  ${pkg.name} ${pkg.version} (${license ?? pkg.license ?? "?"})` +
        (pkg.repository ? ` ${pkg.repository}` : ""),
    );
  }
  lines.push("");
  for (const license of about.licenses) {
    const users = license.used_by
      .map((u) => `${u.crate.name} ${u.crate.version}`)
      .sort((a, b) => a.localeCompare(b));
    lines.push(
      RULE,
      `${license.name} (${license.id})`,
      `Used by: ${users.join(", ")}`,
      RULE,
      "",
      license.text.trimEnd(),
      "",
    );
  }
  return `${lines.join("\n")}\n`;
}

/**
 * licenses/npm-packages.txt : paquets de dist/.vite/license.json ({name,
 * version, identifier, text}) et paquets entrés par le CSS ; texte complet
 * de l'Apache 2.0 à la fin si un paquet l'utilise (streamdown n'en porte que
 * l'en-tête).
 */
export function renderNpmPackages(bundled, { css = [], apacheText = "" } = {}) {
  const packages = [...bundled];
  for (const pkg of css) {
    if (!packages.some((p) => p.name === pkg.name)) packages.push(pkg);
  }
  packages.sort((a, b) => a.name.localeCompare(b.name));
  const lines = [
    "ChatonLM - npm packages compiled into ChatonLM",
    RULE,
    "",
    "Packages of the user interface bundle (list written by Vite when",
    `building), and packages that enter it through the CSS. ${packages.length} packages.`,
    "",
  ];
  for (const pkg of packages) {
    lines.push(
      RULE,
      `${pkg.name} ${pkg.version} (${pkg.identifier || "no license field"})`,
      RULE,
      "",
      pkg.text?.trim()
        ? pkg.text.trimEnd()
        : "The package carries no license file: see THIRD_PARTY_LICENSES, part 3.",
      "",
    );
  }
  if (
    apacheText &&
    packages.some((p) => /Apache-2\.0/.test(p.identifier ?? ""))
  ) {
    lines.push(
      RULE,
      "Apache License 2.0 (full text)",
      RULE,
      "",
      apacheText.trimEnd(),
      "",
    );
  }
  return `${lines.join("\n")}\n`;
}

/** licenses/speech-models.txt : modèles de dictée téléchargés à l'usage. */
export function renderSpeechModels() {
  return `ChatonLM - Speech recognition models
${RULE}

ChatonLM does not ship speech recognition models. A model chosen in Settings,
Dictation, is downloaded from Hugging Face at a pinned revision and used
unmodified. Their creators and licenses:

${heading("Parakeet TDT 0.6B v3")}
Creator:  NVIDIA Corporation
Title:    parakeet-tdt-0.6b-v3
Source:   https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
License:  Creative Commons Attribution 4.0 International (CC BY 4.0),
          https://creativecommons.org/licenses/by/4.0/legalcode
Changes:  converted to the ONNX format, with 8-bit integer weights, by
          Ilya Stupakov (istupakov) for onnx-asr, under the same license:
          https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx
          (revision 8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce). ChatonLM
          downloads these files without changing them.
The model is offered as-is and as-available, without warranties of any kind
(CC BY 4.0, section 5).

${heading("Whisper")}
Creator:  OpenAI, https://github.com/openai/whisper
Models:   tiny, base, small and large-v3-turbo (5-bit quantized)
License:  MIT, Copyright (c) 2022 OpenAI,
          https://github.com/openai/whisper/blob/main/LICENSE
Changes:  converted to the ggml format by the whisper.cpp project:
          https://huggingface.co/ggerganov/whisper.cpp
          (revision 5359861c739e955e79d9a303bcbc70fb988958b1).
`;
}

/** licenses/voice-models.txt : voix de la lecture à voix haute, téléchargées à l'usage. */
export function renderVoiceModels() {
  return `ChatonLM - Text-to-speech models
${RULE}

ChatonLM does not ship text-to-speech models. A voice, used by Read aloud, is
downloaded on demand at a pinned version: the model files converted by
ChatonLM from a GitHub release of ChatonLM, the tokenizer and the voice from
Hugging Face. Their creators and licenses:

${heading("Pocket TTS")}
Creator:  Kyutai, https://kyutai.org
Title:    Pocket TTS (pocket-tts 3.3.0), 6-layer models for French, English
          and Spanish
Source:   https://huggingface.co/kyutai/pocket-tts-without-voice-cloning
          (model weights at the revisions French
          8843db76457a91db32077edf8dfcd1c0e3e755fd, English
          e7205b6ee50e654a5ea19f0e9df2b0813b05e921, Spanish
          4e1e0a3e611c51c0b4ed8174fc10f32a54644303; English tokenizer at
          00eac05ed3d16bdc3f6b5d598874019c34a89214)
License:  Creative Commons Attribution 4.0 International (CC BY 4.0),
          https://creativecommons.org/licenses/by/4.0/legalcode
Changes:  exported to the ONNX format by ChatonLM (text conditioner, FlowLM
          step with its flow head, Mimi decoder), with the transformer
          weights of the FlowLM step quantized to 8-bit integers (per
          channel); published as the files of the GitHub release
          tts-models-v1 of https://github.com/Elwynn18/chatonlm. The export
          scripts are in scripts/tts/ of the ChatonLM source code. The
          tokenizers are downloaded from the source above without changes.
The model is offered as-is and as-available, without warranties of any kind
(CC BY 4.0, section 5).

${heading("Voices")}
Each voice is a model state computed by Kyutai from a recording, downloaded
without changes from the source above (revision
4e1e0a3e611c51c0b4ed8174fc10f32a54644303). Recordings, listed at
https://huggingface.co/kyutai/tts-voices:

  estelle (French)   recorded by Kyutai for Unmute, CC0 1.0
  alba (English)     Alba MacKenna, CC BY 4.0
                     (https://creativecommons.org/licenses/by/4.0/legalcode)
  lola (Spanish)     Mozilla Common Voice (Spanish), CC0 1.0
`;
}

/**
 * licenses/sd-server/README.txt (Linux) ou licenses\sd-server\README.txt
 * (Windows, `windows`) : le moteur d'images, sa provenance, ce qui en est
 * livré, les correctifs de ChatonLM et les licences (`env` :
 * packaging/sdcpp.env ; `licenseFiles` : textes de SD_SERVER_LICENSE_DIR).
 */
export function renderSdServerReadme({ env, licenseFiles, windows = false }) {
  const version = env.SDCPP_VERSION;
  const repo = env.SDCPP_REPO;
  const sep = windows ? "\\" : "/";
  const exe = windows ? "sd-server.exe" : "sd-server";
  const loader = windows ? "vulkan-1.dll" : "libvulkan.so.1";
  const files = [
    ...new Set([...licenseFiles, ...Object.keys(SD_SERVER_EXTRA_TEXTS)]),
  ].sort((a, b) => a.localeCompare(b));
  const describe = (file) =>
    SD_SERVER_LICENSES[file] ??
    (file === "Apache-2.0.txt"
      ? "text of the Apache License, Version 2.0"
      : "");
  const rows = files.flatMap((file) => {
    const text = describe(file);
    return text ? [`  ${file}`, ...wrap(text, "      ", 76)] : [`  ${file}`];
  });
  const patches = Object.entries(SD_SERVER_PATCHES).flatMap(([file, text]) => [
    `  ${file}`,
    ...wrap(text, "      ", 76),
  ]);
  const where = windows
    ? [
        "It is installed in the sd-server folder, next to chatonlm.exe, and runs as",
        "a separate program.",
      ]
    : [
        "It is installed in /usr/lib/chatonlm/sd-server (in the AppImage:",
        "usr/libexec/chatonlm/sd-server, linked from usr/lib/chatonlm/sd-server)",
        "and runs as a separate program.",
      ];
  const runtime = windows
    ? "\n\n" +
      wrap(
        "sd-server.exe is linked with the static Microsoft Visual C++ " +
          "runtime libraries (redistributable code of Microsoft Visual " +
          "Studio, see windows-components.txt in the licenses folder): it " +
          "needs no DLL of its own.",
      ).join("\n")
    : "";
  return `ChatonLM - Built-in image engine (sd-server)
${RULE}

${wrap(
  "ChatonLM generates images with sd-server, the server of " +
    "stable-diffusion.cpp. ChatonLM builds it from the source code of the " +
    "version below, with the changes listed further down (patches in " +
    "packaging/sdcpp/ and script scripts/build-sd-server.sh of the source " +
    `code of ChatonLM, ${REPOSITORY}).`,
).join("\n")}
${where.join("\n")}

Project:      stable-diffusion.cpp
Repository:   ${repo}
Version:      ${version}
Commit:       ${env.SDCPP_COMMIT}
Source code:  ${repo}/tree/${env.SDCPP_COMMIT}
ggml:         https://github.com/leejet/ggml/tree/${env.SDCPP_GGML_COMMIT}
              (submodule of stable-diffusion.cpp at that commit)
License:      MIT, Copyright (c) 2023 leejet

${heading("What is shipped")}
  ${exe}
      the server, with the Vulkan backend of ggml (graphics cards through
      Vulkan; it falls back on the processor without one), built without
      OpenMP, without WebP and WebM support and without the web interface
      of the server
  fallback${sep}${loader}
      the Vulkan loader of the Khronos Group, used only where the system
      has none (see below)
  VERSION
${wrap(
  `version of the build: ${version}+chatonlm.<fingerprint>, the ` +
    "fingerprint covering the pinned version, the patches and the build " +
    "script",
  "      ",
  76,
).join("\n")}${runtime}

${heading("Changes made by ChatonLM")}
${patches.join("\n")}

${heading("Licenses")}
${wrap(
  "stable-diffusion.cpp, ggml and the code they include are under the " +
    `${SD_SERVER_LICENSE_SUMMARY}. The files below, in this ` +
    `${windows ? "folder" : "directory"}, give each license text and the ` +
    "parts it covers. stable-diffusion.cpp also embeds tokenizer data " +
    "published with the models it supports: tokenizer-data.txt names " +
    "their sources and terms.",
).join("\n")}

${rows.join("\n")}

${heading("Vulkan loader")}
${wrap(
  `fallback${sep}${loader} is the Vulkan loader of the Khronos Group ` +
    "(https://github.com/KhronosGroup/Vulkan-Loader), built by ChatonLM from " +
    "its source code, unmodified, at the tag pinned in packaging/sdcpp.env. ChatonLM " +
    "points sd-server to it only where the system has no Vulkan loader of " +
    "its own. It is under the Apache License, Version 2.0 (Apache-2.0.txt), " +
    "and includes cJSON (MIT)" +
    (windows ? " and code of Kevlin Henney under a permissive license" : "") +
    ": see Vulkan-Loader-components.txt.",
).join("\n")}
`;
}

/** Valeurs KEY=valeur d'un fichier comme packaging/appimage-runtime.env. */
export function parseEnv(text) {
  return Object.fromEntries(
    text
      .split("\n")
      .map((line) => /^([A-Z0-9_]+)=(.*)$/.exec(line.trim()))
      .filter(Boolean)
      .map((m) => [m[1], m[2]]),
  );
}

/**
 * appimage-libraries/README.txt : obligations de la LGPL pour les
 * bibliothèques du système embarquées dans l'AppImage et pour son runtime
 * (libfuse en statique). `inventoried` : la liste a été produite (CI,
 * Ubuntu) ; sinon, la notice le dit. `runtime` : packaging/appimage-runtime.env.
 */
export function renderAppImageReadme({ version, inventoried, runtime }) {
  const sources = `chatonlm_${version}_amd64.AppImage-sources.tar`;
  const list = inventoried
    ? [
        "libraries.txt lists each library file of usr/lib with the Ubuntu",
        "package it comes from, its source package and version, and its",
        "license. The copyright file of each package is in copyright/, and the",
        "license texts it refers to are in common-licenses/ and ../licenses/.",
      ]
    : [
        "This AppImage was built for local testing: the list of its libraries",
        "was not generated. Release AppImages carry it in libraries.txt.",
      ];
  return `ChatonLM - System libraries bundled in the AppImage
${RULE}

To run on many Linux distributions, the AppImage carries a copy of system
libraries (GTK, WebKitGTK, GLib and their dependencies) in its usr/lib
directory, taken from the distribution it was built on (Ubuntu 22.04 for
release builds). ChatonLM itself is under the MIT License; these libraries
keep their own licenses. Several of them are under the GNU Lesser General
Public License (LGPL), version 2.1 or 3: their texts are in
../licenses/LGPL-2.1.txt and ../licenses/LGPL-3.0.txt (with
../licenses/GPL-3.0.txt, which LGPL 3 completes).

${list.join("\n")}

${heading("Source code")}
The complete source code of these libraries, in the exact versions bundled,
is published at the same place as the AppImage, in the file
${sources}
of the same release (${REPOSITORY}/releases). It can also be
fetched from Ubuntu with "apt-get source <source package>=<version>".

${heading("Replacing a library")}
The libraries are dynamically linked, so a modified version can be used
instead of the bundled one:

  ./chatonlm_${version}_amd64.AppImage --appimage-extract
  cp /path/to/your/libexample.so.1 squashfs-root/usr/lib/libexample.so.1
  ./squashfs-root/AppRun

${heading("AppImage runtime")}
The executable header of the AppImage file, which mounts or extracts it, is
the type2-runtime of the AppImage project (${runtime.TYPE2_RUNTIME_TAG}, commit
${runtime.TYPE2_RUNTIME_COMMIT}, MIT License,
https://github.com/AppImage/type2-runtime). It is statically linked with
libfuse ${runtime.LIBFUSE_VERSION} (LGPL-2.1, https://github.com/libfuse/libfuse),
squashfuse ${runtime.SQUASHFUSE_VERSION} (BSD-2-Clause), and zstd, zlib, musl and mimalloc
of Alpine Linux ${runtime.ALPINE_RELEASE} (permissive licenses). Their source code,
with the runtime's own, is in the appimage-runtime/ directory of the
source archive above: the runtime can be rebuilt with a modified libfuse,
then joined to the extracted files with appimagetool --runtime-file.
The AppRun launcher and its apprun-hooks come from AppImageKit and
linuxdeploy (MIT License).

The source code of ChatonLM is available at ${REPOSITORY}.
`;
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

function read(file) {
  return readFileSync(path.join(root, file), "utf8");
}

/** Licence d'un paquet npm de node_modules, au format de license.json. */
export function nodePackage(name) {
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

function write(dir, file, content) {
  mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
  writeFileSync(path.join(dir, file), content);
}

function fail(message) {
  console.error(`package-notices : ${message}`);
  process.exit(1);
}

/**
 * Moteur d'images vu depuis la racine `dir` du dépôt : packaging/sdcpp.env,
 * VERSION du dossier construit (ou null), textes de licence (nom → contenu,
 * ceux de SD_SERVER_LICENSE_DIR puis SD_SERVER_EXTRA_TEXTS), correctifs,
 * empreinte attendue et erreurs (sdServerNoticeErrors).
 */
export function readSdServer(dir) {
  const at = (rel) => path.join(dir, rel);
  const envText = existsSync(at("packaging/sdcpp.env"))
    ? readFileSync(at("packaging/sdcpp.env"), "utf8")
    : "";
  const versionFile = at(`${SD_SERVER_DIR}/VERSION`);
  const builtVersion = existsSync(versionFile)
    ? readFileSync(versionFile, "utf8").trim()
    : null;
  const licenseDir = at(SD_SERVER_LICENSE_DIR);
  const licenseFiles = existsSync(licenseDir)
    ? readdirSync(licenseDir).sort()
    : [];
  const patchDir = at("packaging/sdcpp");
  const patches = existsSync(patchDir)
    ? readdirSync(patchDir)
        .filter((f) => f.endsWith(".patch"))
        .sort()
    : [];
  const script = at("scripts/build-sd-server.sh");
  const fingerprint =
    envText && existsSync(script)
      ? sdServerFingerprint({
          env: readFileSync(at("packaging/sdcpp.env")),
          patches: patches.map((f) => readFileSync(path.join(patchDir, f))),
          script: readFileSync(script),
        })
      : null;
  const texts = {};
  for (const file of licenseFiles) {
    texts[file] = readFileSync(path.join(licenseDir, file), "utf8");
  }
  for (const [file, source] of Object.entries(SD_SERVER_EXTRA_TEXTS)) {
    texts[file] = readFileSync(at(source), "utf8");
  }
  return {
    env: parseEnv(envText),
    builtVersion,
    licenseFiles,
    patches,
    fingerprint,
    texts,
    errors: sdServerNoticeErrors(envText, builtVersion, {
      licenseFiles,
      patches,
      fingerprint,
    }),
  };
}

function main() {
  const offline = process.argv.includes("--offline");
  const out = path.join(root, ".tools/package/notices");
  const inventory = path.join(root, ".tools/package/appimage-libs");

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

  const version = JSON.parse(read("src-tauri/tauri.conf.json")).version;
  const about = JSON.parse(
    execFileSync(
      "cargo",
      [
        "about",
        "generate",
        "--format",
        "json",
        "--config",
        "packaging/about.toml",
        "--manifest-path",
        "src-tauri/Cargo.toml",
        "--locked",
        "--fail",
        ...(offline ? ["--offline"] : []),
      ],
      { cwd: root, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
    ),
  );

  const license = read("LICENSE");
  const common = {
    LICENSE: license,
    THIRD_PARTY_LICENSES: read("THIRD_PARTY_LICENSES"),
    "licenses/rust-crates.txt": renderRustCrates(about, {
      cargoAboutVersion: CARGO_ABOUT_VERSION,
    }),
    "licenses/npm-packages.txt": renderNpmPackages(
      JSON.parse(read("dist/.vite/license.json")),
      {
        css: CSS_PACKAGES.map(nodePackage),
        apacheText: read("packaging/licenses/Apache-2.0.txt"),
      },
    ),
    "licenses/speech-models.txt": renderSpeechModels(),
    "licenses/voice-models.txt": renderVoiceModels(),
    "licenses/onnxruntime-LICENSE.txt": read(`${ONNXRUNTIME_DIR}/LICENSE`),
    "licenses/onnxruntime-ThirdPartyNotices.txt": read(
      `${ONNXRUNTIME_DIR}/ThirdPartyNotices.txt`,
    ),
  };
  common["licenses/sd-server/README.txt"] = renderSdServerReadme({
    env: sdServer.env,
    licenseFiles: sdServer.licenseFiles,
  });
  for (const [file, text] of Object.entries(sdServer.texts)) {
    common[`licenses/sd-server/${file}`] = text;
  }
  common["licenses/llama-server/README.txt"] = renderLlamaServerReadme({
    env: llamaServer.env,
    licenseFiles: llamaServer.licenseFiles,
  });
  for (const [file, text] of Object.entries(llamaServer.texts)) {
    common[`licenses/llama-server/${file}`] = text;
  }
  const sd = {
    version: sdServer.env.SDCPP_VERSION,
    commit: sdServer.env.SDCPP_COMMIT,
  };
  const llama = {
    version: llamaServer.env.LLAMA_VERSION,
    commit: llamaServer.env.LLAMA_COMMIT,
  };

  rmSync(out, { recursive: true, force: true });
  const deb = path.join(out, "deb");
  const appimage = path.join(out, "appimage");
  for (const [file, content] of Object.entries(common)) {
    write(deb, file, content);
    write(appimage, file, content);
  }
  write(
    deb,
    "copyright",
    debCopyright({ license, sdServer: sd, llamaServer: llama }),
  );
  write(
    appimage,
    "copyright",
    debCopyright({
      license,
      appimage: true,
      sdServer: sd,
      llamaServer: llama,
    }),
  );
  for (const file of APPIMAGE_LICENSE_TEXTS) {
    write(appimage, `licenses/${file}`, read(`packaging/licenses/${file}`));
  }
  const inventoried = existsSync(path.join(inventory, "libraries.txt"));
  if (inventoried) {
    cpSync(inventory, path.join(appimage, "appimage-libraries"), {
      recursive: true,
    });
  }
  write(
    appimage,
    "appimage-libraries/README.txt",
    renderAppImageReadme({
      version,
      inventoried,
      runtime: parseEnv(read("packaging/appimage-runtime.env")),
    }),
  );
  console.log(
    `Notices écrites dans ${path.relative(root, out)} (${about.crates.length} crates, ` +
      `inventaire de l'AppImage : ${inventoried ? "oui" : "non"}).`,
  );
}

// Comparaison sans la casse : sous Windows, la lettre du lecteur peut
// différer entre l'adresse du module et l'argument de la commande.
if (
  process.argv[1] &&
  import.meta.url.toLowerCase() ===
    pathToFileURL(process.argv[1]).href.toLowerCase()
) {
  if (process.platform === "win32" || process.argv.includes("--windows")) {
    // Sans `await` au niveau du module : package-notices-windows.mjs importe
    // ce fichier, qui ne finirait jamais de s'évaluer (import circulaire).
    import("./package-notices-windows.mjs")
      .then((windows) => windows.main())
      .catch((error) => {
        console.error(error);
        process.exit(1);
      });
  } else {
    main();
  }
}
