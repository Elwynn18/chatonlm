// @vitest-environment node
// Essais des scripts du pack CUDA : manifeste (cuda-pack-manifest.sh),
// vérification d'une Release (cuda-pack-verify.mjs) avec le pack et le
// manifeste signés des essais de l'app (src-tauri/src/engine/cuda/fixtures,
// clé d'essai), toolkit de la CI (fetch-cuda-toolkit.mjs : lecture de l'index
// de NVIDIA), et options CUDA de scripts/build-llama-cuda.sh.
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { packErrors } from "./cuda-pack-verify.mjs";
import {
  TOOLKIT_COMPONENTS,
  parseEnv,
  redistArchive,
} from "./fetch-cuda-toolkit.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const FIXTURES = path.join(ROOT, "src-tauri/src/engine/cuda/fixtures");
const MANIFEST = "chatonlm-cuda-b12110-linux-x64.json";
const ARCHIVE = "chatonlm-cuda-b12110-linux-x64.tar.gz";
const dirs = [];

afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

function temp() {
  const dir = mkdtempSync(path.join(os.tmpdir(), "cuda-pack-"));
  dirs.push(dir);
  return dir;
}

const manifestScript = (...args) =>
  spawnSync(
    "bash",
    [path.join(ROOT, "scripts/cuda-pack-manifest.sh"), ...args],
    {
      encoding: "utf8",
    },
  );

describe("cuda-pack-manifest.sh", () => {
  it("écrit le manifeste de l'archive, celui du pack d'essai de l'app", () => {
    const dir = temp();
    copyFileSync(path.join(FIXTURES, ARCHIVE), path.join(dir, ARCHIVE));
    writeFileSync(
      path.join(dir, "VERSION"),
      "v0.9.0+chatonlm.cuda.0123456789ab\r\n",
    );
    const result = manifestScript(
      path.join(dir, ARCHIVE),
      path.join(dir, "VERSION"),
    );
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(result.stdout.trim()).toBe(path.join(dir, MANIFEST));
    // Octet pour octet celui que l'app vérifie dans ses essais.
    expect(readFileSync(path.join(dir, MANIFEST), "utf8")).toBe(
      readFileSync(path.join(FIXTURES, MANIFEST), "utf8"),
    );
  });

  it("refuse un autre nom d'archive ou une autre VERSION", () => {
    const dir = temp();
    writeFileSync(
      path.join(dir, "VERSION"),
      "v0.9.0+chatonlm.cuda.0123456789ab\n",
    );
    writeFileSync(path.join(dir, "pack.tar.gz"), "x");
    expect(
      manifestScript(path.join(dir, "pack.tar.gz"), path.join(dir, "VERSION"))
        .status,
    ).toBe(1);
    const archive = path.join(dir, "chatonlm-cuda-b1-windows-x64.zip");
    writeFileSync(archive, "x");
    writeFileSync(path.join(dir, "VERSION"), "v0.9.0+chatonlm.abc\n");
    expect(manifestScript(archive, path.join(dir, "VERSION")).stderr).toContain(
      "VERSION inattendue",
    );
  });
});

describe("cuda-pack-verify.mjs", () => {
  const pubkey = readFileSync(path.join(FIXTURES, "test.key.pub"), "utf8");
  const files = () =>
    new Map([
      [MANIFEST, readFileSync(path.join(FIXTURES, MANIFEST))],
      [ARCHIVE, readFileSync(path.join(FIXTURES, ARCHIVE))],
    ]);
  const signatures = () =>
    new Map([
      [
        `${MANIFEST}.sig`,
        readFileSync(path.join(FIXTURES, `${MANIFEST}.sig`), "utf8"),
      ],
    ]);

  it("accepte le pack signé pour cette version", () => {
    expect(
      packErrors({
        files: files(),
        signatures: signatures(),
        version: "9.9.9",
        pubkey,
        systems: ["linux"],
      }),
    ).toEqual([]);
  });

  it("refuse une archive changée, une autre version, une signature absente ou un système manquant", () => {
    const changed = files();
    const bytes = Buffer.from(changed.get(ARCHIVE));
    bytes[20] ^= 1;
    changed.set(ARCHIVE, bytes);
    expect(
      packErrors({
        files: changed,
        signatures: signatures(),
        version: "9.9.9",
        pubkey,
        systems: ["linux"],
      }),
    ).toEqual([`${MANIFEST} : taille ou SHA-256 différents de ${ARCHIVE}`]);
    expect(
      packErrors({
        files: files(),
        signatures: signatures(),
        version: "9.9.10",
        pubkey,
        systems: ["linux"],
      }).join("\n"),
    ).toContain("signée pour la version 9.9.9, pas 9.9.10");
    expect(
      packErrors({
        files: files(),
        signatures: new Map(),
        version: "9.9.9",
        pubkey,
        systems: ["linux"],
      }),
    ).toEqual([`${MANIFEST} : signature absente`]);
    expect(
      packErrors({
        files: files(),
        signatures: signatures(),
        version: "9.9.9",
        pubkey,
      }),
    ).toEqual(["pack CUDA windows absent"]);
    // Clé de production : la signature d'essai est refusée.
    const production = JSON.parse(
      readFileSync(path.join(ROOT, "src-tauri/tauri.conf.json"), "utf8"),
    ).plugins.updater.pubkey;
    expect(
      packErrors({
        files: files(),
        signatures: signatures(),
        version: "9.9.9",
        pubkey: production,
        systems: ["linux"],
      }).join("\n"),
    ).toContain("autre clé");
  });
});

