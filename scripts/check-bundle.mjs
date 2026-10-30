// Contrôle du bundle de production, après « vite build » : ce que le rendu
// Markdown (Streamdown, KaTeX, Shiki) doit embarquer, et ce qu'il ne doit
// jamais embarquer. Les alias et greffons de vite.config.ts retirent Mermaid,
// l'entrée complète de Shiki (moteur WASM, 200 langages), rehype-raw, les
// polices KaTeX autres que woff2 et la garde not-prose des règles de
// typography : une mise à jour de Streamdown ou un import maladroit les ferait
// revenir sans qu'aucun test ne le voie. Les sources (src/, index.html) ne
// doivent pas utiliser la classe not-prose.
//
// Panneau de l'extension VS Code (--webview, vscode/dist/webview par défaut,
// build de vscode/webview.vite.config.mts) : mêmes règles, sans index.html
// (la page est écrite par l'hôte de l'extension) ; entrée index.js et feuille
// index.css à noms fixes, rien de Tauri, aucune ressource distante, seuils
// propres.
//
// Usage : node scripts/check-bundle.mjs [dossier du build, dist/ par défaut]
//         node scripts/check-bundle.mjs --webview [dossier du build]
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = path.resolve(import.meta.dirname, "..");

const MERMAID =
  "arbre de Mermaid ; mermaid doit rester remplacé par src/lib/stubs/mermaid.ts, alias de vite.config.ts";
const FULL_SHIKI =
  "entrée complète de Shiki ; seuls shiki/core, le moteur JavaScript et les langages de src/lib/highlighterCore.ts sont permis, alias /^shiki$/ de vite.config.ts";
const TEST_CODE = "code de test ; seul le code de l'app est embarqué";

