// @vitest-environment node
// Essais des notices du moteur de transcription (whisper-server de
// whisper.cpp) : package-notices-whisper.mjs, utilisé par
// package-notices.mjs (Linux) et package-notices-windows.mjs : contrôles du
// dossier construit, README, textes copiés, paragraphe DEP-5 et partie 12 de
// THIRD_PARTY_LICENSES. Le dossier construit n'est jamais lu : les essais
// fabriquent leur propre arborescence. L'accord de l'empreinte avec
// scripts/build-whisper-server.sh est essayé dans
// build-whisper-server.test.mjs.
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { dep5Errors } from "./inspect-packages.mjs";
import { debCopyright, parseEnv } from "./package-notices.mjs";
import {
  readWhisperServer,
  renderWhisperServerReadme,
  WHISPER_SERVER_DIR,
  WHISPER_SERVER_EXTRA_TEXTS,
  WHISPER_SERVER_LICENSE_DIR,
  WHISPER_SERVER_LICENSE_SUMMARY,
  WHISPER_SERVER_LICENSES,
  WHISPER_SERVER_PATCHES,
  whisperServerCopyright,
  whisperServerNoticeErrors,
} from "./package-notices-whisper.mjs";

// Essais qui lisent et écrivent des dossiers : délai explicite (disque lent sous charge).
vi.setConfig({ testTimeout: 30_000 });

const ROOT = path.resolve(import.meta.dirname, "..");
const ENV = readFileSync(path.join(ROOT, "packaging/whispercpp.env"), "utf8");
const SD_ENV = readFileSync(path.join(ROOT, "packaging/sdcpp.env"), "utf8");
const env = parseEnv(ENV);
const LICENSE_FILES = Object.keys(WHISPER_SERVER_LICENSES);
const PATCHES = Object.keys(WHISPER_SERVER_PATCHES);
const BUILT = `${env.WHISPER_VERSION}+chatonlm.0123456789ab`;
const LICENSE = "MIT License\n\nText.\n";

const tmp = path.join(ROOT, ".tools/tmp");
const dirs = [];
afterEach(() => {
  while (dirs.length > 0) rmSync(dirs.pop(), { recursive: true, force: true });
});

/** Dépôt factice : whispercpp.env, sdcpp.env, licences, correctifs, script, VERSION. */
function fakeRepo({ version = BUILT, omit = [], patches = PATCHES } = {}) {
  mkdirSync(tmp, { recursive: true });
  const dir = mkdtempSync(path.join(tmp, "whisper-notices-"));
  dirs.push(dir);
  const put = (rel, text) => {
    mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
    writeFileSync(path.join(dir, rel), text);
  };
  put("packaging/whispercpp.env", ENV);
  put("packaging/sdcpp.env", SD_ENV);
  for (const patch of patches)
    put(`packaging/whispercpp/${patch}`, `${patch}\n`);
  for (const file of LICENSE_FILES) {
    if (!omit.includes(file)) {
      put(`${WHISPER_SERVER_LICENSE_DIR}/${file}`, `texte de ${file}\n`);
    }
  }
  for (const source of Object.values(WHISPER_SERVER_EXTRA_TEXTS)) {
    put(source, `texte de ${source}\n`);
  }
  put("scripts/build-whisper-server.sh", "#!/usr/bin/env bash\necho essai\n");
  if (version !== null) put(`${WHISPER_SERVER_DIR}/VERSION`, `${version}\n`);
  return dir;
}