describe("fetch-cuda-toolkit.mjs", () => {
  const index = {
    cuda_nvcc: {
      "windows-x86_64": {
        relative_path: "cuda_nvcc/windows-x86_64/cuda_nvcc-13.4.59-archive.zip",
        size: 10,
        sha256: "a".repeat(64),
      },
    },
    evil: {
      "windows-x86_64": {
        relative_path: "../evil.zip",
        size: 10,
        sha256: "a".repeat(64),
      },
    },
  };

  it("lit une archive de l'index et refuse le reste", () => {
    expect(
      redistArchive(
        index,
        "https://example.com/redist/",
        "cuda_nvcc",
        "windows-x86_64",
      ),
    ).toEqual({
      url: "https://example.com/redist/cuda_nvcc/windows-x86_64/cuda_nvcc-13.4.59-archive.zip",
      size: 10,
      sha256: "a".repeat(64),
      name: "cuda_nvcc-13.4.59-archive.zip",
    });
    expect(() =>
      redistArchive(index, "https://x/", "cuda_nvcc", "linux-x86_64"),
    ).toThrow("absent");
    expect(() =>
      redistArchive(index, "https://x/", "evil", "windows-x86_64"),
    ).toThrow("refusé");
  });

  it("prend ses épingles dans packaging/cuda.env, index vérifié par son empreinte", () => {
    const env = parseEnv(
      readFileSync(path.join(ROOT, "packaging/cuda.env"), "utf8"),
    );
    expect(env.CUDA_INDEX).toBe(`redistrib_${env.CUDA_VERSION}.json`);
    expect(env.CUDA_INDEX_SHA256).toMatch(/^[0-9a-f]{64}$/);
    expect(env.CUDA_CONTAINER).toBe(
      `nvidia/cuda:${env.CUDA_VERSION}-devel-ubuntu22.04`,
    );
    expect(TOOLKIT_COMPONENTS).toEqual(
      expect.arrayContaining(["cuda_nvcc", "libcublas", "cuda_cudart"]),
    );
  });
});

describe("build-llama-cuda.sh", () => {
  const script = readFileSync(
    path.join(ROOT, "scripts/build-llama-cuda.sh"),
    "utf8",
  );

  it("construit ggml-cuda seul, du même commit, sans NCCL ni bibliothèque de NVIDIA dans le pack", () => {
    for (const option of [
      "-DGGML_CUDA=ON",
      "-DGGML_CUDA_NCCL=OFF",
      // Runtime CUDA partagé : aucun code de NVIDIA dans le pack.
      "-DCMAKE_CUDA_RUNTIME_LIBRARY=Shared",
      "-DGGML_BACKEND_DL=ON",
      "-DGGML_VULKAN=OFF",
      "--target ggml-cuda",
      '"-DGGML_CUDA_CCCL_VERSION=$CUDA_CCCL_VERSION"',
    ]) {
      expect(script, option).toContain(option);
    }
    // Architectures par défaut de ggml (aucune imposée).
    expect(script).not.toContain("CMAKE_CUDA_ARCHITECTURES=");
    expect(script).toContain("bibliothèque de NVIDIA dans le pack");
    // Le conteneur de la CI est celui des épingles.
    const workflow = readFileSync(
      path.join(ROOT, ".github/workflows/packages.yml"),
      "utf8",
    );
    const env = parseEnv(
      readFileSync(path.join(ROOT, "packaging/cuda.env"), "utf8"),
    );
    expect(workflow).toContain(`container: ${env.CUDA_CONTAINER}`);
  });

  it("répond par build-llama-server.sh --cuda (vérification seule, rien à construire)", () => {
    const result = spawnSync(
      "bash",
      [path.join(ROOT, "scripts/build-llama-server.sh"), "--cuda", "--check"],
      { encoding: "utf8", env: { ...process.env, PATH: "/usr/bin:/bin" } },
    );
    // Sans pack construit dans .tools : à construire (code 1), sans rien lancer.
    if (result.status === 0) {
      expect(result.stdout.trim()).toMatch(
        /chatonlm-cuda-b\d+-linux-x64\.tar\.gz$/,
      );
    } else {
      expect(result.status).toBe(1);
      expect(result.stderr).toMatch(/^pack CUDA à construire \(v/);
    }
  });
});
