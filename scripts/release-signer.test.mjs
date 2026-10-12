// @vitest-environment node
// Essais du projet npm minimal du CLI Tauri (release-signer.mjs), installé
// par le job « sign » de release.yml : version, adresses et empreintes de
// package-lock.json, rien d'autre.
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { CLI, signerProject } from "./release-signer.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const LOCK = JSON.parse(
  readFileSync(path.join(ROOT, "package-lock.json"), "utf8"),
);

describe("release-signer", () => {
  it("reprend le CLI Tauri et ses binaires du verrou du dépôt", () => {
    const { packageJson, packageLock } = signerProject(LOCK);
    const cli = LOCK.packages[`node_modules/${CLI}`];
    expect(packageJson.dependencies).toEqual({ [CLI]: cli.version });
    expect(packageLock.packages[""].dependencies).toEqual({
      [CLI]: cli.version,
    });
    const keys = Object.keys(packageLock.packages).filter((key) => key);
    // Le CLI et un binaire par plateforme, rien d'autre.
    expect(keys).toContain(`node_modules/${CLI}`);
    expect(keys).toContain(`node_modules/${CLI}-linux-x64-gnu`);
    expect(keys).toHaveLength(1 + Object.keys(cli.optionalDependencies).length);
    for (const key of keys) {
      const { integrity, resolved, version } = LOCK.packages[key];
      expect(packageLock.packages[key]).toMatchObject({
        integrity,
        resolved,
        version,
      });
      expect(packageLock.packages[key].dev).toBeUndefined();
    }
  });

  it("refuse un verrou sans le CLI ou sans empreinte", () => {
    expect(() => signerProject({ packages: {} })).toThrow(/absent/);
    const prefix = `node_modules/${CLI}`;
    const broken = structuredClone(LOCK);
    delete broken.packages[`${prefix}-linux-x64-gnu`].integrity;
    expect(() => signerProject(broken)).toThrow(/sans empreinte/);
    const elsewhere = structuredClone(LOCK);
    elsewhere.packages[prefix].resolved = "https://example.com/cli.tgz";
    expect(() => signerProject(elsewhere)).toThrow(/hors du registre/);
  });
});