describe("built-in transcription engine: pinned sources and built directory", () => {
  it("pins a release, its full commit and its ggml", () => {
    expect(env.WHISPER_VERSION).toMatch(/^v\d+\.\d+\.\d+$/);
    expect(env.WHISPER_COMMIT).toMatch(/^[0-9a-f]{40}$/);
    expect(env.WHISPER_REPO).toBe("https://github.com/ggml-org/whisper.cpp");
    expect(env.WHISPER_GGML_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it("ships a description for every license text and patch of the repository, and the reverse", () => {
    const files = readdirSync(
      path.join(ROOT, WHISPER_SERVER_LICENSE_DIR),
    ).sort();
    expect(files).toEqual([...LICENSE_FILES].sort());
    const patches = readdirSync(path.join(ROOT, "packaging/whispercpp"))
      .filter((file) => file.endsWith(".patch"))
      .sort();
    expect(patches).toEqual([...PATCHES].sort());
    // En-têtes Vulkan : le même texte que celui de sd-server (mêmes outils).
    expect(
      readFileSync(
        path.join(
          ROOT,
          WHISPER_SERVER_LICENSE_DIR,
          "Vulkan-Headers-LICENSE.txt",
        ),
        "utf8",
      ),
    ).toBe(
      readFileSync(
        path.join(ROOT, "packaging/sdcpp/licenses/Vulkan-Headers-LICENSE.txt"),
        "utf8",
      ),
    );
  });

  it("accepts the pinned build with every license text and summarized patch", () => {
    const repo = readWhisperServer(fakeRepo());
    expect(repo.builtVersion).toBe(BUILT);
    expect(repo.fingerprint).toMatch(/^[0-9a-f]{12}$/);
    expect(
      whisperServerNoticeErrors(ENV, BUILT, {
        licenseFiles: LICENSE_FILES,
        patches: PATCHES,
      }),
    ).toEqual([]);
    expect(Object.keys(repo.texts).sort()).toEqual(
      [...LICENSE_FILES, "Apache-2.0.txt"].sort(),
    );
  });

  it("refuses a missing or stale build, a missing text, an undescribed or missing patch", () => {
    const rebuild = "lancer scripts/build-whisper-server.sh.";
    const options = { licenseFiles: LICENSE_FILES, patches: PATCHES };
    expect(whisperServerNoticeErrors(ENV, null, options)).toEqual([
      `whisper-server absent de ${WHISPER_SERVER_DIR} : ${rebuild}`,
    ]);
    expect(
      whisperServerNoticeErrors(ENV, "v1.8.0+chatonlm.0123456789ab", options),
    ).toEqual([
      `whisper-server v1.8.0+chatonlm.0123456789ab dans ${WHISPER_SERVER_DIR} au lieu de ${env.WHISPER_VERSION}+chatonlm.<empreinte> : ${rebuild}`,
    ]);
    expect(
      whisperServerNoticeErrors(ENV, BUILT, {
        ...options,
        fingerprint: "ffffffffffff",
      }),
    ).toEqual([
      `whisper-server ${BUILT} dans ${WHISPER_SERVER_DIR} construit avec d'autres épingles, correctifs ou un autre script (empreinte attendue ffffffffffff) : ${rebuild}`,
    ]);
    const repo = readWhisperServer(
      fakeRepo({
        omit: ["ported-code.txt"],
        patches: [...PATCHES, "0099-x.patch"],
      }),
    );
    expect(repo.errors).toContain(
      `Licence du moteur de transcription absente : ${WHISPER_SERVER_LICENSE_DIR}/ported-code.txt`,
    );
    expect(repo.errors).toContain(
      "Correctif packaging/whispercpp/0099-x.patch sans résumé dans WHISPER_SERVER_PATCHES (scripts/package-notices-whisper.mjs).",
    );
    expect(
      readWhisperServer(fakeRepo({ patches: [] })).errors.filter((error) =>
        error.startsWith("Correctif"),
      ),
    ).toEqual(
      PATCHES.map(
        (patch) =>
          `Correctif résumé mais absent : packaging/whispercpp/${patch}`,
      ),
    );
    expect(whisperServerNoticeErrors("", BUILT)).toContain(
      "WHISPER_VERSION absent de packaging/whispercpp.env.",
    );
  });

  it("follows the Vulkan pins of sdcpp.env and its patches, nothing else", () => {
    const one = readWhisperServer(fakeRepo()).fingerprint;
    const other = fakeRepo();
    writeFileSync(
      path.join(other, "packaging/sdcpp.env"),
      SD_ENV.replace(/^SDCPP_COMMIT=.*$/m, "SDCPP_COMMIT=0"),
    );
    expect(readWhisperServer(other).fingerprint).toBe(one);
    writeFileSync(
      path.join(other, "packaging/sdcpp.env"),
      SD_ENV.replace(/^VULKAN_LOADER_COMMIT=.*$/m, "VULKAN_LOADER_COMMIT=0"),
    );
    expect(readWhisperServer(other).fingerprint).not.toBe(one);
    const patched = fakeRepo();
    writeFileSync(
      path.join(patched, `packaging/whispercpp/${PATCHES[0]}`),
      "autre\n",
    );
    expect(readWhisperServer(patched).fingerprint).not.toBe(one);
  });
});

describe("built-in transcription engine: notices", () => {
  it("describes the build, the shipped files, the patch and the licenses (Linux)", () => {
    const text = renderWhisperServerReadme({
      env,
      licenseFiles: LICENSE_FILES,
    });
    const flat = text.replace(/\s+/g, " ");
    expect(text).toContain(
      `Version:      ${env.WHISPER_VERSION} (ggml ${env.WHISPER_GGML_VERSION})`,
    );
    expect(text).toContain(`Commit:       ${env.WHISPER_COMMIT}`);
    expect(text).toContain("/usr/lib/chatonlm/whisper-server");
    expect(text).toContain("libggml-cpu-*.so*");
    expect(text).toContain("libwhisper.so*");
    // Pas de chargeur à lui : celui du moteur de texte.
    expect(flat).toContain("llama-server/fallback/libvulkan.so.1");
    expect(text).not.toMatch(/^ {2}fallback/m);
    expect(flat).toContain("without OpenMP, without curl");
    expect(flat).toContain(WHISPER_SERVER_LICENSE_SUMMARY);
    for (const patch of PATCHES) expect(text).toContain(`\n  ${patch}\n`);
    for (const file of [...LICENSE_FILES, "Apache-2.0.txt"]) {
      expect(text).toContain(`\n  ${file}\n`);
    }
    expect(text).not.toMatch(/Visual C\+\+|\.dll/);
    for (const line of text.split("\n")) {
      if (!/https:\/\/\S{40,}/.test(line)) {
        expect(line.length, line).toBeLessThanOrEqual(80);
      }
    }
  });

  it("describes the Windows build", () => {
    const text = renderWhisperServerReadme({
      env,
      licenseFiles: LICENSE_FILES,
      windows: true,
    });
    expect(text).toContain("whisper-server.exe");
    expect(text).toContain("ggml-cpu-*.dll");
    expect(text.replace(/\s+/g, " ")).toContain(
      "llama-server\\fallback\\vulkan-1.dll",
    );
    expect(text.replace(/\s+/g, " ")).toContain(
      "Microsoft Visual C++ runtime libraries (vcruntime140*.dll, msvcp140*.dll), shipped unmodified in this folder",
    );
    expect(text).not.toContain("/usr/lib");
  });

  it("adds the whisper-server paragraph to the DEP-5 copyright, under usr/libexec in the AppImage", () => {
    const whisper = {
      version: env.WHISPER_VERSION,
      commit: env.WHISPER_COMMIT,
    };
    const deb = debCopyright({ license: LICENSE, whisperServer: whisper });
    expect(dep5Errors(deb)).toEqual([]);
    expect(deb).toContain(
      "\nFiles: usr/lib/chatonlm/whisper-server/*\nCopyright: 2023-2026 The ggml authors, and others\nLicense: Expat\n",
    );
    expect(deb).not.toContain("whisper-server/fallback");
    expect(deb.replace(/\n /g, " ")).toContain(
      `whisper.cpp ${env.WHISPER_VERSION} (https://github.com/ggml-org/whisper.cpp, commit ${env.WHISPER_COMMIT})`,
    );
    expect(deb.replace(/\n /g, " ")).toContain(
      "the transcription engine (whisper-server of whisper.cpp)",
    );
    for (const line of deb.split("\n")) {
      expect(line.length).toBeLessThanOrEqual(80);
    }
    const appimage = whisperServerCopyright({ ...whisper, appimage: true });
    expect(appimage[0]).toBe("Files: usr/libexec/chatonlm/whisper-server/*");
    expect(debCopyright({ license: LICENSE })).not.toContain(
      "chatonlm/whisper-server",
    );
  });
});

describe("built-in transcription engine in THIRD_PARTY_LICENSES", () => {
  const notices = readFileSync(path.join(ROOT, "THIRD_PARTY_LICENSES"), "utf8");
  const part = notices.split(
    "== Part 12. Built-in transcription engine (whisper-server) ==",
  )[1];

  it("names the pinned sources and the patch", () => {
    expect(notices).toContain(
      "  Part 12 Built-in transcription engine (whisper-server)\n",
    );
    expect(part).toBeDefined();
    expect(part).toContain(`whisper.cpp ${env.WHISPER_VERSION},`);
    expect(part).toContain(`(commit ${env.WHISPER_COMMIT})`);
    expect(part).toContain(`ggml ${env.WHISPER_GGML_VERSION}`);
    const flat = part.replace(/\s+/g, " ");
    expect(flat).toContain(WHISPER_SERVER_LICENSE_SUMMARY);
    for (const patch of PATCHES) expect(flat).toContain(patch);
  });

  it("holds the full text of every license file not already in Part 10", () => {
    const part10 = notices
      .split("== Part 10. Built-in image engine (sd-server) ==")[1]
      .split("== Part 11.")[0];
    for (const file of LICENSE_FILES) {
      const text = readFileSync(
        path.join(ROOT, WHISPER_SERVER_LICENSE_DIR, file),
        "utf8",
      );
      const block = `\n-- ${file} --\n\n${text.trimEnd()}\n`;
      expect(part.includes(block) || part10.includes(block), file).toBe(true);
    }
  });
});
