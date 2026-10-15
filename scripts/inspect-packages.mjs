// Inspection des paquets Linux construits par scripts/package.sh : champs de
// contrôle et fichiers du .deb, AppImage extraite (--appimage-extract, dans
// .tools/package/inspect), runtime épinglé, fichiers .desktop, icônes,
// métadonnées AppStream, notices, bibliothèques demandées par le binaire et
// bibliothèques embarquées dans l'AppImage (liste, tailles), Ollama intégré
// (fichiers identiques à ceux de l'archive officielle extraite par
// scripts/fetch-ollama.sh : ni strip ni patchelf de linuxdeploy, sans
// moteur CUDA, ROCm ni MLX, glibc demandée), moteur d'images (sd-server
// construit par scripts/build-sd-server.sh : mêmes contrôles, plus ses
// bibliothèques demandées, sans libgomp). Rapport lisible sur la sortie
// standard ; code 1 si un contrôle échoue.
//
// Usage : node scripts/inspect-packages.mjs [options] <paquet.deb> <paquet.AppImage>
//   --max-glibc <x.y>       échec si le binaire, l'Ollama intégré ou le
//                           moteur d'images demande une glibc plus récente
//   --require-inventory     échec si l'AppImage n'a pas l'inventaire LGPL
//                           (construction de publication, CI)
//   --ollama-sums <fichier> empreintes attendues de l'Ollama intégré
//                           (défaut : .tools/runtimes/ollama/SHA256SUMS)
//   --sd-sums <fichier>     empreintes attendues du moteur d'images (défaut :
//                           .tools/runtimes/sd-server/SHA256SUMS ; VERSION
//                           attendue : celle du même dossier)
//   --report <fichier>      copie du rapport
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  readSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { parseEnv } from "./package-notices.mjs";

const root = path.resolve(import.meta.dirname, "..");
const ID = "com.chatonlm.chatonlm";
const ENV_C = { ...process.env, LC_ALL: "C" };

/** Fichiers attendus dans les deux paquets (chemins sans « / » initial). */
export const COMMON_FILES = [
  "usr/bin/chatonlm",
  "usr/share/applications/chatonlm.desktop",
  "usr/share/icons/hicolor/32x32/apps/chatonlm.png",
  "usr/share/icons/hicolor/64x64/apps/chatonlm.png",
  "usr/share/icons/hicolor/128x128/apps/chatonlm.png",
  "usr/share/icons/hicolor/256x256/apps/chatonlm.png",
  "usr/share/icons/hicolor/512x512/apps/chatonlm.png",
  "usr/share/icons/hicolor/scalable/apps/chatonlm.svg",
  "usr/share/doc/chatonlm/copyright",
  "usr/share/doc/chatonlm/LICENSE",
  "usr/share/doc/chatonlm/THIRD_PARTY_LICENSES",
  "usr/share/doc/chatonlm/licenses/rust-crates.txt",
  "usr/share/doc/chatonlm/licenses/npm-packages.txt",
  "usr/share/doc/chatonlm/licenses/speech-models.txt",
  "usr/share/doc/chatonlm/licenses/voice-models.txt",
  "usr/share/doc/chatonlm/licenses/onnxruntime-LICENSE.txt",
  "usr/share/doc/chatonlm/licenses/onnxruntime-ThirdPartyNotices.txt",
  // ONNX Runtime officiel, chargé à l'exécution depuis le dossier des
  // ressources de Tauri (/usr/lib/chatonlm).
  "usr/lib/chatonlm/libonnxruntime.so.1",
  // Licences de l'Ollama intégré (ses fichiers : OLLAMA_FILES).
  "usr/share/doc/chatonlm/licenses/ollama/README.txt",
  "usr/share/doc/chatonlm/licenses/ollama/GO_LICENSE",
  "usr/share/doc/chatonlm/licenses/ollama/LLAMA_CPP_LICENSE",
  "usr/share/doc/chatonlm/licenses/ollama/XGRAMMAR_NOTICE",
  "usr/share/doc/chatonlm/licenses/ollama/LIBGOMP_GPL-3.0.txt",
  "usr/share/doc/chatonlm/licenses/ollama/LIBGOMP_GCC-RUNTIME-LIBRARY-EXCEPTION-3.1.txt",
  // Licences du moteur d'images (ses fichiers : SD_FILES).
  "usr/share/doc/chatonlm/licenses/sd-server/README.txt",
];

/**
 * Dossier de l'Ollama intégré : resource_dir()/ollama dans le .deb ; dans
 * l'AppImage, hors de usr/lib (linuxdeploy traite chaque ELF de usr/lib),
 * avec un lien depuis resource_dir()/ollama.
 */
export const OLLAMA_PREFIX = "usr/lib/chatonlm/ollama/";
export const APPIMAGE_OLLAMA_PREFIX = "usr/libexec/chatonlm/ollama/";
export const OLLAMA_LINK = "usr/lib/chatonlm/ollama";
export const OLLAMA_LINK_TARGET = "../../libexec/chatonlm/ollama";

/**
 * Fichiers de l'Ollama intégré exigés (chemins relatifs à son dossier) ; le
 * détail est comparé aux empreintes de l'archive (ollamaErrors).
 */
export const OLLAMA_FILES = [
  "bin/ollama",
  "VERSION",
  "lib/ollama/llama-server",
  "lib/ollama/libggml-base.so.0",
  "lib/ollama/vulkan/libggml-vulkan.so",
];

/**
 * Fichiers des moteurs exclus de l'Ollama intégré (CUDA, ROCm/HIP, MLX), où
 * qu'ils soient rangés (dossiers cuda_*, rocm*, mlx* ou premier niveau de
 * lib/ollama). Même motif que excluded_re de scripts/fetch-ollama.sh.
 */
export const EXCLUDED_ENGINES =
  /cuda|cublas|cudart|cudnn|nvrtc|nvidia|rocm|rocblas|hipblas|amdhip|(^|[/_.-])hip([_.-]|$)|mlx/i;

/** Fichiers de l'Ollama intégré qui doivent rester exécutables. */
export const OLLAMA_EXECUTABLES = [
  "bin/ollama",
  "lib/ollama/llama-server",
  "lib/ollama/llama-quantize",
];

/**
 * Bibliothèques que linuxdeploy ne doit pas copier dans usr/lib de l'AppImage
 * en suivant les dépendances de l'Ollama intégré : le chargeur Vulkan est
 * celui du système (avec ses pilotes).
 */
export const HOST_ONLY_LIBRARIES = ["libvulkan.so.1"];

/**
 * Moteur d'images (sd-server de stable-diffusion.cpp, construit par
 * scripts/build-sd-server.sh dans .tools/runtimes/sd-server), rangé comme
 * l'Ollama intégré : resource_dir()/sd-server dans le .deb, usr/libexec avec
 * un lien depuis resource_dir()/sd-server dans l'AppImage.
 */
export const SD_PREFIX = "usr/lib/chatonlm/sd-server/";
export const APPIMAGE_SD_PREFIX = "usr/libexec/chatonlm/sd-server/";
export const SD_LINK = "usr/lib/chatonlm/sd-server";
export const SD_LINK_TARGET = "../../libexec/chatonlm/sd-server";

