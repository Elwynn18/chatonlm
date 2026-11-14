// Vérifie que les paquets npm livrés avec ChatonLM ont une licence de la liste
// blanche (scripts/lib/spdx.mjs). Sont contrôlés :
// - les paquets réellement embarqués dans le bundle, d'après la liste que
//   produit « vite build » (dist/.vite/license.json), qu'ils soient déclarés
//   en dependencies ou en devDependencies ;
// - les paquets qui n'entrent que par le CSS, invisibles pour cette liste ;
// - toutes les dépendances de production, directes et transitives ;
// - les paquets compilés dans les fichiers déjà construits que livrent
//   pdfmake et docx (PREBUILT), que la liste de vite build ne voit pas.
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
 * - png-js@2.0.0 : champ license absent, fichier LICENSE MIT (Copyright (c)
 *   2017 Devon Govett). Dépendance de pdfkit (pdfmake -> pdfkit -> png-js),
 *   compilée dans la version pour navigateur de pdfmake (voir PREBUILT).
 */
export const VERIFIED_LICENSES = {
  "khroma@2.1.0": "MIT",
  "png-js@2.0.0": "MIT",
};

/**
 * Paquets compilés dans un fichier déjà construit d'un paquet embarqué :
 * build/pdfmake.js de pdfmake (webpack, version pour navigateur, chargée par
 * src/lib/export/pdf.ts) et dist/index.mjs de docx (rolldown). vite build
 * n'y voit que pdfmake et docx. Liste relevée dans la carte des sources de
 * pdfmake et dans les commentaires de régions de docx (`list`), licences
 * vérifiées à la main le 14 novembre 2026 (registre npm et fichiers LICENSE,
 * aux versions embarquées) ; notices dans THIRD_PARTY_LICENSES partie 3.
 * Épinglé à la version du paquet : une autre version est refusée tant que la
 * liste n'est pas relue. « webpack » : le code d'amorce de webpack (MIT).
 */
export const PREBUILT = {
  "pdfmake@0.3.13": {
    list: () => sourceMapPackages("node_modules/pdfmake/build/pdfmake.js.map"),
    packages: {
      pdfmake: "MIT",
      webpack: "MIT",
      "@noble/ciphers": "MIT",
      "@noble/hashes": "MIT",
      "@swc/helpers": "Apache-2.0",
      "base64-js": "MIT",
      brotli: "MIT",
      buffer: "MIT",
      clone: "MIT",
      "core-js": "MIT",
      dfa: "MIT",
      events: "MIT",
      "expose-loader": "MIT",
      "fast-deep-equal": "MIT",
      fflate: "MIT",
      "file-saver": "MIT",
      fontkit: "MIT",
      ieee754: "BSD-3-Clause",
      linebreak: "MIT",
      pdfkit: "MIT",
      "png-js": "MIT",
      process: "MIT",
      restructure: "MIT",
      sax: "BlueOak-1.0.0",
      "tiny-inflate": "MIT",
      tslib: "0BSD",
      "unicode-properties": "MIT",
      "unicode-trie": "MIT",
      xmldoc: "MIT",
    },
  },
  "docx@9.15.1": {
    list: () => regionPackages("node_modules/docx/dist/index.mjs"),
    packages: {
      docx: "MIT",
      "@oxc-project/runtime": "MIT",
      fflate: "MIT",
      nanoid: "MIT",
    },
  },
};

