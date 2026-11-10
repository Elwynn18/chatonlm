#!/usr/bin/env node
// Vérification des packs CUDA d'une Release (release.yml, après leur
// signature) : pour chaque système, l'archive du pack, son manifeste
// (scripts/cuda-pack-manifest.sh : taille et SHA-256 de l'archive, nom, build)
// et la signature du manifeste par la clé publique de l'app (version et nom
// de fichier signés), comme l'app les vérifiera (engine/cuda/pack.rs).
//
// Usage : node scripts/cuda-pack-verify.mjs --dir <dossier des packs>
//   --signatures <dossier des .sig> --version <version de l'app>
//   --config src-tauri/tauri.conf.json
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { checkSignature, configuredPublicKey } from "./release-manifest.mjs";

/** Systèmes dont une Release porte toujours le pack. */
export const SYSTEMS = ["linux", "windows"];

const MANIFEST_NAME = /^chatonlm-cuda-(b\d+)-(linux|windows)-x64\.json$/;

/**
 * Erreurs des packs : `files` (nom → contenu) du dossier des packs,
 * `signatures` (nom du .sig → texte), `version` de l'app, clé publique
 * `pubkey` (texte de tauri.conf.json, base64).
 */
export function packErrors({
  files,
  signatures,
  version,
  pubkey,
  systems = SYSTEMS,
}) {
  const errors = [];
  const seen = new Set();
  for (const [name, bytes] of files) {
    const match = MANIFEST_NAME.exec(name);
    if (!match) continue;
    const [, build, system] = match;
    seen.add(system);
    let manifest;
    try {
      manifest = JSON.parse(bytes.toString("utf8"));
    } catch {
      errors.push(`${name} : JSON illisible`);
      continue;
    }
    const archive = `chatonlm-cuda-${build}-${system}-x64.${system === "linux" ? "tar.gz" : "zip"}`;
    if (manifest.build !== build || manifest.file !== archive) {
      errors.push(
        `${name} : build ou fichier inattendu (${manifest.build}, ${manifest.file})`,
      );
    }
    if (
      !/^v[0-9.]+\+chatonlm\.cuda\.[0-9a-f]{12}$/.test(manifest.version ?? "")
    ) {
      errors.push(`${name} : version inattendue (${manifest.version})`);
    }
    const data = files.get(archive);
    if (!data) {
      errors.push(`${name} : archive ${archive} absente`);
    } else {
      const sum = createHash("sha256").update(data).digest("hex");
      if (manifest.size !== data.length || manifest.sha256 !== sum) {
        errors.push(`${name} : taille ou SHA-256 différents de ${archive}`);
      }
    }
    const signature = signatures.get(`${name}.sig`);
    if (!signature) {
      errors.push(`${name} : signature absente`);
      continue;
    }
    try {
      checkSignature(signature, { version, name, bytes, pubkey });
    } catch (error) {
      errors.push(error.message);
    }
  }
  for (const system of systems) {
    if (!seen.has(system)) errors.push(`pack CUDA ${system} absent`);
  }
  return errors;
}

function readDir(dir, filter) {
  return new Map(
    readdirSync(dir)
      .filter(filter)
      .map((name) => [name, readFileSync(path.join(dir, name))]),
  );
}

function main(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    if (
      !["dir", "signatures", "version", "config"].includes(key) ||
      argv[i + 1] === undefined
    ) {
      console.error(`argument inattendu : ${argv[i]}`);
      return 2;
    }
    options[key] = argv[i + 1];
  }
  if (!options.dir || !options.version || !options.config) {
    console.error(
      "usage : cuda-pack-verify.mjs --dir D --signatures S --version V --config tauri.conf.json",
    );
    return 2;
  }
  const signaturesDir = options.signatures ?? options.dir;
  const files = readDir(options.dir, (name) =>
    name.startsWith("chatonlm-cuda-"),
  );
  const signatures = existsSync(signaturesDir)
    ? new Map(
        [...readDir(signaturesDir, (name) => name.endsWith(".json.sig"))].map(
          ([name, bytes]) => [name, bytes.toString("utf8")],
        ),
      )
    : new Map();
  const pubkey = configuredPublicKey(
    JSON.parse(readFileSync(options.config, "utf8")),
  );
  const errors = packErrors({
    files,
    signatures,
    version: options.version,
    pubkey,
  });
  for (const error of errors) console.error(`✗ ${error}`);
  if (errors.length === 0)
    console.log(`✓ packs CUDA vérifiés (${SYSTEMS.join(", ")})`);
  return errors.length === 0 ? 0 : 1;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  process.exit(main(process.argv.slice(2)));
}