/**
 * Fichiers du moteur d'images (chemins relatifs à son dossier) : le serveur,
 * le chargeur Vulkan de secours (pris par l'app seulement si le système n'en
 * a pas) et la version construite. Le détail est comparé aux empreintes du
 * dossier construit (sdServerErrors).
 */
export const SD_FILES = ["sd-server", "fallback/libvulkan.so.1", "VERSION"];

/** Seul fichier exécutable du moteur d'images. */
export const SD_EXECUTABLES = ["sd-server"];

/**
 * Fichiers ELF du moteur d'images → sonames qui peuvent manquer sur la
 * machine (ldd) : le chargeur Vulkan, que l'app prend dans fallback/ quand
 * le système n'en a pas.
 */
export const SD_ELF = {
  "sd-server": ["libvulkan.so.1"],
  "fallback/libvulkan.so.1": [],
};

/**
 * Composants livrés à part dans les paquets, contrôlés de la même façon
 * (builtinErrors) : nom dans les messages, dossiers, lien de l'AppImage,
 * exécutables (`onlyExecutables` : aucun autre fichier exécutable), motif
 * des fichiers exclus, script qui produit la copie de référence.
 */
export const OLLAMA_COMPONENT = {
  name: "Ollama intégré",
  of: "de l'Ollama intégré",
  prefix: OLLAMA_PREFIX,
  appImagePrefix: APPIMAGE_OLLAMA_PREFIX,
  link: OLLAMA_LINK,
  linkTarget: OLLAMA_LINK_TARGET,
  executables: OLLAMA_EXECUTABLES,
  onlyExecutables: false,
  excluded: EXCLUDED_ENGINES,
  hostOnly: HOST_ONLY_LIBRARIES,
  source: "scripts/fetch-ollama.sh",
};
export const SD_COMPONENT = {
  name: "Moteur d'images",
  of: "du moteur d'images",
  prefix: SD_PREFIX,
  appImagePrefix: APPIMAGE_SD_PREFIX,
  link: SD_LINK,
  linkTarget: SD_LINK_TARGET,
  executables: SD_EXECUTABLES,
  onlyExecutables: true,
  excluded: null,
  hostOnly: [],
  source: "scripts/build-sd-server.sh",
};
export const BUILTIN_COMPONENTS = [OLLAMA_COMPONENT, SD_COMPONENT];

/**
 * Fichiers propres au .deb : entrée de menu visible, AppStream, Ollama et
 * moteur d'images.
 */
export const DEB_FILES = [
  `usr/share/applications/${ID}.desktop`,
  `usr/share/metainfo/${ID}.metainfo.xml`,
  ...OLLAMA_FILES.map((f) => `${OLLAMA_PREFIX}${f}`),
  ...SD_FILES.map((f) => `${SD_PREFIX}${f}`),
];

/** Fichiers propres à l'AppImage : textes de la LGPL et notice des bibliothèques. */
export const APPIMAGE_FILES = [
  "AppRun",
  "chatonlm.desktop",
  ".DirIcon",
  "usr/share/doc/chatonlm/licenses/LGPL-2.1.txt",
  "usr/share/doc/chatonlm/licenses/LGPL-3.0.txt",
  "usr/share/doc/chatonlm/licenses/GPL-3.0.txt",
  "usr/share/doc/chatonlm/appimage-libraries/README.txt",
  ...OLLAMA_FILES.map((f) => `${APPIMAGE_OLLAMA_PREFIX}${f}`),
  ...SD_FILES.map((f) => `${APPIMAGE_SD_PREFIX}${f}`),
];

/** Dépendances du .deb exigées (les deux premières viennent du CLI Tauri). */
export const REQUIRED_DEPENDS = [
  "libwebkit2gtk-4.1-0",
  "libgtk-3-0",
  "libasound2",
];

/**
 * libappindicator, sous ses deux noms : le CLI Tauri l'ajoute aux dépendances
 * du .deb et la copie dans l'AppImage dès qu'il lit `tray-icon` parmi les
 * fonctionnalités de `tauri` dans le manifeste de l'app, même sous
 * `cfg(windows)`. La zone de notification Linux passe par `ksni`, sans elle.
 */
export const APPINDICATOR = /appindicator/;

/**
 * Paquets qu'apporte toujours une installation qui satisfait Depends
 * (paquets requis de Debian/Ubuntu, ou dépendances de libwebkit2gtk-4.1-0 et
 * libgtk-3-0) ; suffixe t64 d'Ubuntu 24.04 accepté.
 */
export const IMPLIED_PACKAGES = [
  "libc6",
  "libgcc-s1",
  "libstdc++6",
  "libglib2.0-0",
  "libgdk-pixbuf-2.0-0",
  "libcairo2",
  "libdbus-1-3",
  "libsoup-3.0-0",
  "libjavascriptcoregtk-4.1-0",
];

// --- Formats ------------------------------------------------------------

/** Membres d'une archive ar (format des .deb). */
export function parseAr(buffer) {
  if (buffer.subarray(0, 8).toString("latin1") !== "!<arch>\n") {
    throw new Error("archive ar invalide (en-tête !<arch> absent)");
  }
  const members = [];
  let offset = 8;
  while (offset + 60 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 60).toString("latin1");
    if (header.slice(58, 60) !== "`\n") {
      throw new Error(`membre ar invalide à l'octet ${offset}`);
    }
    const name = header.slice(0, 16).trim().replace(/\/$/, "");
    const size = Number.parseInt(header.slice(48, 58).trim(), 10);
    const start = offset + 60;
    members.push({ name, size, data: buffer.subarray(start, start + size) });
    offset = start + size + (size % 2);
  }
  return members;
}

/** Champs d'un fichier control de Debian (lignes de suite comprises). */
export function parseControl(text) {
  const fields = {};
  let last = null;
  for (const line of text.split("\n")) {
    if (/^\s/.test(line) && last) {
      fields[last] += `\n${line.trim()}`;
    } else {
      const match = /^([A-Za-z0-9-]+):\s?(.*)$/.exec(line);
      if (match) {
        last = match[1];
        fields[last] = match[2];
      }
    }
  }
  return fields;
}

/** Noms des paquets d'un champ Depends (versions et alternatives retirées). */
export function dependsNames(value = "") {
  return value
    .split(",")
    .flatMap((item) => item.split("|"))
    .map((item) =>
      item
        .trim()
        .replace(/\s*\(.*\)$/, "")
        .replace(/:\S+$/, ""),
    )
    .filter(Boolean);
}

/** Groupes et clés d'un fichier .desktop. */
export function parseDesktop(text) {
  const groups = {};
  let group = null;
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const header = /^\[(.+)\]$/.exec(line);
    if (header) {
      group = header[1];
      groups[group] = {};
      continue;
    }
    const eq = line.indexOf("=");
    if (group && eq > 0) {
      groups[group][line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
    }
  }
  return groups;
}

/** Fin d'un fichier ELF 64 bits (fin de la table des sections) : taille du runtime. */
export function elfEnd(buffer) {
  if (buffer.readUInt32BE(0) !== 0x7f454c46 || buffer[4] !== 2) {
    throw new Error("pas un ELF 64 bits");
  }
  const shoff = Number(buffer.readBigUInt64LE(0x28));
  const shentsize = buffer.readUInt16LE(0x3a);
  const shnum = buffer.readUInt16LE(0x3c);
  return shoff + shentsize * shnum;
}

