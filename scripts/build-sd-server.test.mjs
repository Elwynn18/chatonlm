// @vitest-environment node
// Essais du verrou de scripts/build-sd-server.sh, sur une copie du script dans
// une arborescence fabriquée : rien n'est construit (git, cmake et glslc sont
// remplacés par des outils factices ; le faux git s'arrête au clonage).
import { spawnSync } from "node:child_process";
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

const ROOT = path.resolve(import.meta.dirname, "..");
const roots = [];

afterEach(() => {
  for (const dir of roots.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

/** Copie du script, de packaging/sdcpp.env et d'un correctif ; sans .tools/. */
function fakeRoot() {
  const root = mkdtempSync(path.join(os.tmpdir(), "sdcpp-lock-"));
  roots.push(root);
  mkdirSync(path.join(root, "scripts"));
  mkdirSync(path.join(root, "packaging/sdcpp"), { recursive: true });
  copyFileSync(
    path.join(ROOT, "scripts/build-sd-server.sh"),
    path.join(root, "scripts/build-sd-server.sh"),
  );
  copyFileSync(
    path.join(ROOT, "packaging/sdcpp.env"),
    path.join(root, "packaging/sdcpp.env"),
  );
  writeFileSync(path.join(root, "packaging/sdcpp/essai.patch"), "");
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

function run({ root, bin }) {
  return spawnSync("bash", [path.join(root, "scripts/build-sd-server.sh")], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
  });
}

describe("verrou de build-sd-server.sh", () => {
  it("crée .tools/ s'il manque, au lieu de croire le verrou pris", () => {
    const place = fakeRoot();
    expect(existsSync(path.join(place.root, ".tools"))).toBe(false);
    const result = run(place);
    expect(result.status).not.toBe(0);
    expect(result.stderr).not.toContain("une autre construction");
    // Arrivé au clonage, puis verrou rendu à la sortie.
    expect(readFileSync(path.join(place.root, "calls"), "utf8")).toMatch(
      /^git .*clone/m,
    );
    expect(existsSync(path.join(place.root, ".tools/sdcpp-build.lock"))).toBe(
      false,
    );
  });

  it("refuse une deuxième construction quand le verrou est pris", () => {
    const place = fakeRoot();
    mkdirSync(path.join(place.root, ".tools/sdcpp-build.lock"), {
      recursive: true,
    });
    const result = run(place);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("une autre construction est en cours");
    expect(existsSync(path.join(place.root, "calls"))).toBe(false);
    expect(existsSync(path.join(place.root, ".tools/sdcpp-build.lock"))).toBe(
      true,
    );
  });
});
