// @vitest-environment node
// Essais de scripts/build-whisper-server.sh, sur une copie du script dans une
// arborescence fabriquée : rien n'est construit (git, cmake et glslc sont
// remplacés par des outils factices ; le faux git s'arrête au clonage ;
// scripts/build-sd-server.sh, qui fournit le chargeur Vulkan de secours des
// contrôles, est remplacé par un faux). Vérifie le verrou, l'ordre (sd-server
// d'abord) et que l'empreinte calculée par les notices
// (package-notices-whisper.mjs) est celle du script, correctifs compris.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { vulkanPins } from "./package-notices-llama.mjs";
import { whisperServerFingerprint } from "./package-notices-whisper.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
/** Correctifs de packaging/whispercpp (chemins relatifs à packaging/). */
const PATCHES = readdirSync(path.join(ROOT, "packaging/whispercpp"))
  .filter((file) => file.endsWith(".patch"))
  .sort()
  .map((file) => `whispercpp/${file}`);
const roots = [];

afterEach(() => {
  for (const dir of roots.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

/**
 * Copie du script, de packaging/whispercpp.env, de ses correctifs et de
 * packaging/sdcpp.env ; faux
 * build-sd-server.sh (qui réussit, avec son chargeur, ou échoue si
 * `sdFails`) ; sans .tools/.
 */
function fakeRoot({ sdFails = false } = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), "whisper-build-"));
  roots.push(root);
  mkdirSync(path.join(root, "scripts"));
  mkdirSync(path.join(root, "packaging/whispercpp"), { recursive: true });
  copyFileSync(
    path.join(ROOT, "scripts/build-whisper-server.sh"),
    path.join(root, "scripts/build-whisper-server.sh"),
  );
  for (const file of ["whispercpp.env", "sdcpp.env", ...PATCHES]) {
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
    [path.join(root, "scripts/build-whisper-server.sh"), ...args],
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

describe("verrou et ordre de build-whisper-server.sh", () => {
  it("crée .tools/ s'il manque, prend le chargeur de sd-server puis clone", () => {
    const place = fakeRoot();
    expect(existsSync(path.join(place.root, ".tools"))).toBe(false);
    const result = run(place);
    expect(result.status).not.toBe(0);
    expect(result.stderr).not.toContain("une autre construction");
    // sd-server d'abord (chargeur Vulkan de secours partagé), puis le clone
    // du tag épinglé ; verrou rendu à la sortie.
    expect(calls(place.root)).toMatch(
      /^sd \ngit .*clone .*--branch v\d+\.\d+\.\d+ /m,
    );
    expect(calls(place.root).indexOf("sd")).toBeLessThan(
      calls(place.root).indexOf("git"),
    );
    expect(existsSync(path.join(place.root, ".tools/whisper-build.lock"))).toBe(
      false,
    );
  });

  it("s'arrête avant de cloner si sd-server (son chargeur de secours) échoue", () => {
    const place = fakeRoot({ sdFails: true });
    const result = run(place);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("chargeur Vulkan de secours");
    expect(calls(place.root)).not.toMatch(/^git/m);
    expect(existsSync(path.join(place.root, ".tools/whisper-build.lock"))).toBe(
      false,
    );
  });

  it("refuse une deuxième construction quand le verrou est pris", () => {
    const place = fakeRoot();
    mkdirSync(path.join(place.root, ".tools/whisper-build.lock"), {
      recursive: true,
    });
    const result = run(place);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("une autre construction est en cours");
    expect(calls(place.root)).toBe("");
    expect(existsSync(path.join(place.root, ".tools/whisper-build.lock"))).toBe(
      true,
    );
  });
});

describe("empreinte de build-whisper-server.sh", () => {
  /** Dossier construit factice, de VERSION `version`, avec ses empreintes. */
  function built(root, version) {
    const out = path.join(root, ".tools/runtimes/whisper-server");
    mkdirSync(out, { recursive: true });
    writeFileSync(path.join(out, "whisper-server"), "binaire");
    writeFileSync(path.join(out, "VERSION"), `${version}\n`);
    const sum = (file) =>
      createHash("sha256")
        .update(readFileSync(path.join(out, file)))
        .digest("hex");
    writeFileSync(
      path.join(out, "SHA256SUMS"),
      ["VERSION", "whisper-server"].map((f) => `${sum(f)}  ${f}\n`).join(""),
    );
    return out;
  }

  const expected = (root) =>
    whisperServerFingerprint({
      env: readFileSync(path.join(root, "packaging/whispercpp.env")),
      vulkan: vulkanPins(
        readFileSync(path.join(root, "packaging/sdcpp.env"), "utf8"),
      ),
      patches: PATCHES.map((file) =>
        readFileSync(path.join(root, "packaging", file)),
      ),
      script: readFileSync(path.join(root, "scripts/build-whisper-server.sh")),
    });

  it("est celle que calculent les notices : dossier à jour, rien à refaire", () => {
    const place = fakeRoot();
    const env = readFileSync(
      path.join(place.root, "packaging/whispercpp.env"),
      "utf8",
    );
    const pinned = /^WHISPER_VERSION=(.*)$/m.exec(env)[1];
    const out = built(place.root, `${pinned}+chatonlm.${expected(place.root)}`);
    const result = run(place, "--check");
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(result.stdout.trim()).toBe(path.join(out, "whisper-server"));
    expect(calls(place.root)).toBe("");
  });

  it("change avec les épingles du chargeur Vulkan de sd-server", () => {
    const place = fakeRoot();
    const env = readFileSync(
      path.join(place.root, "packaging/whispercpp.env"),
      "utf8",
    );
    const pinned = /^WHISPER_VERSION=(.*)$/m.exec(env)[1];
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
    expect(stale.stderr).toMatch(/^whisper-server à construire \(v/);
  });

  it("change avec ses correctifs", () => {
    expect(PATCHES.length).toBeGreaterThan(0);
    const place = fakeRoot();
    const env = readFileSync(
      path.join(place.root, "packaging/whispercpp.env"),
      "utf8",
    );
    const pinned = /^WHISPER_VERSION=(.*)$/m.exec(env)[1];
    built(place.root, `${pinned}+chatonlm.${expected(place.root)}`);
    expect(run(place, "--check").status).toBe(0);
    const patch = path.join(place.root, "packaging", PATCHES[0]);
    writeFileSync(patch, `${readFileSync(patch, "utf8")}\n`);
    expect(run(place, "--check").status).toBe(1);
  });
});
