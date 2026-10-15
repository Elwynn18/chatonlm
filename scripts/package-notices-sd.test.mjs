// @vitest-environment node
// Essais des notices du moteur d'images (sd-server de stable-diffusion.cpp)
// écrites par package-notices.mjs (Linux) et package-notices-windows.mjs :
// contrôles du dossier construit, empreinte de la construction (comme
// scripts/build-sd-server.sh), README, textes copiés, paragraphes DEP-5 et
// partie 10 de THIRD_PARTY_LICENSES. Le dossier construit n'est jamais lu :
// les essais fabriquent leur propre arborescence.
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { dep5Errors } from "./inspect-packages.mjs";
import {
  debCopyright,
  parseEnv,
  readSdServer,
  renderSdServerReadme,
  SD_SERVER_DIR,
  SD_SERVER_EXTRA_TEXTS,
  SD_SERVER_LICENSE_DIR,
  SD_SERVER_LICENSE_SUMMARY,
  SD_SERVER_LICENSES,
  SD_SERVER_PATCHES,
  sdServerFingerprint,
  sdServerNoticeErrors,
} from "./package-notices.mjs";
import { renderWindowsComponents } from "./package-notices-windows.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const ENV = readFileSync(path.join(ROOT, "packaging/sdcpp.env"), "utf8");
const env = parseEnv(ENV);
const LICENSE_FILES = Object.keys(SD_SERVER_LICENSES);
const PATCHES = Object.keys(SD_SERVER_PATCHES);
const FINGERPRINT = "0123456789ab";
const BUILT = `${env.SDCPP_VERSION}+chatonlm.${FINGERPRINT}`;
/** Fins de ligne LF, comme les fichiers du dépôt (.gitattributes). */
const lf = (text) => text.replace(/\r\n/g, "\n");
/** Sources épinglées de stable-diffusion.cpp (copie locale, facultative). */
const SOURCES = path.join(ROOT, ".tools/sdcpp/src");

const tmp = path.join(ROOT, ".tools/tmp");
const dirs = [];
afterEach(() => {
  while (dirs.length > 0) rmSync(dirs.pop(), { recursive: true, force: true });
});

/**
 * Dépôt factice : packaging/sdcpp.env, correctifs, textes de licence,
 * Apache-2.0, script de construction et, si `version` n'est pas null, le
 * VERSION du dossier construit. `omit` : textes de licence à ne pas écrire.
 */
function fakeRepo({ version = BUILT, omit = [], patches = PATCHES } = {}) {
  mkdirSync(tmp, { recursive: true });
  const dir = mkdtempSync(path.join(tmp, "sd-notices-"));
  dirs.push(dir);
  const put = (rel, text) => {
    mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
    writeFileSync(path.join(dir, rel), text);
  };
  put("packaging/sdcpp.env", ENV);
  for (const patch of patches) put(`packaging/sdcpp/${patch}`, `${patch}\n`);
  for (const file of LICENSE_FILES) {
    if (!omit.includes(file)) {
      put(`${SD_SERVER_LICENSE_DIR}/${file}`, `texte de ${file}\n`);
    }
  }
  for (const source of Object.values(SD_SERVER_EXTRA_TEXTS)) {
    put(source, `texte de ${source}\n`);
  }
  put("scripts/build-sd-server.sh", "#!/usr/bin/env bash\necho essai\n");
  if (version !== null) put(`${SD_SERVER_DIR}/VERSION`, `${version}\n`);
  return dir;
}

/** Empreinte calculée comme scripts/build-sd-server.sh (cat | sha256sum). */
function shellFingerprint(dir) {
  return execFileSync(
    "bash",
    [
      "-c",
      'cd "$1" && cat packaging/sdcpp.env packaging/sdcpp/*.patch ' +
        "scripts/build-sd-server.sh | sha256sum | cut -c1-12",
      "bash",
      dir,
    ],
    { encoding: "utf8" },
  ).trim();
}