function compareVersions(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/** Versions les plus récentes demandées (GLIBC, GLIBCXX, CXXABI), d'après objdump -T. */
export function maxSymbolVersions(objdump) {
  const max = {};
  for (const match of objdump.matchAll(
    /\b(GLIBCXX|GLIBC|CXXABI)_([0-9][0-9.]*)\b/g,
  )) {
    const [, name, version] = match;
    if (!max[name] || compareVersions(version, max[name]) > 0)
      max[name] = version;
  }
  return max;
}

/** Sonames demandés (DT_NEEDED), d'après readelf -d (LC_ALL=C). */
export function neededLibraries(readelf) {
  return [...readelf.matchAll(/\(NEEDED\).*\[([^\]]+)\]/g)].map((m) => m[1]);
}

// --- Contrôles ----------------------------------------------------------

/** Erreurs d'un .desktop : `hidden` vrai pour l'entrée cachée du bundler. */
export function desktopErrors(name, text, { hidden }) {
  const entry = parseDesktop(text)["Desktop Entry"];
  if (!entry) return [`${name} : groupe [Desktop Entry] absent`];
  const errors = [];
  const want = {
    Type: "Application",
    Name: "ChatonLM",
    Exec: "chatonlm",
    Icon: "chatonlm",
    Terminal: "false",
  };
  for (const [key, value] of Object.entries(want)) {
    if (entry[key] !== value) {
      errors.push(
        `${name} : ${key}=${entry[key] ?? "(absent)"} au lieu de ${value}`,
      );
    }
  }
  if (hidden && entry.NoDisplay !== "true") {
    errors.push(`${name} : NoDisplay=true attendu (entrée cachée du bundler)`);
  }
  if (!hidden) {
    if (entry.NoDisplay === "true")
      errors.push(`${name} : ne doit pas être caché`);
    if (entry.StartupWMClass !== ID) {
      errors.push(`${name} : StartupWMClass=${ID} attendu (app_id Wayland)`);
    }
    for (const key of ["GenericName", "Comment", "Keywords"]) {
      for (const lang of ["", "[es]", "[fr]"]) {
        if (!entry[`${key}${lang}`])
          errors.push(`${name} : ${key}${lang} absent`);
      }
    }
  }
  return errors;
}

/**
 * Composants livrés dans l'exécutable, sans fichier à eux dans le paquet (rien
 * d'autre ne trahirait leur oubli dans les notices) : fichier → mentions
 * attendues. Silero VAD : modèle du mode vocal (src-tauri/models) ; sonora :
 * annulation d'écho, crates listées par cargo-about (« nom version (licence) »).
 * Notice du moteur d'images : son projet et son chargeur Vulkan de secours,
 * avec la licence de celui-ci.
 */
export const NOTICE_MENTIONS = [
  ["usr/share/doc/chatonlm/copyright", [/Silero VAD/]],
  ["usr/share/doc/chatonlm/THIRD_PARTY_LICENSES", [/Silero VAD/]],
  [
    "usr/share/doc/chatonlm/licenses/rust-crates.txt",
    [
      "sonora",
      "sonora-aec3",
      "sonora-agc2",
      "sonora-common-audio",
      "sonora-fft",
      "sonora-ns",
      "sonora-simd",
    ].map((crate) => new RegExp(`^\\s*${crate} \\d`, "m")),
  ],
  [
    "usr/share/doc/chatonlm/licenses/sd-server/README.txt",
    [/stable-diffusion\.cpp/, /fallback\/libvulkan\.so\.1/, /Apache License/],
  ],
];

/**
 * Notices qui ne citent pas un composant livré dans l'exécutable : `files`
 * (chemins du paquet), `read(chemin)`. Un fichier absent est signalé ailleurs.
 */
export function noticeErrors({ files, read }) {
  const errors = [];
  for (const [file, mentions] of NOTICE_MENTIONS) {
    if (!files.has(file)) continue;
    const text = read(file);
    for (const mention of mentions) {
      if (!mention.test(text)) {
        errors.push(`${file} : mention absente (${mention.source})`);
      }
    }
  }
  return errors;
}

/**
 * Contrôles du .deb : `control` (champs), `files` (chemins → {size, mode,
 * link}), `read(chemin)` (contenu texte), `version` attendue.
 */
export function debErrors({ control, files, read, version }) {
  const errors = [];
  const expect = {
    Package: "chatonlm",
    Version: version,
    Architecture: "amd64",
  };
  for (const [key, value] of Object.entries(expect)) {
    if (control[key] !== value) {
      errors.push(
        `control : ${key}: ${control[key] ?? "(absent)"} au lieu de ${value}`,
      );
    }
  }
  for (const key of [
    "Maintainer",
    "Description",
    "Section",
    "Homepage",
    "Installed-Size",
  ]) {
    if (!control[key]) errors.push(`control : champ ${key} absent`);
  }
  const depends = dependsNames(control.Depends);
  for (const dep of REQUIRED_DEPENDS) {
    if (!depends.includes(dep)) errors.push(`control : Depends sans ${dep}`);
  }
  for (const dep of depends.filter((name) => APPINDICATOR.test(name))) {
    errors.push(
      `control : Depends avec ${dep} (tray-icon lu par le CLI Tauri)`,
    );
  }
  for (const file of [...COMMON_FILES, ...DEB_FILES]) {
    if (!files.has(file)) errors.push(`fichier absent : /${file}`);
  }
  for (const file of files.keys()) {
    if (
      /\/(LGPL|GPL)-[0-9.]+\.txt$|appimage-libraries\/|^usr\/libexec\//.test(
        file,
      )
    ) {
      errors.push(`fichier propre à l'AppImage dans le .deb : /${file}`);
    }
  }
  const bin = files.get("usr/bin/chatonlm");
  if (bin && (bin.mode & 0o111) === 0)
    errors.push("/usr/bin/chatonlm n'est pas exécutable");
  const apps = [...files.keys()].filter((f) =>
    /^usr\/share\/applications\/[^/]+\.desktop$/.test(f),
  );
  if (apps.length !== 2) {
    errors.push(
      `2 fichiers .desktop attendus dans /usr/share/applications (${apps.length})`,
    );
  }
  if (files.has("usr/share/applications/chatonlm.desktop")) {
    errors.push(
      ...desktopErrors(
        "chatonlm.desktop",
        read("usr/share/applications/chatonlm.desktop"),
        {
          hidden: true,
        },
      ),
    );
  }
  if (files.has(`usr/share/applications/${ID}.desktop`)) {
    errors.push(
      ...desktopErrors(
        `${ID}.desktop`,
        read(`usr/share/applications/${ID}.desktop`),
        {
          hidden: false,
        },
      ),
    );
  }
  if (files.has(`usr/share/metainfo/${ID}.metainfo.xml`)) {
    const xml = read(`usr/share/metainfo/${ID}.metainfo.xml`);
    if (!xml.includes(`<id>${ID}</id>`))
      errors.push("metainfo : <id> inattendu");
    if (
      !xml.includes(`<launchable type="desktop-id">${ID}.desktop</launchable>`)
    ) {
      errors.push("metainfo : launchable différent de l'entrée de menu");
    }
  }
  if (files.has("usr/share/doc/chatonlm/copyright")) {
    errors.push(...dep5Errors(read("usr/share/doc/chatonlm/copyright")));
  }
  errors.push(...noticeErrors({ files, read }));
  return errors;
}

