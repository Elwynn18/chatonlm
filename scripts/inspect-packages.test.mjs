// @vitest-environment node
// Essais des scripts d'empaquetage : inspection des paquets
// (inspect-packages.mjs, sur un .deb et une AppDir fabriqués ici), notices
// (package-notices.mjs), inventaire des bibliothèques de l'AppImage
// (appimage-libs.sh : classement des licences, liste, comparaison),
// configuration du bundle (src-tauri/tauri.conf.json), moteur d'images
// (sd-server : contrôles des paquets, rangement dans le bundle), moteur de
// texte (llama-server : de même), aucun fichier CUDA, ROCm ni HIP ni de
// l'ancien Ollama intégré dans les paquets.
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  APPIMAGE_FILES,
  APPIMAGE_SD_PREFIX,
  appImageErrors,
  builtinErrors,
  appImageRuntime,
  COMMON_FILES,
  DEB_FILES,
  debErrors,
  dep5Errors,
  dependsNames,
  desktopErrors,
  elfEnd,
  EXCLUDED_ENGINES,
  excludedEngineErrors,
  inspectDeb,
  APPIMAGE_LLAMA_PREFIX,
  inspectLlamaServerElf,
  LLAMA_COMPONENT,
  LLAMA_FILES,
  LLAMA_LINK,
  LLAMA_LINK_TARGET,
  LLAMA_PREFIX,
  llamaServerErrors,
  llamaServerLines,
  testOnlyErrors,
  TEST_ONLY_MARKERS,
  maxSymbolVersions,
  neededLibraries,
  noticeErrors,
  parseAr,
  parseControl,
  parseDesktop,
  parseSums,
  SD_FILES,
  SD_LINK,
  SD_LINK_TARGET,
  SD_PREFIX,
  sdServerErrors,
  sdServerLines,
  walk,
} from "./inspect-packages.mjs";
import {
  CSS_PACKAGES,
  debCopyright,
  ONNXRUNTIME_DIR,
  onnxRuntimeErrors,
  parseEnv,
  renderAppImageReadme,
  renderNpmPackages,
  renderRustCrates,
  renderSdServerReadme,
  renderSpeechModels,
  renderVoiceModels,
} from "./package-notices.mjs";

// Essais qui lancent des scripts (dpkg factice, AppDir, archives) : plus de
// 5 s quand la machine compile en même temps, d'où le délai explicite.
vi.setConfig({ testTimeout: 30_000 });

const ROOT = path.resolve(import.meta.dirname, "..");
const tmp = path.join(ROOT, ".tools/tmp");
const LIBS = path.join(ROOT, "scripts/appimage-libs.sh");
const ID = "com.chatonlm.chatonlm";
const VISIBLE = readFileSync(
  path.join(ROOT, `packaging/linux/${ID}.desktop`),
  "utf8",
);
const HIDDEN = readFileSync(
  path.join(ROOT, "packaging/linux/chatonlm.desktop.in"),
  "utf8",
)
  .replace("{{exec}}", "chatonlm")
  .replace("{{icon}}", "chatonlm");
const METAINFO = readFileSync(
  path.join(ROOT, `packaging/linux/${ID}.metainfo.xml`),
  "utf8",
);
const CONFIG = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tauri.conf.json"), "utf8"),
);
// Petit exécutable ELF du système (coreutils), présent sous Arch et Ubuntu.
const ELF = "/usr/bin/true";

const dirs = [];
afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

function tempDir(prefix) {
  mkdirSync(tmp, { recursive: true });
  const dir = mkdtempSync(path.join(tmp, prefix));
  dirs.push(dir);
  return dir;
}

/** Arbre de fichiers : chemin → contenu (chaîne) ou { elf: true } ou { link }. */
function tree(dir, files) {
  for (const [rel, content] of Object.entries(files)) {
    const file = path.join(dir, rel);
    mkdirSync(path.dirname(file), { recursive: true });
    if (typeof content === "object" && content.elf) {
      copyFileSync(ELF, file);
      chmodSync(file, 0o755);
    } else if (typeof content === "object" && content.link) {
      symlinkSync(content.link, file);
    } else {
      writeFileSync(file, content);
    }
  }
  return dir;
}

/** Archive ar (format des .deb). */
function ar(members) {
  const parts = [Buffer.from("!<arch>\n")];
  for (const { name, data } of members) {
    const header =
      name.padEnd(16) +
      "0".padEnd(12) +
      "0".padEnd(6) +
      "0".padEnd(6) +
      "100644".padEnd(8) +
      String(data.length).padEnd(10) +
      "`\n";
    parts.push(Buffer.from(header, "latin1"), data);
    if (data.length % 2 === 1) parts.push(Buffer.from("\n"));
  }
  return Buffer.concat(parts);
}

const CONTROL = `Package: chatonlm
Version: 0.1.0
Architecture: amd64
Installed-Size: 70000
Maintainer: Elwynn18 <216957512+Elwynn18@users.noreply.github.com>
Section: utils
Priority: optional
Homepage: https://github.com/Elwynn18/chatonlm
Depends: libasound2, libwebkit2gtk-4.1-0, libgtk-3-0
Description: Unofficial desktop chat client for Ollama
 ChatonLM is an independent, open-source desktop app.
 .
 Second paragraph.
`;

const THIRD_PARTY = "usr/share/doc/chatonlm/THIRD_PARTY_LICENSES";
const RUST_CRATES = "usr/share/doc/chatonlm/licenses/rust-crates.txt";
/** Erreur d'un fichier copyright d'essai qui ne cite pas le modèle livré. */
const SILERO_MISSING =
  "usr/share/doc/chatonlm/copyright : mention absente (Silero VAD)";
/** Crates de l'annulation d'écho, comme les liste cargo-about. */
const SONORA = [
  "sonora",
  "sonora-aec3",
  "sonora-agc2",
  "sonora-common-audio",
  "sonora-fft",
  "sonora-ns",
  "sonora-simd",
];
/** Notice du moteur d'images, réduite à ce que noticeErrors exige. */
const SD_README = "usr/share/doc/chatonlm/licenses/sd-server/README.txt";
const SD_README_TEXT =
  "sd-server of stable-diffusion.cpp\nfallback/libvulkan.so.1: Apache License\n";
/** Notice du moteur de texte, réduite à ce que noticeErrors exige. */
const LLAMA_README = "usr/share/doc/chatonlm/licenses/llama-server/README.txt";
const LLAMA_README_TEXT =
  "llama-server of llama.cpp\nfallback/libvulkan.so.1: Apache License\n";
const crateLines = (names) =>
  `Crates\n${names.map((name) => `  ${name} 0.2.0 (BSD-3-Clause) https://example.org\n`).join("")}`;

/** Fichiers d'un paquet conforme (contenu court, binaire ELF). */
function packageFiles(extra = {}) {
  const files = {};
  for (const rel of COMMON_FILES) files[rel] = "x";
  files[THIRD_PARTY] = "-- Silero VAD --\n";
  files[RUST_CRATES] = crateLines(SONORA);
  files[SD_README] = SD_README_TEXT;
  files[LLAMA_README] = LLAMA_README_TEXT;
  files["usr/bin/chatonlm"] = { elf: true };
  files["usr/share/applications/chatonlm.desktop"] = HIDDEN;
  files["usr/share/doc/chatonlm/copyright"] = debCopyright({
    license: "MIT License\n\nText.\n",
  });
  return { ...files, ...extra };
}

function debFiles(extra = {}) {
  const files = packageFiles({
    [`usr/share/applications/${ID}.desktop`]: VISIBLE,
    [`usr/share/metainfo/${ID}.metainfo.xml`]: METAINFO,
    ...extra,
  });
  for (const rel of DEB_FILES) files[rel] ??= "x";
  return files;
}

/** Vue « fichiers » de debErrors / appImageErrors depuis un objet d'arbre. */
function filesView(files) {
  const map = new Map();
  for (const [rel, content] of Object.entries(files)) {
    map.set(rel, {
      size: 1,
      mode: content?.elf ? 0o100755 : 0o100644,
      link: content?.link ?? null,
    });
  }
  const read = (rel) => (typeof files[rel] === "string" ? files[rel] : "");
  return { files: map, read };
}

describe("downloaded models", () => {
  /** Texte sans les lignes vides (mise en page propre à chaque fichier). */
  const compact = (text) => text.replace(/\n\s*\n/g, "\n");
  /** Attributions : du « Creator: » ou « Each voice » au titre suivant. */
  const blocks = (text) =>
    compact(text)
      .split(/\n(?=[A-Za-z][^\n]*\n-+\n)/)
      .map((block) => block.replace(/^[A-Za-z][^\n]*\n-+\n/, "").trim())
      .filter(
        (block) =>
          block.startsWith("Creator:") || block.startsWith("Each voice"),
      );

  it("gives the same attributions in the notices and THIRD_PARTY_LICENSES", () => {
    const thirdParty = compact(
      readFileSync(path.join(ROOT, "THIRD_PARTY_LICENSES"), "utf8"),
    );
    for (const notices of [renderSpeechModels(), renderVoiceModels()]) {
      const found = blocks(notices);
      expect(found.length).toBeGreaterThanOrEqual(2);
      for (const block of found) expect(thirdParty).toContain(block);
    }
  });
});