describe("built-in image engine: checks", () => {
  it("pins the sources of sd-server", () => {
    expect(env.SDCPP_VERSION).toMatch(/^master-\d+-[0-9a-f]{7}$/);
    expect(env.SDCPP_COMMIT).toMatch(/^[0-9a-f]{40}$/);
    expect(env.SDCPP_VERSION.endsWith(env.SDCPP_COMMIT.slice(0, 7))).toBe(true);
    expect(env.SDCPP_GGML_COMMIT).toMatch(/^[0-9a-f]{40}$/);
    expect(env.SDCPP_REPO).toBe(
      "https://github.com/leejet/stable-diffusion.cpp",
    );
    expect(SD_SERVER_DIR).toBe(".tools/runtimes/sd-server");
  });

  it("accepts the pinned build with every license text", () => {
    expect(
      sdServerNoticeErrors(ENV, BUILT, {
        licenseFiles: LICENSE_FILES,
        patches: PATCHES,
        fingerprint: FINGERPRINT,
      }),
    ).toEqual([]);
  });

  it("refuses a missing or inconsistent build", () => {
    const options = { licenseFiles: LICENSE_FILES, patches: PATCHES };
    expect(sdServerNoticeErrors(ENV, null, options)).toEqual([
      "sd-server absent de .tools/runtimes/sd-server : lancer scripts/build-sd-server.sh.",
    ]);
    expect(
      sdServerNoticeErrors(
        ENV,
        "master-1-abcdef0+chatonlm.0123456789ab",
        options,
      ),
    ).toEqual([
      `sd-server master-1-abcdef0+chatonlm.0123456789ab dans .tools/runtimes/sd-server au lieu de ${env.SDCPP_VERSION}+chatonlm.<empreinte> : lancer scripts/build-sd-server.sh.`,
    ]);
    // Version épinglée sans l'empreinte de ChatonLM : pas notre construction.
    expect(sdServerNoticeErrors(ENV, env.SDCPP_VERSION, options)).toHaveLength(
      1,
    );
    // Empreinte d'autres correctifs ou d'un autre script.
    expect(
      sdServerNoticeErrors(ENV, BUILT, {
        ...options,
        fingerprint: "ba9876543210",
      }),
    ).toEqual([
      `sd-server ${BUILT} dans .tools/runtimes/sd-server construit avec d'autres correctifs ou un autre script (empreinte attendue ba9876543210) : lancer scripts/build-sd-server.sh.`,
    ]);
    expect(sdServerNoticeErrors("X=1\n", BUILT, options)).toEqual([
      "SDCPP_VERSION absent de packaging/sdcpp.env.",
      "SDCPP_REPO absent de packaging/sdcpp.env.",
      "SDCPP_COMMIT absent de packaging/sdcpp.env.",
      "SDCPP_GGML_COMMIT absent de packaging/sdcpp.env.",
    ]);
  });

  it("refuses a missing license text and an undescribed patch", () => {
    expect(
      sdServerNoticeErrors(ENV, BUILT, {
        licenseFiles: LICENSE_FILES.filter((f) => f !== "tokenizer-data.txt"),
        patches: PATCHES,
      }),
    ).toEqual([
      "Licence du moteur d'images absente : packaging/sdcpp/licenses/tokenizer-data.txt",
    ]);
    expect(
      sdServerNoticeErrors(ENV, BUILT, {
        licenseFiles: LICENSE_FILES,
        patches: [...PATCHES, "0003-new.patch"].slice(1),
      }),
    ).toEqual([
      "Correctif packaging/sdcpp/0003-new.patch sans résumé dans SD_SERVER_PATCHES (scripts/package-notices.mjs).",
      `Correctif résumé mais absent : packaging/sdcpp/${PATCHES[0]}`,
    ]);
  });

  it("computes the fingerprint of the build script", () => {
    const dir = fakeRepo();
    const state = readSdServer(dir);
    expect(state.fingerprint).toBe(shellFingerprint(dir));
    expect(
      sdServerFingerprint({ env: "a", patches: ["b", "c"], script: "d" }),
    ).toBe(sdServerFingerprint({ env: "ab", patches: [], script: "cd" }));
    // Construction faite avec ces fichiers : aucune erreur.
    const built = fakeRepo({
      version: `${env.SDCPP_VERSION}+chatonlm.${shellFingerprint(dir)}`,
    });
    expect(readSdServer(built).errors).toEqual([]);
    // Autre empreinte (correctif ou script modifié depuis la construction).
    expect(readSdServer(dir).errors).toHaveLength(1);
    expect(readSdServer(dir).errors[0]).toContain(
      `(empreinte attendue ${state.fingerprint})`,
    );
  });

  it("reads a missing build and a missing text as errors", () => {
    expect(readSdServer(fakeRepo({ version: null })).errors).toEqual([
      "sd-server absent de .tools/runtimes/sd-server : lancer scripts/build-sd-server.sh.",
    ]);
    const dir = fakeRepo({ omit: ["ggml-LICENSE.txt"] });
    expect(readSdServer(dir).errors).toContain(
      "Licence du moteur d'images absente : packaging/sdcpp/licenses/ggml-LICENSE.txt",
    );
  });

  it("matches the build script and the patches of the repository", () => {
    const state = readSdServer(ROOT);
    expect(state.fingerprint).toBe(shellFingerprint(ROOT));
    expect(state.patches).toEqual(PATCHES);
    for (const file of LICENSE_FILES)
      expect(state.licenseFiles).toContain(file);
    // Seule erreur admise ici : le dossier construit (absent ou périmé).
    for (const error of state.errors) {
      expect(error).toMatch(
        /^sd-server .*lancer scripts\/build-sd-server\.sh\.$/,
      );
    }
    // Même formule que le script de construction.
    const script = readFileSync(
      path.join(ROOT, "scripts/build-sd-server.sh"),
      "utf8",
    );
    expect(script).toContain(
      'fingerprint="$(cat "$env_file" "$patches_dir"/*.patch "${BASH_SOURCE[0]}" | sha256 | cut -c1-12)"',
    );
    expect(script).toContain('version="$SDCPP_VERSION+chatonlm.$fingerprint"');
  });
});

