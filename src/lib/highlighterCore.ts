/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/lib/highlighter.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: Shiki core with the JavaScript regex engine (no WASM)
 * and the same 19 languages imported statically, created synchronously in a
 * chunk loaded on demand (see highlighter.ts); ES2018 regex
 * target with a shared cache warmed up in the background; CSS, Go and C++
 * patterns rewritten against catastrophic backtracking (same matches).
 * Theme colors derived from Atom One Light / One Dark (MIT, GitHub Inc.),
 * see THIRD_PARTY_LICENSES part 2: same hues, with the lightness of the
 * tokens under 4.5:1 adjusted for ChatonLM's warm code backgrounds (#fbf1e4
 * and #382820, the neutral-100 and neutral-800 of src/index.css), and the
 * theme's warm ink and secondary text for plain code and comments.
 */

// Chargé à part (import() depuis highlighter.ts) : les grammaires (~2 Mo)
// restent hors du morceau principal. Imports statiques seulement : le bundle
// contient ces 19 grammaires (et celles qu'elles embarquent), le moteur regex
// JavaScript, aucun WASM.
import {
  createHighlighterCoreSync,
  type HighlighterCore,
  type LanguageRegistration,
  type ThemeRegistration,
} from "shiki/core";
import {
  createJavaScriptRegexEngine,
  defaultJavaScriptRegexConstructor,
} from "shiki/engine/javascript";
import javascript from "@shikijs/langs/javascript";
import typescript from "@shikijs/langs/typescript";
import python from "@shikijs/langs/python";
import bash from "@shikijs/langs/bash";
import shell from "@shikijs/langs/shell";
import json from "@shikijs/langs/json";
import html from "@shikijs/langs/html";
import css from "@shikijs/langs/css";
import tsx from "@shikijs/langs/tsx";
import jsx from "@shikijs/langs/jsx";
import go from "@shikijs/langs/go";
import rust from "@shikijs/langs/rust";
import java from "@shikijs/langs/java";
import c from "@shikijs/langs/c";
import cpp from "@shikijs/langs/cpp";
import sql from "@shikijs/langs/sql";
import swift from "@shikijs/langs/swift";
import yaml from "@shikijs/langs/yaml";
import markdown from "@shikijs/langs/markdown";

export const oneLightTheme: ThemeRegistration = {
  name: "one-light",
  type: "light",
  colors: {
    "editor.background": "#fbf1e4",
    "editor.foreground": "#382820",
  },
  tokenColors: [
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: "#766352" },
    },
    {
      scope: ["keyword", "storage.type", "storage.modifier"],
      settings: { foreground: "#a626a4" },
    },
    { scope: ["string", "string.quoted"], settings: { foreground: "#3c7a3c" } },
    {
      scope: ["function", "entity.name.function", "support.function"],
      settings: { foreground: "#2162f0" },
    },
    {
      scope: [
        "constant.numeric",
        "constant.language",
        "constant.character",
        "number",
      ],
      settings: { foreground: "#916301" },
    },
    {
      scope: ["variable", "support.variable"],
      settings: { foreground: "#cf2d1f" },
    },
    {
      scope: ["entity.name.tag", "entity.name.type", "entity.name.class"],
      settings: { foreground: "#cf2d1f" },
    },
    {
      scope: ["entity.other.attribute-name"],
      settings: { foreground: "#916301" },
    },
    {
      scope: ["keyword.operator", "operator"],
      settings: { foreground: "#a626a4" },
    },
    { scope: ["punctuation"], settings: { foreground: "#382820" } },
    {
      scope: ["markup.heading"],
      settings: { foreground: "#cf2d1f", fontStyle: "bold" },
    },
    {
      scope: ["markup.bold"],
      settings: { foreground: "#916301", fontStyle: "bold" },
    },
    {
      scope: ["markup.italic"],
      settings: { foreground: "#a626a4", fontStyle: "italic" },
    },
  ],
};