/** Champs d'un paragraphe DEP-5 : nom → lignes (la première, puis la suite). */
function dep5Fields(paragraph) {
  const fields = new Map();
  let current = null;
  for (const line of paragraph.split("\n")) {
    const field = /^([A-Za-z-]+):\s*(.*)$/.exec(line);
    if (field) {
      current = [field[2].trim()];
      fields.set(field[1], current);
    } else if (current && /^\s/.test(line)) {
      current.push(line.trim());
    }
  }
  return fields;
}

/**
 * Fichier copyright au format DEP-5 : chaque licence nommée par un
 * paragraphe Files a son texte, dans ce paragraphe ou dans un paragraphe
 * License autonome du même nom (lintian :
 * missing-license-paragraph-in-dep5-copyright) ; une exception s'écrit
 * « <licence> with <mots-clés> exception ».
 */
export function dep5Errors(text) {
  if (
    !text.startsWith(
      "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/",
    )
  ) {
    return ["copyright : pas au format DEP-5"];
  }
  const errors = [];
  const used = new Set();
  const defined = new Set();
  const [, ...paragraphs] = text.split(/\n[ \t]*\n/).map(dep5Fields);
  for (const fields of paragraphs) {
    const license = fields.get("License");
    if (!license) continue;
    const [name, ...body] = license;
    const hasText = body.some((line) => line !== "" && line !== ".");
    if (fields.has("Files")) {
      for (const one of name.split(/\s+(?:or|and)\s+/)) {
        if (hasText) defined.add(one);
        else used.add(one);
      }
    } else if (hasText) {
      defined.add(name);
    } else {
      errors.push(
        `copyright : paragraphe de la licence « ${name} » sans texte`,
      );
    }
  }
  for (const name of used) {
    if (!defined.has(name)) {
      errors.push(`copyright : licence « ${name} » sans paragraphe autonome`);
    }
    if (/\swith\s/.test(name) && !/\swith\s+\S+\s+exception$/.test(name)) {
      errors.push(
        `copyright : « ${name} » : exception à écrire « with <mots-clés> exception »`,
      );
    }
  }
  return errors;
}

/**
 * Contrôles de l'AppImage extraite : `files` (chemins → {size, mode, link}),
 * `read(chemin)`, `rootDesktops` (fichiers .desktop à la racine),
 * `requireInventory`.
 */
export function appImageErrors({
  files,
  read,
  rootDesktops,
  requireInventory,
}) {
  const errors = [];
  for (const file of [...COMMON_FILES, ...APPIMAGE_FILES]) {
    if (!files.has(file)) errors.push(`fichier absent : ${file}`);
  }
  for (const file of [...files.keys()].filter((name) =>
    APPINDICATOR.test(name),
  )) {
    errors.push(
      `${file} : libappindicator livrée (tray-icon lu par le CLI Tauri)`,
    );
  }
  if (rootDesktops.length !== 1 || rootDesktops[0] !== "chatonlm.desktop") {
    errors.push(
      `un seul .desktop attendu à la racine, chatonlm.desktop (${rootDesktops.join(", ") || "aucun"})`,
    );
  }
  for (const file of DEB_FILES) {
    if (files.has(file))
      errors.push(`fichier propre au .deb dans l'AppImage : ${file}`);
  }
  if (files.has("usr/share/applications/chatonlm.desktop")) {
    errors.push(
      ...desktopErrors(
        "chatonlm.desktop",
        read("usr/share/applications/chatonlm.desktop"),
        {
          hidden: true,
        },
      ),
    );
  }
  // Ollama intégré et moteur d'images : hors de usr/lib, derrière un lien.
  for (const component of BUILTIN_COMPONENTS) {
    const link = files.get(component.link);
    if (link?.link !== component.linkTarget) {
      errors.push(
        `${component.link} : lien vers ${component.linkTarget} attendu (${link ? (link.link ?? "fichier ou dossier") : "absent"})`,
      );
    }
    for (const file of files.keys()) {
      if (file.startsWith(component.prefix)) {
        errors.push(
          `${component.name} dans usr/lib (linuxdeploy le traiterait) : ${file}`,
        );
      }
    }
  }
  if (files.has("usr/share/doc/chatonlm/copyright")) {
    errors.push(...dep5Errors(read("usr/share/doc/chatonlm/copyright")));
  }
  errors.push(...noticeErrors({ files, read }));
  const inventory = "usr/share/doc/chatonlm/appimage-libraries/libraries.txt";
  if (requireInventory && !files.has(inventory)) {
    errors.push("inventaire LGPL absent (appimage-libraries/libraries.txt)");
  }
  return errors;
}

/** Empreintes d'un fichier SHA256SUMS (sortie de sha256sum) : chemin → empreinte. */
export function parseSums(text) {
  const sums = new Map();
  for (const line of text.split("\n")) {
    const match = /^([0-9a-f]{64}) [ *](.+)$/.exec(line.trim());
    if (match) sums.set(match[2], match[1]);
  }
  return sums;
}

/**
 * Contrôles d'un composant livré à part (`component` : OLLAMA_COMPONENT ou
 * SD_COMPONENT) : `files` (chemins du paquet → {size, mode, link}),
 * `hash(chemin)` (sha256 d'un fichier du paquet), `read(chemin)`, `sums`
 * (parseSums du SHA256SUMS de sa copie de référence, chemins relatifs à son
 * dossier), `version` attendue, `prefix` (son dossier dans le paquet).
 * Fichiers identiques à ceux de la copie (aucun strip, patchelf ou RPATH de
 * linuxdeploy), aucun fichier de plus (moteurs exclus), aucun lien
 * symbolique, exécutables exécutables (et seulement eux si
 * `onlyExecutables`), et aucune de ses bibliothèques copiée ailleurs dans
 * usr/lib.
 */
