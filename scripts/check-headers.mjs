// Vérifie la concordance entre THIRD_PARTY_LICENSES (partie 1) et les fichiers
// repris d'Ollama : chaque fichier listé existe et porte l'en-tête
// d'attribution, et chaque fichier du dépôt qui porte l'en-tête est listé.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = path.resolve(import.meta.dirname, "..");
export const MARKER = "SPDX-FileCopyrightText: Ollama";
const LINE = /^ {2}(\S+) <- (\S+) : (copied|adapted|translated to Rust)$/;
// Fichiers qui citent le marqueur sans être repris d'Ollama.
const IGNORED = new Set(["THIRD_PARTY_LICENSES", "scripts/check-headers.mjs"]);

/**
 * Fichiers de la partie 1 : chemin ChatonLM -> fichiers d'Ollama dont il est
 * repris (une ligne par fichier d'Ollama quand il y en a plusieurs).
 */
export function parseListed(notices) {
  const listed = new Map();
  const errors = [];
  for (const line of notices.split("== Part 2.")[0].split("\n")) {
    // Seules les lignes indentées de la liste décrivent des fichiers.
    if (!line.startsWith("  ") || !line.includes(" <- ")) continue;
    const match = LINE.exec(line);
    if (!match) {
      errors.push(`THIRD_PARTY_LICENSES : ligne mal formée « ${line.trim()} »`);
      continue;
    }
    listed.set(match[1], [...(listed.get(match[1]) ?? []), match[2]]);
  }
  return { listed, errors };
}

/** Erreurs d'en-tête d'un fichier listé : chaque origine doit y figurer. */
export function headerErrors(file, origins, content) {
  return origins
    .filter((origin) => !content.includes(MARKER) || !content.includes(origin))
    .map(
      (origin) =>
        `${file} : en-tête d'attribution manquant ou sans « ${origin} »`,
    );
}

function main() {
  const notices = readFileSync(path.join(root, "THIRD_PARTY_LICENSES"), "utf8");
  const { listed, errors } = parseListed(notices);

  for (const [file, origins] of listed) {
    let content;
    try {
      content = readFileSync(path.join(root, file), "utf8");
    } catch {
      errors.push(`${file} : listé dans THIRD_PARTY_LICENSES mais introuvable`);
      continue;
    }
    errors.push(...headerErrors(file, origins, content));
  }

  // Tous les fichiers suivis ou à ajouter, hors fichiers ignorés par git.
  const files = execFileSync(
    "git",
    ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    { cwd: root, encoding: "utf8" },
  )
    .split("\0")
    .filter(Boolean);

  for (const file of files) {
    if (IGNORED.has(file) || listed.has(file)) continue;
    let content;
    try {
      content = readFileSync(path.join(root, file), "utf8");
    } catch {
      continue;
    }
    if (content.includes(MARKER)) {
      errors.push(`${file} : porte l'en-tête Ollama mais n'est pas listé`);
    }
  }

  for (const error of errors) console.error(error);
  if (errors.length > 0) process.exit(1);
  console.log(
    `${listed.size} fichier(s) repris d'Ollama, attribution conforme.`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