export const oneDarkTheme: ThemeRegistration = {
  name: "one-dark",
  type: "dark",
  colors: {
    "editor.background": "#382820",
    "editor.foreground": "#e8d2ba",
  },
  tokenColors: [
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: "#a6917e" },
    },
    {
      scope: ["keyword", "storage.type", "storage.modifier"],
      settings: { foreground: "#c678dd" },
    },
    { scope: ["string", "string.quoted"], settings: { foreground: "#98c379" } },
    {
      scope: ["function", "entity.name.function", "support.function"],
      settings: { foreground: "#61afef" },
    },
    {
      scope: [
        "constant.numeric",
        "constant.language",
        "constant.character",
        "number",
      ],
      settings: { foreground: "#d19a66" },
    },
    {
      scope: ["variable", "support.variable"],
      settings: { foreground: "#e1727b" },
    },
    {
      scope: ["entity.name.tag", "entity.name.type", "entity.name.class"],
      settings: { foreground: "#e1727b" },
    },
    {
      scope: ["entity.other.attribute-name"],
      settings: { foreground: "#d19a66" },
    },
    {
      scope: ["keyword.operator", "operator"],
      settings: { foreground: "#c678dd" },
    },
    { scope: ["punctuation"], settings: { foreground: "#e8d2ba" } },
    {
      scope: ["markup.heading"],
      settings: { foreground: "#e1727b", fontStyle: "bold" },
    },
    {
      scope: ["markup.bold"],
      settings: { foreground: "#d19a66", fontStyle: "bold" },
    },
    {
      scope: ["markup.italic"],
      settings: { foreground: "#c678dd", fontStyle: "italic" },
    },
  ],
};

// Le moteur JavaScript n'a pas les bornes d'Oniguruma (l'officiel) : un motif
// qui peut lire une même suite de plusieurs façons essaie toutes les
// combinaisons avant d'échouer (retour arrière exponentiel). Une seule
// expression ne peut pas être interrompue (la limite de temps de Shiki joue
// entre deux recherches) : une ligne d'une réponse peut figer l'interface des
// secondes, voire des heures. Les fragments fautifs, trouvés en exécutant
// chaque expression sur des chaînes adverses (fuzz, voir
// highlighterCore.test.ts), sont réécrits au chargement sous une forme à une
// seule lecture. Une réécriture ne retire que des chemins qui mènent à un état
// déjà essayé (même position, mêmes captures) : la première correspondance
// trouvée, donc chaque jeton, reste la même (les tests comparent aussi les
// deux formes).

export interface PatternRewrite {
  /** Grammaires où le fragment est remplacé (copies embarquées comprises). */
  langs: readonly string[];
  /** Fragment d'origine, en syntaxe Oniguruma, remplacé partout où il paraît. */
  from: string;
  to: string;
}

// Go : un passage de la boucle des types canal (« chan <- chan T »).
const CHAN_ITERATION = String.raw`[]*\[]+{0,1}(?:<-\s*)?\bchan\b(?:\s*<-)?\s*`;
// Entre deux « chan » : la flèche va au passage précédent quand il peut la
// prendre, les espaces à la fin du passage précédent.
const CHAN_SEPARATOR = String.raw`(?:\s*<-\s*(?:[]*\[]+)?(?:<-\s*)?|\s*(?:[]*\[]+(?:<-\s*)?)?)`;
const CHAN_LOOP = String.raw`[]*\[]+{0,1}(?:<-\s*)?\bchan\b(?:${CHAN_SEPARATOR}\bchan\b)*(?:\s*<-)?\s*`;