describe("formats", () => {
  it("reads the members of an ar archive, padding included", () => {
    const members = parseAr(
      ar([
        { name: "debian-binary", data: Buffer.from("2.0\n") },
        { name: "control.tar.gz", data: Buffer.from("abc") },
        { name: "data.tar.gz", data: Buffer.from("defg") },
      ]),
    );
    expect(members.map((m) => [m.name, m.data.toString()])).toEqual([
      ["debian-binary", "2.0\n"],
      ["control.tar.gz", "abc"],
      ["data.tar.gz", "defg"],
    ]);
    expect(() => parseAr(Buffer.from("not an archive"))).toThrow(/ar invalide/);
  });

  it("parses control fields with continuation lines", () => {
    const control = parseControl(CONTROL);
    expect(control.Package).toBe("chatonlm");
    expect(control.Description).toContain("\n.\nSecond paragraph.");
    expect(
      dependsNames(
        "libc6 (>= 2.35), libasound2 | libasound2t64, libgtk-3-0:amd64",
      ),
    ).toEqual(["libc6", "libasound2", "libasound2t64", "libgtk-3-0"]);
  });

  it("reads desktop entries with localized keys", () => {
    const entry = parseDesktop(VISIBLE)["Desktop Entry"];
    expect(entry.Exec).toBe("chatonlm");
    expect(entry["GenericName[fr]"]).toBe("Client de conversation IA");
  });

  it("finds the end of an ELF file and the runtime commit", () => {
    const header = Buffer.alloc(0x40 + 64);
    header.writeUInt32BE(0x7f454c46, 0);
    header[4] = 2;
    header.write("AI\x02", 8, "latin1");
    header.writeBigUInt64LE(0x40n, 0x28);
    header.writeUInt16LE(64, 0x3a);
    header.writeUInt16LE(1, 0x3c);
    header.write(
      "https://github.com/AppImage/type2-runtime/commit/dd6cebe",
      0x40,
      "latin1",
    );
    expect(elfEnd(header)).toBe(0x80);
    const runtime = appImageRuntime(
      Buffer.concat([header, Buffer.from("hsqs")]),
    );
    expect(runtime).toMatchObject({
      size: 0x80,
      type2: true,
      commit: "dd6cebe",
    });
    expect(() => elfEnd(Buffer.alloc(0x40))).toThrow(/ELF/);
  });

  it("keeps the newest symbol versions, compared as numbers", () => {
    const objdump = [
      "0 DF *UND* 0 (GLIBC_2.9) memcpy",
      "0 DF *UND* 0 (GLIBC_2.17) clock_gettime",
      "0 DF *UND* 0 (GLIBCXX_3.4.9) x",
      "0 DF *UND* 0 (GLIBCXX_3.4.30) y",
      "0 DF *UND* 0 (CXXABI_1.3.11) z",
    ].join("\n");
    expect(maxSymbolVersions(objdump)).toEqual({
      GLIBC: "2.17",
      GLIBCXX: "3.4.30",
      CXXABI: "1.3.11",
    });
    expect(
      neededLibraries(
        " 0x1 (NEEDED)  Shared library: [libasound.so.2]\n 0x1 (NEEDED)  Shared library: [libc.so.6]",
      ),
    ).toEqual(["libasound.so.2", "libc.so.6"]);
  });

  it("reads the pinned AppImage runtime", () => {
    const env = parseEnv(
      readFileSync(path.join(ROOT, "packaging/appimage-runtime.env"), "utf8"),
    );
    expect(env.TYPE2_RUNTIME_COMMIT).toMatch(/^[0-9a-f]{40}$/);
    expect(env.TYPE2_RUNTIME_SHA256).toMatch(/^[0-9a-f]{64}$/);
    expect(env.TYPE2_RUNTIME_URL).toContain(env.TYPE2_RUNTIME_TAG);
    expect(env.LIBFUSE_URL).toContain(env.LIBFUSE_VERSION);
  });
});

describe("desktop entries", () => {
  it("accepts the packaged entries", () => {
    expect(desktopErrors("visible", VISIBLE, { hidden: false })).toEqual([]);
    expect(desktopErrors("hidden", HIDDEN, { hidden: true })).toEqual([]);
  });

  it("requires NoDisplay on the bundler entry only", () => {
    expect(
      desktopErrors("hidden", HIDDEN.replace("NoDisplay=true\n", ""), {
        hidden: true,
      }),
    ).toEqual(["hidden : NoDisplay=true attendu (entrée cachée du bundler)"]);
    expect(
      desktopErrors("visible", `${VISIBLE}NoDisplay=true\n`, { hidden: false }),
    ).toContain("visible : ne doit pas être caché");
  });

  it("requires the Wayland app id and the translations of the menu entry", () => {
    const errors = desktopErrors(
      "visible",
      VISIBLE.replace(/^StartupWMClass=.*\n/m, "").replace(
        /^Comment\[fr\]=.*\n/m,
        "",
      ),
      { hidden: false },
    );
    expect(errors).toEqual([
      `visible : StartupWMClass=${ID} attendu (app_id Wayland)`,
      "visible : Comment[fr] absent",
    ]);
  });
});

describe("testOnlyErrors", () => {
  it("accepts a binary without the test plugin", () => {
    expect(testOnlyErrors(Buffer.from("\x7fELF chatonlm"), "bin")).toEqual([]);
  });

  it("refuses each marker of the WebDriver plugin", () => {
    for (const marker of TEST_ONLY_MARKERS) {
      expect(
        testOnlyErrors(Buffer.from(`\x7fELF…${marker}…`), "/usr/bin/chatonlm"),
      ).toEqual([
        `/usr/bin/chatonlm contient le greffon WebDriver des essais (« ${marker} ») : build avec la fonctionnalité e2e`,
      ]);
    }
  });
});

describe("debErrors", () => {
  const check = (files, control = CONTROL) => {
    const view = filesView(files);
    return debErrors({
      control: parseControl(control),
      ...view,
      version: "0.1.0",
    });
  };

  it("accepts a conforming package", () => {
    expect(check(debFiles())).toEqual([]);
  });

  it("requires the dependencies, the version and the menu entry", () => {
    const control = CONTROL.replace("libasound2, ", "").replace(
      "Version: 0.1.0",
      "Version: 0.2.0",
    );
    const files = debFiles();
    delete files[`usr/share/applications/${ID}.desktop`];
    expect(check(files, control)).toEqual([
      "control : Version: 0.2.0 au lieu de 0.1.0",
      "control : Depends sans libasound2",
      `fichier absent : /usr/share/applications/${ID}.desktop`,
      "2 fichiers .desktop attendus dans /usr/share/applications (1)",
    ]);
  });

  it("refuses the appindicator dependency the Tauri CLI adds for its tray", () => {
    const control = CONTROL.replace(
      "Depends: libasound2, ",
      "Depends: libasound2, libappindicator3-1, libayatana-appindicator3-1, ",
    );
    expect(check(debFiles(), control)).toEqual([
      "control : Depends avec libappindicator3-1 (tray-icon lu par le CLI Tauri)",
      "control : Depends avec libayatana-appindicator3-1 (tray-icon lu par le CLI Tauri)",
    ]);
  });

  it("refuses AppImage-only notices and a binary that cannot run", () => {
    const files = debFiles({
      "usr/share/doc/chatonlm/licenses/LGPL-2.1.txt": "x",
      [`${APPIMAGE_LLAMA_PREFIX}llama-server`]: { elf: true },
      "usr/bin/chatonlm": "not executable",
    });
    expect(check(files)).toEqual([
      "fichier propre à l'AppImage dans le .deb : /usr/share/doc/chatonlm/licenses/LGPL-2.1.txt",
      `fichier propre à l'AppImage dans le .deb : /${APPIMAGE_LLAMA_PREFIX}llama-server`,
      "/usr/bin/chatonlm n'est pas exécutable",
    ]);
  });

  it("refuses any CUDA, ROCm or HIP file, and the old built-in Ollama", () => {
    const cuda = [
      `${LLAMA_PREFIX}libggml-cuda.so`,
      "usr/lib/chatonlm/cuda/libcudart.so.13",
      "usr/lib/x86_64-linux-gnu/libcublasLt.so.13",
      "usr/lib/chatonlm/libamdhip64.so.6",
      `${LLAMA_PREFIX}libggml-hip.so`,
    ];
    const old = [
      "usr/lib/chatonlm/ollama/bin/ollama",
      "usr/share/doc/chatonlm/licenses/ollama/README.txt",
    ];
    const files = debFiles(
      Object.fromEntries([...cuda, ...old].map((rel) => [rel, "x"])),
    );
    expect(check(files)).toEqual([
      ...cuda.map(
        (rel) =>
          `fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : ${rel}`,
      ),
      ...old.map((rel) => `fichier de l'ancien Ollama intégré livré : ${rel}`),
    ]);
    expect(excludedEngineErrors(filesView(debFiles()).files)).toEqual([]);
    // Noms des fichiers du pack CUDA et des bibliothèques de NVIDIA (Linux
    // et Windows), de ROCm/HIP ; aucun fichier livré n'y ressemble.
    for (const name of [
      "libggml-cuda.so",
      "ggml-cuda.dll",
      "libcudart.so.13",
      "cudart64_13.dll",
      "libcublas.so.13",
      "libcublasLt.so.13",
      "cublas64_13.dll",
      "cublasLt64_13.dll",
      "libggml-hip.so",
      "libamdhip64.so.6",
      "librocblas.so.4",
    ]) {
      expect(EXCLUDED_ENGINES.test(name), name).toBe(true);
    }
    for (const rel of [
      ...COMMON_FILES,
      ...DEB_FILES,
      ...APPIMAGE_FILES,
      ...LLAMA_FILES,
      ...SD_FILES,
      "libggml-cpu-haswell.so",
      "libggml-vulkan.so",
      "libmtmd.so.0",
      "chatonlm.png",
      // ICU de l'AppImage (« icudata » contient « cuda » : vu en CI).
      "usr/lib/libicudata.so.70",
      "usr/lib/libicuuc.so.70",
      "usr/share/icons/hicolor/128x128/apps/chatonlm.png",
    ]) {
      expect(EXCLUDED_ENGINES.test(rel), rel).toBe(false);
    }
  });

  it("checks the AppStream launchable and the DEP-5 copyright", () => {
    const files = debFiles({
      [`usr/share/metainfo/${ID}.metainfo.xml`]: METAINFO.replace(
        `${ID}.desktop<`,
        "chatonlm.desktop<",
      ),
      "usr/share/doc/chatonlm/copyright": "Copyright 2026\n",
    });
    expect(check(files)).toEqual([
      "metainfo : launchable différent de l'entrée de menu",
      "copyright : pas au format DEP-5",
      SILERO_MISSING,
    ]);
  });

  it("requires a standalone paragraph for each license of the copyright file", () => {
    const head =
      "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\nUpstream-Name: ChatonLM\n\n";
    const files = (license) =>
      `Files: *\nCopyright: x\nLicense: Expat\n\nFiles: lib/*\nCopyright: y\nLicense: ${license}\nComment: z\n\n`;
    const expat = "License: Expat\n Permission is hereby granted.\n";
    const gpl =
      "License: GPL-3+ with GCC-3.1 exception\n Libgomp is free software.\n";
    expect(
      dep5Errors(
        `${head}${files("GPL-3+ with GCC-3.1 exception")}${gpl}\n${expat}`,
      ),
    ).toEqual([]);
    // Paragraphe autonome absent, ou sans texte.
    expect(
      dep5Errors(`${head}${files("GPL-3+ with GCC-3.1 exception")}${expat}`),
    ).toEqual([
      "copyright : licence « GPL-3+ with GCC-3.1 exception » sans paragraphe autonome",
    ]);
    expect(
      dep5Errors(
        `${head}${files("GPL-3+ with GCC-3.1 exception")}License: GPL-3+ with GCC-3.1 exception\n\n${expat}`,
      ),
    ).toEqual([
      "copyright : paragraphe de la licence « GPL-3+ with GCC-3.1 exception » sans texte",
      "copyright : licence « GPL-3+ with GCC-3.1 exception » sans paragraphe autonome",
    ]);
    // Exception écrite hors de la forme « with <mots-clés> exception ».
    expect(
      dep5Errors(
        `${head}${files("GPL-3+ with GCC-exception-3.1")}License: GPL-3+ with GCC-exception-3.1\n Text.\n\n${expat}`,
      ),
    ).toEqual([
      "copyright : « GPL-3+ with GCC-exception-3.1 » : exception à écrire « with <mots-clés> exception »",
    ]);
    // Licences au choix : chacune a son paragraphe.
    expect(
      dep5Errors(`${head}${files("Expat or Apache-2.0")}${expat}`),
    ).toEqual(["copyright : licence « Apache-2.0 » sans paragraphe autonome"]);
    // Dans le .deb comme dans l'AppImage.
    const broken = `${head}${files("GPL-3+ with GCC-3.1 exception")}${expat}`;
    expect(
      check(debFiles({ "usr/share/doc/chatonlm/copyright": broken })),
    ).toEqual([
      "copyright : licence « GPL-3+ with GCC-3.1 exception » sans paragraphe autonome",
      SILERO_MISSING,
    ]);
  });
});

