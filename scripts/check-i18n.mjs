#!/usr/bin/env node
// Contrôle des traductions (anglais, espagnol, français) :
// - mêmes fichiers et mêmes clés dans chaque langue (l'anglais est la
//   source), même nature (texte ou pluriel), aucun texte vide ;
// - mêmes valeurs « {nom} » et mêmes balises « <nom> » dans chaque langue ;
//   un pluriel a au moins les formes « one » et « other » ;
// - chaque clé employée dans le code existe (t("…"), k="…", t!("…") du
//   moteur ; code de la page et du panneau VS Code, vscode/webview) et
//   chaque clé des catalogues est employée ;
// - textes d'interface en dur suspects : chaînes en anglais dans le code du
//   frontend (hors JSX, déjà vu par ESLint) et erreurs ou notifications du
//   moteur. Une ligne voulue (journal, identifiant...) porte « i18n-ignore »
//   sur elle-même ou sur la ligne précédente.
//
// Usage : node scripts/check-i18n.mjs [--list-suspects]

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const LOCALES = path.join(ROOT, "src/locales");
const LANGS = ["en", "es", "fr"];
const SOURCE = "en";
/** Espace du moteur : lu par src-tauri/src/i18n.rs, pas par la page. */
const BACKEND = "backend";
const PLURAL_FORMS = new Set(["zero", "one", "two", "few", "many", "other"]);

const errors = [];
const fail = (message) => errors.push(message);

function walk(dir, filter, out = []) {
  for (const name of readdirSync(dir)) {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) walk(full, filter, out);
    else if (filter(full)) out.push(full);
  }
  return out;
}

const rel = (file) => path.relative(ROOT, file);

/** Textes et pluriels d'un catalogue, par chemin pointé. */
function flatten(node, prefix, out, file) {
  for (const [key, value] of Object.entries(node)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") {
      out.set(full, { kind: "text", forms: { text: value } });
    } else if (
      value &&
      typeof value === "object" &&
      "other" in value &&
      Object.values(value).every((form) => typeof form === "string")
    ) {
      for (const form of Object.keys(value)) {
        if (!PLURAL_FORMS.has(form)) {
          fail(`${file} : ${full} : forme de pluriel inconnue « ${form} »`);
        }
      }
      out.set(full, { kind: "plural", forms: value });
    } else if (value && typeof value === "object") {
      flatten(value, full, out, file);
    } else {
      fail(`${file} : ${full} : ni texte, ni groupe, ni pluriel`);
    }
  }
  return out;
}

/**
 * Typographie (contrat 44) : en français, espace insécable (U+00A0 ou
 * U+202F) avant « : ; ! ? % » et dans les guillemets « » ; en espagnol,
 * pas d'espace avant « ; ! ? % » ni d'insécable avant « : ». Une espace normale y couperait la
 * ligne au mauvais endroit (l'outil Edit en remet parfois une).
 */
function typography(lang, text) {
  // Balises et valeurs ne comptent pas (« <kbd>Ctrl</kbd> : »).
  const plain = text.replace(/\{\w+\}/g, "x");
  if (lang === "fr") {
    if (/ [:;!?%»]/.test(plain) || /« /.test(plain)) {
      return "espace normale avant « : ; ! ? % » ou dans « » (U+00A0 attendue)";
    }
  }
  // Espagnol : « : » seul entre deux espaces reste permis (liste de
  // caractères), une espace insécable non (reste d'une copie du français).
  if (lang === "es" && /[ \u00a0\u202f][;!?%]|[\u00a0\u202f]:/.test(plain)) {
    return "espace avant la ponctuation (pas d'espace en espagnol)";
  }
  return null;
}