export const PATTERN_REWRITES: readonly PatternRewrite[] = [
  // CSS (aussi dans les <style> de HTML et les blocs css de Markdown) :
  // l'échappement, répété dans une boucle comme « (?:[-\w]|\\(?:\h{1,6}|.))+ »,
  // lit « \b » par \h ou par « . », et « \bb » en un échappement de deux
  // chiffres ou d'un chiffre suivi d'une lettre : quand la suite du motif
  // échoue, « b\ » × 24 fige WebKitGTK 4,7 s (× 4 toutes les deux unités).
  // Forme gardée : six chiffres, ou un à cinq chiffres non suivis d'un autre,
  // ou un caractère non hexadécimal ; les chiffres au-delà du sixième restent
  // lus par la classe de la boucle (« \h{1,6}(?!\h)|\h{6} » lirait encore six
  // chiffres de deux façons). L'ancienne forme ne trouvait rien de plus :
  // s'arrêter au milieu d'une suite de chiffres n'aide aucun des 14 motifs (la
  // boucle finit le motif et son premier essai, le plus long, réussit ; ou la
  // suite attend « | », une ponctuation, un espace ou la fin de la ligne).
  {
    langs: ["css"],
    from: String.raw`\\(?:\h{1,6}|.)`,
    to: String.raw`\\(?:\h{6}|\h{1,5}(?!\h)|(?!\h).)`,
  },
  // CSS : en ES2018, la classe imbriquée devient « (?:[-\w]|[^\x00-\x7F]) »,
  // où une lettre non ASCII passe des deux côtés (« a[ » + « é » × 24 : 2,4 s
  // dans WebKitGTK). \w ne garde en ASCII que [0-9A-Z_a-z] et le reste est
  // déjà non ASCII : même ensemble de caractères, en deux parties disjointes.
  {
    langs: ["css"],
    from: String.raw`[-\w[^\x00-\x7F]]`,
    to: String.raw`[-0-9A-Z_a-z[^\x00-\x7F]]`,
  },
  // Go, champs d'une structure écrite sur une ligne : chaque passage de la
  // boucle « (?:\s*NOMS? CANAUX? [^/\s]+;?)+ » peut couper n'importe où une
  // suite sans espace (« type Tag struct { Name string `json:"name"` » : 2,7 s
  // dans WebKitGTK, plus de 20 s avec deux champs, 13 ms avec Oniguruma ; la
  // garde « (?=.*\}) » d'avant ne suffisait pas : « { }aaa…a/} »). Noms et
  // canaux n'ajoutent que des espaces et des caractères de [^/\s] : le groupe
  // lit en tout « \s*[^/\s]+(?:\s+[^/\s]+)* », et les deux formes s'arrêtent
  // à la fin la plus lointaine suivie de « \s*} ». À remplacer avant les
  // canaux.
  {
    langs: ["go"],
    from: String.raw`(?<=\{)((?:\s*(?:(?:\w+,\s*)+{0,1}\w+\s+)?(?:\s*[]*\[]+{0,1}(?:<-\s*)?\bchan\b(?:\s*<-)?\s*)+{0,1}[^/\s]+;?)+)\s*(?=})`,
    to: String.raw`(?<=\{)(\s*[^/\s]+(?:\s+[^/\s]+)*)\s*(?=})`,
  },
  // Go, types canal : dans la boucle, les espaces entre deux « chan » vont à la
  // fin d'un passage ou au début du suivant, une flèche à l'un ou à l'autre
  // (« func f(a » + « chan » × 24 : 11 s dans WebKitGTK). Forme gardée :
  // premier passage, séparateurs à une seule lecture, puis la fin du dernier
  // passage.
  {
    langs: ["go"],
    from: String.raw`(?:\s*${CHAN_ITERATION})+`,
    to: String.raw`(?:\s*${CHAN_LOOP})`,
  },
  { langs: ["go"], from: `(?:${CHAN_ITERATION})+`, to: `(?:${CHAN_LOOP})` },
  // C++ et ses macros : « (?<=\W)|(?=\W) » réussit deux fois à vide entre deux
  // « & » ou « * » ; dans une boucle comme « (?:[&*](…|(?<=\W)|(?=\W)|…))* »,
  // une suite de « * » double le travail à chaque caractère (« int » + « * »
  // × 20 : 8,5 s dans WebKitGTK). La seconde n'est plus essayée là où la
  // première l'a été.
  {
    langs: ["cpp", "cpp-macro"],
    from: String.raw`(?<=\W)|(?=\W)`,
    to: String.raw`(?<=\W)|(?<!\W)(?=\W)`,
  },
];

// Clés d'une règle TextMate qui portent une expression.
const PATTERN_KEYS = new Set(["match", "begin", "end", "while"]);