describe("appImageErrors", () => {
  const appFiles = (extra = {}) => {
    const files = packageFiles({
      [SD_LINK]: { link: SD_LINK_TARGET },
      [LLAMA_LINK]: { link: LLAMA_LINK_TARGET },
      ...extra,
    });
    for (const rel of APPIMAGE_FILES) files[rel] ??= "x";
    // `undefined` dans `extra` : fichier retiré.
    for (const [rel, content] of Object.entries(extra)) {
      if (content === undefined) delete files[rel];
    }
    return files;
  };
  const check = (
    files,
    rootDesktops = ["chatonlm.desktop"],
    requireInventory = false,
  ) => appImageErrors({ ...filesView(files), rootDesktops, requireInventory });

  it("accepts a conforming AppDir", () => {
    expect(check(appFiles())).toEqual([]);
  });

  it("refuses the appindicator library the Tauri CLI copies for its tray", () => {
    expect(
      check(appFiles({ "usr/lib/libayatana-appindicator3.so.1": "x" })),
    ).toEqual([
      "usr/lib/libayatana-appindicator3.so.1 : libappindicator livrée (tray-icon lu par le CLI Tauri)",
    ]);
  });

  it("checks the notices of what the executable embeds", () => {
    expect(check(appFiles({ [THIRD_PARTY]: "x" }))).toEqual([
      `${THIRD_PARTY} : mention absente (Silero VAD)`,
    ]);
  });

  it("checks the DEP-5 copyright file too", () => {
    const copyright = debCopyright({ license: "MIT License\n" }).replace(
      /\nLicense: Expat\n MIT License\n$/,
      "\n",
    );
    expect(
      check(appFiles({ "usr/share/doc/chatonlm/copyright": copyright })),
    ).toEqual(["copyright : licence « Expat » sans paragraphe autonome"]);
  });

  it("refuses a second desktop entry at the root and the deb-only files", () => {
    const files = appFiles({
      [`usr/share/applications/${ID}.desktop`]: VISIBLE,
    });
    expect(check(files, ["chatonlm.desktop", `${ID}.desktop`])).toEqual([
      `un seul .desktop attendu à la racine, chatonlm.desktop (chatonlm.desktop, ${ID}.desktop)`,
      `fichier propre au .deb dans l'AppImage : usr/share/applications/${ID}.desktop`,
    ]);
  });

  it("refuses any CUDA, ROCm or HIP file, and the old built-in Ollama", () => {
    const files = appFiles({
      [`${APPIMAGE_LLAMA_PREFIX}libggml-cuda.so`]: "x",
      "usr/lib/libcudart.so.13": "x",
      "usr/libexec/chatonlm/ollama/bin/ollama": "x",
      "usr/lib/chatonlm/ollama": { link: "../../libexec/chatonlm/ollama" },
    });
    expect(check(files)).toEqual([
      `fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : ${APPIMAGE_LLAMA_PREFIX}libggml-cuda.so`,
      "fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : usr/lib/libcudart.so.13",
      "fichier de l'ancien Ollama intégré livré : usr/libexec/chatonlm/ollama/bin/ollama",
      "fichier de l'ancien Ollama intégré livré : usr/lib/chatonlm/ollama",
    ]);
  });

  it("requires the LGPL texts, and the inventory for a release build", () => {
    const files = appFiles();
    delete files["usr/share/doc/chatonlm/licenses/LGPL-2.1.txt"];
    expect(check(files, ["chatonlm.desktop"], true)).toEqual([
      "fichier absent : usr/share/doc/chatonlm/licenses/LGPL-2.1.txt",
      "inventaire LGPL absent (appimage-libraries/libraries.txt)",
    ]);
  });
});

describe("noticeErrors", () => {
  const check = (files) => noticeErrors(filesView(files));

  it("requires Silero VAD in the notices of both packages", () => {
    for (const rel of [THIRD_PARTY, "usr/share/doc/chatonlm/copyright"]) {
      const errors = check(packageFiles({ [rel]: "No model here.\n" }));
      expect(errors).toEqual([`${rel} : mention absente (Silero VAD)`]);
    }
    const deb = filesView(debFiles({ [THIRD_PARTY]: "x" }));
    expect(
      debErrors({ control: parseControl(CONTROL), ...deb, version: "0.1.0" }),
    ).toEqual([`${THIRD_PARTY} : mention absente (Silero VAD)`]);
  });

  it("requires every sonora crate in the list of Rust crates", () => {
    for (const missing of SONORA) {
      const listed = SONORA.filter((name) => name !== missing);
      const errors = check(packageFiles({ [RUST_CRATES]: crateLines(listed) }));
      expect(errors).toHaveLength(1);
      expect(errors[0]).toContain(`${missing} `);
    }
    // Un nom cité ailleurs que sur sa ligne « nom version » ne compte pas.
    const prose = packageFiles({
      [RUST_CRATES]: `${crateLines(SONORA.slice(1))}Used by: sonora 0.2.0\n`,
    });
    expect(check(prose)).toHaveLength(1);
  });

  it("requires the project and the fallback Vulkan loader in the image engine notice", () => {
    expect(
      check(packageFiles({ [SD_README]: "stable-diffusion.cpp\n" })),
    ).toEqual([
      `${SD_README} : mention absente (fallback\\/libvulkan\\.so\\.1)`,
      `${SD_README} : mention absente (Apache License)`,
    ]);
    // Notice écrite par package-notices.mjs.
    const env = parseEnv(
      readFileSync(path.join(ROOT, "packaging/sdcpp.env"), "utf8"),
    );
    const readme = renderSdServerReadme({ env, licenseFiles: [] });
    expect(check(packageFiles({ [SD_README]: readme }))).toEqual([]);
  });

  it("leaves a missing file to the list of required files", () => {
    const files = packageFiles();
    delete files[RUST_CRATES];
    expect(check(files)).toEqual([]);
  });
});