/** Sources d'une carte webpack hors node_modules, et leur paquet. */
const MAP_SOURCES = [
  [/^webpack:\/\/[^/]+\/\.\/src\//, "pdfmake"],
  [/^webpack:\/\/[^/]+\/webpack\//, "webpack"],
  // Carte de xmldoc reprise par webpack (xmldoc.ts, à côté de son lib/).
  [/^webpack:\/\/[^/]+\/\.\.\/xmldoc\.ts$/, "xmldoc"],
];

/** Paquet d'un chemin « …/node_modules/<paquet>/… », ou null. */
function packageOf(source) {
  return /node_modules\/((?:@[^/]+\/)?[^/]+)\//.exec(source)?.[1] ?? null;
}

/** Paquets d'une carte des sources webpack ; « ? <source> » si inconnu. */
export function sourceMapPackages(file) {
  const { sources } = readJson(file);
  return new Set(
    sources.map(
      (source) =>
        packageOf(source) ??
        MAP_SOURCES.find(([pattern]) => pattern.test(source))?.[1] ??
        `? ${source}`,
    ),
  );
}

/**
 * Paquets d'un module construit par rolldown, d'après ses commentaires de
 * régions (« // node_modules/x/… », « // \0@scope+nom@version/… ») ; le
 * reste (src/) est le paquet lui-même.
 */
export function regionPackages(file) {
  const [own] = path.basename(path.dirname(path.dirname(file))).split("@");
  const names = new Set([own]);
  const text = readFileSync(path.join(root, file), "utf8");
  for (const [, region] of text.matchAll(/^\/\/ (\S+)$/gm)) {
    if (region.startsWith("src/")) continue;
    const helper = /^\\0(@[^+]+)\+([^@/]+)@/.exec(region);
    if (helper) names.add(`${helper[1]}/${helper[2]}`);
    else if (region.startsWith("\\0rolldown/")) continue;
    else names.add(packageOf(`/${region}`) ?? `? ${region}`);
  }
  return names;
}

/**
 * Erreurs des paquets construits d'avance : version relue, paquets compilés
 * tous connus, licences permises. Les entrées sans paquet installé sont
 * ignorées (paquet retiré : signalé comme inutilisé).
 */
export function checkPrebuilt(prebuilt = PREBUILT) {
  const errors = [];
  const notes = [];
  let count = 0;
  for (const [key, { list, packages }] of Object.entries(prebuilt)) {
    const name = key.slice(0, key.lastIndexOf("@"));
    let installed;
    try {
      installed = readJson(`node_modules/${name}/package.json`).version;
    } catch {
      notes.push(`Entrée inutilisée dans PREBUILT : ${key}`);
      continue;
    }
    if (`${name}@${installed}` !== key) {
      errors.push(
        `${name}@${installed} : paquets compilés dans son fichier construit à relire (PREBUILT de scripts/check-npm-licenses.mjs, relu pour ${key})`,
      );
      continue;
    }
    const found = list();
    for (const embedded of found) {
      const license = packages[embedded];
      if (license === undefined) {
        errors.push(
          `Paquet compilé dans ${key} non relu : ${embedded} (PREBUILT de scripts/check-npm-licenses.mjs)`,
        );
      } else if (!isAllowed(license)) {
        errors.push(
          `Licence refusée (compilé dans ${key}) : ${embedded} (${license})`,
        );
      }
    }
    for (const listed of Object.keys(packages)) {
      if (!found.has(listed)) {
        notes.push(
          `Paquet absent de ${key}, à retirer de PREBUILT : ${listed}`,
        );
      }
    }
    count += found.size;
  }
  return { errors, notes, count };
}

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
  const prebuilt = checkPrebuilt();
  for (const error of prebuilt.errors) console.error(error);
  for (const note of prebuilt.notes) console.log(note);
  if (refused.length > 0 || prebuilt.errors.length > 0) process.exit(1);

  // Une exception qui ne sert plus (paquet retiré ou mis à jour) est à enlever.
  const used = new Set(checked.map((pkg) => `${pkg.name}@${pkg.version}`));
  for (const key of Object.keys(VERIFIED_LICENSES)) {
    if (!used.has(key)) {
      console.log(`Exception inutilisée dans VERIFIED_LICENSES : ${key}`);
    }
  }
  console.log(
    `${bundled.length} paquets embarqués, ${CSS_PACKAGES.length} par le CSS, ${prebuilt.count} compilés dans des fichiers construits, licences conformes.`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