export function builtinErrors(
  component,
  { files, hash, read, sums, version, prefix = component.prefix },
) {
  const { name } = component;
  const errors = [];
  if (!sums || sums.size === 0) {
    return [`empreintes ${component.of} absentes (lancer ${component.source})`];
  }
  for (const [rel, sum] of sums) {
    const file = `${prefix}${rel}`;
    const info = files.get(file);
    if (!info) errors.push(`${name} : fichier absent : ${file}`);
    else if (info.link) errors.push(`${name} : lien symbolique : ${file}`);
    else if (hash(file) !== sum) {
      errors.push(`${name} : fichier modifié (strip, patchelf ?) : ${file}`);
    }
  }
  for (const [file, info] of files) {
    if (!file.startsWith(prefix)) continue;
    const rel = file.slice(prefix.length);
    if (component.excluded?.test(rel)) {
      errors.push(`${name} : moteur exclu livré : ${file}`);
    } else if (!sums.has(rel) && rel !== "VERSION") {
      errors.push(
        `${name} : fichier inattendu : ${file}${info.link ? ` -> ${info.link}` : ""}`,
      );
    }
  }
  for (const rel of component.executables) {
    const info = files.get(`${prefix}${rel}`);
    if (info && !info.link && (info.mode & 0o111) === 0) {
      errors.push(`${name} : ${prefix}${rel} n'est pas exécutable`);
    }
  }
  if (component.onlyExecutables) {
    for (const [file, info] of files) {
      const rel = file.slice(prefix.length);
      if (
        file.startsWith(prefix) &&
        !info.link &&
        !component.executables.includes(rel) &&
        (info.mode & 0o111) !== 0
      ) {
        errors.push(`${name} : ${file} ne doit pas être exécutable`);
      }
    }
  }
  if (files.has(`${prefix}VERSION`)) {
    const shipped = read(`${prefix}VERSION`).trim();
    if (shipped !== version) {
      errors.push(
        `${name} : VERSION ${shipped} au lieu de ${version ?? "(inconnue)"}`,
      );
    }
  }
  // Bibliothèques du composant (ou chargeur Vulkan de la machine de
  // construction) copiées hors de son dossier par linuxdeploy ; les dossiers
  // des autres composants ont leurs propres contrôles.
  const own = BUILTIN_COMPONENTS.flatMap((c) => [c.prefix, c.appImagePrefix]);
  const names = new Set([
    ...[...sums.keys()]
      .filter((rel) => rel !== "VERSION")
      .map((rel) => path.posix.basename(rel)),
    ...component.hostOnly,
  ]);
  for (const [file, info] of files) {
    if (
      info.link ||
      file.startsWith(prefix) ||
      own.some((dir) => file.startsWith(dir)) ||
      !file.startsWith("usr/lib/")
    ) {
      continue;
    }
    if (names.has(path.posix.basename(file))) {
      errors.push(
        `bibliothèque ${component.of} copiée hors de son dossier : ${file}`,
      );
    }
  }
  return errors;
}

/**
 * Contrôles de l'Ollama intégré (builtinErrors) : `sums` de
 * .tools/runtimes/ollama/SHA256SUMS, `version` épinglée (packaging/ollama.env).
 */
export function ollamaErrors(options) {
  return builtinErrors(OLLAMA_COMPONENT, options);
}

/**
 * Contrôles du moteur d'images (builtinErrors) : `sums` de
 * .tools/runtimes/sd-server/SHA256SUMS, `version` du VERSION de ce dossier,
 * `pinned` (SDCPP_VERSION de packaging/sdcpp.env) : la version livrée doit
 * être « <pinned>+chatonlm.<empreinte de 12 caractères> ».
 */
export function sdServerErrors({ pinned, ...options }) {
  const errors = builtinErrors(SD_COMPONENT, options);
  const prefix = options.prefix ?? SD_PREFIX;
  if (
    pinned &&
    options.sums?.size > 0 &&
    options.files.has(`${prefix}VERSION`)
  ) {
    const shipped = options.read(`${prefix}VERSION`).trim();
    const escaped = pinned.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (!new RegExp(`^${escaped}\\+chatonlm\\.[0-9a-f]{12}$`).test(shipped)) {
      errors.push(
        `Moteur d'images : VERSION ${shipped} hors de la forme ${pinned}+chatonlm.<empreinte> (packaging/sdcpp.env)`,
      );
    }
  }
  return errors;
}

/**
 * Fichiers ELF de l'Ollama intégré dans un paquet extrait (`dir`) : versions
 * de symboles les plus récentes, sur l'ensemble (objdump -T).
 */
export function inspectOllamaElf(dir, files, prefix = OLLAMA_PREFIX) {
  const versions = {};
  let count = 0;
  for (const [rel, info] of files) {
    if (!rel.startsWith(prefix) || info.link) continue;
    const file = path.join(dir, rel);
    if (!isElf(file)) continue;
    count += 1;
    const max = maxSymbolVersions(run("objdump", ["-T", file]));
    for (const [name, version] of Object.entries(max)) {
      if (!versions[name] || compareVersions(version, versions[name]) > 0) {
        versions[name] = version;
      }
    }
  }
  return { count, versions };
}

/**
 * Fichiers ELF du moteur d'images (SD_ELF) dans un paquet extrait (`dir`) :
 * pour chacun présent, { rel, elf, needed, versions, missing, linked }
 * (inspectBinary, sans LD_LIBRARY_PATH).
 */
export function inspectSdServerElf(dir, files, prefix = SD_PREFIX) {
  const entries = [];
  for (const rel of Object.keys(SD_ELF)) {
    const info = files.get(`${prefix}${rel}`);
    if (!info || info.link) continue;
    const file = path.join(dir, prefix, rel);
    entries.push(
      isElf(file)
        ? { rel, elf: true, ...inspectBinary(file, { cleanEnv: true }) }
        : { rel, elf: false },
    );
  }
  return entries;
}

/** Vrai si le fichier commence par la marque ELF. */
function isElf(file) {
  const head = Buffer.alloc(4);
  const fd = openSync(file, "r");
  try {
    readSync(fd, head, 0, 4, 0);
  } finally {
    closeSync(fd);
  }
  return head.readUInt32BE(0) === 0x7f454c46;
}

/** sha256 d'un fichier. */
function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

// --- Lecture des paquets -------------------------------------------------

/** Fichiers d'un dossier : chemin relatif → {size, mode, link}. */
export function walk(dir) {
  const files = new Map();
  const visit = (sub) => {
    for (const entry of readdirSync(path.join(dir, sub))) {
      const rel = sub ? `${sub}/${entry}` : entry;
      const full = path.join(dir, rel);
      const stat = lstatSync(full);
      if (stat.isDirectory()) visit(rel);
      else {
        files.set(rel, {
          size: stat.size,
          mode: stat.mode,
          link: stat.isSymbolicLink() ? readlinkSync(full) : null,
        });
      }
    }
  };
  visit("");
  return files;
}

function freshDir(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  return dir;
}

