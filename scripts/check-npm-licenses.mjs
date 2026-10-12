// Vérifie que les paquets npm livrés avec ChatonLM ont une licence de la liste
// blanche (scripts/lib/spdx.mjs). Sont contrôlés :
// - les paquets réellement embarqués dans le bundle, d'après la liste que
//   produit « vite build » (dist/.vite/license.json), qu'ils soient déclarés
//   en dependencies ou en devDependencies ;
// - les paquets qui n'entrent que par le CSS, invisibles pour cette liste ;
// - toutes les dépendances de production, directes et transitives.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { isAllowed } from "./lib/spdx.mjs";

const root = path.resolve(import.meta.dirname, "..");

/**
 * Paquets embarqués par le CSS (Tailwind et ses greffons, imports CSS) :
 * typography n'entre que par @plugin ; katex fournit aussi la CSS et les
 * polices (sous OFL-1.1, voir THIRD_PARTY_LICENSES partie 3).
 */
const CSS_PACKAGES = ["tailwindcss", "@tailwindcss/typography", "katex"];

/**
 * Licences vérifiées à la main pour des paquets dont le package.json ne
 * déclare rien, épinglées à la version exacte : une autre version repasse
 * par le contrôle normal. Ne servent que si le champ license est vide.
 * - khroma@2.1.0 : champ license absent, fichier « license » MIT
 *   (Copyright (c) 2019-present Fabio Spampinato, Andrew Maney). Dépendance
 *   de mermaid (streamdown -> mermaid -> khroma), jamais embarquée : mermaid
 *   est remplacé par un module vide (vite.config.ts), et
 *   scripts/check-bundle.mjs vérifie que ni l'un ni l'autre n'est dans le
 *   bundle.
 */
export const VERIFIED_LICENSES = { "khroma@2.1.0": "MIT" };

/** Licence déclarée, ou à défaut celle vérifiée à la main pour cette version. */
export function effectiveLicense(name, version, declared) {
  return declared || VERIFIED_LICENSES[`${name}@${version}`] || "";
}

function licenseOf(pkg) {
  if (typeof pkg.license === "string") return pkg.license;
  if (pkg.license?.type) return pkg.license.type;
  if (Array.isArray(pkg.licenses)) {
    return pkg.licenses.map((l) => l.type ?? l).join(" OR ");
  }
  return "";
}

function readJson(file) {
  return JSON.parse(readFileSync(path.join(root, file), "utf8"));
}

function main() {
  let bundled;
  try {
    bundled = readJson("dist/.vite/license.json");
  } catch {
    console.error(
      "dist/.vite/license.json introuvable : lancer d'abord vite build.",
    );
    process.exit(1);
  }

  const checked = [
    ...bundled.map((pkg) => ({
      name: pkg.name,
      version: pkg.version,
      license: pkg.identifier ?? "",
      origin: "bundle",
    })),
    ...CSS_PACKAGES.map((name) => {
      const pkg = readJson(`node_modules/${name}/package.json`);
      return {
        name,
        version: pkg.version,
        license: licenseOf(pkg),
        origin: "CSS",
      };
    }),
    ...JSON.parse(
      execFileSync("npm", ["query", ".prod"], { cwd: root, encoding: "utf8" }),
    )
      .filter((pkg) => pkg.name !== "chatonlm")
      .map((pkg) => ({
        name: pkg.name,
        version: pkg.version,
        license: licenseOf(pkg),
        origin: "production",
      })),
  ];

  const refused = checked.filter(
    (pkg) => !isAllowed(effectiveLicense(pkg.name, pkg.version, pkg.license)),
  );
  for (const pkg of refused) {
    console.error(
      `Licence refusée (${pkg.origin}) : ${pkg.name}@${pkg.version} (${pkg.license || "aucune"})`,
    );
  }
  if (refused.length > 0) process.exit(1);

  // Une exception qui ne sert plus (paquet retiré ou mis à jour) est à enlever.
  const used = new Set(checked.map((pkg) => `${pkg.name}@${pkg.version}`));
  for (const key of Object.keys(VERIFIED_LICENSES)) {
    if (!used.has(key)) {
      console.log(`Exception inutilisée dans VERIFIED_LICENSES : ${key}`);
    }
  }
  console.log(
    `${bundled.length} paquets embarqués, ${CSS_PACKAGES.length} par le CSS, licences conformes.`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