const placeholders = (text) =>
  [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
const tags = (text) =>
  [...text.matchAll(/<\/?(\w+)\/?>/g)].map((match) => match[0]).sort();
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

// --- Catalogues ---

/** catalogs[lang][namespace] = Map(clé → entrée). */
const catalogs = {};
for (const lang of LANGS) {
  catalogs[lang] = {};
  for (const name of readdirSync(path.join(LOCALES, lang))) {
    if (!name.endsWith(".json")) continue;
    const file = path.join(LOCALES, lang, name);
    let json;
    try {
      json = JSON.parse(readFileSync(file, "utf8"));
    } catch (error) {
      fail(`${rel(file)} : JSON invalide (${error.message})`);
      continue;
    }
    catalogs[lang][name.slice(0, -5)] = flatten(json, "", new Map(), rel(file));
  }
}

const namespaces = Object.keys(catalogs[SOURCE]).sort();
for (const lang of LANGS) {
  const own = Object.keys(catalogs[lang]).sort();
  if (!same(own, namespaces)) {
    fail(
      `src/locales/${lang} : fichiers ${own.join(", ")} au lieu de ${namespaces.join(", ")}`,
    );
  }
}

for (const ns of namespaces) {
  const source = catalogs[SOURCE][ns];
  for (const lang of LANGS) {
    const catalog = catalogs[lang][ns];
    if (!catalog) continue;
    const where = `src/locales/${lang}/${ns}.json`;
    for (const [key, entry] of catalog) {
      for (const [form, text] of Object.entries(entry.forms)) {
        if (text.trim() === "") fail(`${where} : ${key} (${form}) vide`);
        else if (text !== text.trim()) {
          fail(`${where} : ${key} (${form}) : espaces en début ou en fin`);
        }
      }
      for (const [form, text] of Object.entries(entry.forms)) {
        const problem = typography(lang, text);
        if (problem) fail(`${where} : ${key} (${form}) : ${problem}`);
      }
      if (entry.kind === "plural") {
        for (const form of ["one", "other"]) {
          if (!(form in entry.forms)) {
            fail(`${where} : ${key} : pluriel sans forme « ${form} »`);
          }
        }
      }
      if (lang === SOURCE) continue;
      const original = source.get(key);
      if (!original) {
        fail(`${where} : ${key} absente de l'anglais (clé en trop)`);
        continue;
      }
      if (original.kind !== entry.kind) {
        fail(`${where} : ${key} : ${entry.kind} au lieu de ${original.kind}`);
        continue;
      }
      // Valeurs et balises : celles de la forme « other » (texte : le texte).
      const reference = original.forms.other ?? original.forms.text;
      for (const [form, text] of Object.entries(entry.forms)) {
        const expected = placeholders(reference).filter(
          (name) => entry.kind === "text" || name !== "count",
        );
        const found = placeholders(text).filter(
          (name) => entry.kind === "text" || name !== "count",
        );
        if (!same(found, expected)) {
          fail(
            `${where} : ${key} (${form}) : valeurs {${found.join("}, {")}} au lieu de {${expected.join("}, {")}}`,
          );
        }
        if (!same(tags(text), tags(reference))) {
          fail(
            `${where} : ${key} (${form}) : balises ${tags(text).join(" ")} au lieu de ${tags(reference).join(" ")}`,
          );
        }
      }
    }
    if (lang !== SOURCE) {
      for (const key of source.keys()) {
        if (!catalog.has(key)) fail(`${where} : ${key} manquante`);
      }
    }
  }
}

// --- Clés employées ---

const isFrontendSource = (file) =>
  /\.(ts|tsx)$/.test(file) &&
  !/\.test\.(ts|tsx)$/.test(file) &&
  !file.includes(`${path.sep}src${path.sep}test${path.sep}`) &&
  !file.includes(`${path.sep}bindings${path.sep}`) &&
  !file.endsWith("routeTree.gen.ts");
// Page de l'app, et panneau de l'extension VS Code (mêmes catalogues, espace
// « vscode »).
const WEBVIEW = path.join(ROOT, "vscode/webview");
const frontendFiles = [
  ...walk(path.join(ROOT, "src"), isFrontendSource),
  ...(existsSync(WEBVIEW) ? walk(WEBVIEW, isFrontendSource) : []),
];
const rustFiles = walk(path.join(ROOT, "src-tauri/src"), (file) =>
  file.endsWith(".rs"),
);

const frontendNamespaces = namespaces.filter((ns) => ns !== BACKEND);
const keyPattern = new RegExp(
  `["'\`]((?:${frontendNamespaces.join("|")})\\.[A-Za-z0-9_.]+?)(\\.\\$\\{|["'\`])`,
  "g",
);
const used = new Set();
const usedPrefixes = new Set();
/** Code sans ses commentaires (une clé citée en commentaire n'est pas employée). */
const withoutComments = (text) =>
  text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");

for (const file of frontendFiles) {
  const text = withoutComments(readFileSync(file, "utf8"));
  for (const match of text.matchAll(keyPattern)) {
    if (match[2] === "${" || match[2] === ".${") usedPrefixes.add(match[1]);
    else used.add(match[1]);
  }
}
const allFrontendKeys = new Set();
for (const ns of frontendNamespaces) {
  for (const key of catalogs[SOURCE][ns]?.keys() ?? []) {
    allFrontendKeys.add(`${ns}.${key}`);
  }
}
for (const key of used) {
  // Préfixe d'une clé construite (`common.units.${unit}`) : déjà compté.
  if (
    !allFrontendKeys.has(key) &&
    ![...allFrontendKeys].some((k) => k.startsWith(`${key}.`))
  ) {
    fail(`clé inconnue employée dans le frontend : ${key}`);
  }
}
for (const key of allFrontendKeys) {
  const prefixed = [...usedPrefixes].some((prefix) =>
    key.startsWith(`${prefix}.`),
  );
  if (!used.has(key) && !prefixed) {
    fail(`clé jamais employée dans le frontend : ${key}`);
  }
}

const backendKeys = catalogs[SOURCE][BACKEND] ?? new Map();
const backendUsed = new Set();
for (const file of rustFiles) {
  // Sans les commentaires (exemples de la documentation de i18n.rs).
  const text = readFileSync(file, "utf8")
    .split("\n")
    .filter((line) => !line.trim().startsWith("//"))
    .join("\n");
  for (const match of text.matchAll(/\btn?!\(\s*"([^"]+)"/g)) {
    backendUsed.add(match[1]);
    if (!backendKeys.has(match[1])) {
      fail(`${rel(file)} : clé inconnue « ${match[1]} » (backend.json)`);
    }
  }
}
for (const key of backendKeys.keys()) {
  if (!backendUsed.has(key)) {
    fail(`clé jamais employée par le moteur : ${BACKEND}.${key}`);
  }
}

// --- Textes en dur suspects ---

const suspects = [];
const ignored = (lines, index) =>
  lines[index].includes("i18n-ignore") ||
  (index > 0 && lines[index - 1].includes("i18n-ignore"));

// Frontend : chaîne d'au moins deux mots dont le premier commence par une
// majuscule (« Server address », « Failed to load »), hors commentaires,
// imports, journaux et classes CSS.
const ENGLISH = /(["'`])([A-Z][a-z]+(?:[ ,.'’-]+[A-Za-z]+)+[.!?…:]?)\1/g;
for (const file of frontendFiles) {
  const lines = readFileSync(file, "utf8").split("\n");
  let inComment = false;
  lines.forEach((line, index) => {
    const trimmed = line.trim();
    if (inComment) {
      if (trimmed.includes("*/")) inComment = false;
      return;
    }
    if (trimmed.startsWith("/*")) {
      inComment = !trimmed.includes("*/");
      return;
    }
    if (
      trimmed.startsWith("//") ||
      trimmed.startsWith("*") ||
      trimmed.startsWith("import ") ||
      /console\.(error|warn|log|info|debug)\(/.test(line) ||
      ignored(lines, index)
    ) {
      return;
    }
    const code = line.replace(/\/\/.*$/, "");
    for (const match of code.matchAll(ENGLISH)) {
      suspects.push(`${rel(file)}:${index + 1} : ${match[0]}`);
    }
  });
}

// Moteur : erreurs montrées à l'utilisateur et notifications écrites en dur.
// Erreurs montrées (Invalid, NotFound, System, Ollama ; Internal reste un
// détail technique en anglais), échecs de la dictée, notifications et titres
// écrits en dur, même coupés sur plusieurs lignes par rustfmt.
const RUST_SUSPECT =
  /AppError::(Invalid|NotFound|System|Ollama)\(\s*(format!\(\s*)?"|End::Failed\(\s*(format!\(\s*)?"|\.(summary|body)\(\s*"|set_title\(\s*"/g;
for (const file of rustFiles) {
  // Les essais (modules et fichiers `tests`) peuvent écrire du texte en dur.
  if (path.basename(file) === "tests.rs") continue;
  const source = readFileSync(file, "utf8");
  const lines = source.split("\n");
  const testsStart = lines.findIndex((line) => /^\s*mod tests\s*\{/.test(line));
  for (const match of source.matchAll(RUST_SUSPECT)) {
    const index = source.slice(0, match.index).split("\n").length - 1;
    if (testsStart >= 0 && index > testsStart) continue;
    const line = lines[index];
    if (line.trim().startsWith("//") || ignored(lines, index)) continue;
    suspects.push(`${rel(file)}:${index + 1} : ${line.trim()}`);
  }
}

for (const suspect of suspects) {
  fail(`texte en dur suspect (t() / t!(), ou « i18n-ignore ») : ${suspect}`);
}

if (process.argv.includes("--list-suspects")) {
  for (const suspect of suspects) console.log(suspect);
  process.exit(0);
}

if (errors.length > 0) {
  for (const error of errors) console.error(`✗ ${error}`);
  console.error(`${errors.length} problème(s) de traduction.`);
  process.exit(1);
}
const count =
  [...(catalogs[SOURCE][BACKEND]?.keys() ?? [])].length + allFrontendKeys.size;
console.log(
  `Traductions conformes : ${count} textes en ${LANGS.length} langues (${namespaces.join(", ")}).`,
);