type Grammar = LanguageRegistration;

/**
 * Nœud de grammaire où chaque expression passe par `rewrite` : copie des
 * seuls objets qui changent, les autres sont partagés avec l'original.
 */
function rewritePatterns<T>(node: T, rewrite: (source: string) => string): T {
  if (!node || typeof node !== "object") return node;
  let changed = false;
  const visit = (key: string | null, value: unknown) => {
    const out =
      key !== null && PATTERN_KEYS.has(key) && typeof value === "string"
        ? rewrite(value)
        : rewritePatterns(value, rewrite);
    if (out !== value) changed = true;
    return out;
  };
  if (Array.isArray(node)) {
    const items = node.map((item: unknown) => visit(null, item));
    return changed ? (items as T) : node;
  }
  const entries = Object.entries(node).map(
    ([key, value]: [string, unknown]) => [key, visit(key, value)],
  );
  return changed ? (Object.fromEntries(entries) as T) : node;
}

/** Grammaire avec ses réécritures, dans l'ordre du tableau. */
export function guardGrammar(grammar: Grammar): Grammar {
  const rewrites = PATTERN_REWRITES.filter((rewrite) =>
    rewrite.langs.includes(grammar.name),
  );
  if (rewrites.length === 0) return grammar;
  return rewritePatterns(grammar, (source) =>
    rewrites.reduce((out, { from, to }) => out.replaceAll(from, to), source),
  );
}

// Une grammaire embarquée par une autre (css dans html, cpp-macro dans cpp)
// est le même objet dans les deux tableaux : elle reste un seul objet une
// fois gardée, quelle que soit la copie que Shiki enregistre.
const guardedGrammars = new Map<Grammar, Grammar>();

function withGuards(registration: Grammar[]): Grammar[] {
  return registration.map((grammar) => {
    let guarded = guardedGrammars.get(grammar);
    if (!guarded) {
      guarded = guardGrammar(grammar);
      guardedGrammars.set(grammar, guarded);
    }
    return guarded;
  });
}

/** Les 19 langages de l'officiel (bash et shell sont deux alias de shellscript). */
export const LANGS = [
  javascript,
  typescript,
  python,
  bash,
  shell,
  json,
  html,
  css,
  tsx,
  jsx,
  go,
  rust,
  java,
  c,
  cpp,
  sql,
  swift,
  yaml,
  markdown,
].map(withGuards);

/**
 * Les deux thèmes ont les mêmes 13 règles (mêmes portées, même ordre) : un
 * jeton coloré avec one-light prend sa couleur one-dark dans cette table, ce
 * qui évite une seconde tokenisation. Clés et valeurs en majuscules, comme
 * les couleurs des jetons de Shiki.
 */
export const LIGHT_TO_DARK: ReadonlyMap<string, string> = (() => {
  const map = new Map<string, string>();
  const add = (light?: string, dark?: string) => {
    if (!light || !dark) return;
    const key = light.toUpperCase();
    const value = dark.toUpperCase();
    // Une couleur claire partagée par deux règles doit avoir la même couleur
    // sombre dans les deux, sinon la table ne peut pas remplacer le thème.
    if (map.get(key) !== undefined && map.get(key) !== value) {
      throw new Error(`ambiguous theme color ${light}`);
    }
    map.set(key, value);
  };
  add(
    oneLightTheme.colors?.["editor.foreground"],
    oneDarkTheme.colors?.["editor.foreground"],
  );
  oneLightTheme.tokenColors?.forEach((rule, i) =>
    add(
      rule.settings.foreground,
      oneDarkTheme.tokenColors?.[i]?.settings.foreground,
    ),
  );
  return map;
})();

// ES2018 = expressions « u » simples, identiques dans les tests (Node) et
// dans l'app : « auto » choisit ES2025 dans les deux ; ses groupes de
// modificateurs « (?i:…) » touchent un bogue de V8 sous Node 24, et WebKitGTK
// 2.52 compile les expressions « v » (ES2024/ES2025) environ trois fois plus
// lentement au premier usage. Les 3 711 motifs des grammaires compilent tous
// en ES2018.
const TARGET = "ES2018";