describe("inspectDeb", () => {
  /** .deb fabriqué dans `dir` avec les fichiers `files` (tree). */
  function buildDeb(dir, files, after = () => {}) {
    const data = tree(path.join(dir, "data"), files);
    after(data);
    const control = tree(path.join(dir, "control"), { control: CONTROL });
    execFileSync("tar", [
      "-czf",
      path.join(dir, "data.tar.gz"),
      "-C",
      data,
      ".",
    ]);
    execFileSync("tar", [
      "-czf",
      path.join(dir, "control.tar.gz"),
      "-C",
      control,
      ".",
    ]);
    const deb = path.join(dir, "chatonlm_0.1.0_amd64.deb");
    writeFileSync(
      deb,
      ar([
        { name: "debian-binary", data: Buffer.from("2.0\n") },
        {
          name: "control.tar.gz",
          data: readFileSync(path.join(dir, "control.tar.gz")),
        },
        {
          name: "data.tar.gz",
          data: readFileSync(path.join(dir, "data.tar.gz")),
        },
      ]),
    );
    return deb;
  }

  it("extracts a .deb and inspects its files and binary", () => {
    const dir = tempDir("inspect-deb-");
    const deb = buildDeb(dir, debFiles());
    const result = inspectDeb(deb, {
      work: path.join(dir, "work"),
      version: "0.1.0",
    });
    expect(result.errors).toEqual([]);
    expect(result.control.Depends).toBe(
      "libasound2, libwebkit2gtk-4.1-0, libgtk-3-0",
    );
    expect(result.files.get("usr/bin/chatonlm").mode & 0o111).not.toBe(0);
    expect(result.binary.needed).toContain("libc.so.6");
    expect(Object.keys(result.binary.versions)).toContain("GLIBC");
    expect(result.sdServerElf).toBeNull();
  });

  it("compares the image engine with the built directory and reads its ELF files", () => {
    const dir = tempDir("inspect-deb-sd-");
    const version = "master-964-1d0a58a+chatonlm.0123456789ab";
    const deb = buildDeb(
      dir,
      debFiles({
        [`${SD_PREFIX}sd-server`]: { elf: true },
        [`${SD_PREFIX}fallback/libvulkan.so.1`]: { elf: true },
        [`${SD_PREFIX}VERSION`]: `${version}\n`,
      }),
      (data) =>
        chmodSync(path.join(data, SD_PREFIX, "fallback/libvulkan.so.1"), 0o644),
    );
    const sum = (rel) =>
      createHash("sha256")
        .update(readFileSync(path.join(dir, "data", SD_PREFIX, rel)))
        .digest("hex");
    const sdServer = {
      sums: new Map(SD_FILES.map((rel) => [rel, sum(rel)])),
      version,
      pinned: "master-964-1d0a58a",
    };
    const result = inspectDeb(deb, {
      work: path.join(dir, "work"),
      version: "0.1.0",
      sdServer,
    });
    expect(result.errors).toEqual([]);
    expect(result.sdServerElf.map((e) => [e.rel, e.elf])).toEqual([
      ["sd-server", true],
      ["fallback/libvulkan.so.1", true],
    ]);
    for (const entry of result.sdServerElf) {
      expect(entry.needed).toContain("libc.so.6");
      expect(entry.linked).toContain("libc.so.6");
      expect(entry.missing).toEqual([]);
      expect(entry.versions.GLIBC).toMatch(/^2\.\d+$/);
    }
    const report = sdServerLines(result.files, result.sdServerElf, {
      maxGlibc: "2.0",
    });
    expect(report.lines[0]).toMatch(
      /^Moteur d'images \(sd-server\) : 3 fichiers, /,
    );
    expect(report.errors).toHaveLength(2);
    expect(report.errors[0]).toMatch(
      /^le moteur d'images \(sd-server\) demande GLIBC_2\.\d+ \(maximum permis : 2\.0\)$/,
    );

    // Autre construction du moteur dans le dossier : le paquet est périmé.
    const stale = inspectDeb(deb, {
      work: path.join(dir, "work"),
      version: "0.1.0",
      sdServer: {
        ...sdServer,
        sums: new Map([...sdServer.sums, ["sd-server", "0".repeat(64)]]),
      },
    });
    expect(stale.errors).toEqual([
      `Moteur d'images : fichier modifié (strip, patchelf ?) : ${SD_PREFIX}sd-server`,
    ]);
  });
});

describe("walk", () => {
  it("lists files and symbolic links, not directories", () => {
    const dir = tree(tempDir("walk-"), {
      "a/b.txt": "bb",
      "a/c": { link: "b.txt" },
    });
    expect([...walk(dir)]).toEqual([
      ["a/b.txt", { size: 2, mode: expect.any(Number), link: null }],
      [
        "a/c",
        { size: expect.any(Number), mode: expect.any(Number), link: "b.txt" },
      ],
    ]);
  });
});

describe("package-notices", () => {
  const MIT =
    "MIT License\n\nCopyright (c) 2026 Elwynn18\n\nPermission is hereby granted.\n";

  it("writes a DEP-5 copyright file with the full license", () => {
    const text = debCopyright({ license: MIT });
    expect(
      text.startsWith(
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n",
      ),
    ).toBe(true);
    expect(text).toContain("Upstream-Name: ChatonLM\n");
    expect(text).toContain(
      "\nFiles: *\nCopyright: 2026 Elwynn18 and ChatonLM contributors\nLicense: Expat\n",
    );
    expect(text).toContain("Portions Copyright (c) Ollama");
    expect(text).toContain(
      "\nLicense: Expat\n MIT License\n .\n Copyright (c) 2026 Elwynn18\n .\n Permission",
    );
    expect(text).not.toContain("appimage-libraries");
    expect(debCopyright({ license: MIT, appimage: true })).toContain(
      "appimage-libraries/README.txt",
    );
  });

  it("lists the MPL-2.0 crates with their source code", () => {
    const pkg = (name, repository) => ({
      name,
      version: "1.0.0",
      id: `${name}@1.0.0`,
      repository,
    });
    const about = {
      overview: [
        { id: "MIT", count: 1 },
        { id: "MPL-2.0", count: 1 },
      ],
      crates: [
        { package: pkg("chatonlm"), license: "MIT" },
        {
          package: pkg("serde", "https://github.com/serde-rs/serde"),
          license: "MIT OR Apache-2.0",
        },
        {
          package: pkg("cssparser", "https://github.com/servo/rust-cssparser"),
          license: "MPL-2.0",
        },
      ],
      licenses: [
        {
          name: "MIT License",
          id: "MIT",
          text: "MIT text",
          used_by: [{ crate: pkg("serde") }],
        },
        {
          name: "Mozilla Public License 2.0",
          id: "MPL-2.0",
          text: "MPL text",
          used_by: [{ crate: pkg("cssparser") }],
        },
      ],
    };
    const text = renderRustCrates(about, { cargoAboutVersion: "0.9.2" });
    const mpl = text
      .split("Crates under the Mozilla Public License 2.0")[1]
      .split("\nCrates\n")[0];
    expect(mpl).toContain(
      "cssparser 1.0.0\n    https://github.com/servo/rust-cssparser\n",
    );
    expect(mpl).toContain(
      "https://crates.io/api/v1/crates/cssparser/1.0.0/download",
    );
    expect(mpl).not.toContain("serde");
    expect(text).toContain("2 crates.");
    expect(text).not.toMatch(/^ {2}chatonlm /m);
    expect(text).toContain("Used by: serde 1.0.0");
    expect(text).toContain("MPL text");
  });

  it("lists npm packages, refers to THIRD_PARTY_LICENSES when a text is missing", () => {
    const text = renderNpmPackages(
      [
        {
          name: "streamdown",
          version: "1.4.0",
          identifier: "Apache-2.0",
          text: "short",
        },
        { name: "remark-math", version: "6.0.0", identifier: "MIT" },
      ],
      {
        css: [
          {
            name: "katex",
            version: "0.16.47",
            identifier: "MIT",
            text: "katex MIT",
          },
          {
            name: "streamdown",
            version: "1.4.0",
            identifier: "Apache-2.0",
            text: "dup",
          },
        ],
        apacheText: "APACHE FULL TEXT",
      },
    );
    expect(text).toContain("3 packages.");
    expect(text).toContain("remark-math 6.0.0 (MIT)");
    expect(text).toContain("see THIRD_PARTY_LICENSES, part 3");
    expect(text).toContain("APACHE FULL TEXT");
    expect(text).not.toContain("dup");
    expect(
      renderNpmPackages(
        [{ name: "a", version: "1", identifier: "MIT", text: "t" }],
        {
          apacheText: "APACHE FULL TEXT",
        },
      ),
    ).not.toContain("APACHE FULL TEXT");
    expect(CSS_PACKAGES).toContain("katex");
  });

  it("requires the pinned ONNX Runtime, fetched with its notices", () => {
    const env = readFileSync(
      path.join(ROOT, "packaging/onnxruntime.env"),
      "utf8",
    );
    const pinned = parseEnv(env).ORT_VERSION;
    expect(pinned).toMatch(/^\d+\.\d+\.\d+$/);
    expect(onnxRuntimeErrors(env, pinned)).toEqual([]);
    expect(onnxRuntimeErrors(env, "1.27.0")[0]).toMatch(
      /^ONNX Runtime 1\.27\.0 dans \.tools\/runtimes\/onnxruntime au lieu de /,
    );
    expect(onnxRuntimeErrors(env, null)[0]).toMatch(/^ONNX Runtime absent /);
    expect(onnxRuntimeErrors("X=1\n", pinned)).toEqual([
      "ORT_VERSION absent de packaging/onnxruntime.env.",
    ]);
    expect(ONNXRUNTIME_DIR).toBe(".tools/runtimes/onnxruntime");
  });

  it("explains how to get the sources and replace a library in the AppImage", () => {
    const runtime = parseEnv(
      readFileSync(path.join(ROOT, "packaging/appimage-runtime.env"), "utf8"),
    );
    const local = renderAppImageReadme({
      version: "0.9.0",
      inventoried: false,
      runtime,
    });
    expect(local).toContain("chatonlm_0.9.0_amd64.AppImage-sources.tar");
    expect(local).toContain(
      "./chatonlm_0.9.0_amd64.AppImage --appimage-extract",
    );
    expect(local).toContain("was not generated");
    expect(local).toContain(`libfuse ${runtime.LIBFUSE_VERSION} (LGPL-2.1`);
    expect(local).toContain(`commit\n${runtime.TYPE2_RUNTIME_COMMIT}`);
    const release = renderAppImageReadme({
      version: "0.9.0",
      inventoried: true,
      runtime,
    });
    expect(release).toContain("libraries.txt lists each library file");
    expect(release).not.toContain("was not generated");
  });
});

