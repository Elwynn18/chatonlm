// @vitest-environment node
// Essais des notices du moteur de texte (llama-server de llama.cpp) :
// package-notices-llama.mjs, utilisé par package-notices.mjs (Linux) et
// package-notices-windows.mjs : contrôles du dossier construit, README,
// textes copiés, paragraphes DEP-5 et partie 11 de THIRD_PARTY_LICENSES. Le
// dossier construit n'est jamais lu : les essais fabriquent leur propre
// arborescence. L'accord de l'empreinte avec scripts/build-llama-server.sh
// est essayé dans build-llama-server.test.mjs.
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { dep5Errors } from "./inspect-packages.mjs";
import {
  LLAMA_SERVER_DIR,
  LLAMA_SERVER_EXTRA_TEXTS,
  LLAMA_SERVER_LICENSE_DIR,
  LLAMA_SERVER_LICENSE_SUMMARY,
  LLAMA_SERVER_LICENSES,
  llamaServerCopyright,
  llamaServerNoticeErrors,
  readLlamaServer,
  renderLlamaServerReadme,
  vulkanPins,
} from "./package-notices-llama.mjs";
import { debCopyright, parseEnv } from "./package-notices.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const ENV = readFileSync(path.join(ROOT, "packaging/llama.env"), "utf8");
const SD_ENV = readFileSync(path.join(ROOT, "packaging/sdcpp.env"), "utf8");
const env = parseEnv(ENV);
const LICENSE_FILES = Object.keys(LLAMA_SERVER_LICENSES);
const BUILT = `${env.LLAMA_VERSION}+chatonlm.0123456789ab`;
const LICENSE = "MIT License\n\nText.\n";

const tmp = path.join(ROOT, ".tools/tmp");
const dirs = [];
afterEach(() => {
  while (dirs.length > 0) rmSync(dirs.pop(), { recursive: true, force: true });
});

/** Dépôt factice : llama.env, sdcpp.env, licences, script, VERSION. */
function fakeRepo({ version = BUILT, omit = [], patches = [] } = {}) {
  mkdirSync(tmp, { recursive: true });
  const dir = mkdtempSync(path.join(tmp, "llama-notices-"));
  dirs.push(dir);
  const put = (rel, text) => {
    mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
    writeFileSync(path.join(dir, rel), text);
  };
  put("packaging/llama.env", ENV);
  put("packaging/sdcpp.env", SD_ENV);
  for (const patch of patches) put(`packaging/llama/${patch}`, `${patch}\n`);
  for (const file of LICENSE_FILES) {
    if (!omit.includes(file)) {
      put(`${LLAMA_SERVER_LICENSE_DIR}/${file}`, `texte de ${file}\n`);
    }
  }
  for (const source of Object.values(LLAMA_SERVER_EXTRA_TEXTS)) {
    put(source, `texte de ${source}\n`);
  }
  put("scripts/build-llama-server.sh", "#!/usr/bin/env bash\necho essai\n");
  if (version !== null) put(`${LLAMA_SERVER_DIR}/VERSION`, `${version}\n`);
  return dir;
}