function compileRegex(pattern: string): RegExp {
  return defaultJavaScriptRegexConstructor(pattern, { target: TARGET });
}

// Expressions compilées, partagées par le moteur et le préchauffage : celles
// que le préchauffage a compilées ne le sont plus au premier surlignage.
const regexCache = new Map<string, RegExp | Error>();

export function createLibreHighlighter(): HighlighterCore {
  return createHighlighterCoreSync({
    themes: [oneLightTheme, oneDarkTheme],
    langs: LANGS,
    engine: createJavaScriptRegexEngine({
      target: TARGET,
      regexConstructor: compileRegex,
      cache: regexCache,
    }),
  });
}

// Fin de règle qui cite une capture du début (\1…) : vscode-textmate la
// complète à l'usage, sa forme brute n'est jamais compilée.
const BACK_REFERENCE = /\\\d/;

/**
 * Formes sous lesquelles vscode-textmate passe un motif au moteur
 * (RegExpSource de @shikijs/vscode-textmate 10) : \z réécrit et, si le motif
 * contient \A ou \G, ses quatre variantes où chaque ancre est neutralisée
 * (\uFFFF). Une forme mal devinée ne coûte qu'une compilation inutile : le
 * cache est indexé par la source exacte.
 */
function engineForms(source: string): string[] {
  let anchors = false;
  const rewritten = source.replace(/\\([\s\S])/g, (escape, char: string) => {
    if (char === "z") return "$(?!\\n)(?<!\\n)";
    if (char === "A" || char === "G") anchors = true;
    return escape;
  });
  if (!anchors) return [rewritten];
  const variant = (a: boolean, g: boolean) =>
    rewritten.replace(/\\([\s\S])/g, (escape, char: string) =>
      (char === "A" && !a) || (char === "G" && !g) ? "\\\uFFFF" : escape,
    );
  return [
    variant(true, true),
    variant(true, false),
    variant(false, true),
    variant(false, false),
  ];
}

function* walkPatterns(node: unknown): Generator<string> {
  if (Array.isArray(node)) {
    for (const item of node) yield* walkPatterns(item);
  } else if (node && typeof node === "object") {
    for (const [key, value] of Object.entries(node)) {
      if (PATTERN_KEYS.has(key) && typeof value === "string") {
        const dynamic = key !== "match" && key !== "begin";
        if (!(dynamic && BACK_REFERENCE.test(value))) yield* engineForms(value);
      } else {
        yield* walkPatterns(value);
      }
    }
  }
}

/**
 * Toutes les expressions que le moteur peut avoir à compiler pour les
 * grammaires chargées (y compris celles qu'elles embarquent), sans doublon,
 * dans l'ordre des langages.
 */
export function* regexSources(): Generator<string> {
  const seen = new Set<string>();
  for (const registration of LANGS) {
    for (const source of walkPatterns(registration)) {
      if (seen.has(source)) continue;
      seen.add(source);
      yield source;
    }
  }
}

/**
 * Préchauffage : chaque appel compile une expression pas encore en cache et
 * l'exécute une fois (WebKit et V8 ne la traduisent qu'à la première
 * exécution ; les motifs de plus de 3 000 caractères ne sont même analysés
 * qu'à ce moment). Renvoie false quand il n'y en a plus. Une expression
 * refusée est signalée à onError et laissée de côté : le surlignage
 * retrouvera l'erreur et le bloc restera en texte brut.
 */
export function createRegexWarmUp(
  onError?: (source: string, error: unknown) => void,
): () => boolean {
  const sources = regexSources();
  return () => {
    for (;;) {
      const next = sources.next();
      if (next.done) return false;
      const cached = regexCache.get(next.value);
      if (cached instanceof Error) onError?.(next.value, cached);
      if (cached) continue;
      try {
        const regex = compileRegex(next.value);
        regex.lastIndex = 0;
        regex.exec("");
        regexCache.set(next.value, regex);
      } catch (error) {
        onError?.(next.value, error);
      }
      return true;
    }
  };
}
