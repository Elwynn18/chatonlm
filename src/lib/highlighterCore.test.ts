import cppOriginal from "@shikijs/langs/cpp";
import cssOriginal from "@shikijs/langs/css";
import goOriginal from "@shikijs/langs/go";
import htmlOriginal from "@shikijs/langs/html";
import javascriptOriginal from "@shikijs/langs/javascript";
import markdownOriginal from "@shikijs/langs/markdown";
import {
  createHighlighterCoreSync,
  type LanguageRegistration,
} from "shiki/core";
import {
  createJavaScriptRegexEngine,
  defaultJavaScriptRegexConstructor,
} from "shiki/engine/javascript";
import { describe, expect, it, vi } from "vitest";
import {
  CATASTROPHIC_LINES,
  CODE_SAMPLES,
  REWRITTEN_PATTERN_SAMPLES,
} from "@/test/codeSamples";
import {
  createLibreHighlighter,
  createRegexWarmUp,
  guardGrammar,
  LANGS,
  LIGHT_TO_DARK,
  oneDarkTheme,
  oneLightTheme,
  PATTERN_REWRITES,
  regexSources,
} from "./highlighterCore";

// Tests lourds (toutes les expressions des grammaires, 19 langages en entier) :
// quelques secondes seuls, davantage quand toute la suite tourne en parallèle
// (check.sh). Le délai par défaut de 5 s les ferait échouer au hasard.
vi.setConfig({ testTimeout: 30_000 });

// Compte les compilations d'expressions faites par le moteur.
vi.mock("shiki/engine/javascript", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("shiki/engine/javascript")>();
  return {
    ...actual,
    defaultJavaScriptRegexConstructor: vi.fn(
      actual.defaultJavaScriptRegexConstructor,
    ),
  };
});

const OFFICIAL_LANGS = [
  "javascript",
  "typescript",
  "python",
  "bash",
  "shell",
  "json",
  "html",
  "css",
  "tsx",
  "jsx",
  "go",
  "rust",
  "java",
  "c",
  "cpp",
  "sql",
  "swift",
  "yaml",
  "markdown",
];

// Grammaires d'origine (objets des modules @shikijs/langs, non gardés), avec
// celles qu'elles embarquent.
const ORIGINAL_GRAMMARS: LanguageRegistration[] = [
  ...new Set(
    [
      cssOriginal,
      htmlOriginal,
      markdownOriginal,
      goOriginal,
      cppOriginal,
      javascriptOriginal,
    ].flat(),
  ),
];

const PATTERN_KEYS = new Set(["match", "begin", "end", "while"]);

/** Expressions d'une grammaire, dans l'ordre de ses clés. */
function* patternsOf(node: unknown): Generator<string> {
  if (Array.isArray(node)) {
    for (const item of node) yield* patternsOf(item);
  } else if (node && typeof node === "object") {
    for (const [key, value] of Object.entries(node)) {
      if (PATTERN_KEYS.has(key) && typeof value === "string") yield value;
      else yield* patternsOf(value);
    }
  }
}

/** Motif brut compilé comme le moteur, ancres \A et \G neutralisées. */
function compile(source: string): RegExp {
  const neutral = source.replace(/\\([\s\S])/g, (escape, char: string) => {
    if (char === "A" || char === "G") return "\\\uFFFF";
    if (char === "z") return "$(?!\\n)(?<!\\n)";
    return escape;
  });
  return defaultJavaScriptRegexConstructor(neutral, { target: "ES2018" });
}

/**
 * Correspondances successives depuis le début, comme le tokenizer qui
 * reprend après chaque jeton : position et bornes de chaque capture.
 */
function allMatches(regex: RegExp, text: string): string[] {
  const out: string[] = [];
  regex.lastIndex = 0;
  for (let match = regex.exec(text); match; match = regex.exec(text)) {
    out.push(
      (match.indices ?? [])
        .map((range) => (range ? range.join(":") : "-"))
        .join(),
    );
    if (regex.lastIndex === match.index) regex.lastIndex += 1;
    if (regex.lastIndex > text.length || out.length > 100) break;
  }
  return out;
}