describe("built-in image engine: notices", () => {
  it("copies every license text, and the Apache License", () => {
    const dir = fakeRepo();
    const { texts } = readSdServer(dir);
    expect(Object.keys(texts).sort()).toEqual(
      [...LICENSE_FILES, ...Object.keys(SD_SERVER_EXTRA_TEXTS)].sort(),
    );
    expect(texts["ggml-LICENSE.txt"]).toBe("texte de ggml-LICENSE.txt\n");
    expect(texts["Apache-2.0.txt"]).toBe(
      "texte de packaging/licenses/Apache-2.0.txt\n",
    );
    const real = readSdServer(ROOT).texts;
    expect(real["Apache-2.0.txt"]).toContain(
      "Apache License\n                           Version 2.0, January 2004",
    );
  });

  it.skipIf(!existsSync(SOURCES))(
    "keeps the license files of the pinned sources unchanged",
    () => {
      const copies = {
        "stable-diffusion.cpp-LICENSE.txt": "LICENSE",
        "ggml-LICENSE.txt": "ggml/LICENSE",
        "darts-clone-LICENSE.txt": "thirdparty/LICENSE.darts_clone.txt",
        "oniguruma-COPYING.txt": "thirdparty/oniguruma/COPYING",
        "utf8proc-LICENSE.txt": "thirdparty/utf8proc/LICENSE.md",
      };
      for (const [file, source] of Object.entries(copies)) {
        expect(
          readFileSync(path.join(ROOT, SD_SERVER_LICENSE_DIR, file), "utf8"),
        ).toBe(lf(readFileSync(path.join(SOURCES, source), "utf8")));
      }
      // En-têtes repris tels quels.
      const text = (file) =>
        readFileSync(path.join(ROOT, SD_SERVER_LICENSE_DIR, file), "utf8");
      const source = (file) =>
        lf(readFileSync(path.join(SOURCES, "thirdparty", file), "utf8"));
      expect(text("cpp-httplib-LICENSE.txt")).toContain(
        source("httplib.h").split("\n").slice(0, 6).join("\n"),
      );
      expect(text("zip-LICENSE.txt")).toContain(
        source("zip.h").split("\n").slice(0, 9).join("\n"),
      );
      const stb = source("stb_image.h");
      const block = stb.slice(
        stb.lastIndexOf(
          "------------------------------------------------------------------------------\nThis software is available under 2 licenses",
        ),
        stb.lastIndexOf("*/"),
      );
      expect(text("stb-LICENSE.txt")).toContain(block);
      expect(source("stb_image_write.h")).toContain(block);
    },
  );

  it("describes the build, the patches and the licenses (Linux)", () => {
    const licenseFiles = [...LICENSE_FILES, "EXTRA.txt"];
    const text = renderSdServerReadme({ env, licenseFiles });
    expect(text).toMatch(/^ChatonLM - Built-in image engine \(sd-server\)\n/);
    expect(text).toContain(`Version:      ${env.SDCPP_VERSION}\n`);
    expect(text).toContain(`Commit:       ${env.SDCPP_COMMIT}\n`);
    expect(text).toContain(`${env.SDCPP_REPO}/tree/${env.SDCPP_COMMIT}\n`);
    expect(text).toContain(
      `https://github.com/leejet/ggml/tree/${env.SDCPP_GGML_COMMIT}\n`,
    );
    expect(text).toContain("/usr/lib/chatonlm/sd-server");
    expect(text).toContain("usr/libexec/chatonlm/sd-server");
    expect(text).toContain("  fallback/libvulkan.so.1\n");
    expect(text.replace(/\s+/g, " ")).toContain(
      "without OpenMP, without WebP and WebM",
    );
    expect(text).not.toContain("sd-server.exe");
    for (const file of [
      ...licenseFiles,
      ...Object.keys(SD_SERVER_EXTRA_TEXTS),
    ]) {
      expect(text).toContain(`\n  ${file}\n`);
    }
    for (const file of PATCHES) expect(text).toContain(`\n  ${file}\n`);
    expect(text.replace(/\s+/g, " ")).toContain(
      "refuses any request that carries an Origin header",
    );
    expect(text.replace(/\s+/g, " ")).toContain(SD_SERVER_LICENSE_SUMMARY);
    expect(text).toContain("Vulkan-Loader-components.txt");
    for (const line of text.split("\n")) {
      if (!line.includes("https://")) {
        expect(line.length).toBeLessThanOrEqual(78);
      }
    }
  });

  it("describes the Windows build", () => {
    const text = renderSdServerReadme({
      env,
      licenseFiles: LICENSE_FILES,
      windows: true,
    });
    expect(text).toContain("  sd-server.exe\n");
    expect(text).toContain("  fallback\\vulkan-1.dll\n");
    expect(text).toContain("the sd-server folder, next to chatonlm.exe");
    expect(text).toContain("static Microsoft Visual C++");
    expect(text).not.toContain("/usr/lib/chatonlm");
    expect(text).not.toContain("libvulkan.so.1");
    for (const line of text.split("\n")) {
      if (!line.includes("https://")) {
        expect(line.length).toBeLessThanOrEqual(78);
      }
    }
    expect(renderWindowsComponents()).toContain("sd-server\\sd-server.exe");
  });

  const SD = { version: env.SDCPP_VERSION, commit: env.SDCPP_COMMIT };
  const LICENSE = "MIT License\n\nText.\n";

  it("adds the sd-server paragraphs to the DEP-5 copyright of the .deb", () => {
    const text = debCopyright({ license: LICENSE, sdServer: SD });
    expect(dep5Errors(text)).toEqual([]);
    expect(text).toContain(
      "\nFiles: usr/lib/chatonlm/sd-server/*\nCopyright: 2023 leejet, the ggml authors, and others\nLicense: Expat\nComment: Image engine of ChatonLM: sd-server of stable-diffusion.cpp\n",
    );
    expect(text.replace(/\n /g, " ")).toContain(
      `stable-diffusion.cpp ${SD.version} (https://github.com/leejet/stable-diffusion.cpp, commit ${SD.commit})`,
    );
    expect(text.replace(/\n /g, " ")).toContain(SD_SERVER_LICENSE_SUMMARY);
    expect(text).toContain(
      "\nFiles: usr/lib/chatonlm/sd-server/fallback/*\nCopyright: The Khronos Group Inc., Valve Corporation, LunarG, Inc.,\n and others\nLicense: Apache-2.0\n",
    );
    // Paragraphe autonome de l'Apache-2.0, avec le renvoi au texte de Debian.
    expect(text).toContain(
      "\n\nLicense: Apache-2.0\n Licensed under the Apache",
    );
    expect(text).toContain("/usr/share/common-licenses/Apache-2.0.");
    expect(text).toContain("licenses/sd-server/Apache-2.0.txt");
    // Paragraphe autonome de la licence Expat toujours à la fin.
    expect(text.trimEnd().endsWith(" Text.")).toBe(true);
    expect(text).toContain(
      "the image engine (sd-server of stable-diffusion.cpp)",
    );
    for (const line of text.split("\n")) {
      expect(line.length).toBeLessThanOrEqual(80);
    }
    // Sans moteur d'images, ni ses paragraphes ni l'Apache-2.0.
    const without = debCopyright({ license: LICENSE });
    expect(without).not.toContain("chatonlm/sd-server");
    expect(without).not.toContain("License: Apache-2.0");
    expect(dep5Errors(without)).toEqual([]);
  });

  it("puts sd-server under usr/libexec in the AppImage copyright", () => {
    const text = debCopyright({
      license: LICENSE,
      appimage: true,
      sdServer: SD,
    });
    expect(dep5Errors(text)).toEqual([]);
    expect(text).toContain("\nFiles: usr/libexec/chatonlm/sd-server/*\n");
    expect(text).toContain(
      "\nFiles: usr/libexec/chatonlm/sd-server/fallback/*\n",
    );
    expect(text).not.toContain("Files: usr/lib/chatonlm/sd-server");
    expect(text).not.toContain("/usr/share/common-licenses");
    expect(text).toContain("licenses/sd-server/Apache-2.0.txt");
  });

  it("checks that a license named by a paragraph has its text", () => {
    // Sans le paragraphe autonome de l'Apache-2.0, le fichier est refusé.
    const text = debCopyright({ license: LICENSE, sdServer: SD }).replace(
      /\nLicense: Apache-2\.0\n Licensed[^]*?Apache-2\.0\.txt\.\n/,
      "\n",
    );
    expect(dep5Errors(text)).toEqual([
      "copyright : licence « Apache-2.0 » sans paragraphe autonome",
    ]);
  });
});

describe("built-in image engine in THIRD_PARTY_LICENSES", () => {
  const notices = readFileSync(path.join(ROOT, "THIRD_PARTY_LICENSES"), "utf8");
  const part = notices.split(
    "== Part 10. Built-in image engine (sd-server) ==",
  )[1];

  it("names the pinned sources", () => {
    expect(notices).toContain("  Part 10 Built-in image engine (sd-server)\n");
    expect(part).toBeDefined();
    expect(part).toContain(`stable-diffusion.cpp ${env.SDCPP_VERSION},`);
    expect(part).toContain(`(commit ${env.SDCPP_COMMIT}),`);
    expect(part).toContain(`(commit ${env.SDCPP_GGML_COMMIT}),`);
    expect(part.replace(/\s+/g, " ")).toContain(SD_SERVER_LICENSE_SUMMARY);
  });

  it("holds the full text of every license file", () => {
    for (const file of LICENSE_FILES) {
      const text = readFileSync(
        path.join(ROOT, SD_SERVER_LICENSE_DIR, file),
        "utf8",
      );
      expect(part).toContain(`\n-- ${file} --\n\n${text.trimEnd()}\n`);
    }
  });
});