function run(cmd, args, options = {}) {
  return execFileSync(cmd, args, {
    encoding: "utf8",
    env: ENV_C,
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
}

/**
 * Binaire : sonames demandés, versions de symboles, sonames introuvables et
 * sonames chargés (ldd ; `cleanEnv` : sans LD_LIBRARY_PATH, comme sur une
 * machine d'utilisateur).
 */
export function inspectBinary(file, { cleanEnv = false } = {}) {
  const needed = neededLibraries(run("readelf", ["-d", file]));
  const versions = maxSymbolVersions(run("objdump", ["-T", file]));
  const env = { ...ENV_C };
  if (cleanEnv) delete env.LD_LIBRARY_PATH;
  const ldd = spawnSync("ldd", [file], { encoding: "utf8", env });
  const out = ldd.stdout ?? "";
  const missing = [...out.matchAll(/^\s*(\S+) => not found/gm)].map(
    (m) => m[1],
  );
  const linked = [...out.matchAll(/^\s*(\S+) => /gm)].map((m) => m[1]);
  return { needed, versions, missing, linked };
}

/**
 * Validateurs externes, s'ils sont installés (desktop-file-utils, appstream) :
 * erreurs, et outils absents (notés dans le rapport).
 */
export function externalValidation(dir, { desktops, appstream }) {
  const errors = [];
  const missing = [];
  const tool = (name, args) => {
    const out = spawnSync(name, args, { encoding: "utf8", env: ENV_C });
    if (out.error?.code === "ENOENT") {
      missing.push(name);
      return;
    }
    if (out.status !== 0) {
      errors.push(
        `${name} : ${(out.stdout + out.stderr).trim().split("\n").slice(0, 5).join(" / ")}`,
      );
    }
  };
  for (const desktop of desktops)
    tool("desktop-file-validate", [path.join(dir, desktop)]);
  if (appstream) tool("appstreamcli", ["validate-tree", "--no-net", dir]);
  return { errors, missing: [...new Set(missing)] };
}

/** Paquet Debian d'un soname sur cette machine (dpkg), ou null. */
function packageOfSoname(soname) {
  const out = spawnSync("dpkg", ["-S", `*/${soname}`], {
    encoding: "utf8",
    env: ENV_C,
  });
  if (out.status !== 0) return null;
  const line =
    out.stdout.split("\n").find((l) => /x86_64-linux-gnu/.test(l)) ??
    out.stdout;
  return line.split(":")[0].split(",")[0].trim() || null;
}

/**
 * Ollama intégré d'un paquet extrait dans `dir` : erreurs (ollamaErrors) et
 * versions de symboles de ses fichiers ELF. `ollama` : {sums, version}, ou
 * null pour ne pas le contrôler (essais).
 */
function inspectOllama(dir, files, ollama, prefix) {
  if (!ollama) return { errors: [], elf: null };
  const read = (rel) => readFileSync(path.join(dir, rel), "utf8");
  const errors = ollamaErrors({
    files,
    read,
    hash: (rel) => sha256(path.join(dir, rel)),
    prefix,
    ...ollama,
  });
  return { errors, elf: inspectOllamaElf(dir, files, prefix) };
}

/**
 * Moteur d'images d'un paquet extrait dans `dir` : erreurs (sdServerErrors)
 * et inspection de ses fichiers ELF. `sdServer` : {sums, version, pinned},
 * ou null pour ne pas le contrôler (essais).
 */
function inspectSdServer(dir, files, sdServer, prefix) {
  if (!sdServer) return { errors: [], elf: null };
  const read = (rel) => readFileSync(path.join(dir, rel), "utf8");
  const errors = sdServerErrors({
    files,
    read,
    hash: (rel) => sha256(path.join(dir, rel)),
    prefix,
    ...sdServer,
  });
  return { errors, elf: inspectSdServerElf(dir, files, prefix) };
}

/** Inspection d'un .deb, extrait dans `work`. */
export function inspectDeb(
  file,
  { work, version, ollama = null, sdServer = null },
) {
  const members = parseAr(readFileSync(file));
  const member = (prefix) => members.find((m) => m.name.startsWith(prefix));
  const errors = [];
  if (member("debian-binary")?.data.toString().trim() !== "2.0") {
    errors.push("debian-binary : 2.0 attendu");
  }
  freshDir(work);
  const extract = (prefix, dir) => {
    const m = member(prefix);
    if (!m) throw new Error(`${prefix}* absent du .deb`);
    const archive = path.join(work, m.name);
    writeFileSync(archive, m.data);
    mkdirSync(dir, { recursive: true });
    run("tar", ["-xf", archive, "-C", dir]);
  };
  extract("control.tar", path.join(work, "control"));
  extract("data.tar", path.join(work, "data"));
  const control = parseControl(
    readFileSync(path.join(work, "control/control"), "utf8"),
  );
  const dataDir = path.join(work, "data");
  const files = walk(dataDir);
  const read = (rel) => readFileSync(path.join(dataDir, rel), "utf8");
  errors.push(...debErrors({ control, files, read, version }));
  const validation = externalValidation(dataDir, {
    desktops: [...files.keys()].filter((f) =>
      f.startsWith("usr/share/applications/"),
    ),
    appstream: true,
  });
  errors.push(...validation.errors);
  const binary = files.has("usr/bin/chatonlm")
    ? inspectBinary(path.join(dataDir, "usr/bin/chatonlm"))
    : null;
  const builtin = inspectOllama(dataDir, files, ollama, OLLAMA_PREFIX);
  errors.push(...builtin.errors);
  const images = inspectSdServer(dataDir, files, sdServer, SD_PREFIX);
  errors.push(...images.errors);
  return {
    file,
    size: statSync(file).size,
    control,
    files,
    errors,
    binary,
    ollamaElf: builtin.elf,
    sdServerElf: images.elf,
    missingTools: validation.missing,
  };
}

/**
 * Runtime de l'AppImage : taille, empreinte, marque du type 2 et commit de
 * type2-runtime qu'il affiche (« …/type2-runtime/commit/dd6cebe »).
 * appimagetool réécrit des sections du runtime (empreinte MD5, informations
 * de mise à jour) : l'empreinte diffère de celle du fichier téléchargé, le
 * commit identifie la version.
 */
export function appImageRuntime(buffer) {
  const end = elfEnd(buffer);
  const runtime = buffer.subarray(0, end);
  const commit = /type2-runtime\/commit\/([0-9a-f]{7,40})/.exec(
    runtime.toString("latin1"),
  )?.[1];
  return {
    size: end,
    sha256: createHash("sha256").update(runtime).digest("hex"),
    type2: buffer.subarray(8, 11).toString("latin1") === "AI\x02",
    commit: commit ?? null,
  };
}

/** Inspection d'une AppImage, extraite dans `work` (--appimage-extract). */
export function inspectAppImage(
  file,
  { work, requireInventory, runtimePin, ollama = null, sdServer = null },
) {
  const buffer = readFileSync(file);
  const runtime = appImageRuntime(buffer);
  const errors = [];
  if (!runtime.type2)
    errors.push("pas une AppImage de type 2 (marque AI\\x02 absente)");
  if (
    runtimePin &&
    !(runtime.commit && runtimePin.startsWith(runtime.commit))
  ) {
    errors.push(
      `runtime non épinglé (commit ${runtime.commit ?? "inconnu"} au lieu de ${runtimePin.slice(0, 7)})`,
    );
  }
  freshDir(work);
  // Seule l'extraction du runtime s'exécute (aucune fenêtre) ; environnement
  // réduit pour qu'aucune variable ne lance l'app à la place.
  const extracted = spawnSync(path.resolve(file), ["--appimage-extract"], {
    cwd: work,
    encoding: "utf8",
    env: {
      PATH: process.env.PATH,
      LC_ALL: "C",
      TMPDIR: process.env.TMPDIR ?? work,
    },
    maxBuffer: 64 * 1024 * 1024,
  });
  const dir = path.join(work, "squashfs-root");
  if (extracted.status !== 0 || !existsSync(dir)) {
    throw new Error(
      `extraction impossible : ${extracted.stderr || extracted.error}`,
    );
  }
  const files = walk(dir);
  const read = (rel) => readFileSync(path.join(dir, rel), "utf8");
  const rootDesktops = readdirSync(dir).filter((f) => f.endsWith(".desktop"));
  errors.push(
    ...appImageErrors({ files, read, rootDesktops, requireInventory }),
  );
  const validation = externalValidation(dir, {
    desktops: rootDesktops,
    appstream: false,
  });
  errors.push(...validation.errors);
  const libraries = [...files]
    .filter(
      ([rel, info]) =>
        !info.link &&
        !rel.startsWith(APPIMAGE_OLLAMA_PREFIX) &&
        !rel.startsWith(APPIMAGE_SD_PREFIX) &&
        /\.so(\.[0-9.]+)?$/.test(rel),
    )
    .map(([rel, info]) => ({ rel, size: info.size }))
    .sort((a, b) => b.size - a.size || a.rel.localeCompare(b.rel));
  const binary = files.has("usr/bin/chatonlm")
    ? inspectBinary(path.join(dir, "usr/bin/chatonlm"))
    : null;
  const builtin = inspectOllama(dir, files, ollama, APPIMAGE_OLLAMA_PREFIX);
  errors.push(...builtin.errors);
  const images = inspectSdServer(dir, files, sdServer, APPIMAGE_SD_PREFIX);
  errors.push(...images.errors);
  return {
    file,
    size: buffer.length,
    runtime,
    dir,
    files,
    errors,
    libraries,
    binary,
    ollamaElf: builtin.elf,
    sdServerElf: images.elf,
    missingTools: validation.missing,
  };
}

// --- Rapport ------------------------------------------------------------

export function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} Mio`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} kio`;
  return `${bytes} o`;
}