describe("bundle configuration", () => {
  const linux = CONFIG.bundle.linux;

  it("generates the notices before bundling", () => {
    expect(CONFIG.build.beforeBundleCommand).toBe(
      "node scripts/package-notices.mjs",
    );
    expect(CONFIG.productName).toBe("chatonlm");
  });

  it("points to files that exist", () => {
    const sources = [
      linux.deb.desktopTemplate,
      ...Object.values(linux.deb.files),
      ...Object.values(linux.appimage.files),
    ].filter((source) => !source.startsWith("../.tools/"));
    for (const source of sources) {
      expect(() =>
        readFileSync(path.resolve(ROOT, "src-tauri", source)),
      ).not.toThrow();
    }
  });

  it("keeps a single desktop entry in the AppImage", () => {
    // linuxdeploy relie à la racine de l'AppDir chaque .desktop qu'il trouve :
    // le second ferait deux entrées à la racine de l'AppImage.
    const appimage = Object.keys(linux.appimage.files);
    expect(appimage.filter((dest) => dest.endsWith(".desktop"))).toEqual([]);
    expect(Object.keys(linux.deb.files)).toContain(
      `/usr/share/applications/${ID}.desktop`,
    );
    expect(linux.deb.files["/usr/share/doc/chatonlm"]).toBe(
      "../.tools/package/notices/deb",
    );
    expect(linux.appimage.files["/usr/share/doc/chatonlm"]).toBe(
      "../.tools/package/notices/appimage",
    );
    expect(linux.deb.depends).toContain("libasound2");
    // Chargeur Vulkan du système, pour les moteurs.
    expect(linux.deb.recommends).toContain("libvulkan1");
  });

  it("installs nothing of the old built-in Ollama", () => {
    for (const files of [linux.deb.files, linux.appimage.files]) {
      for (const [dest, source] of Object.entries(files)) {
        expect(`${dest} ${source}`).not.toMatch(/ollama/i);
      }
    }
  });
});

describe("appimage-libs.sh", () => {
  const classify = (text) => {
    const file = path.join(tempDir("copyright-"), "copyright");
    writeFileSync(file, text);
    return execFileSync(LIBS, ["--classify", file], {
      encoding: "utf8",
    }).trim();
  };
  const dep5 = (...paragraphs) =>
    [
      "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/",
      "Upstream-Name: x",
      "",
      ...paragraphs.map(
        ([files, license]) =>
          `Files: ${files}\nCopyright: someone\nLicense: ${license}\n`,
      ),
    ].join("\n");

  it("classifies machine-readable copyright files", () => {
    expect(classify(dep5(["*", "LGPL-2.1+"], ["tools/*", "GPL-2+"]))).toBe(
      "lgpl",
    );
    expect(classify(dep5(["*", "GPL-2+"]))).toBe("gpl-only");
    expect(classify(dep5(["*", "GPL-2+ or AFL-2.1"]))).toBe("permissive");
    expect(
      classify(dep5(["lib/*", "BSD-2-clause"], ["programs/*", "GPL-2+"])),
    ).toBe("mixed");
    expect(classify(dep5(["*", "GPL-3+ with GCC-exception-3.1"]))).toBe(
      "gpl-exception",
    );
    expect(classify(dep5(["*", "Expat"], ["debian/*", "GPL-2+"]))).toBe(
      "permissive",
    );
    expect(classify(dep5(["*", "Apache-2.0 with LLVM-exception"]))).toBe(
      "permissive",
    );
    expect(classify(dep5(["*", "GPL-2+"], ["doc/*", "GFDL-1.3+"]))).toBe(
      "gpl-only",
    );
    // GPL avec exception dans le texte du paragraphe autonome (libltdl).
    expect(
      classify(
        `${dep5(["*", "GPL-2+"])}\nLicense: GPL-2+\n As a special exception to the GNU General Public\n License, you may include this file.\n`,
      ),
    ).toBe("gpl-exception");
    expect(classify(dep5(["*", "LGPL-2.1 or MPL-1.1"]))).toBe("lgpl");
    expect(classify(dep5(["*", "MPL-2.0"]))).toBe("lgpl");
  });

  it("classifies free-form copyright files by keywords", () => {
    expect(
      classify(
        "This library is under the GNU Lesser General Public License.\n",
      ),
    ).toBe("lgpl");
    expect(
      classify(
        "GNU General Public License, see /usr/share/common-licenses/GPL-2\n",
      ),
    ).toBe("gpl-only");
    expect(
      classify(
        "GNU General Public License for the tools.\nPermission is hereby granted, free of charge\n",
      ),
    ).toBe("mixed");
    expect(classify("Permission is hereby granted, free of charge\n")).toBe(
      "permissive",
    );
    // Mots-clés coupés par un retour à la ligne (cairo), MPL/GPL/LGPL (hunspell).
    expect(
      classify("either the GNU Lesser\nGeneral Public License version 2.1\n"),
    ).toBe("lgpl");
    expect(classify("MPL 1.1/GPL 2.0/LGPL 2.1\n")).toBe("lgpl");
  });

  it("lists the bundled ELF files, without ChatonLM, its launchers and its own components", () => {
    const appdir = tree(tempDir("appdir-"), {
      "usr/lib/libfoo.so.1": { elf: true },
      "usr/lib/gtk-3.0/modules/libbar.so": { elf: true },
      "usr/lib/libfoo.so": { link: "libfoo.so.1" },
      "usr/share/doc/notes.txt": "text",
      "usr/bin/chatonlm": { elf: true },
      // Composants livrés par ChatonLM (ONNX Runtime, moteurs rangés hors de
      // usr/lib dans l'AppImage, derrière un lien).
      "usr/lib/chatonlm/libonnxruntime.so.1": { elf: true },
      "usr/lib/chatonlm/llama-server/llama-server": { elf: true },
      "usr/libexec/chatonlm/llama-server/llama-server": { elf: true },
      "usr/libexec/chatonlm/llama-server/libggml-base.so.0": { elf: true },
      "usr/libexec/chatonlm/sd-server/sd-server": { elf: true },
      AppRun: { elf: true },
      "AppRun.wrapped": { elf: true },
    });
    const list = execFileSync(LIBS, ["--list", appdir], { encoding: "utf8" });
    expect(list).toBe(
      "usr/lib/gtk-3.0/modules/libbar.so\nusr/lib/libfoo.so.1\n",
    );

    const expected = path.join(appdir, "..", `${path.basename(appdir)}.txt`);
    writeFileSync(expected, list);
    dirs.push(expected);
    expect(spawnSync(LIBS, ["--compare", expected, appdir]).status).toBe(0);
    copyFileSync(ELF, path.join(appdir, "usr/lib/libnew.so.2"));
    const changed = spawnSync(LIBS, ["--compare", expected, appdir], {
      encoding: "utf8",
    });
    expect(changed.status).toBe(1);
    expect(changed.stdout).toContain("+usr/lib/libnew.so.2");
  });

  it("refuses an unknown mode", () => {
    expect(spawnSync(LIBS, ["--nope"]).status).toBe(2);
  });

  /**
   * Inventaire avec un faux dpkg (paquet selon le nom du fichier) et de faux
   * dossiers /usr/share/doc et /usr/share/common-licenses.
   */
  function fakeDebian(dir) {
    const bin = path.join(dir, "bin");
    tree(bin, {
      dpkg: `#!/bin/sh
case "$2" in
  */libfoo.so.1) echo "libfoo1:amd64: $2" ;;
  */libbar.so.2) echo "libbar2:amd64: $2" ;;
  */libjbig.so.0) echo "libjbig0:amd64: $2" ;;
  *) exit 1 ;;
esac
`,
      "dpkg-query": `#!/bin/sh
eval "pkg=\\$$#"
case "$pkg" in
  libfoo1:amd64) printf '1.2-3\\tfoo\\t1.2-3\\n' ;;
  libbar2:amd64) printf '2.0-1\\tbar\\t2.0-1\\n' ;;
  libjbig0:amd64) printf '2.1-3.1\\tjbigkit\\t2.1-3.1\\n' ;;
esac
`,
    });
    for (const tool of ["dpkg", "dpkg-query"])
      chmodSync(path.join(bin, tool), 0o755);
    tree(path.join(dir, "doc"), {
      "libfoo1/copyright": `${dep5(["*", "LGPL-2.1+"])}\nLicense: LGPL-2.1+\n See /usr/share/common-licenses/LGPL-2.1.\n`,
      "libbar2/copyright": dep5(["*", "Expat"]),
      "libjbig0/copyright": dep5(["*", "GPL-2+"]),
    });
    tree(path.join(dir, "licenses"), { "LGPL-2.1": "LGPL 2.1 text" });
    return {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      APPIMAGE_LIBS_DOC_ROOT: path.join(dir, "doc"),
      APPIMAGE_LIBS_LICENSES_ROOT: path.join(dir, "licenses"),
    };
  }

  it("inventories the bundled libraries and the sources to publish", () => {
    const dir = tempDir("inventory-");
    const env = fakeDebian(dir);
    const appdir = tree(path.join(dir, "AppDir"), {
      "usr/lib/libfoo.so.1": { elf: true },
      "usr/lib/libbar.so.2": { elf: true },
      "usr/bin/chatonlm": { elf: true },
    });
    const out = path.join(dir, "out");
    const ok = spawnSync(LIBS, [appdir, out], { encoding: "utf8", env });
    expect(ok.stderr).toBe("");
    expect(ok.status).toBe(0);
    expect(readFileSync(path.join(out, "files.txt"), "utf8")).toBe(
      "usr/lib/libbar.so.2\nusr/lib/libfoo.so.1\n",
    );
    // Seul le paquet sous LGPL a son code source publié.
    expect(readFileSync(path.join(out, "sources.txt"), "utf8")).toBe(
      "foo=1.2-3\n",
    );
    const tsv = readFileSync(path.join(out, "packages.tsv"), "utf8");
    expect(tsv).toContain(
      "usr/lib/libfoo.so.1\tlibfoo1\t1.2-3\tfoo\t1.2-3\tlgpl\tLGPL-2.1+\n",
    );
    expect(tsv).toContain(
      "usr/lib/libbar.so.2\tlibbar2\t2.0-1\tbar\t2.0-1\tpermissive\tExpat\n",
    );
    const libraries = readFileSync(path.join(out, "libraries.txt"), "utf8");
    expect(libraries).toContain(
      "usr/lib/libfoo.so.1\n  package:   libfoo1 1.2-3\n  source:    foo 1.2-3\n",
    );
    expect(readFileSync(path.join(out, "copyright/libfoo1"), "utf8")).toContain(
      "LGPL-2.1+",
    );
    expect(
      readFileSync(path.join(out, "common-licenses/LGPL-2.1"), "utf8"),
    ).toBe("LGPL 2.1 text");
  });

  it("fails on a GPL-only library, and on a file without a package", () => {
    const dir = tempDir("inventory-");
    const env = fakeDebian(dir);
    const inventory = (files) => {
      const appdir = path.join(
        dir,
        `AppDir-${Object.keys(files).length}-${Math.random()}`,
      );
      tree(appdir, files);
      const out = path.join(appdir, "out");
      return {
        out,
        ...spawnSync(LIBS, [appdir, out], { encoding: "utf8", env }),
      };
    };
    const gpl = inventory({ "usr/lib/libjbig.so.0": { elf: true } });
    expect(gpl.status).toBe(1);
    expect(gpl.stderr).toContain(
      "Bibliothèque sous GPL seulement : usr/lib/libjbig.so.0 (libjbig0, GPL-2+)",
    );
    // L'inventaire reste écrit pour le diagnostic.
    expect(readFileSync(path.join(gpl.out, "packages.tsv"), "utf8")).toContain(
      "\tgpl-only\t",
    );
    const unknown = inventory({ "usr/lib/libunknown.so.9": { elf: true } });
    expect(unknown.status).toBe(1);
    expect(unknown.stderr).toContain(
      "Paquet introuvable pour usr/lib/libunknown.so.9",
    );
  });
});

