#!/usr/bin/env node
// Toolkit CUDA de la construction du pack CUDA sous Windows (CI) : archives
// « redist » de NVIDIA listées par l'index épinglé de packaging/cuda.env
// (empreinte de l'index vérifiée, puis taille et SHA-256 de chaque archive),
// gardées dans .tools/cuda-redist/ et dépliées l'une sur l'autre dans le
// dossier donné, comme le fait la CI de llama.cpp. Rien n'est installé hors de
// ce dossier ; l'appelant pose CUDA_PATH et PATH.
//
// Usage : node scripts/fetch-cuda-toolkit.mjs <dossier> [plateforme]
//   plateforme : windows-x86_64 (défaut) ou linux-x86_64.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** Composants du toolkit nécessaires à nvcc et à ggml-cuda. */
export const TOOLKIT_COMPONENTS = [
  "cuda_crt",
  "cuda_cudart",
  "cuda_nvcc",
  "cuda_nvrtc",
  "libcublas",
  "libnvvm",
  "cuda_nvtx",
  "cuda_profiler_api",
  "cccl",
];

/** Variables d'un fichier .env (lignes NOM=valeur, commentaires ignorés). */
export function parseEnv(text) {
  const env = {};
  for (const line of text.split("\n")) {
    const match = /^([A-Z0-9_]+)=(.*)$/.exec(line.trim());
    if (match) env[match[1]] = match[2];
  }
  return env;
}

/**
 * Archive d'un composant pour une plateforme, lue dans l'index de NVIDIA :
 * { url, size, sha256, name } ; erreur si l'index ne l'a pas.
 */
export function redistArchive(index, baseUrl, component, platform) {
  const entry = index[component]?.[platform];
  if (
    !entry ||
    typeof entry.relative_path !== "string" ||
    !/^[0-9a-f]{64}$/.test(entry.sha256 ?? "") ||
    !(Number(entry.size) > 0)
  ) {
    throw new Error(`${component} (${platform}) absent de l'index de NVIDIA`);
  }
  if (
    entry.relative_path.includes("..") ||
    entry.relative_path.startsWith("/")
  ) {
    throw new Error(`chemin refusé : ${entry.relative_path}`);
  }
  return {
    url: new URL(entry.relative_path, baseUrl).href,
    size: Number(entry.size),
    sha256: entry.sha256,
    name: path.posix.basename(entry.relative_path),
  };
}

const sha256Of = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function download(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} : HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

function unpack(archive, into) {
  const args = archive.endsWith(".zip")
    ? ["unzip", ["-q", "-o", archive, "-d", into]]
    : ["tar", ["-xJf", archive, "-C", into]];
  const result = spawnSync(args[0], args[1], { stdio: "inherit" });
  if (result.status !== 0) throw new Error(`extraction en échec : ${archive}`);
}

async function main() {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const [dest, platform = "windows-x86_64"] = process.argv.slice(2);
  if (!dest) {
    console.error("usage : fetch-cuda-toolkit.mjs <dossier> [plateforme]");
    process.exit(2);
  }
  const env = parseEnv(
    readFileSync(path.join(root, "packaging/cuda.env"), "utf8"),
  );
  const cache = path.join(root, ".tools/cuda-redist");
  mkdirSync(cache, { recursive: true });
  const indexBytes = await download(
    new URL(env.CUDA_INDEX, env.CUDA_REDIST_URL).href,
  );
  if (sha256Of(indexBytes) !== env.CUDA_INDEX_SHA256) {
    throw new Error(`${env.CUDA_INDEX} : empreinte inattendue`);
  }
  const index = JSON.parse(indexBytes.toString("utf8"));
  mkdirSync(dest, { recursive: true });
  for (const component of TOOLKIT_COMPONENTS) {
    const archive = redistArchive(
      index,
      env.CUDA_REDIST_URL,
      component,
      platform,
    );
    const file = path.join(cache, archive.name);
    let ok = false;
    if (existsSync(file)) {
      const bytes = readFileSync(file);
      ok = bytes.length === archive.size && sha256Of(bytes) === archive.sha256;
    }
    if (!ok) {
      console.log(`== ${archive.name}`);
      const bytes = await download(archive.url);
      if (bytes.length !== archive.size || sha256Of(bytes) !== archive.sha256) {
        throw new Error(`${archive.name} : taille ou empreinte inattendue`);
      }
      writeFileSync(file, bytes);
    }
    // Chaque archive a un dossier racine (<nom>-archive) : son contenu est
    // déplié sur le dossier du toolkit.
    const temp = mkdtempSync(path.join(cache, "unpack-"));
    try {
      unpack(file, temp);
      for (const top of readdirSync(temp)) {
        cpSync(path.join(temp, top), dest, { recursive: true, force: true });
      }
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  }
  console.log(dest);
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main().catch((error) => {
    console.error(`fetch-cuda-toolkit : ${error.message}`);
    process.exit(1);
  });
}