function total(files) {
  let sum = 0;
  for (const info of files.values()) if (!info.link) sum += info.size;
  return sum;
}

/** Versions de symboles « GLIBC_2.35, GLIBCXX_3.4.30 », ou « aucune ». */
const symbolList = (versions) =>
  Object.entries(versions)
    .map(([k, v]) => `${k}_${v}`)
    .join(", ") || "aucune";

/** Lignes du rapport sur l'Ollama intégré : taille, glibc demandée. */
export function ollamaLines(files, elf, { maxGlibc, prefix = OLLAMA_PREFIX }) {
  if (!elf) return { lines: [], errors: [] };
  let size = 0;
  let count = 0;
  for (const [rel, info] of files) {
    if (rel.startsWith(prefix) && !info.link) {
      size += info.size;
      count += 1;
    }
  }
  const versions = symbolList(elf.versions);
  const lines = [
    `Ollama intégré : ${count} fichiers, ${formatSize(size)}, ${elf.count} ELF ; ` +
      `versions de symboles les plus récentes : ${versions}`,
  ];
  const errors = [];
  if (
    maxGlibc &&
    elf.versions.GLIBC &&
    compareVersions(elf.versions.GLIBC, maxGlibc) > 0
  ) {
    errors.push(
      `l'Ollama intégré demande GLIBC_${elf.versions.GLIBC} (maximum permis : ${maxGlibc})`,
    );
  }
  return { lines, errors };
}

/**
 * Lignes du rapport sur le moteur d'images : taille, bibliothèques et glibc
 * demandées par chaque fichier ELF (`elf` : inspectSdServerElf). Erreurs :
 * fichier qui n'est pas un ELF, glibc plus récente que `maxGlibc`,
 * bibliothèque introuvable autre que celles de SD_ELF, libgomp (OpenMP doit
 * être coupé à la construction).
 */
export function sdServerLines(files, elf, { maxGlibc, prefix = SD_PREFIX }) {
  if (!elf) return { lines: [], errors: [] };
  let size = 0;
  let count = 0;
  for (const [rel, info] of files) {
    if (rel.startsWith(prefix) && !info.link) {
      size += info.size;
      count += 1;
    }
  }
  const lines = [
    `Moteur d'images (sd-server) : ${count} fichiers, ${formatSize(size)}`,
  ];
  const errors = [];
  for (const entry of elf) {
    if (!entry.elf) {
      errors.push(`moteur d'images : ${prefix}${entry.rel} n'est pas un ELF`);
      continue;
    }
    lines.push(
      `  ${entry.rel} : ${entry.needed.join(", ") || "aucune bibliothèque"} ; ` +
        `versions de symboles les plus récentes : ${symbolList(entry.versions)}`,
    );
    const glibc = entry.versions.GLIBC;
    if (maxGlibc && glibc && compareVersions(glibc, maxGlibc) > 0) {
      errors.push(
        `le moteur d'images (${entry.rel}) demande GLIBC_${glibc} (maximum permis : ${maxGlibc})`,
      );
    }
    const allowed = SD_ELF[entry.rel] ?? [];
    const missing = entry.missing.filter((name) => !allowed.includes(name));
    if (missing.length > 0) {
      errors.push(
        `moteur d'images : ${entry.rel} : bibliothèques introuvables (ldd) : ${missing.join(", ")}`,
      );
    }
    if ([...entry.needed, ...entry.linked].some((n) => /^libgomp\./.test(n))) {
      errors.push(
        `moteur d'images : ${entry.rel} dépend de libgomp (OpenMP devait être coupé)`,
      );
    }
  }
  return { lines, errors };
}

function binaryLines(binary, { maxGlibc, depends }) {
  if (!binary) return { lines: [], errors: [] };
  const { needed, versions, missing } = binary;
  const lines = [
    `Binaire : ${needed.length} bibliothèques demandées : ${needed.join(", ")}`,
    `  versions de symboles les plus récentes : ${symbolList(versions)}`,
  ];
  const errors = [];
  if (missing.length > 0) {
    lines.push(
      `  introuvables sur cette machine (ldd) : ${missing.join(", ")}`,
    );
  }
  if (
    maxGlibc &&
    versions.GLIBC &&
    compareVersions(versions.GLIBC, maxGlibc) > 0
  ) {
    errors.push(
      `le binaire demande GLIBC_${versions.GLIBC} (maximum permis : ${maxGlibc})`,
    );
  }
  if (depends) {
    // Sur Debian ou Ubuntu : paquet de chaque soname, couvert par Depends ?
    const allowed = new Set([...depends, ...IMPLIED_PACKAGES]);
    const uncovered = [];
    for (const soname of needed) {
      const pkg = packageOfSoname(soname);
      if (pkg === null) continue;
      if (!allowed.has(pkg) && !allowed.has(pkg.replace(/t64$/, ""))) {
        uncovered.push(`${soname} (${pkg})`);
      }
    }
    if (uncovered.length > 0) {
      errors.push(
        `bibliothèques sans paquet dans Depends : ${uncovered.join(", ")}`,
      );
    }
  }
  return { lines, errors };
}