describe("parseSums", () => {
  it("reads sha256sum output", () => {
    const a = "0123456789abcdef".repeat(4);
    expect([
      ...parseSums(`${a}  bin/server\n${a} *lib/a b.so\nnoise\n\n`),
    ]).toEqual([
      ["bin/server", a],
      ["lib/a b.so", a],
    ]);
  });
});

describe("image engine (sd-server) in the packages", () => {
  const PINNED = "master-964-1d0a58a";
  const VERSION = `${PINNED}+chatonlm.0123456789ab`;
  // Empreintes du dossier construit : contenu du fichier fabriqué (hash =
  // contenu dans view), VERSION compris.
  const SUMS = new Map([
    ["sd-server", "a".repeat(64)],
    ["fallback/libvulkan.so.1", "b".repeat(64)],
    ["VERSION", VERSION],
  ]);

  /**
   * Vue d'un paquet : le moteur de SUMS sous `prefix` (sd-server seul
   * exécutable), puis `changes` (chemin → {content, mode, link} ou null pour
   * retirer le fichier).
   */
  function view(changes = {}, prefix = SD_PREFIX) {
    const entries = new Map();
    for (const [rel, sum] of SUMS) {
      entries.set(`${prefix}${rel}`, {
        content: sum,
        mode: rel === "sd-server" ? 0o100755 : 0o100644,
      });
    }
    for (const [rel, change] of Object.entries(changes)) {
      if (change === null) entries.delete(rel);
      else entries.set(rel, { ...entries.get(rel), ...change });
    }
    const files = new Map(
      [...entries].map(([rel, e]) => [
        rel,
        { size: 1, mode: e.mode ?? 0o100644, link: e.link ?? null },
      ]),
    );
    const content = (rel) => entries.get(rel)?.content ?? "";
    return { files, hash: content, read: content };
  }
  const check = (
    changes,
    { sums = SUMS, version = VERSION, pinned = PINNED } = {},
  ) => sdServerErrors({ ...view(changes), sums, version, pinned });

  it("accepts the files of the built directory, unchanged", () => {
    expect(check({})).toEqual([]);
  });

  it("refuses a missing, changed or linked file, and any other file", () => {
    expect(
      check({
        [`${SD_PREFIX}sd-server`]: null,
        [`${SD_PREFIX}fallback/libvulkan.so.1`]: { content: "stripped" },
        [`${SD_PREFIX}sd-cli`]: { content: "x" },
        [`${SD_PREFIX}fallback/libvulkan.so`]: { link: "libvulkan.so.1" },
      }),
    ).toEqual([
      `Moteur d'images : fichier absent : ${SD_PREFIX}sd-server`,
      `Moteur d'images : fichier modifié (strip, patchelf ?) : ${SD_PREFIX}fallback/libvulkan.so.1`,
      `Moteur d'images : fichier inattendu : ${SD_PREFIX}sd-cli`,
      `Moteur d'images : fichier inattendu : ${SD_PREFIX}fallback/libvulkan.so -> libvulkan.so.1`,
    ]);
    expect(
      check({ [`${SD_PREFIX}sd-server`]: { link: "../bin/sd-server" } }),
    ).toEqual([`Moteur d'images : lien symbolique : ${SD_PREFIX}sd-server`]);
  });

  it("keeps the executable bit on sd-server only", () => {
    expect(
      check({
        [`${SD_PREFIX}sd-server`]: { mode: 0o100644 },
        [`${SD_PREFIX}fallback/libvulkan.so.1`]: { mode: 0o100755 },
      }),
    ).toEqual([
      `Moteur d'images : ${SD_PREFIX}sd-server n'est pas exécutable`,
      `Moteur d'images : ${SD_PREFIX}fallback/libvulkan.so.1 ne doit pas être exécutable`,
    ]);
  });

  it("requires the version of the built directory, of the pinned release", () => {
    // Paquet construit avec un ancien moteur : VERSION et son empreinte
    // diffèrent de celles du dossier construit.
    const old = `master-963-0000000+chatonlm.0123456789ab`;
    expect(check({ [`${SD_PREFIX}VERSION`]: { content: `${old}\n` } })).toEqual(
      [
        `Moteur d'images : fichier modifié (strip, patchelf ?) : ${SD_PREFIX}VERSION`,
        `Moteur d'images : VERSION ${old} au lieu de ${VERSION}`,
        `Moteur d'images : VERSION ${old} hors de la forme ${PINNED}+chatonlm.<empreinte> (packaging/sdcpp.env)`,
      ],
    );
    // packaging/sdcpp.env changé sans reconstruire le moteur.
    expect(check({}, { pinned: "master-965-abcdef0" })).toEqual([
      `Moteur d'images : VERSION ${VERSION} hors de la forme master-965-abcdef0+chatonlm.<empreinte> (packaging/sdcpp.env)`,
    ]);
    // Dossier construit sans VERSION.
    expect(check({}, { version: null })).toEqual([
      `Moteur d'images : VERSION ${VERSION} au lieu de (inconnue)`,
    ]);
  });

  it("requires the checksums of the built directory", () => {
    expect(check({}, { sums: new Map() })).toEqual([
      "empreintes du moteur d'images absentes (lancer scripts/build-sd-server.sh)",
    ]);
  });

  it("refuses its libraries copied out of its directory, the text engine checks leaving its fallback alone", () => {
    expect(
      check({
        "usr/lib/libvulkan.so.1": {},
        "usr/lib/x86_64-linux-gnu/sd-server": {},
        [`${LLAMA_PREFIX}VERSION`]: {},
      }),
    ).toEqual([
      "bibliothèque du moteur d'images copiée hors de son dossier : usr/lib/libvulkan.so.1",
      "bibliothèque du moteur d'images copiée hors de son dossier : usr/lib/x86_64-linux-gnu/sd-server",
    ]);
    // Le chargeur de secours, dans usr/lib/chatonlm/sd-server/fallback du
    // .deb, n'est pas une copie du chargeur faite par linuxdeploy : le moteur
    // de texte, qui livre le même, ne le prend pas pour une copie du sien.
    const { files } = view();
    files.set(`${LLAMA_PREFIX}fallback/libvulkan.so.1`, {
      size: 1,
      mode: 0o100644,
    });
    expect(
      builtinErrors(LLAMA_COMPONENT, {
        files,
        hash: () => "",
        read: () => "",
        sums: new Map([["fallback/libvulkan.so.1", ""]]),
        version: undefined,
      }),
    ).toEqual([]);
    // Ailleurs dans usr/lib, c'en est une.
    files.set("usr/lib/libvulkan.so.1", { size: 1, mode: 0o100644 });
    expect(
      builtinErrors(LLAMA_COMPONENT, {
        files,
        hash: () => "",
        read: () => "",
        sums: new Map([["fallback/libvulkan.so.1", ""]]),
        version: undefined,
      }),
    ).toEqual([
      "bibliothèque du moteur de texte copiée hors de son dossier : usr/lib/libvulkan.so.1",
    ]);
  });

  it("checks the AppImage copy in usr/libexec", () => {
    const { files, hash, read } = view({}, APPIMAGE_SD_PREFIX);
    files.set(SD_LINK, { size: 1, mode: 0o120777, link: SD_LINK_TARGET });
    expect(
      sdServerErrors({
        files,
        hash,
        read,
        sums: SUMS,
        version: VERSION,
        pinned: PINNED,
        prefix: APPIMAGE_SD_PREFIX,
      }),
    ).toEqual([]);
  });

  it("requires the engine and its notice in both packages", () => {
    for (const rel of SD_FILES) {
      expect(DEB_FILES).toContain(`${SD_PREFIX}${rel}`);
      expect(APPIMAGE_FILES).toContain(`${APPIMAGE_SD_PREFIX}${rel}`);
    }
    expect(SD_FILES).toEqual([
      "sd-server",
      "fallback/libvulkan.so.1",
      "VERSION",
    ]);
    expect(COMMON_FILES).toContain(
      "usr/share/doc/chatonlm/licenses/sd-server/README.txt",
    );
    const deb = debFiles();
    delete deb[`${SD_PREFIX}sd-server`];
    expect(
      debErrors({
        control: parseControl(CONTROL),
        ...filesView(deb),
        version: "0.1.0",
      }),
    ).toEqual([`fichier absent : /${SD_PREFIX}sd-server`]);
  });

  it("keeps the AppImage copy out of the .deb, and the .deb copy out of the AppImage", () => {
    const deb = debFiles({ [`${APPIMAGE_SD_PREFIX}sd-server`]: { elf: true } });
    expect(
      debErrors({
        control: parseControl(CONTROL),
        ...filesView(deb),
        version: "0.1.0",
      }),
    ).toEqual([
      `fichier propre à l'AppImage dans le .deb : /${APPIMAGE_SD_PREFIX}sd-server`,
    ]);
    const appimage = packageFiles({
      [LLAMA_LINK]: { link: LLAMA_LINK_TARGET },
      [`${SD_PREFIX}sd-server`]: { elf: true },
    });
    for (const rel of APPIMAGE_FILES) appimage[rel] ??= "x";
    const check = (files) =>
      appImageErrors({
        ...filesView(files),
        rootDesktops: ["chatonlm.desktop"],
        requireInventory: false,
      });
    // Lien absent et copie dans usr/lib (que linuxdeploy traiterait).
    expect(check(appimage)).toEqual([
      `fichier propre au .deb dans l'AppImage : ${SD_PREFIX}sd-server`,
      `${SD_LINK} : lien vers ${SD_LINK_TARGET} attendu (absent)`,
      `Moteur d'images dans usr/lib (linuxdeploy le traiterait) : ${SD_PREFIX}sd-server`,
    ]);
    delete appimage[`${SD_PREFIX}sd-server`];
    expect(check({ ...appimage, [SD_LINK]: { link: "../sd-server" } })).toEqual(
      [`${SD_LINK} : lien vers ${SD_LINK_TARGET} attendu (../sd-server)`],
    );
    expect(check({ ...appimage, [SD_LINK]: { link: SD_LINK_TARGET } })).toEqual(
      [],
    );
  });

  it("reports the libraries and the glibc of each ELF file, within the limits", () => {
    const { files } = view();
    const entry = (rel, more = {}) => ({
      rel,
      elf: true,
      needed: ["libvulkan.so.1", "libstdc++.so.6", "libc.so.6"],
      versions: { GLIBC: "2.34", GLIBCXX: "3.4.30" },
      missing: [],
      linked: ["libvulkan.so.1", "libstdc++.so.6", "libc.so.6"],
      ...more,
    });
    const elf = [
      entry("sd-server", { missing: ["libvulkan.so.1"] }),
      entry("fallback/libvulkan.so.1", {
        needed: ["libc.so.6"],
        versions: { GLIBC: "2.17" },
        linked: ["libc.so.6"],
      }),
    ];
    const ok = sdServerLines(files, elf, { maxGlibc: "2.35" });
    expect(ok.errors).toEqual([]);
    expect(ok.lines).toEqual([
      "Moteur d'images (sd-server) : 3 fichiers, 3 o",
      "  sd-server : libvulkan.so.1, libstdc++.so.6, libc.so.6 ; versions de symboles les plus récentes : GLIBC_2.34, GLIBCXX_3.4.30",
      "  fallback/libvulkan.so.1 : libc.so.6 ; versions de symboles les plus récentes : GLIBC_2.17",
    ]);
    expect(sdServerLines(files, null, { maxGlibc: "2.35" })).toEqual({
      lines: [],
      errors: [],
    });
    // glibc trop récente, pour le serveur comme pour le chargeur.
    const recent = [
      entry("sd-server", { versions: { GLIBC: "2.38" } }),
      entry("fallback/libvulkan.so.1", { versions: { GLIBC: "2.36" } }),
    ];
    expect(sdServerLines(files, recent, { maxGlibc: "2.35" }).errors).toEqual([
      "le moteur d'images (sd-server) demande GLIBC_2.38 (maximum permis : 2.35)",
      "le moteur d'images (fallback/libvulkan.so.1) demande GLIBC_2.36 (maximum permis : 2.35)",
    ]);
    expect(sdServerLines(files, recent, { maxGlibc: null }).errors).toEqual([]);
  });

  it("refuses libgomp, a missing library other than Vulkan and a file that is not ELF", () => {
    const { files } = view();
    const elf = [
      {
        rel: "sd-server",
        elf: true,
        needed: ["libvulkan.so.1", "libgomp.so.1"],
        versions: { GLIBC: "2.34" },
        missing: ["libvulkan.so.1", "libwebp.so.7"],
        linked: ["libvulkan.so.1", "libgomp.so.1", "libwebp.so.7"],
      },
      {
        rel: "fallback/libvulkan.so.1",
        elf: true,
        needed: ["libc.so.6"],
        versions: {},
        // Le chargeur de secours ne peut pas manquer de lui-même.
        missing: ["libvulkan.so.1"],
        linked: ["libvulkan.so.1"],
      },
    ];
    expect(sdServerLines(files, elf, { maxGlibc: "2.35" }).errors).toEqual([
      "moteur d'images : sd-server : bibliothèques introuvables (ldd) : libwebp.so.7",
      "moteur d'images : sd-server dépend de libgomp (OpenMP devait être coupé)",
      "moteur d'images : fallback/libvulkan.so.1 : bibliothèques introuvables (ldd) : libvulkan.so.1",
    ]);
    // libgomp chargée par une autre bibliothèque (ldd seulement).
    const indirect = [
      {
        ...elf[0],
        needed: ["libc.so.6"],
        missing: [],
        linked: ["libgomp.so.1"],
      },
    ];
    expect(sdServerLines(files, indirect, {}).errors).toEqual([
      "moteur d'images : sd-server dépend de libgomp (OpenMP devait être coupé)",
    ]);
    expect(
      sdServerLines(files, [{ rel: "sd-server", elf: false }], {}).errors,
    ).toEqual([`moteur d'images : ${SD_PREFIX}sd-server n'est pas un ELF`]);
  });
});

