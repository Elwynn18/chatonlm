// @vitest-environment node
// Essais de scripts/build-llama-server.sh, sur une copie du script dans une
// arborescence fabriquée : rien n'est construit (git, cmake et glslc sont
// remplacés par des outils factices ; le faux git s'arrête au clonage ;
// scripts/build-sd-server.sh, qui fournit le chargeur Vulkan de secours, est
// remplacé par un faux). Vérifie le verrou, l'ordre (sd-server d'abord) et
// que l'empreinte calculée par les notices (package-notices-llama.mjs) est
// celle du script.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  llamaServerFingerprint,
  vulkanPins,
} from "./package-notices-llama.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const roots = [];

afterEach(() => {
  for (const dir of roots.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

/**
 * Copie du script, de packaging/llama.env et de packaging/sdcpp.env ; faux
 * build-sd-server.sh (qui réussit, avec son chargeur, ou échoue si
 * `sdFails`) ; sans .tools/.
 */
function fakeRoot({ sdFails = false } = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), "llama-build-"));
  roots.push(root);
  mkdirSync(path.join(root, "scripts"));
  mkdirSync(path.join(root, "packaging"));
  copyFileSync(
    path.join(ROOT, "scripts/build-llama-server.sh"),
    path.join(root, "scripts/build-llama-server.sh"),
  );
  for (const file of ["llama.env", "sdcpp.env"]) {
    copyFileSync(
      path.join(ROOT, "packaging", file),
      path.join(root, "packaging", file),
    );
  }
  const sd = path.join(root, "scripts/build-sd-server.sh");
  writeFileSync(
    sd,
    sdFails
      ? `#!/bin/sh\necho "sd $*" >> "${root}/calls"\nexit 1\n`
      : `#!/bin/sh
echo "sd $*" >> "${root}/calls"
mkdir -p "${root}/.tools/runtimes/sd-server/fallback"
: > "${root}/.tools/runtimes/sd-server/fallback/libvulkan.so.1"
echo "${root}/.tools/runtimes/sd-server/sd-server"
`,
  );
  chmodSync(sd, 0o755);
  const bin = path.join(root, "bin");
  mkdirSync(bin);
  for (const tool of ["git", "cmake", "glslc"]) {
    const file = path.join(bin, tool);
    writeFileSync(
      file,
      `#!/bin/sh\necho "${tool} $*" >> "${root}/calls"\nexit 1\n`,
    );
    chmodSync(file, 0o755);
  }
  return { root, bin };
}

function run({ root, bin }, ...args) {
  return spawnSync(
    "bash",
    [path.join(root, "scripts/build-llama-server.sh"), ...args],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
}

const calls = (root) =>
  existsSync(path.join(root, "calls"))
    ? readFileSync(path.join(root, "calls"), "utf8")
    : "";

describe("verrou et ordre de build-llama-server.sh", () => {
  it("crée .tools/ s'il manque, prend le chargeur de sd-server puis clone", () => {
    const place = fakeRoot();
    expect(existsSync(path.join(place.root, ".tools"))).toBe(false);
    const result = run(place);
    expect(result.status).not.toBe(0);
    expect(result.stderr).not.toContain("une autre construction");
    // sd-server d'abord (chargeur Vulkan de secours partagé), puis le clone
    // du tag épinglé ; verrou rendu à la sortie.
    expect(calls(place.root)).toMatch(/^sd \ngit .*clone .*--branch b\d+ /m);
    expect(calls(place.root).indexOf("sd")).toBeLessThan(
      calls(place.root).indexOf("git"),
    );
    expect(existsSync(path.join(place.root, ".tools/llama-build.lock"))).toBe(
      false,
    );
  });

  it("s'arrête avant de cloner si sd-server (son chargeur de secours) échoue", () => {
    const place = fakeRoot({ sdFails: true });
    const result = run(place);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("chargeur Vulkan de secours");
    expect(calls(place.root)).not.toMatch(/^git/m);
    expect(existsSync(path.join(place.root, ".tools/llama-build.lock"))).toBe(
      false,
    );
  });

  it("refuse une deuxième construction quand le verrou est pris", () => {
    const place = fakeRoot();
    mkdirSync(path.join(place.root, ".tools/llama-build.lock"), {
      recursive: true,
    });
    const result = run(place);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("une autre construction est en cours");
    expect(calls(place.root)).toBe("");
    expect(existsSync(path.join(place.root, ".tools/llama-build.lock"))).toBe(
      true,
    );
  });
});

describe("empreinte de build-llama-server.sh", () => {
  /** Dossier construit factice, de VERSION `version`, avec ses empreintes. */
  function built(root, version) {
    const out = path.join(root, ".tools/runtimes/llama-server");
    mkdirSync(out, { recursive: true });
    writeFileSync(path.join(out, "llama-server"), "binaire");
    writeFileSync(path.join(out, "VERSION"), `${version}\n`);
    const sum = (file) =>
      createHash("sha256")
        .update(readFileSync(path.join(out, file)))
        .digest("hex");
    writeFileSync(
      path.join(out, "SHA256SUMS"),
      ["VERSION", "llama-server"].map((f) => `${sum(f)}  ${f}\n`).join(""),
    );
    return out;
  }

  const expected = (root) =>
    llamaServerFingerprint({
      env: readFileSync(path.join(root, "packaging/llama.env")),
      vulkan: vulkanPins(
        readFileSync(path.join(root, "packaging/sdcpp.env"), "utf8"),
      ),
      patches: [],
      script: readFileSync(path.join(root, "scripts/build-llama-server.sh")),
    });

  it("est celle que calculent les notices : dossier à jour, rien à refaire", () => {
    const place = fakeRoot();
    const env = readFileSync(
      path.join(place.root, "packaging/llama.env"),
      "utf8",
    );
    const pinned = /^LLAMA_VERSION=(.*)$/m.exec(env)[1];
    const out = built(place.root, `${pinned}+chatonlm.${expected(place.root)}`);
    const result = run(place, "--check");
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(result.stdout.trim()).toBe(path.join(out, "llama-server"));
    expect(calls(place.root)).toBe("");
  });

  it("change avec les épingles du chargeur Vulkan de sd-server", () => {
    const place = fakeRoot();
    const env = readFileSync(
      path.join(place.root, "packaging/llama.env"),
      "utf8",
    );
    const pinned = /^LLAMA_VERSION=(.*)$/m.exec(env)[1];
    built(place.root, `${pinned}+chatonlm.${expected(place.root)}`);
    // Autre chargeur Vulkan : à reconstruire. Une autre ligne de sdcpp.env
    // (version de sd-server) n'y change rien.
    const sdEnv = path.join(place.root, "packaging/sdcpp.env");
    const original = readFileSync(sdEnv, "utf8");
    writeFileSync(
      sdEnv,
      original.replace(/^SDCPP_VERSION=.*$/m, "SDCPP_VERSION=autre"),
    );
    expect(run(place, "--check").status).toBe(0);
    writeFileSync(
      sdEnv,
      original.replace(
        /^VULKAN_LOADER_TAG=.*$/m,
        "VULKAN_LOADER_TAG=vulkan-sdk-0",
      ),
    );
    const stale = run(place, "--check");
    expect(stale.status).toBe(1);
    expect(stale.stderr).toMatch(/^llama-server à construire \(v/);
  });
});