// Jetons des chaînes de la comparaison : échappements CSS, flèches et
// canaux de Go, pointeurs et commentaires de C++, et de quoi casser les
// boucles (fermants, barre oblique, caractère de contrôle, non ASCII).
const DIFF_TOKENS: Partial<Record<string, string[]>> = {
  css: [
    "a",
    "b",
    "f",
    "0",
    "9",
    "g",
    "_",
    "\\",
    "\\\\",
    "|",
    "-",
    "--",
    ".",
    "#",
    ":",
    ",",
    "(",
    ")",
    " ",
    "*",
    "é",
    "\u212A",
    "[",
    "]",
    "=",
    "~",
    "/",
    "/*",
    "{",
    "}",
    ";",
    "!",
    '"',
    "\u0001",
    "abcdef",
    "1234567",
    "\\31 ",
    "@media",
    "+",
    ">",
  ],
  go: [
    "chan",
    "chan ",
    " ",
    "  ",
    "<-",
    "<- ",
    "[",
    "]",
    "[]",
    "*",
    "a",
    "b1",
    ",",
    ", ",
    "{",
    "}",
    "(",
    ")",
    "/",
    ";",
    "struct",
    "interface",
    "func",
    "map",
    "int",
    ".",
    "`",
    '"',
    "\t",
    "\u0001",
    "=",
    "é",
  ],
  cpp: [
    "&",
    "*",
    " ",
    "/**/",
    "/* a */",
    "a",
    "int",
    "const",
    "::",
    "<",
    ">",
    "(",
    ")",
    "operator",
    "struct",
    "unsigned",
    "[[",
    "]]",
    ",",
    ";",
    "=",
    '"',
    "\\",
    "/",
    "#",
    "template",
    "\u0001",
    "é",
    "\t",
  ],
};
DIFF_TOKENS["cpp-macro"] = DIFF_TOKENS.cpp;

/** Suites de un à six jetons tirées au hasard (graine : le motif). */
function corpus(tokens: string[], seedText: string, random: number): string[] {
  let seed = 0;
  for (const char of seedText) seed = (seed * 31 + char.charCodeAt(0)) >>> 0;
  const next = () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 2 ** 32;
  };
  const out: string[] = [];
  for (let i = 0; i < random; i++) {
    let text = "";
    const length = 1 + Math.floor(next() * 6);
    for (let j = 0; j < length; j++) {
      text += tokens[Math.floor(next() * tokens.length)];
    }
    out.push(text + "\n");
  }
  return out;
}

// Fuzz rapide : préfixe, unité répétée, fin, nombres d'unités essayés (pas
// plus fins pour les unités longues, dont chaque ajout multiplie davantage le
// temps d'une expression explosive). Chacune des neuf premières familles
// figeait au moins une expression d'origine (css, go, cpp-macro) ; les autres
// couvrent les boucles courantes (mots, espaces, guillemets, points).
const SHORT = [8, 12, 16, 20, 24, 28];
const FUZZ_CASES: [string, string, string, number[]][] = [
  ["", "b\\", "", SHORT],
  [".", "\\a", "", SHORT],
  // Six chiffres : piège aussi la garde « \h{1,6}(?!\h)|\h{6} » (deux
  // lectures de six chiffres), proposée d'abord.
  ["", "\\abcdef", "", [4, 8, 12, 16, 20]],
  ["", "é", "", SHORT],
  ["", "chan ", "", [8, 12, 16, 20, 24]],
  ["", "chan <- ", "", [4, 6, 8, 10, 12]],
  ["{", "a", "/}", SHORT],
  ["", "&", "", SHORT],
  ["", "*", "", SHORT],
  ["", "a ", "!", SHORT],
  ["", " ", "!", SHORT],
  ["", "a", "!", SHORT],
  ["", "a,", "!", SHORT],
  ["", "a.", "!", SHORT],
  ["", "\\", "", SHORT],
  ["", '"', "", SHORT],
  ["(", "a ", "", SHORT],
  ["<", "a ", "", SHORT],
  ["", "/**/", "!", [8, 16, 24]],
];
const FUZZ_BUDGET_MS = 100;