describe("image engine in the bundle configuration", () => {
  it("installs sd-server, its fallback loader and VERSION in both packages", () => {
    const { deb, appimage } = CONFIG.bundle.linux;
    const built = "../.tools/runtimes/sd-server";
    const entries = (files, prefix) =>
      Object.fromEntries(
        Object.entries(files).filter(([dest]) => dest.startsWith(`/${prefix}`)),
      );
    expect(entries(deb.files, SD_PREFIX)).toEqual({
      "/usr/lib/chatonlm/sd-server/sd-server": `${built}/sd-server`,
      "/usr/lib/chatonlm/sd-server/fallback": `${built}/fallback`,
      "/usr/lib/chatonlm/sd-server/VERSION": `${built}/VERSION`,
    });
    // AppImage : hors de usr/lib (linuxdeploy), lien posé par package.sh.
    expect(entries(appimage.files, APPIMAGE_SD_PREFIX)).toEqual({
      "/usr/libexec/chatonlm/sd-server/sd-server": `${built}/sd-server`,
      "/usr/libexec/chatonlm/sd-server/fallback": `${built}/fallback`,
      "/usr/libexec/chatonlm/sd-server/VERSION": `${built}/VERSION`,
    });
    expect(entries(appimage.files, SD_PREFIX)).toEqual({});
    expect(entries(deb.files, APPIMAGE_SD_PREFIX)).toEqual({});
    // SHA256SUMS reste dans le dossier construit.
    for (const files of [deb.files, appimage.files]) {
      expect(Object.values(files)).not.toContain(`${built}/SHA256SUMS`);
      expect(Object.values(files)).not.toContain(built);
    }
  });

  it("builds the engine and links it in the AppImage", () => {
    const script = readFileSync(path.join(ROOT, "scripts/package.sh"), "utf8");
    expect(script).toContain(`ln -s ${SD_LINK_TARGET} "$dir/sd-server"`);
    expect(script).toContain(
      `run "Moteur d'images épinglé" scripts/build-sd-server.sh`,
    );
    // Le lien est posé dans la même fonction que celui du moteur de texte ;
    // plus aucun vers l'ancien Ollama intégré.
    const lib = script.split("appimage_lib() {")[1].split("\n}")[0];
    expect(lib).toContain(`ln -s ${SD_LINK_TARGET} "$dir/sd-server"`);
    expect(lib).toContain(`ln -s ${LLAMA_LINK_TARGET} "$dir/llama-server"`);
    expect(script).not.toMatch(/ollama/i);
    // Construit avant l'empaquetage.
    expect(script.indexOf("scripts/build-sd-server.sh")).toBeLessThan(
      script.indexOf('run "Construction ('),
    );
  });

  it("pins the release built by build-sd-server.sh", () => {
    const env = parseEnv(
      readFileSync(path.join(ROOT, "packaging/sdcpp.env"), "utf8"),
    );
    expect(env.SDCPP_VERSION).toMatch(/^master-\d+-[0-9a-f]{7}$/);
    expect(env.SDCPP_COMMIT).toMatch(/^[0-9a-f]{40}$/);
    expect(env.SDCPP_COMMIT.startsWith(env.SDCPP_VERSION.split("-")[2])).toBe(
      true,
    );
  });
});