describe("built-in text engine: pinned sources and built directory", () => {
  it("pins a stable version, its nightly build and its full commit", () => {
    expect(env.LLAMA_VERSION).toMatch(/^v\d+\.\d+\.\d+$/);
    expect(env.LLAMA_BUILD).toMatch(/^b\d+$/);
    expect(env.LLAMA_COMMIT).toMatch(/^[0-9a-f]{40}$/);
    expect(env.LLAMA_REPO).toBe("https://github.com/ggml-org/llama.cpp");
    expect(env.LLAMA_GGML_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it("ships a description for every license text of the repository, and the reverse", () => {
    const files = readdirSync(path.join(ROOT, LLAMA_SERVER_LICENSE_DIR)).sort();
    expect(files).toEqual([...LICENSE_FILES].sort());
    // Textes du chargeur et des en-têtes Vulkan : ceux de sd-server (même
    // chargeur, mêmes épingles).
    for (const file of [
      "Vulkan-Headers-LICENSE.txt",
      "Vulkan-Loader-components.txt",
    ]) {
      expect(
        readFileSync(path.join(ROOT, LLAMA_SERVER_LICENSE_DIR, file), "utf8"),
      ).toBe(
        readFileSync(path.join(ROOT, "packaging/sdcpp/licenses", file), "utf8"),
      );
    }
  });

  it("accepts the pinned build with every license text", () => {
    const repo = readLlamaServer(fakeRepo());
    expect(repo.builtVersion).toBe(BUILT);
    expect(repo.fingerprint).toMatch(/^[0-9a-f]{12}$/);
    expect(
      llamaServerNoticeErrors(ENV, BUILT, {
        licenseFiles: LICENSE_FILES,
      }),
    ).toEqual([]);
    expect(Object.keys(repo.texts).sort()).toEqual(
      [...LICENSE_FILES, "Apache-2.0.txt"].sort(),
    );
  });

  it("refuses a missing or stale build, a missing text and an undescribed patch", () => {
    const rebuild = "lancer scripts/build-llama-server.sh.";
    expect(
      llamaServerNoticeErrors(ENV, null, { licenseFiles: LICENSE_FILES }),
    ).toEqual([`llama-server absent de ${LLAMA_SERVER_DIR} : ${rebuild}`]);
    expect(
      llamaServerNoticeErrors(ENV, "v0.8.0+chatonlm.0123456789ab", {
        licenseFiles: LICENSE_FILES,
      }),
    ).toEqual([
      `llama-server v0.8.0+chatonlm.0123456789ab dans ${LLAMA_SERVER_DIR} au lieu de ${env.LLAMA_VERSION}+chatonlm.<empreinte> : ${rebuild}`,
    ]);
    expect(
      llamaServerNoticeErrors(ENV, BUILT, {
        licenseFiles: LICENSE_FILES,
        fingerprint: "ffffffffffff",
      }),
    ).toEqual([
      `llama-server ${BUILT} dans ${LLAMA_SERVER_DIR} construit avec d'autres épingles, correctifs ou un autre script (empreinte attendue ffffffffffff) : ${rebuild}`,
    ]);
    const repo = readLlamaServer(
      fakeRepo({ omit: ["ported-code.txt"], patches: ["0001-x.patch"] }),
    );
    expect(repo.errors).toContain(
      `Licence du moteur de texte absente : ${LLAMA_SERVER_LICENSE_DIR}/ported-code.txt`,
    );
    expect(repo.errors).toContain(
      "Correctif packaging/llama/0001-x.patch sans résumé dans LLAMA_SERVER_PATCHES (scripts/package-notices-llama.mjs).",
    );
    expect(llamaServerNoticeErrors("", BUILT)).toContain(
      "LLAMA_VERSION absent de packaging/llama.env.",
    );
  });

  it("takes the Vulkan pins of sdcpp.env, nothing else", () => {
    expect(vulkanPins("A=1\nVULKAN_X=2\nB=3\nVULKAN_Y=4\n")).toBe(
      "VULKAN_X=2\nVULKAN_Y=4\n",
    );
    const one = readLlamaServer(fakeRepo()).fingerprint;
    const other = fakeRepo();
    writeFileSync(
      path.join(other, "packaging/sdcpp.env"),
      SD_ENV.replace(/^SDCPP_COMMIT=.*$/m, "SDCPP_COMMIT=0"),
    );
    expect(readLlamaServer(other).fingerprint).toBe(one);
    writeFileSync(
      path.join(other, "packaging/sdcpp.env"),
      SD_ENV.replace(/^VULKAN_LOADER_COMMIT=.*$/m, "VULKAN_LOADER_COMMIT=0"),
    );
    expect(readLlamaServer(other).fingerprint).not.toBe(one);
  });
});

describe("built-in text engine: notices", () => {
  it("describes the build, the shipped files and the licenses (Linux)", () => {
    const text = renderLlamaServerReadme({ env, licenseFiles: LICENSE_FILES });
    const flat = text.replace(/\s+/g, " ");
    expect(text).toContain(`Version:      ${env.LLAMA_VERSION} (build`);
    expect(text).toContain(`Commit:       ${env.LLAMA_COMMIT}`);
    expect(text).toContain("/usr/lib/chatonlm/llama-server");
    expect(text).toContain("fallback/libvulkan.so.1");
    expect(text).toContain("libggml-cpu-*.so*");
    expect(flat).toContain("without OpenMP, without OpenSSL");
    expect(flat).toContain(LLAMA_SERVER_LICENSE_SUMMARY);
    expect(text).toContain("Apache License, Version 2.0");
    for (const file of [...LICENSE_FILES, "Apache-2.0.txt"]) {
      expect(text).toContain(`\n  ${file}\n`);
    }
    expect(text).not.toMatch(/Visual C\+\+|\.dll/);
    // Lignes courtes, sauf une adresse qu'on ne coupe pas.
    for (const line of text.split("\n")) {
      if (!/https:\/\/\S{40,}/.test(line)) {
        expect(line.length, line).toBeLessThanOrEqual(80);
      }
    }
  });

  it("describes the Windows build", () => {
    const text = renderLlamaServerReadme({
      env,
      licenseFiles: LICENSE_FILES,
      windows: true,
    });
    expect(text).toContain("llama-server.exe");
    expect(text).toContain("fallback\\vulkan-1.dll");
    expect(text).toContain("ggml-cpu-*.dll");
    expect(text.replace(/\s+/g, " ")).toContain(
      "Microsoft Visual C++ runtime libraries (vcruntime140*.dll, msvcp140*.dll), shipped unmodified in this folder",
    );
    expect(text).not.toContain("/usr/lib");
  });

  it("adds the llama-server paragraphs to the DEP-5 copyright, under usr/libexec in the AppImage", () => {
    const llama = { version: env.LLAMA_VERSION, commit: env.LLAMA_COMMIT };
    const deb = debCopyright({ license: LICENSE, llamaServer: llama });
    expect(dep5Errors(deb)).toEqual([]);
    expect(deb).toContain(
      "\nFiles: usr/lib/chatonlm/llama-server/*\nCopyright: 2023-2026 The ggml authors, and others\nLicense: Expat\n",
    );
    expect(deb).toContain(
      "\nFiles: usr/lib/chatonlm/llama-server/fallback/*\n",
    );
    expect(deb.replace(/\n /g, " ")).toContain(
      `llama.cpp ${env.LLAMA_VERSION} (https://github.com/ggml-org/llama.cpp, commit ${env.LLAMA_COMMIT})`,
    );
    // Le chargeur est sous Apache-2.0 : son paragraphe autonome est là.
    expect(deb).toContain(
      "\n\nLicense: Apache-2.0\n Licensed under the Apache",
    );
    expect(deb).toContain("text engine (llama-server of llama.cpp)");
    for (const line of deb.split("\n")) {
      expect(line.length).toBeLessThanOrEqual(80);
    }
    const appimage = llamaServerCopyright({ ...llama, appimage: true });
    expect(appimage[0]).toBe("Files: usr/libexec/chatonlm/llama-server/*");
    const without = debCopyright({ license: LICENSE });
    expect(without).not.toContain("chatonlm/llama-server");
  });
});

describe("built-in text engine in THIRD_PARTY_LICENSES", () => {
  const notices = readFileSync(path.join(ROOT, "THIRD_PARTY_LICENSES"), "utf8");
  const part = notices.split(
    "== Part 11. Built-in text engine (llama-server) ==",
  )[1];

  it("names the pinned sources", () => {
    expect(notices).toContain(
      "  Part 11 Built-in text engine (llama-server)\n",
    );
    expect(part).toBeDefined();
    expect(part).toContain(
      `llama.cpp ${env.LLAMA_VERSION} (build ${env.LLAMA_BUILD})`,
    );
    expect(part).toContain(`(commit ${env.LLAMA_COMMIT})`);
    expect(part.replace(/\s+/g, " ")).toContain(
      LLAMA_SERVER_LICENSE_SUMMARY.replace(
        /, the Unlicense/,
        ", the Unlicense",
      ),
    );
  });

  it("holds the full text of every license file not already in Part 10", () => {
    const part10 = notices
      .split("== Part 10. Built-in image engine (sd-server) ==")[1]
      .split("== Part 11.")[0];
    for (const file of LICENSE_FILES) {
      const text = readFileSync(
        path.join(ROOT, LLAMA_SERVER_LICENSE_DIR, file),
        "utf8",
      );
      const block = `\n-- ${file} --\n\n${text.trimEnd()}\n`;
      expect(part.includes(block) || part10.includes(block), file).toBe(true);
    }
  });
});