export function formatReport({ deb, appimage, maxGlibc }) {
  const lines = [];
  const errors = [];
  const hasDpkg = spawnSync("dpkg", ["--version"], { env: ENV_C }).status === 0;
  if (deb) {
    lines.push(`== ${path.basename(deb.file)} (${formatSize(deb.size)}) ==`);
    const c = deb.control;
    lines.push(
      `Contrôle : Package ${c.Package}, Version ${c.Version}, Architecture ${c.Architecture}, ` +
        `Section ${c.Section ?? "-"}, Installed-Size ${c["Installed-Size"]} kio`,
      `Depends : ${c.Depends ?? "(aucun)"}`,
      `Fichiers : ${deb.files.size}, ${formatSize(total(deb.files))} installés`,
    );
    for (const [rel, info] of [...deb.files].sort()) {
      lines.push(
        `  /${rel}  ${info.link ? `-> ${info.link}` : formatSize(info.size)}`,
      );
    }
    const bin = binaryLines(deb.binary, {
      maxGlibc,
      depends: hasDpkg ? dependsNames(c.Depends) : null,
    });
    lines.push(...bin.lines);
    const builtin = ollamaLines(deb.files, deb.ollamaElf, { maxGlibc });
    lines.push(...builtin.lines);
    const images = sdServerLines(deb.files, deb.sdServerElf, { maxGlibc });
    lines.push(...images.lines);
    errors.push(
      ...deb.errors.map((e) => `.deb : ${e}`),
      ...bin.errors.map((e) => `.deb : ${e}`),
      ...builtin.errors.map((e) => `.deb : ${e}`),
      ...images.errors.map((e) => `.deb : ${e}`),
    );
    lines.push("");
  }
  if (appimage) {
    lines.push(
      `== ${path.basename(appimage.file)} (${formatSize(appimage.size)}) ==`,
    );
    lines.push(
      `Runtime : type2-runtime ${appimage.runtime.commit ?? "?"}, ${formatSize(appimage.runtime.size)}`,
      `Extraite : ${path.relative(root, appimage.dir)} (${appimage.files.size} fichiers, ` +
        `${formatSize(total(appimage.files))})`,
      `Bibliothèques .so embarquées : ${appimage.libraries.length}, ` +
        `${formatSize(appimage.libraries.reduce((s, l) => s + l.size, 0))}`,
    );
    for (const lib of appimage.libraries) {
      lines.push(`  ${formatSize(lib.size).padStart(10)}  ${lib.rel}`);
    }
    const others = [...appimage.files]
      .filter(
        ([rel, info]) =>
          !info.link && /webkit2gtk-4\.1\/[^/]+$|^usr\/bin\//.test(rel),
      )
      .map(([rel, info]) => `${rel} (${formatSize(info.size)})`);
    if (others.length > 0) lines.push(`Exécutables : ${others.join(", ")}`);
    lines.push(
      `Inventaire LGPL : ${
        appimage.files.has(
          "usr/share/doc/chatonlm/appimage-libraries/libraries.txt",
        )
          ? "présent"
          : "absent (construction locale)"
      }`,
    );
    const bin = binaryLines(appimage.binary, { maxGlibc, depends: null });
    lines.push(...bin.lines);
    const builtin = ollamaLines(appimage.files, appimage.ollamaElf, {
      maxGlibc,
      prefix: APPIMAGE_OLLAMA_PREFIX,
    });
    lines.push(...builtin.lines);
    const images = sdServerLines(appimage.files, appimage.sdServerElf, {
      maxGlibc,
      prefix: APPIMAGE_SD_PREFIX,
    });
    lines.push(...images.lines);
    errors.push(
      ...appimage.errors.map((e) => `AppImage : ${e}`),
      ...bin.errors.map((e) => `AppImage : ${e}`),
      ...builtin.errors.map((e) => `AppImage : ${e}`),
      ...images.errors.map((e) => `AppImage : ${e}`),
    );
    lines.push("");
  }
  const missing = [
    ...new Set([
      ...(deb?.missingTools ?? []),
      ...(appimage?.missingTools ?? []),
    ]),
  ];
  if (missing.length > 0)
    lines.push(`Validateurs absents (non lancés) : ${missing.join(", ")}`);
  lines.push(
    errors.length === 0 ? "✓ Paquets conformes." : "✗ Contrôles en échec :",
  );
  lines.push(...errors.map((e) => `  ${e}`));
  return { text: lines.join("\n"), errors };
}

function main() {
  const args = process.argv.slice(2);
  const options = {
    maxGlibc: null,
    requireInventory: false,
    report: null,
    ollamaSums: path.join(root, ".tools/runtimes/ollama/SHA256SUMS"),
    sdSums: path.join(root, ".tools/runtimes/sd-server/SHA256SUMS"),
  };
  const packages = [];
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--max-glibc") options.maxGlibc = args[++i];
    else if (args[i] === "--require-inventory") options.requireInventory = true;
    else if (args[i] === "--report") options.report = args[++i];
    else if (args[i] === "--ollama-sums") options.ollamaSums = args[++i];
    else if (args[i] === "--sd-sums") options.sdSums = args[++i];
    else packages.push(args[i]);
  }
  const debFile = packages.find((p) => p.endsWith(".deb"));
  const appFile = packages.find((p) => p.endsWith(".AppImage"));
  if ((!debFile && !appFile) || packages.length > 2) {
    console.error(
      "Usage : node scripts/inspect-packages.mjs [--max-glibc x.y] [--require-inventory] " +
        "[--ollama-sums fichier] [--sd-sums fichier] [--report fichier] " +
        "<paquet.deb> <paquet.AppImage>",
    );
    process.exit(2);
  }
  const version = JSON.parse(
    readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8"),
  ).version;
  const pin = parseEnv(
    readFileSync(path.join(root, "packaging/appimage-runtime.env"), "utf8"),
  ).TYPE2_RUNTIME_COMMIT;
  // Ollama intégré : empreintes de la copie extraite par fetch-ollama.sh
  // (vides si elle manque : erreur dans le rapport).
  const ollama = {
    sums: existsSync(options.ollamaSums)
      ? parseSums(readFileSync(options.ollamaSums, "utf8"))
      : new Map(),
    version: parseEnv(
      readFileSync(path.join(root, "packaging/ollama.env"), "utf8"),
    ).OLLAMA_VERSION,
  };
  // Moteur d'images : empreintes et version du dossier construit par
  // build-sd-server.sh (vides s'il manque : erreur dans le rapport).
  const sdVersion = path.join(path.dirname(options.sdSums), "VERSION");
  const sdServer = {
    sums: existsSync(options.sdSums)
      ? parseSums(readFileSync(options.sdSums, "utf8"))
      : new Map(),
    version: existsSync(sdVersion)
      ? readFileSync(sdVersion, "utf8").trim()
      : null,
    pinned: parseEnv(
      readFileSync(path.join(root, "packaging/sdcpp.env"), "utf8"),
    ).SDCPP_VERSION,
  };
  const work = path.join(root, ".tools/package/inspect");
  const deb = debFile
    ? inspectDeb(debFile, {
        work: path.join(work, "deb"),
        version,
        ollama,
        sdServer,
      })
    : null;
  const appimage = appFile
    ? inspectAppImage(appFile, {
        work: path.join(work, "appimage"),
        requireInventory: options.requireInventory,
        runtimePin: pin,
        ollama,
        sdServer,
      })
    : null;
  const { text, errors } = formatReport({
    deb,
    appimage,
    maxGlibc: options.maxGlibc,
  });
  console.log(text);
  if (options.report) writeFileSync(options.report, `${text}\n`);
  process.exit(errors.length === 0 ? 0 : 1);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