describe("text engine (llama-server) in the packages", () => {
  const PINNED = "v0.9.0";
  const VERSION = `${PINNED}+chatonlm.0123456789ab`;
  // Empreintes du dossier construit (hash = contenu dans view, sauf VERSION,
  // qui se lit) ; SHA256SUMS, livré, les reprend.
  const SUMS = new Map([
    ["llama-server", "a".repeat(64)],
    ["libllama.so.0", "b".repeat(64)],
    ["libggml-base.so.0", "c".repeat(64)],
    ["libggml-cpu-haswell.so", "d".repeat(64)],
    ["fallback/libvulkan.so.1", "e".repeat(64)],
    ["VERSION", "f".repeat(64)],
  ]);
  const SUMS_TEXT = [...SUMS].map(([rel, sum]) => `${sum}  ${rel}\n`).join("");

  /** Vue d'un paquet : le moteur de SUMS sous `prefix`, puis `changes`. */
  function view(changes = {}, prefix = LLAMA_PREFIX) {
    const entries = new Map();
    for (const [rel, sum] of SUMS) {
      entries.set(`${prefix}${rel}`, {
        content: rel === "VERSION" ? `${VERSION}\n` : sum,
        sum,
        mode: rel === "llama-server" ? 0o100755 : 0o100644,
      });
    }
    entries.set(`${prefix}SHA256SUMS`, { content: SUMS_TEXT, mode: 0o100644 });
    for (const [rel, change] of Object.entries(changes)) {
      if (change === null) entries.delete(rel);
      else entries.set(rel, { ...entries.get(rel), ...change });
    }
    const files = new Map(
      [...entries].map(([rel, e]) => [
        rel,
        { size: 1, mode: e.mode ?? 0o100644, link: e.link ?? null },
      ]),
    );
    const content = (rel) => entries.get(rel)?.content ?? "";
    const hash = (rel) =>
      rel.endsWith("/VERSION") ? entries.get(rel)?.sum : content(rel);
    return { files, hash, read: content };
  }
  const check = (
    changes,
    { sums = SUMS, version = VERSION, pinned = PINNED, prefix } = {},
  ) =>
    llamaServerErrors({
      ...view(changes, prefix),
      sums,
      version,
      pinned,
      ...(prefix ? { prefix } : {}),
    });

  it("accepts the files of the built directory, with their checksums", () => {
    expect(check({})).toEqual([]);
  });

  it("refuses a missing, changed or extra file, a CUDA backend and a stray executable bit", () => {
    expect(
      check({
        [`${LLAMA_PREFIX}libllama.so.0`]: null,
        [`${LLAMA_PREFIX}libggml-base.so.0`]: { content: "stripped" },
        [`${LLAMA_PREFIX}libggml-cpu-old.so`]: { content: "x" },
        [`${LLAMA_PREFIX}libggml-cuda.so`]: { content: "x" },
        [`${LLAMA_PREFIX}libggml-cpu-haswell.so`]: { mode: 0o100755 },
      }),
    ).toEqual([
      `Moteur de texte : fichier absent : ${LLAMA_PREFIX}libllama.so.0`,
      `Moteur de texte : fichier modifié (strip, patchelf ?) : ${LLAMA_PREFIX}libggml-base.so.0`,
      `Moteur de texte : fichier inattendu : ${LLAMA_PREFIX}libggml-cpu-old.so`,
      `Moteur de texte : moteur exclu livré : ${LLAMA_PREFIX}libggml-cuda.so`,
      `Moteur de texte : ${LLAMA_PREFIX}libggml-cpu-haswell.so ne doit pas être exécutable`,
    ]);
    expect(
      check({ [`${LLAMA_PREFIX}llama-server`]: { mode: 0o100644 } }),
    ).toEqual([
      `Moteur de texte : ${LLAMA_PREFIX}llama-server n'est pas exécutable`,
    ]);
  });

  it("requires the pinned version and the shipped checksums of the built directory", () => {
    expect(check({}, { pinned: "v0.9.1" })).toEqual([
      `Moteur de texte : VERSION ${VERSION} hors de la forme v0.9.1+chatonlm.<empreinte> (packaging/llama.env)`,
    ]);
    expect(
      check({
        [`${LLAMA_PREFIX}SHA256SUMS`]: { content: SUMS_TEXT.split("\n")[0] },
      }),
    ).toEqual([
      `Moteur de texte : ${LLAMA_PREFIX}SHA256SUMS différent des empreintes de référence`,
    ]);
    expect(check({}, { sums: new Map() })).toEqual([
      "empreintes du moteur de texte absentes (lancer scripts/build-llama-server.sh)",
    ]);
  });

  it("checks the AppImage copy in usr/libexec, behind a link", () => {
    expect(check({}, { prefix: APPIMAGE_LLAMA_PREFIX })).toEqual([]);
    for (const rel of LLAMA_FILES) {
      expect(DEB_FILES).toContain(`${LLAMA_PREFIX}${rel}`);
      expect(APPIMAGE_FILES).toContain(`${APPIMAGE_LLAMA_PREFIX}${rel}`);
    }
    const appimage = packageFiles({
      [SD_LINK]: { link: SD_LINK_TARGET },
      [`${LLAMA_PREFIX}llama-server`]: { elf: true },
    });
    for (const rel of APPIMAGE_FILES) appimage[rel] ??= "x";
    expect(
      appImageErrors({
        ...filesView(appimage),
        rootDesktops: ["chatonlm.desktop"],
        requireInventory: false,
      }),
    ).toEqual([
      `fichier propre au .deb dans l'AppImage : ${LLAMA_PREFIX}llama-server`,
      `${LLAMA_LINK} : lien vers ${LLAMA_LINK_TARGET} attendu (absent)`,
      `Moteur de texte dans usr/lib (linuxdeploy le traiterait) : ${LLAMA_PREFIX}llama-server`,
    ]);
  });

  it("refuses libraries out of the base system, a missing library other than Vulkan and a recent glibc", () => {
    const { files } = view();
    const entry = (rel, more = {}) => ({
      rel,
      needed: ["libggml-base.so.0", "libstdc++.so.6", "libc.so.6"],
      versions: { GLIBC: "2.34", GLIBCXX: "3.4.30" },
      missing: [],
      linked: ["libggml-base.so.0", "libstdc++.so.6", "libc.so.6"],
      ...more,
    });
    const ok = llamaServerLines(
      files,
      [
        entry("llama-server"),
        entry("libggml-vulkan.so", {
          needed: ["libvulkan.so.1", "libggml-base.so.0"],
          missing: ["libvulkan.so.1"],
        }),
      ],
      { maxGlibc: "2.35" },
    );
    expect(ok.errors).toEqual([]);
    expect(ok.lines).toEqual([
      "Moteur de texte (llama-server) : 7 fichiers, 7 o, 2 ELF ; versions de symboles les plus récentes : GLIBC_2.34, GLIBCXX_3.4.30",
    ]);
    const bad = llamaServerLines(
      files,
      [
        entry("libllama-common.so.0", {
          needed: ["libssl.so.3", "libggml-base.so.0"],
          linked: ["libssl.so.3", "libcrypto.so.3"],
        }),
        entry("libggml-cpu-haswell.so", {
          linked: ["libgomp.so.1"],
          missing: ["libfoo.so.1"],
          versions: { GLIBC: "2.38" },
        }),
      ],
      { maxGlibc: "2.35" },
    );
    expect(bad.errors).toEqual([
      "le moteur de texte demande GLIBC_2.38 (maximum permis : 2.35)",
      "moteur de texte : libllama-common.so.0 dépend de libssl.so.3, libcrypto.so.3 (hors du système de base)",
      "moteur de texte : libggml-cpu-haswell.so : bibliothèques introuvables (ldd) : libfoo.so.1",
      "moteur de texte : libggml-cpu-haswell.so dépend de libgomp.so.1 (hors du système de base)",
    ]);
    expect(llamaServerLines(files, [], {}).errors).toEqual([
      `moteur de texte : aucun fichier ELF dans ${LLAMA_PREFIX}`,
    ]);
    expect(llamaServerLines(files, null, {})).toEqual({
      lines: [],
      errors: [],
    });
  });

  it("reads every ELF file of its directory, and only those", () => {
    const dir = tree(tempDir("llama-elf-"), {
      [`${LLAMA_PREFIX}llama-server`]: { elf: true },
      [`${LLAMA_PREFIX}fallback/libvulkan.so.1`]: { elf: true },
      [`${LLAMA_PREFIX}VERSION`]: VERSION,
      [`${SD_PREFIX}sd-server`]: { elf: true },
    });
    const entries = inspectLlamaServerElf(dir, walk(dir));
    expect(entries.map((entry) => entry.rel)).toEqual([
      "fallback/libvulkan.so.1",
      "llama-server",
    ]);
    expect(entries[1].needed).toContain("libc.so.6");
    expect(entries[1].missing).toEqual([]);
  });
});

describe("text engine in the bundle configuration", () => {
  it("installs the whole built directory in both packages, the AppImage copy out of usr/lib", () => {
    const { deb, appimage } = CONFIG.bundle.linux;
    const built = "../.tools/runtimes/llama-server";
    const entries = (files, dest) =>
      Object.fromEntries(
        Object.entries(files).filter(
          ([to, from]) => to === dest || from === built,
        ),
      );
    expect(entries(deb.files, `/${LLAMA_LINK}`)).toEqual({
      [`/${LLAMA_LINK}`]: built,
    });
    expect(
      entries(appimage.files, `/${APPIMAGE_LLAMA_PREFIX.slice(0, -1)}`),
    ).toEqual({
      [`/${APPIMAGE_LLAMA_PREFIX.slice(0, -1)}`]: built,
    });
  });

  it("builds the engine after sd-server and links it in the AppImage", () => {
    const script = readFileSync(path.join(ROOT, "scripts/package.sh"), "utf8");
    const lib = script.split("appimage_lib() {")[1].split("\n}")[0];
    expect(lib).toContain(`ln -s ${LLAMA_LINK_TARGET} "$dir/llama-server"`);
    const sd = script.indexOf("scripts/build-sd-server.sh\n");
    const llama = script.indexOf(
      `run "Moteur de texte épinglé" scripts/build-llama-server.sh`,
    );
    expect(sd).toBeGreaterThan(0);
    expect(llama).toBeGreaterThan(sd);
  });
});