describe("highlighterCore", () => {
  it("has a code sample for each of the 19 languages of the official app", () => {
    expect(Object.keys(CODE_SAMPLES).sort()).toEqual(
      [...OFFICIAL_LANGS].sort(),
    );
  });

  // En premier : rien n'est encore en cache, tout est réellement compilé.
  it("compiles and runs every grammar pattern with the ES2018 target", () => {
    const sources = [...regexSources()];
    // Plus de 2 000 expressions distinctes (3 711 motifs bruts, variantes
    // d'ancres comprises) : le parcours des grammaires n'en oublie pas.
    expect(sources.length).toBeGreaterThan(2000);
    expect(new Set(sources).size).toBe(sources.length);
    const failures: string[] = [];
    const step = createRegexWarmUp((source, error) =>
      failures.push(`${String(error)} <${source.slice(0, 80)}>`),
    );
    let compiled = 0;
    while (step()) compiled += 1;
    expect(failures).toEqual([]);
    expect(compiled).toBe(sources.length);
    // Une fois tout en cache, un nouveau préchauffage n'a plus rien à faire.
    expect(createRegexWarmUp()()).toBe(false);
  });

  it("loads the 19 languages of the official app", () => {
    const core = createLibreHighlighter();
    expect(core.getLoadedLanguages()).toEqual(
      expect.arrayContaining(OFFICIAL_LANGS),
    );
    expect(core.getLoadedThemes()).toEqual(["one-light", "one-dark"]);
    core.dispose();
  });

  it("maps each light color to the dark color of the same rule", () => {
    expect(oneDarkTheme.tokenColors?.map((rule) => rule.scope)).toEqual(
      oneLightTheme.tokenColors?.map((rule) => rule.scope),
    );
    expect(Object.fromEntries(LIGHT_TO_DARK)).toEqual({
      "#382820": "#E8D2BA",
      "#766352": "#A6917E",
      "#A626A4": "#C678DD",
      "#3C7A3C": "#98C379",
      "#2162F0": "#61AFEF",
      "#916301": "#D19A66",
      "#CF2D1F": "#E1727B",
    });
  });

  it("keeps every token at 4.5:1 or more on the code block background", () => {
    const luminance = (hex: string) => {
      const [r, g, b] = [1, 3, 5].map((i) => {
        const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
        return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (a: string, b: string) => {
      const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
      return (high + 0.05) / (low + 0.05);
    };
    // Fonds des blocs de code : neutral-100 et neutral-800 de src/index.css,
    // repris par les thèmes (StreamingMarkdownContent pose ces classes).
    for (const [theme, background] of [
      [oneLightTheme, "#fbf1e4"],
      [oneDarkTheme, "#382820"],
    ] as const) {
      expect(theme.colors?.["editor.background"]).toBe(background);
      const colors = [
        theme.colors?.["editor.foreground"],
        ...(theme.tokenColors ?? []).map((rule) => rule.settings.foreground),
      ];
      expect(colors.length).toBeGreaterThan(10);
      for (const color of colors) {
        expect(color).toMatch(/^#[0-9a-f]{6}$/);
        expect(
          contrast(color!, background),
          `${color} on ${background}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("gives with LIGHT_TO_DARK exactly the tokens of the dark theme, in all 19 languages", () => {
    const core = createLibreHighlighter();
    const used = new Set<string>();
    for (const [lang, code] of Object.entries(CODE_SAMPLES)) {
      const options = { lang, tokenizeTimeLimit: 0 } as const;
      const light = core.codeToTokensBase(code, {
        ...options,
        theme: "one-light",
      });
      const dark = core.codeToTokensBase(code, {
        ...options,
        theme: "one-dark",
      });
      const mapped = light.map((line) =>
        line.map((token) => {
          if (token.color) used.add(token.color);
          return { ...token, color: LIGHT_TO_DARK.get(token.color ?? "") };
        }),
      );
      expect(mapped, lang).toEqual(dark);
    }
    // Toutes les couleurs des thèmes servent : les échantillons sont colorés.
    expect([...used].sort()).toEqual([...LIGHT_TO_DARK.keys()].sort());
    core.dispose();
  });

  it("rewrites every fragment prone to catastrophic backtracking where it appears", () => {
    // Nombre d'occurrences dans les grammaires d'origine (@shikijs/langs
    // 3.14.0) : une mise à jour qui le change doit être revue, et le fuzz
    // ci-dessous relancé.
    const expected = [15, 2, 1, 16, 9, 562];
    expect(PATTERN_REWRITES).toHaveLength(expected.length);
    PATTERN_REWRITES.forEach((rewrite, i) => {
      const originals = ORIGINAL_GRAMMARS.filter((grammar) =>
        rewrite.langs.includes(grammar.name),
      );
      const count = (grammars: LanguageRegistration[], text: string) =>
        grammars
          .flatMap((grammar) => [...patternsOf(grammar)])
          .reduce((n, source) => n + source.split(text).length - 1, 0);
      expect(count(originals, rewrite.from), rewrite.from).toBe(expected[i]);
      const guarded = LANGS.flat().filter((grammar) =>
        rewrite.langs.includes(grammar.name),
      );
      expect(count(guarded, rewrite.from), rewrite.from).toBe(0);
      expect(count(guarded, rewrite.to), rewrite.to).toBeGreaterThan(0);
    });
    // La copie de css embarquée par html est la même grammaire gardée, comme
    // celle de cpp-macro embarquée par cpp.
    for (const name of ["css", "cpp-macro", "go"]) {
      const copies = new Set(
        LANGS.flat().filter((grammar) => grammar.name === name),
      );
      expect(copies.size, name).toBe(1);
      const [copy] = copies;
      expect(ORIGINAL_GRAMMARS).not.toContain(copy);
    }
    // Grammaires sans fragment fautif : objets d'origine, sans copie.
    const javascript = LANGS.flat().find((g) => g.name === "javascript");
    expect(ORIGINAL_GRAMMARS).toContain(javascript);
  });

  it("keeps every match of the rewritten patterns", () => {
    // Chaque motif réécrit contre son original, sur des chaînes courtes (où
    // l'original reste rapide) : mêmes correspondances, mêmes captures. Passe
    // plus large (29 millions de chaînes) : voir le rapport de l'étape 8.
    const mismatches: string[] = [];
    let pairs = 0;
    let strings = 0;
    for (const original of ORIGINAL_GRAMMARS) {
      const guarded = guardGrammar(original);
      if (guarded === original) continue;
      const tokens = DIFF_TOKENS[original.name] ?? [];
      expect(tokens.length, original.name).toBeGreaterThan(0);
      const before = [...patternsOf(original)];
      const after = [...patternsOf(guarded)];
      expect(after).toHaveLength(before.length);
      // C++ : 250 motifs réécrits, souvent longs ; moins de chaînes chacun.
      const random = original.name.startsWith("cpp") ? 30 : 1000;
      before.forEach((source, i) => {
        if (source === after[i]) return;
        pairs += 1;
        const a = compile(source);
        const b = compile(after[i]);
        for (const text of corpus(tokens, source, random)) {
          strings += 1;
          const expected = allMatches(a, text).join(" ");
          const actual = allMatches(b, text).join(" ");
          if (actual !== expected && mismatches.length < 10) {
            mismatches.push(`${after[i]} <${text}> ${actual} ≠ ${expected}`);
          }
        }
      });
    }
    expect(mismatches).toEqual([]);
    expect(pairs).toBeGreaterThan(250);
    expect(strings).toBeGreaterThan(40_000);
  });

  it("gives the tokens of the original grammars on lines that use the rewritten patterns", () => {
    const original = createHighlighterCoreSync({
      themes: [oneLightTheme],
      langs: [
        cssOriginal,
        htmlOriginal,
        markdownOriginal,
        goOriginal,
        cppOriginal,
      ],
      engine: createJavaScriptRegexEngine({ target: "ES2018" }),
    });
    const core = createLibreHighlighter();
    const cases: [string, string][] = [
      ["css", CODE_SAMPLES.css],
      ["html", CODE_SAMPLES.html],
      ["go", CODE_SAMPLES.go],
      ["cpp", CODE_SAMPLES.cpp],
      ...Object.entries(REWRITTEN_PATTERN_SAMPLES).flatMap(([lang, lines]) =>
        lines.map((line): [string, string] => [lang, line]),
      ),
      ...REWRITTEN_PATTERN_SAMPLES.css.map((line): [string, string] => [
        "html",
        `<style>${line}</style>`,
      ]),
      [
        "markdown",
        "```css\n" + REWRITTEN_PATTERN_SAMPLES.css.join("\n") + "\n```",
      ],
      [
        "markdown",
        "```go\n" + REWRITTEN_PATTERN_SAMPLES.go.join("\n") + "\n```",
      ],
    ];
    for (const [lang, code] of cases) {
      const options = {
        lang,
        theme: "one-light",
        tokenizeTimeLimit: 0,
      } as const;
      expect(core.codeToTokensBase(code, options), `${lang}: ${code}`).toEqual(
        original.codeToTokensBase(code, options),
      );
    }
    original.dispose();
    core.dispose();
  });

  it("tokenizes quickly the lines that froze the JavaScript engine", () => {
    const core = createLibreHighlighter();
    for (const { lang, code } of CATASTROPHIC_LINES) {
      // Compilation hors du temps mesuré : début de la ligne, deux fois (V8
      // interprète une expression avant de la compiler en code natif).
      const options = { lang, theme: "one-light", tokenizeTimeLimit: 0 };
      core.codeToTokensBase(code.slice(0, 24), options);
      core.codeToTokensBase(code.slice(0, 24), options);
      // Sans les réécritures : de 1 s à 25 s selon la ligne.
      const start = performance.now();
      core.codeToTokensBase(code, options);
      expect(performance.now() - start, `${lang}: ${code}`).toBeLessThan(100);
    }
    core.dispose();
  });

  it("finds no expression that blows up on repeated adversarial units", () => {
    // Petit fuzz de toutes les expressions (passe complète, unités tirées des
    // motifs et chaînes de 490 caractères : voir le rapport de l'étape 8).
    // Longueur croissante, arrêt au premier dépassement : une croissance
    // exponentielle échoue vite au lieu de bloquer le test.
    const slow: string[] = [];
    for (const source of regexSources()) {
      const regex = defaultJavaScriptRegexConstructor(source, {
        target: "ES2018",
      });
      // Compilation hors du temps mesuré : V8 interprète une expression à sa
      // première exécution et ne la compile en code natif qu'à la suivante.
      allMatches(regex, "a b\n");
      allMatches(regex, "a b\n");
      for (const [prefix, unit, suffix, counts] of FUZZ_CASES) {
        for (const count of counts) {
          const text = prefix + unit.repeat(count) + suffix + "\n";
          const start = performance.now();
          allMatches(regex, text);
          const elapsed = performance.now() - start;
          if (elapsed > FUZZ_BUDGET_MS) {
            slow.push(
              `${elapsed.toFixed(0)} ms <${text}> ${source.slice(0, 120)}`,
            );
            break;
          }
        }
      }
    }
    expect(slow).toEqual([]);
  });

  it("leaves almost nothing to compile at first use after the warm-up", async () => {
    // Module neuf : cache vide, comme au démarrage de l'app.
    vi.resetModules();
    const fresh = await import("./highlighterCore");
    const engine = await import("shiki/engine/javascript");
    const compile = vi.mocked(engine.defaultJavaScriptRegexConstructor);
    const step = fresh.createRegexWarmUp();
    while (step());
    compile.mockClear();

    const core = fresh.createLibreHighlighter();
    for (const [lang, code] of Object.entries(CODE_SAMPLES)) {
      core.codeToTokensBase(code, { lang, theme: "one-light" });
    }
    // Sans préchauffage : environ 1 700 compilations. Restent les fins de
    // règle complétées par une capture du début (chaîne délimitée de Rust,
    // here-doc…), imprévisibles.
    const late = compile.mock.calls.map(([source]) => source);
    expect(late.length).toBeLessThanOrEqual(20);
    core.dispose();
  });
});