/** Paquets interdits dans dist/.vite/license.json, avec la raison affichée. */
const FORBIDDEN_PACKAGES = [
  [/^mermaid$/, MERMAID],
  [/^@mermaid-js\//, MERMAID],
  [/^cytoscape/, MERMAID],
  [/^d3(-|$)/, MERMAID],
  [/^dagre/, MERMAID],
  [/^khroma$/, MERMAID],
  [/^dompurify$/, MERMAID],
  [/^lodash-es$/, MERMAID],
  [/^@shikijs\/(engine-oniguruma|themes|langs-precompiled)$/, FULL_SHIKI],
  [
    /^micromark-extension-math$/,
    "alias vers micromark-extension-llm-math non appliqué ; \\(…\\) et \\[…\\] ne seraient plus des maths",
  ],
  [
    /^(rehype-raw|parse5)$/,
    "HTML brut ; rehype-raw doit rester remplacé par src/lib/stubs/rehype-raw.ts",
  ],
  [/^vitest$/, TEST_CODE],
  [/^@testing-library\//, TEST_CODE],
];

/** Paquets qui doivent figurer dans le bundle : rendu Markdown complet. */
export const REQUIRED_PACKAGES = [
  "streamdown",
  "katex",
  "micromark-extension-llm-math",
  "@shikijs/engine-javascript",
  "@shikijs/langs",
];

/**
 * Grammaires TextMate attendues (scopeName), ni plus ni moins : les 19
 * langages de src/lib/highlighterCore.ts (bash et shell partagent
 * source.shell), plus cpp-macro et regexp, que cpp inclut, et glsl, que cpp
 * réclame aussi mais que vite.config.ts remplace par une grammaire vide.
 */
export const EXPECTED_SCOPES = [
  "source.c",
  "source.cpp",
  "source.cpp.embedded.macro",
  "source.css",
  "source.glsl",
  "source.go",
  "source.java",
  "source.js",
  "source.js.jsx",
  "source.json",
  "source.python",
  "source.regexp.python",
  "source.rust",
  "source.shell",
  "source.sql",
  "source.swift",
  "source.ts",
  "source.tsx",
  "source.yaml",
  "text.html.basic",
  "text.html.markdown",
];

/**
 * Motifs refusés par la CSP de src-tauri/tauri.conf.json (script-src 'self',
 * sans 'unsafe-eval' ni 'wasm-unsafe-eval', aucun worker).
 */
const CSP_PATTERNS = [
  [/\bnew\s+Function\s*\(/, "new Function("],
  [/\beval\s*\(/, "eval("],
  [/\bimportScripts\s*\(/, "importScripts("],
  [/\bnew\s+(?:Shared)?Worker\s*\(/, "new Worker("],
  [/\bWebAssembly\./, "WebAssembly."],
];

/** Chaînes propres au code de test, absentes du code de l'app. */
const TEST_MARKERS = [
  ["chatonlm-ipc-simulator", "simulateur IPC de src/test/simulator.ts"],
  ["Couldn't find callback id", "mocks de @tauri-apps/api"],
];

/**
 * Classes des composants par défaut de Streamdown que l'officiel ne génère
 * pas (Tailwind n'analyse pas node_modules) : générées chez nous, elles
 * changeraient les listes, les liens, les citations ou les tableaux.
 */
const ABSENT_CLASSES = [
  "ml-4",
  "list-outside",
  "list-disc",
  "list-decimal",
  "wrap-anywhere",
  "text-primary",
  "border-border",
  "divide-y",
  "border-l-4",
  "bg-muted",
];

/**
 * Garde « not-prose » des règles de @tailwindcss/typography, retirée par le
 * greffon withoutNotProse de vite.config.ts : restée dans la CSS, elle rend
 * chaque recalcul de style d'une longue réponse ~20 fois plus lent dans
 * WebKitGTK. Le retrait ne change rien tant que la classe not-prose n'est
 * jamais posée : elle ne doit pas apparaître dans les sources.
 */
const NOT_PROSE = /not-prose/;
const NOT_PROSE_REASON =
  "greffon withoutNotProse de vite.config.ts, sinon recalcul de style ~20 fois plus lent dans WebKitGTK";

/**
 * Tailles maximales en octets, non compressées, ≈ 10 % au-dessus du build de
 * l'étape 8 (4 octobre 2026) : surligneur 1 993 089 o, woff2 256 168 o ;
 * entrée : ≈ 10 % au-dessus du build des modèles d'images (16 octobre 2026,
 * 1 560 307 o : rubrique Images du ModelHub, ses textes en/es/fr chargés
 * d'emblée et l'écouteur de ses téléchargements ; Easy Link, le contexte et
 * le pré-prompt avaient rempli la marge précédente), 1 415 447 o à la lecture
 * à voix haute (9 octobre 2026 : bouton sous les réponses, état de la lecture, onglet
 * des réglages, textes en/es/fr ; la correction, Study et les mises à jour
 * avaient rempli la marge précédente), 1 274 843 o aux langues (6 octobre
 * 2026 : catalogues anglais, espagnol et français, ~100 Ko, chargés d'emblée
 * pour un changement de langue immédiat), 1 120 219 o à l'étape 16,
 * 1 018 754 o à l'étape 8 ;
 * CSS : ≈ 10 % au-dessus du build de l'onglet Study (8 octobre 2026,
 * 133 069 o : parcours, cartes, bulles, animations), 120 965 o à la
 * correction (page Proofread, onglet Keyboard shortcuts), 105 891 o à
 * l'étape 15a (ModelHub), 90 329 o à l'étape 12, 80 290 o à l'étape 8.
 */
const LIMITS = {
  jsEntry: 1_716_000,
  highlighter: 2_200_000,
  css: 146_000,
  woff2: 280_000,
};

/**
 * Seuils du panneau VS Code, ≈ 10 % au-dessus du premier build (lot 1b,
 * 30 octobre 2026) : entrée 1 091 010 o (React, rendu des réponses, les
 * catalogues en/es/fr de toute l'app, chargés par src/i18n), surligneur
 * 1 993 080 o, CSS 138 840 o (thème de l'app et classes de src/ et du
 * panneau), woff2 256 168 o (KaTeX).
 */
export const WEBVIEW_LIMITS = {
  jsEntry: 1_200_000,
  highlighter: 2_200_000,
  css: 153_000,
  woff2: 280_000,
};

/** Entrée et feuille du panneau, à noms fixes : l'hôte les lit par leur nom. */
export const WEBVIEW_ENTRY = "index.js";
export const WEBVIEW_CSS = "index.css";

/** Code de Tauri, absent du panneau (src/lib/appHost.ts remplacé au build). */
const TAURI_MARKERS = ["__TAURI_INTERNALS__", "__TAURI__", "@tauri-apps"];

/** Ressource distante : CSP du panneau sans réseau, rien ne doit en venir. */
const REMOTE_CSS = /(?:url\(\s*["']?|@import\s+["'])(?:https?:)?\/\//i;
const REMOTE_IMPORT =
  /(?:\bimport\s*\(\s*|\bfrom\s*|\bimport\s*)["'`](?:https?:)?\/\//i;

const LIMIT_LABELS = {
  jsEntry: "JS d'entrée",
  highlighter: "morceau du surligneur",
  css: "CSS",
  woff2: "polices woff2",
};

const NOT_WOFF2 =
  "polices KaTeX en woff2 seulement ; greffon katexWoff2Only de vite.config.ts";
const FORBIDDEN_EXTENSIONS = {
  ".wasm": "moteur WASM ; Shiki doit utiliser le moteur JavaScript",
  ".woff": NOT_WOFF2,
  ".ttf": NOT_WOFF2,
  ".otf": NOT_WOFF2,
  ".eot": NOT_WOFF2,
};

// Définition d'une grammaire : "scopeName":"…" dans le JSON des grammaires
// (éventuellement échappé), ou scopeName:`…` dans un littéral objet (glsl vide).
const SCOPE_DEFINITION = /scopeName\\?["'`]?\s*:\s*\\?["'`]([\w.+-]+)\\?["'`]/g;
// Corps de la vraie grammaire GLSL (licence inconnue).
const GLSL_BODY = [
  /displayName\\?["'`]?\s*:\s*\\?["'`]GLSL\\?["'`]/,
  /\b(?:keyword\.control|storage\.(?:type|modifier)|support\.(?:constant|function|variable))\.glsl\b/,
];
// Grammaire vide de vite.config.ts : patterns juste après scopeName.
const GLSL_EMPTY =
  /scopeName\\?["'`]?\s*:\s*\\?["'`]source\.glsl\\?["'`]\s*,\s*\\?["'`]?patterns\\?["'`]?\s*:\s*\[\s*\]/;

// Import statique minifié (import{a}from"./x.js", import"./x.js",
// export*from"./x.js") ; import("./x.js"), chargé à la demande, n'en est pas un.
const STATIC_IMPORT =
  /(?:^|[^\w$.])(?:import|export)\s*(?:[\w$*{},\s]*?from\s*)?["'](\.{1,2}\/[^"']+\.js)["']/g;

const SHOWN = 8;

/** « a, b, c » ou « a, b, … (+n) » au-delà de SHOWN éléments. */
function list(items) {
  const shown = items.slice(0, SHOWN).join(", ");
  return items.length > SHOWN
    ? `${shown}, … (+${items.length - SHOWN})`
    : shown;
}

/** Regroupe les éléments par raison, dans l'ordre de première apparition. */
function byReason(entries) {
  const groups = new Map();
  for (const [item, reason] of entries) {
    groups.set(reason, [...(groups.get(reason) ?? []), item]);
  }
  return groups;
}

/** Sélecteur de la classe nue (sans variante), opacité « /80 » comprise. */
function classSelector(name) {
  const escaped = name.replace(/[^\w-]/g, "\\$&");
  return new RegExp(
    `(?<![\\\\\\w-])\\.${escaped}(?:\\\\/\\d+)?(?![\\w-]|\\\\)`,
  );
}

function walk(dir, base = dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    return entry.isDirectory()
      ? walk(full, base)
      : [path.relative(base, full).split(path.sep).join("/")];
  });
}

/** Fichiers chargés au démarrage : les racines et leurs imports statiques. */
function eagerFiles(roots, code) {
  const seen = new Set();
  const queue = [...roots];
  while (queue.length > 0) {
    const file = queue.pop();
    if (seen.has(file) || !code.has(file)) continue;
    seen.add(file);
    for (const [, spec] of code.get(file).matchAll(STATIC_IMPORT)) {
      queue.push(path.posix.join(path.posix.dirname(file), spec));
    }
  }
  return seen;
}

function excerpt(text, index) {
  return text
    .slice(Math.max(0, index - 30), index + 40)
    .replace(/\s+/g, " ")
    .trim();
}

function formatBytes(bytes) {
  return `${String(bytes).replace(/\B(?=(\d{3})+(?!\d))/g, " ")} o`;
}

/**
 * Contrôle le build du dossier `dist`. Renvoie les erreurs (messages
 * lisibles, un par règle enfreinte), les mesures et ce qui a été trouvé ; ne
 * lève que si le dossier est illisible.
 */
export function checkBundle(dist, { limits = LIMITS, entry: fixedEntry } = {}) {
  const errors = [];
  // Ordre alphabétique : même sortie quel que soit le système de fichiers.
  const files = walk(dist).sort();
  const read = (file) => readFileSync(path.join(dist, file), "utf8");
  const size = (file) => statSync(path.join(dist, file)).size;
  const sum = (items) => items.reduce((total, file) => total + size(file), 0);

  // 1. Paquets embarqués, d'après la liste que produit vite build.
  let packages = [];
  try {
    packages = JSON.parse(read(".vite/license.json"));
  } catch {
    errors.push(
      ".vite/license.json illisible : vite build pas lancé, ou build.license retiré de vite.config.ts",
    );
  }
  const forbidden = packages.flatMap((pkg) => {
    const rule = FORBIDDEN_PACKAGES.find(([pattern]) => pattern.test(pkg.name));
    return rule ? [[`${pkg.name}@${pkg.version}`, rule[1]]] : [];
  });
  for (const [reason, items] of byReason(forbidden)) {
    errors.push(`Paquets interdits (${reason}) : ${list(items)}`);
  }
  const names = new Set(packages.map((pkg) => pkg.name));
  const missing = REQUIRED_PACKAGES.filter((name) => !names.has(name));
  if (packages.length > 0 && missing.length > 0) {
    errors.push(
      `Paquets attendus absents (rendu Markdown, maths ou coloration incomplets) : ${list(missing)}`,
    );
  }

  // 2. Types de fichiers : pas de WASM, polices en woff2 seulement.
  const badFiles = files.flatMap((file) => {
    const reason = FORBIDDEN_EXTENSIONS[path.extname(file).toLowerCase()];
    return reason ? [[file, reason]] : [];
  });
  for (const [reason, items] of byReason(badFiles)) {
    errors.push(`Fichiers interdits (${reason}) : ${list(items)}`);
  }

  const js = files.filter((file) => file.endsWith(".js"));
  const css = files.filter((file) => file.endsWith(".css"));
  const woff2 = files.filter((file) => file.endsWith(".woff2"));
  const code = new Map(js.map((file) => [file, read(file)]));

  // 3. index.html : script d'entrée, rien en ligne. Un <style> ferait
  //    ajouter par Tauri un nonce à style-src, et 'unsafe-inline' cesserait
  //    de valoir pour les styles en ligne de KaTeX. Entrée donnée (panneau
  //    VS Code) : pas d'index.html.
  let entry = null;
  let preloaded = [];
  let html = "";
  if (fixedEntry !== undefined) {
    entry = code.has(fixedEntry) ? fixedEntry : null;
    if (!entry) {
      errors.push(`Script d'entrée ${fixedEntry} introuvable dans le build`);
    }
  } else {
    try {
      html = read("index.html");
    } catch {
      errors.push("index.html introuvable dans le build");
    }
  }
  if (html) {
    const src = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+\.js)["']/i.exec(html);
    entry = src ? src[1].replace(/^\.?\//, "") : null;
    if (!entry || !code.has(entry)) {
      errors.push(
        `index.html : script d'entrée introuvable dans le build (${entry ?? "aucun <script src>"})`,
      );
      entry = null;
    }
    preloaded = [
      ...html.matchAll(/<link\b[^>]*\brel\s*=\s*["']modulepreload["'][^>]*>/gi),
    ].flatMap(([tag]) => {
      const href = /\bhref\s*=\s*["']([^"']+\.js)["']/i.exec(tag);
      return href ? [href[1].replace(/^\.?\//, "")] : [];
    });
    if (/<script\b(?![^>]*\bsrc\s*=)[^>]*>/i.test(html)) {
      errors.push(
        "index.html : <script> en ligne, refusé par script-src 'self' (Tauri y ajouterait un hash)",
      );
    }
    if (/<style\b/i.test(html)) {
      errors.push(
        "index.html : <style> en ligne ; Tauri ajouterait un nonce à style-src, qui désactive 'unsafe-inline' pour les styles de KaTeX",
      );
    }
  }

  // JS chargé au démarrage : l'entrée, les modules préchargés par index.html
  // et leurs imports statiques ; le reste ne l'est que par import().
  const eager = entry ? eagerFiles([entry, ...preloaded], code) : new Set();

  // 4. Grammaires Shiki : ensemble exact, glsl vide, chargées à la demande.
  const scopes = new Set();
  const grammarFiles = [];
  for (const [file, text] of code) {
    const found = [...text.matchAll(SCOPE_DEFINITION)].map((m) => m[1]);
    if (found.length === 0) continue;
    grammarFiles.push(file);
    for (const scope of found) scopes.add(scope);
  }
  const unexpected = [...scopes].filter((s) => !EXPECTED_SCOPES.includes(s));
  if (unexpected.length > 0) {
    errors.push(
      `Grammaires inattendues, ${unexpected.length} (seuls les langages de src/lib/highlighterCore.ts sont embarqués) : ${list(unexpected.sort())}`,
    );
  }
  const absent = EXPECTED_SCOPES.filter((scope) => !scopes.has(scope));
  if (absent.length > 0) {
    errors.push(`Grammaires manquantes : ${list(absent)}`);
  }
  const allCode = [...code.values()].join("\n");
  if (GLSL_BODY.some((pattern) => pattern.test(allCode))) {
    errors.push(
      "Grammaire GLSL complète embarquée (licence inconnue) : le greffon emptyGlslGrammar de vite.config.ts doit la remplacer par une grammaire vide",
    );
  } else if (scopes.has("source.glsl") && !GLSL_EMPTY.test(allCode)) {
    errors.push(
      "Grammaire source.glsl non vide : attendu la grammaire vide du greffon emptyGlslGrammar de vite.config.ts",
    );
  }
  const eagerGrammars = grammarFiles.filter((file) => eager.has(file));
  if (eagerGrammars.length > 0) {
    errors.push(
      `Grammaires Shiki chargées au démarrage, dans le JS d'entrée ou ses imports statiques (${list(eagerGrammars)}) : src/lib/highlighterCore.ts doit rester chargé à part, par import() depuis src/lib/highlighter.ts`,
    );
  }

  // 5. Motifs incompatibles avec la CSP, et code de test.
  for (const [pattern, label] of CSP_PATTERNS) {
    const hits = [...code].filter(([, text]) => pattern.test(text));
    if (hits.length === 0) continue;
    const [file, text] = hits[0];
    errors.push(
      `Motif refusé par la CSP, ${label} : ${list(hits.map(([f]) => f))} (« ${excerpt(text, pattern.exec(text).index)} » dans ${file})`,
    );
  }
  for (const [marker, origin] of TEST_MARKERS) {
    const hits = [...code].filter(([, text]) => text.includes(marker));
    if (hits.length > 0) {
      errors.push(
        `Code de test embarqué, « ${marker} » (${origin}) : ${list(hits.map(([f]) => f))}`,
      );
    }
  }

  // 6. CSS : polices KaTeX en woff2 seulement, chacune une fois ; pas de
  //    classes des composants par défaut de Streamdown.
  for (const file of css) {
    const text = read(file);
    const format =
      /format\(\s*["']?(woff|truetype|opentype|embedded-opentype)["']?\s*\)/.exec(
        text,
      );
    if (format) {
      errors.push(
        `Police non woff2 déclarée dans ${file}, format("${format[1]}") : ${NOT_WOFF2}`,
      );
    }
    const faces = new Map();
    for (const [face] of text.matchAll(/@font-face\s*\{[^}]*\}/g)) {
      const prop = (name, fallback) =>
        new RegExp(`${name}:\\s*([^;}]+)`).exec(face)?.[1].trim() ?? fallback;
      const key = `${prop("font-family", "?")} ${prop("font-style", "normal")} ${prop("font-weight", "400")}`;
      faces.set(key, (faces.get(key) ?? 0) + 1);
    }
    const twice = [...faces].filter(([, count]) => count > 1).map(([k]) => k);
    if (twice.length > 0) {
      errors.push(
        `@font-face en double dans ${file} (CSS de KaTeX importée deux fois ? Streamdown l'importe déjà) : ${list(twice)}`,
      );
    }
    const generated = ABSENT_CLASSES.filter((name) =>
      classSelector(name).test(text),
    );
    if (generated.length > 0) {
      errors.push(
        `Classes Tailwind des composants par défaut de Streamdown générées dans ${file} (absentes chez l'officiel ; ne pas les écrire dans src/ ni ajouter d'@source vers streamdown) : ${generated.map((c) => `.${c}`).join(", ")}`,
      );
    }
    const guard = NOT_PROSE.exec(text);
    if (guard) {
      errors.push(
        `Garde not-prose de @tailwindcss/typography restée dans ${file} (${NOT_PROSE_REASON}) : « ${excerpt(text, guard.index)} »`,
      );
    }
  }

  // 7. Tailles. Le JS d'entrée compte ses imports statiques.
  const measures = {
    jsEntry: sum([...eager]),
    highlighter: sum(grammarFiles.filter((file) => !eager.has(file))),
    css: sum(css),
    woff2: sum(woff2),
    jsTotal: sum(js),
  };
  for (const [key, limit] of Object.entries(limits)) {
    if (measures[key] > limit) {
      errors.push(
        `Taille dépassée, ${LIMIT_LABELS[key]} : ${formatBytes(measures[key])} > ${formatBytes(limit)} (seuils dans scripts/check-bundle.mjs)`,
      );
    }
  }

  return {
    errors,
    measures,
    packages,
    scopes,
    js,
    grammarFiles,
    entry,
    eager: [...eager].sort(),
  };
}

/**
 * Build du panneau de l'extension VS Code : les règles de checkBundle (entrée
 * index.js, seuils propres), plus la feuille index.css, rien de Tauri et
 * aucune ressource distante. Renvoie comme checkBundle.
 */
export function checkWebviewBundle(dist, { limits = WEBVIEW_LIMITS } = {}) {
  const result = checkBundle(dist, { limits, entry: WEBVIEW_ENTRY });
  const errors = [...result.errors];
  const files = walk(dist);
  const read = (file) => readFileSync(path.join(dist, file), "utf8");
  if (!files.includes(WEBVIEW_CSS)) {
    errors.push(
      `Feuille ${WEBVIEW_CSS} introuvable : l'hôte de l'extension la lit sous ce nom (assetFileNames de vscode/webview.vite.config.mts)`,
    );
  }
  const js = files.filter((file) => file.endsWith(".js"));
  const tauri = js.filter((file) =>
    TAURI_MARKERS.some((marker) => read(file).includes(marker)),
  );
  if (tauri.length > 0) {
    errors.push(
      `Code de Tauri dans le panneau (alias @/lib/appHost de vscode/webview.vite.config.mts ?) : ${list(tauri)}`,
    );
  }
  const remote = [
    ...files
      .filter((file) => file.endsWith(".css"))
      .filter((file) => REMOTE_CSS.test(read(file))),
    ...js.filter((file) => REMOTE_IMPORT.test(read(file))),
  ];
  if (remote.length > 0) {
    errors.push(
      `Ressource distante dans le panneau (la CSP n'autorise que les fichiers de l'extension) : ${list(remote)}`,
    );
  }
  return { ...result, errors };
}

/** Sources de l'app lues par checkSources. */
const SOURCE_EXTENSIONS = new Set([
  ".ts",
  ".tsx",
  ".js",
  ".jsx",
  ".mjs",
  ".css",
  ".html",
]);

/**
 * Sources de l'app (src/ et index.html du dossier `base`) et du panneau VS
 * Code (vscode/webview, s'il existe) : aucune classe not-prose, que le
 * retrait de la garde de typography rendrait sans effet. Renvoie les erreurs.
 */
export function checkSources(base) {
  const under = (dir) =>
    existsSync(path.join(base, dir))
      ? walk(path.join(base, dir))
          .sort()
          .map((file) => `${dir}/${file}`)
      : [];
  const files = [
    ...under("src"),
    ...under("vscode/webview"),
    "index.html",
  ].filter((file) => SOURCE_EXTENSIONS.has(path.extname(file)));
  const hits = files.flatMap((file) => {
    let text;
    try {
      text = readFileSync(path.join(base, file), "utf8");
    } catch {
      return [];
    }
    return text
      .split("\n")
      .flatMap((line, i) => (NOT_PROSE.test(line) ? [`${file}:${i + 1}`] : []));
  });
  return hits.length > 0
    ? [
        `Classe not-prose dans les sources, sans effet ici (${NOT_PROSE_REASON}) : ${list(hits)}`,
      ]
    : [];
}

function main() {
  const args = process.argv.slice(2);
  const webview = args[0] === "--webview";
  if (webview) args.shift();
  const dist = path.resolve(
    args[0] ?? path.join(root, webview ? "vscode/dist/webview" : "dist"),
  );
  let result;
  try {
    result = webview ? checkWebviewBundle(dist) : checkBundle(dist);
  } catch (error) {
    console.error(`Build illisible (${dist}) : ${error.message}`);
    console.error(
      webview
        ? "Lancer d'abord vite build -c vscode/webview.vite.config.mts."
        : "Lancer d'abord vite build.",
    );
    process.exit(1);
  }
  const { measures, packages, scopes, js } = result;
  const errors = [...result.errors, ...checkSources(root)];
  console.log(
    `Mesures : JS d'entrée ${formatBytes(measures.jsEntry)}, surligneur ${formatBytes(measures.highlighter)}, CSS ${formatBytes(measures.css)}, woff2 ${formatBytes(measures.woff2)}, JS total ${formatBytes(measures.jsTotal)}.`,
  );
  for (const error of errors) console.error(`✗ ${error}`);
  if (errors.length > 0) process.exit(1);
  console.log(
    `Bundle conforme : ${packages.length} paquets, ${scopes.size} grammaires, ${js.length} morceaux JS.`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
