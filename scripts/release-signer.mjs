#!/usr/bin/env node
// Projet npm minimal du CLI Tauri, pour le job « sign » de
// .github/workflows/release.yml : seuls `@tauri-apps/cli` et ses binaires
// par plateforme, à la version, aux adresses et aux empreintes (integrity)
// de package-lock.json. Installé par `npm ci --ignore-scripts` (empreintes
// vérifiées, aucun script), il signe les paquets sans que la clé privée ne
// voie le reste des dépendances du projet.
//
//   node scripts/release-signer.mjs <dossier>
//
// Écrit <dossier>/package.json et <dossier>/package-lock.json depuis le
// package-lock.json du dépôt.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const CLI = "@tauri-apps/cli";
const NAME = "chatonlm-signer";
const REGISTRY = "https://registry.npmjs.org/";

/**
 * package.json et package-lock.json du projet minimal, tirés du verrou du
 * dépôt (`lock`, lockfileVersion 3). Erreur si le CLI, l'un de ses binaires
 * ou une empreinte manque.
 */
export function signerProject(lock) {
  const prefix = `node_modules/${CLI}`;
  const cli = lock.packages?.[prefix];
  if (!cli?.version) {
    throw new Error(`package-lock.json : ${CLI} absent`);
  }
  const wanted = [
    prefix,
    ...Object.keys(cli.optionalDependencies ?? {}).map(
      (name) => `node_modules/${name}`,
    ),
  ];
  const packages = {};
  for (const key of wanted) {
    const entry = lock.packages[key];
    if (!entry?.integrity?.startsWith("sha512-")) {
      throw new Error(`package-lock.json : ${key} sans empreinte sha512`);
    }
    if (!entry.resolved?.startsWith(REGISTRY)) {
      throw new Error(`package-lock.json : ${key} hors du registre npm`);
    }
    // Dépendance du projet minimal, plus « dev » comme dans le dépôt.
    const kept = { ...entry };
    delete kept.dev;
    delete kept.devOptional;
    packages[key] = kept;
  }
  const dependencies = { [CLI]: cli.version };
  return {
    packageJson: { name: NAME, private: true, dependencies },
    packageLock: {
      name: NAME,
      lockfileVersion: 3,
      requires: true,
      packages: { "": { name: NAME, dependencies }, ...packages },
    },
  };
}

function main() {
  const [dir] = process.argv.slice(2);
  if (!dir) throw new Error("dossier manquant");
  const root = path.resolve(import.meta.dirname, "..");
  const lock = JSON.parse(
    readFileSync(path.join(root, "package-lock.json"), "utf8"),
  );
  const { packageJson, packageLock } = signerProject(lock);
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    path.join(dir, "package.json"),
    `${JSON.stringify(packageJson, null, 2)}\n`,
  );
  writeFileSync(
    path.join(dir, "package-lock.json"),
    `${JSON.stringify(packageLock, null, 2)}\n`,
  );
  console.log(`${dir} : ${CLI} ${packageJson.dependencies[CLI]}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(`release-signer : ${error.message}`);
    process.exit(1);
  }
}
