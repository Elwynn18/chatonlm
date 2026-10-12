import { act, renderHook } from "@testing-library/react";
import type { ThemedToken } from "shiki/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CODE_SAMPLES } from "@/test/codeSamples";
import {
  createBlockTokenizer,
  type Highlighter,
  resolveLanguage,
  scheduleHighlighterWarmUp,
} from "./highlighter";
import { createLibreHighlighter, LIGHT_TO_DARK } from "./highlighterCore";

// Tests lourds (toutes les expressions des grammaires, 19 langages en entier) :
// quelques secondes seuls, davantage quand toute la suite tourne en parallèle
// (check.sh). Le délai par défaut de 5 s les ferait échouer au hasard.
vi.setConfig({ testTimeout: 30_000 });

vi.mock("./highlighterCore", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./highlighterCore")>();
  return {
    ...actual,
    createLibreHighlighter: vi.fn(actual.createLibreHighlighter),
    // Préchauffage simulé en trois pas : le vrai est testé dans
    // highlighterCore.test.ts.
    createRegexWarmUp: vi.fn(() => {
      let left = 3;
      return () => left-- > 0;
    }),
  };
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const core = createLibreHighlighter();
const highlighter: Highlighter = {
  core,
  lightToDark: LIGHT_TO_DARK,
  langs: new Set(core.getLoadedLanguages()),
};

/**
 * Highlighter distinct (même cœur) : les tokenizers ne reprennent que
 * l'avancement des blocs du même highlighter, d'où des tests indépendants.
 */
function freshHighlighter(): Highlighter {
  return { ...highlighter };
}

// Constantes de highlighter.ts.
const MAX_LINE = 500;
const TASK_BUDGET = 40;
const SLICE_MS = 8;
const LINE_TIME_LIMIT = 300;
const TAIL_TIME_LIMIT = 10;
const TAIL_SETTLE_MS = 300;
const COLD_ATTEMPTS = 3;

// Cœur à part pour les références : les espions posés sur les grammaires de
// `core` ne comptent que le travail des tokenizers.
const referenceCore = createLibreHighlighter();

/** Référence : tout le texte d'un coup, comme l'officiel (thème clair). */
function full(code: string, lang: string, maxLine = MAX_LINE): ThemedToken[][] {
  return referenceCore.codeToTokensBase(code, {
    lang,
    theme: "one-light",
    tokenizeTimeLimit: 0,
    tokenizeMaxLineLength: maxLine,
  });
}

/** Préfixes croissants, par fragments de 1 à `step` caractères (suite fixe). */
function prefixes(text: string, seed: number, step = 12): string[] {
  const out: string[] = [];
  let x = seed;
  for (let end = 0; end < text.length;) {
    x = (x * 1103515245 + 12345) % 2 ** 31;
    end = Math.min(text.length, end + 1 + (x % step));
    out.push(text.slice(0, end));
  }
  return out;
}

/**
 * Temps figé : la limite de temps de vscode-textmate ne coupe jamais une
 * ligne et le budget de la tâche n'est jamais entamé, quelle que soit la
 * vitesse de la machine.
 */
function freezeTime() {
  vi.spyOn(Date, "now").mockReturnValue(1_000_000_000);
  vi.spyOn(performance, "now").mockReturnValue(1_000_000_000);
}

/**
 * Temps simulé : performance.now et Date.now n'avancent que par tick (ou
 * wait) ; minuteries simulées, jouées à la demande. tasks : durée de chaque
 * minuterie jouée (une tâche), sur l'horloge simulée.
 */
function fakeClock() {
  let now = 0;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.spyOn(Date, "now").mockImplementation(() => 1_000_000 + now);
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const tasks: number[] = [];
  const schedule = globalThis.setTimeout;
  vi.spyOn(globalThis, "setTimeout").mockImplementation(((
    handler: () => void,
    delay?: number,
  ) =>
    schedule(() => {
      const before = now;
      handler();
      tasks.push(now - before);
    }, delay)) as unknown as typeof setTimeout);
  return {
    tasks,
    now: () => now,
    tick(ms: number) {
      now += ms;
    },
    /** Laisse passer ms millisecondes, minuteries comprises. */
    wait(ms: number) {
      now += ms;
      vi.advanceTimersByTime(ms);
    },
  };
}

type Clock = ReturnType<typeof fakeClock>;

/**
 * La grammaire d'un langage, dont chaque ligne coûte cost(line) ms sur
 * l'horloge simulée ; arrêtée (stoppedEarly) si ce coût dépasse sa limite.
 */
function costly(
  lang: string,
  clock: Pick<Clock, "tick">,
  cost: (line: string, timeLimit: number) => number,
) {
  const grammar = core.getLanguage(lang);
  const original = grammar.tokenizeLine2.bind(grammar);
  return vi
    .spyOn(grammar, "tokenizeLine2")
    .mockImplementation((line, state, timeLimit = 0) => {
      const ms = cost(line, timeLimit);
      if (timeLimit > 0 && ms > timeLimit) {
        clock.tick(timeLimit + 1);
        return {
          tokens: new Uint32Array([0, 0]),
          ruleStack: state!,
          stoppedEarly: true,
        };
      }
      clock.tick(ms);
      return original(line, state, 0);
    });
}

/** Compte les lignes réellement tokenisées par la grammaire d'un langage. */
function spyLines(lang: string) {
  const grammar = core.getLanguage(lang);
  return vi.spyOn(grammar, "tokenizeLine2");
}

const isPlain = (line: ThemedToken[]) =>
  line.length > 0 && line.every((token) => token.color === "");
const isColored = (line: ThemedToken[]) =>
  line.length > 0 && line.every((token) => token.color !== "");

/** Bloc python de n lignes non vides, terminé par "\n" comme chez Streamdown. */
function pythonBlock(n: number, name = "v"): string {
  const lines = Array.from({ length: n }, (_, i) => `${name}${i} = ${i}`);
  return `${lines.join("\n")}\n`;
}

/** Envoie le bloc ligne par ligne, comme un flux, à un tokenizer. */
function streamLines(
  tokenize: (h: Highlighter, code: string, lang: string) => unknown,
  h: Highlighter,
  code: string,
  lang: string,
) {
  const lines = code.replace(/\n$/, "").split("\n");
  for (let k = 1; k <= lines.length; k++) {
    tokenize(h, `${lines.slice(0, k).join("\n")}\n`, lang);
  }
}

const LONG_LINE = `const data = "${"x".repeat(MAX_LINE)}";`;
const STREAMS: [string, string, number?][] = [
  ["typescript", CODE_SAMPLES.typescript],
  ["python", CODE_SAMPLES.python],
  ["cpp", CODE_SAMPLES.cpp],
  ["html", CODE_SAMPLES.html],
  ["markdown", CODE_SAMPLES.markdown],
  ["bash", CODE_SAMPLES.bash],
  ["yaml", CODE_SAMPLES.yaml],
  // Fins de ligne Windows.
  ["javascript", CODE_SAMPLES.javascript.replace(/\n/g, "\r\n")],
  // Lignes vides, lignes trop longues (texte brut, état inchangé), l'une dans
  // un commentaire de bloc ; fragments plus gros (lignes longues coûteuses).
  [
    "javascript",
    `/* start\n\n${LONG_LINE}\nstill comment */\n\n${LONG_LINE}\nlet y = 2;`,
    60,
  ],
];

describe("createBlockTokenizer", () => {
  it.each(STREAMS)(
    "gives at each fragment of a stream the same lines as the whole text (%s)",
    (lang, sample, step) => {
      freezeTime();
      const h = freshHighlighter();
      const tokenize = createBlockTokenizer();
      for (const prefix of prefixes(sample, sample.length, step)) {
        // Le texte d'un bloc se termine toujours par "\n", même en cours de flux.
        const code = `${prefix}\n`;
        expect(tokenize(h, code, lang), JSON.stringify(prefix)).toEqual(
          full(code, lang),
        );
      }
    },
  );

  it("gives the same lines as the whole text in all 19 languages", () => {
    freezeTime();
    for (const [lang, sample] of Object.entries(CODE_SAMPLES)) {
      const code = `${sample}\n`;
      expect(createBlockTokenizer()(freshHighlighter(), code, lang)).toEqual(
        full(code, lang),
      );
      // Sans "\n" final aussi.
      expect(createBlockTokenizer()(freshHighlighter(), sample, lang)).toEqual(
        full(sample, lang),
      );
    }
  });

  it("only tokenizes the new lines and the last line while streaming", () => {
    freezeTime();
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    const lines = CODE_SAMPLES.typescript.split("\n");
    // Lignes 0 à 9 : la 9 est la dernière, elle peut encore s'allonger.
    tokenize(h, `${lines.slice(0, 10).join("\n")}\n`, "typescript");
    const tokenizeLine = spyLines("typescript");
    const code = `${lines.slice(0, 12).join("\n")}\n`;
    const result = tokenize(h, code, "typescript");
    const seen = tokenizeLine.mock.calls.map(([line]) => line);
    // La 9 (maintenant terminée), la 10, et la 11, nouvelle dernière ligne ;
    // une ligne vide n'est jamais tokenisée (comme chez Shiki).
    expect(seen).toEqual(
      [lines[9], lines[10], lines[11]].filter((line) => line !== ""),
    );
    expect(result).toEqual(full(code, "typescript"));
  });

  it("returns the same array without work for the same text", () => {
    freezeTime();
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    const code = `${CODE_SAMPLES.python}\n`;
    const first = tokenize(h, code, "python");
    const tokenizeLine = spyLines("python");
    expect(tokenize(h, code, "python")).toBe(first);
    expect(tokenizeLine).not.toHaveBeenCalled();
  });

  it("resumes after the last unchanged line when a finished line changes or the text gets shorter", () => {
    freezeTime();
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    const lines = CODE_SAMPLES.typescript.split("\n");
    const code = `${lines.join("\n")}\n`;
    tokenize(h, code, "typescript");
    const tokenizeLine = spyLines("typescript");

    // Ligne 8 modifiée : les lignes 0 à 7 sont gardées.
    const edited = [...lines];
    edited[8] = "export async function loaded(id: string) {";
    const editedCode = `${edited.join("\n")}\n`;
    expect(tokenize(h, editedCode, "typescript")).toEqual(
      full(editedCode, "typescript"),
    );
    expect(tokenizeLine.mock.calls.map(([line]) => line)).toEqual(
      edited.slice(8).filter((line) => line !== ""),
    );

    // Texte raccourci : seule sa nouvelle dernière ligne est tokenisée.
    tokenizeLine.mockClear();
    const shorter = `${edited.slice(0, 5).join("\n")}\n`;
    expect(tokenize(h, shorter, "typescript")).toEqual(
      full(shorter, "typescript"),
    );
    expect(tokenizeLine.mock.calls.map(([line]) => line)).toEqual(
      [edited[4]].filter((line) => line !== ""),
    );

    // Un texte qui n'a qu'une ligne commune : plus de chaîne ouverte.
    const reopened = '`open\n"x"\n';
    tokenize(h, '`open\n"x', "javascript");
    expect(tokenize(h, reopened, "javascript")).toEqual(
      full(reopened, "javascript"),
    );
    // Même dernière ligne, au même endroit, après une autre première ligne :
    // elle n'est plus dans le commentaire.
    tokenize(h, "/*\nlet x = 1;\n", "javascript");
    expect(tokenize(h, "//\nlet x = 1;\n", "javascript")).toEqual(
      full("//\nlet x = 1;\n", "javascript"),
    );
  });

  it("does not tokenize the block again when its closing fence arrives in two fragments", () => {
    freezeTime();
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    const lines = CODE_SAMPLES.typescript.split("\n");
    const body = lines.join("\n");
    streamLines(tokenize, h, `${body}\n`, "typescript");
    // « `` » : pas encore une clôture, une ligne de code de plus.
    tokenize(h, `${body}\n\`\`\n`, "typescript");
    const tokenizeLine = spyLines("typescript");
    // « ``` » : clôture, la ligne « `` » disparaît et la dernière ligne
    // redevient la dernière.
    const closed = `${body}\n`;
    expect(tokenize(h, closed, "typescript")).toEqual(
      full(closed, "typescript"),
    );
    expect(tokenizeLine.mock.calls.map(([line]) => line)).toEqual([
      lines[lines.length - 1],
    ]);
  });

  it("tokenizes each fragment in bounded work when a repair line follows an open block", () => {
    // Réparation du Markdown incomplet (Streamdown 1.4) : « \n$$ » ajouté à
    // la fin d'un bloc ouvert qui contient un nombre impair de « $$ ». La
    // ligne en cours passe donc pour terminée, puis change au fragment
    // suivant.
    freezeTime();
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    const script = Array.from(
      { length: 60 },
      (_, i) => `echo "pid: $$" > /tmp/run.${i}.log # step ${i}`,
    ).join("\n");
    const tokenizeLine = spyLines("bash");
    let before = "";
    for (const prefix of prefixes(script, 7, 24)) {
      const code = `${prefix}\n$$\n`;
      tokenizeLine.mockClear();
      expect(tokenize(h, code, "bash"), JSON.stringify(prefix)).toEqual(
        full(code, "bash"),
      );
      // La ligne en cours (modifiée), les lignes arrivées dans ce fragment,
      // et la ligne « $$ ».
      const newLines = prefix.slice(before.length).split("\n").length - 1;
      expect(tokenizeLine.mock.calls.length).toBeLessThanOrEqual(newLines + 2);
      before = prefix;
    }
  });

  it("tokenizes all the lines once for a new language, and resumes the old one from the cache", () => {
    freezeTime();
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    const lines = CODE_SAMPLES.typescript.split("\n");
    const code = `${lines.join("\n")}\n`;
    streamLines(tokenize, h, code, "typescript");
    const tokenizeLine = spyLines("tsx");
    expect(tokenize(h, code, "tsx")).toEqual(full(code, "tsx"));
    expect(tokenizeLine.mock.calls.length).toBe(
      lines.filter((line) => line !== "").length,
    );
    const tokenizeTs = spyLines("typescript");
    expect(tokenize(h, code, "typescript")).toEqual(full(code, "typescript"));
    expect(tokenizeTs.mock.calls.map(([line]) => line)).toEqual([
      lines[lines.length - 1],
    ]);
  });

  it("lets a remounted block resume the lines already done for the same highlighter", () => {
    freezeTime();
    const h = freshHighlighter();
    const lines = CODE_SAMPLES.java.split("\n");
    createBlockTokenizer()(h, `${lines.slice(0, 15).join("\n")}\n`, "java");
    const tokenizeLine = spyLines("java");
    const code = `${lines.join("\n")}\n`;
    // Bloc remonté : un tokenizer neuf, le texte final.
    const result = createBlockTokenizer()(h, code, "java");
    const seen = tokenizeLine.mock.calls.map(([line]) => line);
    expect(seen).toEqual(lines.slice(14).filter((line) => line !== ""));
    expect(result).toEqual(full(code, "java"));

    // Autre highlighter : rien n'est repris.
    tokenizeLine.mockClear();
    createBlockTokenizer()(freshHighlighter(), code, "java");
    expect(tokenizeLine.mock.calls.length).toBe(
      lines.filter((line) => line !== "").length,
    );
  });

  it("only tokenizes the last lines when a response of 20 code blocks is remounted", async () => {
    const { hl } = await freshModules();
    freezeTime();
    const h = freshHighlighter();
    const blocks = Array.from({ length: 20 }, (_, i) =>
      pythonBlock(10, `b${i}_`),
    );
    for (const block of blocks) {
      streamLines(hl.createBlockTokenizer(), h, block, "python");
    }
    const tokenizeLine = spyLines("python");
    for (const block of blocks) {
      expect(hl.createBlockTokenizer()(h, block, "python")).toEqual(
        full(block, "python"),
      );
    }
    // La dernière ligne de chaque bloc (elle pouvait encore s'allonger).
    expect(tokenizeLine).toHaveBeenCalledTimes(20);
  });

  it("keeps the blocks of a response when another one is remounted", async () => {
    // Deux réponses de 30 blocs : la seconde, plus récente, est remontée ; la
    // première n'est pas chassée du cache par ses copies.
    const { hl } = await freshModules();
    freezeTime();
    const h = freshHighlighter();
    const first = Array.from({ length: 30 }, (_, i) =>
      pythonBlock(4, `f${i}_`),
    );
    const second = Array.from({ length: 30 }, (_, i) =>
      pythonBlock(4, `s${i}_`),
    );
    for (const block of [...first, ...second]) {
      streamLines(hl.createBlockTokenizer(), h, block, "python");
    }
    for (const block of second) hl.createBlockTokenizer()(h, block, "python");
    const tokenizeLine = spyLines("python");
    for (const block of first) {
      expect(hl.createBlockTokenizer()(h, block, "python")).toEqual(
        full(block, "python"),
      );
    }
    expect(tokenizeLine).toHaveBeenCalledTimes(30);
  });

  it("does not fill the cache with restarts or blocks without a finished line", async () => {
    const { hl } = await freshModules();
    freezeTime();
    const h = freshHighlighter();
    const kept = `${CODE_SAMPLES.java}\n`;
    streamLines(hl.createBlockTokenizer(), h, kept, "java");
    // 70 blocs d'une ligne : aucune ligne terminée.
    for (let i = 0; i < 70; i++) {
      hl.createBlockTokenizer()(h, `x${i} = ${i}\n`, "java");
    }
    // 70 redémarrages d'un même bloc (clôture coupée puis complète).
    const restarted = hl.createBlockTokenizer();
    const body = CODE_SAMPLES.java.replace(/class/g, "interface");
    for (let i = 0; i < 70; i++) {
      restarted(h, `${body}\n\`\`\n`, "java");
      restarted(h, `${body}\n`, "java");
    }
    const tokenizeLine = spyLines("java");
    expect(hl.createBlockTokenizer()(h, kept, "java")).toEqual(
      full(kept, "java"),
    );
    expect(tokenizeLine).toHaveBeenCalledTimes(1);
  });

  it("keeps the lines of each block when three blocks start the same way", () => {
    freezeTime();
    const h = freshHighlighter();
    const texts = [
      ["a = 1", "b = 2", "e = 5", "h = 8"],
      ["a = 1", "c = 3", "d = 4", "f = 6"],
      ["a = 1", "x = 9", "g = 7", "i = 9"],
    ];
    const tokenizers = texts.map(() => createBlockTokenizer());
    // Flux entrelacés : chaque bloc reprend l'avancement d'un autre.
    for (let k = 2; k <= 4; k++) {
      texts.forEach((lines, i) => {
        const code = `${lines.slice(0, k).join("\n")}\n`;
        expect(tokenizers[i](h, code, "python"), code).toEqual(
          full(code, "python"),
        );
      });
    }
  });

  it("keeps a slow last line plain while it streams, and colors it once complete", () => {
    freezeTime();
    const grammar = core.getLanguage("javascript");
    const original = grammar.tokenizeLine2.bind(grammar);
    // Ligne « lente » : dépasse la limite de la dernière ligne pendant le
    // flux, pas le budget d'une ligne terminée.
    const tokenizeLine = vi
      .spyOn(grammar, "tokenizeLine2")
      .mockImplementation((line, state, timeLimit = 0) =>
        line.includes("slow") && timeLimit > 0 && timeLimit < 20
          ? {
              tokens: new Uint32Array([0, 0]),
              ruleStack: state!,
              stoppedEarly: true,
            }
          : original(line, state, timeLimit),
      );
    const h = freshHighlighter();
    const tokenize = createBlockTokenizer();
    // Premier calcul (bloc neuf) : la dernière ligne a le budget.
    expect(tokenize(h, "let a = 1;\nslow(1\n", "javascript")).toEqual(
      full("let a = 1;\nslow(1\n", "javascript"),
    );
    // Pendant le flux : trop lente, en texte brut.
    const plain = tokenize(h, "let a = 1;\nslow(1, 2\n", "javascript");
    expect(plain[1]).toEqual([
      { content: "slow(1, 2", offset: 11, color: "", fontStyle: 0 },
    ]);
    expect(plain[0]).toEqual(full("let a = 1;\n", "javascript")[0]);
    // Les fragments suivants ne la retentent pas.
    tokenizeLine.mockClear();
    tokenize(h, "let a = 1;\nslow(1, 2, 3\n", "javascript");
    expect(tokenizeLine).not.toHaveBeenCalled();
    // Terminée : colorée comme le reste.
    const done = "let a = 1;\nslow(1, 2, 3);\nb\n";
    expect(tokenize(h, done, "javascript")).toEqual(full(done, "javascript"));
  });

  it("colors a slow last line in the background once the text stops changing", async () => {
    const { hl } = await freshModules();
    const clock = fakeClock();
    const tokenizeLine = costly("javascript", clock, (line) =>
      line.includes("slow") ? TAIL_TIME_LIMIT + 5 : 1,
    );
    const h = freshHighlighter();
    const tokenize = hl.createBlockTokenizer();
    const onProgress = vi.fn();
    tokenize.subscribe(onProgress);
    tokenize(h, "let a = 1;\n", "javascript");
    expect(isPlain(tokenize(h, "let a = 1;\nslow(1\n", "javascript")[1])).toBe(
      true,
    );
    clock.wait(TAIL_SETTLE_MS - 1);
    expect(onProgress).not.toHaveBeenCalled();
    // Le texte change encore : ni retentée au premier plan, ni colorée avant
    // que le texte ne soit stable.
    tokenizeLine.mockClear();
    const code = "let a = 1;\nslow(1, 2\n";
    expect(isPlain(tokenize(h, code, "javascript")[1])).toBe(true);
    clock.wait(TAIL_SETTLE_MS - 1);
    expect(tokenizeLine).not.toHaveBeenCalled();
    expect(onProgress).not.toHaveBeenCalled();
    // Stable depuis 300 ms, puis la tranche suivante.
    clock.wait(2);
    expect(onProgress).toHaveBeenCalledTimes(1);
    expect(tokenizeLine).toHaveBeenCalledTimes(1);
    expect(tokenizeLine.mock.calls[0][2]).toBe(LINE_TIME_LIMIT);
    expect(tokenize.getVersion()).toBe(1);
    expect(tokenize(h, code, "javascript")).toEqual(full(code, "javascript"));
  });

  it("spends at most the task budget on the blocks of one render, then colors the rest in idle slices", async () => {
    const { hl } = await freshModules();
    const clock = fakeClock();
    const cost = 5;
    costly("python", clock, () => cost);
    const h = freshHighlighter();
    const codes = [pythonBlock(20, "a"), pythonBlock(20, "b")];
    const tokenizers = codes.map(() => hl.createBlockTokenizer());

    // Un même rendu : les deux blocs partagent le budget de la tâche.
    const results = tokenizers.map((tokenize, i) =>
      tokenize(h, codes[i], "python"),
    );
    expect(clock.now()).toBe(TASK_BUDGET);
    const colored = TASK_BUDGET / cost;
    expect(results[0].slice(0, colored).every(isColored)).toBe(true);
    expect(results[0].slice(colored, 20).every(isPlain)).toBe(true);
    expect(results[1].slice(0, 20).every(isPlain)).toBe(true);
    // Les lignes colorées sont déjà exactes.
    expect(results[0].slice(0, colored)).toEqual(
      full(codes[0], "python").slice(0, colored),
    );

    // Temps morts : par tranches d'environ 8 ms, chaque ligne colorée
    // prévient les abonnés. Comme CodeBlock, l'abonné rend le bloc de
    // nouveau dans la même tâche : ce rendu ne tokenise rien de plus.
    const notified = tokenizers.map((tokenize, i) => {
      const onProgress = vi.fn(() => void tokenize(h, codes[i], "python"));
      tokenize.subscribe(onProgress);
      return onProgress;
    });
    // Sans attendre que le texte soit stable : aucune dernière ligne lente.
    vi.advanceTimersByTime(TAIL_SETTLE_MS - 1);
    expect(Math.max(...clock.tasks)).toBeLessThan(SLICE_MS + cost);
    const slices = clock.tasks.filter((ms) => ms > 0);
    expect(slices.length).toBeGreaterThanOrEqual((40 - colored) / 2);
    expect(tokenizers[0].getVersion()).toBe(20 - colored);
    expect(tokenizers[1].getVersion()).toBe(20);
    expect(notified[0]).toHaveBeenCalledTimes(20 - colored);
    expect(notified[1]).toHaveBeenCalledTimes(20);
    tokenizers.forEach((tokenize, i) =>
      expect(tokenize(h, codes[i], "python")).toEqual(full(codes[i], "python")),
    );
  });

  it("gives a new budget at the next task", async () => {
    const { hl } = await freshModules();
    const clock = fakeClock();
    costly("python", clock, () => 5);
    const h = freshHighlighter();
    const first = hl.createBlockTokenizer()(h, pythonBlock(20, "a"), "python");
    expect(first.filter(isColored)).toHaveLength(8);
    // Même tâche : plus rien.
    const second = hl.createBlockTokenizer()(h, pythonBlock(20, "b"), "python");
    expect(second.filter(isColored)).toHaveLength(0);
    vi.advanceTimersToNextTimer();
    const third = hl.createBlockTokenizer()(h, pythonBlock(20, "c"), "python");
    expect(third.filter(isColored)).toHaveLength(8);
  });

  it("works in the background on the last text it was given", async () => {
    const { hl } = await freshModules();
    const clock = fakeClock();
    costly("python", clock, () => 5);
    const h = freshHighlighter();
    const tokenize = hl.createBlockTokenizer();
    tokenize.subscribe(() => {});
    tokenize(h, pythonBlock(20), "python");
    vi.advanceTimersToNextTimer();
    vi.advanceTimersToNextTimer();
    // Le bloc s'allonge, puis une ligne déjà colorée change.
    const longer = pythonBlock(30);
    tokenize(h, longer, "python");
    vi.advanceTimersToNextTimer();
    const edited = longer.replace("v3 = 3", "v3 = [3]");
    tokenize(h, edited, "python");
    vi.runAllTimers();
    expect(tokenize(h, edited, "python")).toEqual(full(edited, "python"));
  });

  it("stops working in the background once nobody listens, and resumes on a new subscription", async () => {
    const { hl } = await freshModules();
    const clock = fakeClock();
    const tokenizeLine = costly("python", clock, () => 5);
    const h = freshHighlighter();
    const tokenize = hl.createBlockTokenizer();
    const code = pythonBlock(20);
    tokenize(h, code, "python");
    // Sans abonné : rien en tâche de fond.
    tokenizeLine.mockClear();
    vi.runAllTimers();
    expect(tokenizeLine).not.toHaveBeenCalled();
    // Désabonné avant la première tranche, puis entre deux tranches.
    tokenize.subscribe(() => {})();
    vi.runAllTimers();
    expect(tokenizeLine).not.toHaveBeenCalled();
    const unsubscribe = tokenize.subscribe(() => {});
    for (let i = 0; i < 10 && tokenizeLine.mock.calls.length === 0; i++) {
      vi.advanceTimersToNextTimer();
    }
    expect(tokenizeLine).toHaveBeenCalled();
    unsubscribe();
    const calls = tokenizeLine.mock.calls.length;
    const version = tokenize.getVersion();
    vi.runAllTimers();
    expect(tokenizeLine).toHaveBeenCalledTimes(calls);
    expect(tokenize.getVersion()).toBe(version);
    // Nouvel abonné : le travail reprend jusqu'au bout.
    tokenize.subscribe(() => {});
    vi.runAllTimers();
    expect(tokenize(h, code, "python")).toEqual(full(code, "python"));
  });

  it("gives a line stopped while cold one pass per idle slice, and the foreground only its budget", async () => {
    const { hl } = await freshModules(); // préchauffage pas fini
    const clock = fakeClock();
    const tokenizeLine = costly("python", clock, (line) =>
      line.startsWith("slow") ? 1000 : 1,
    );
    const h = freshHighlighter();
    const tokenize = hl.createBlockTokenizer();
    const code = "a = 1\nslow = 2\nb = 3\nc = 4\n";
    tokenize(h, code, "python");
    // Premier plan : une passe, bornée par le budget.
    expect(clock.now()).toBeLessThanOrEqual(TASK_BUDGET + 1);
    const slowCalls = () =>
      tokenizeLine.mock.calls.filter(([line]) => line.startsWith("slow"));
    expect(slowCalls()).toHaveLength(1);
    expect(slowCalls()[0][2]).toBeLessThanOrEqual(TASK_BUDGET);
    // Tâche suivante, texte prolongé : pas d'autre essai au premier plan.
    vi.advanceTimersToNextTimer();
    const longer = `${code}d = 5\n`;
    tokenize(h, longer, "python");
    expect(slowCalls()).toHaveLength(1);

    tokenize.subscribe(() => {});
    vi.runAllTimers();
    // Une passe complète (LINE_TIME_LIMIT + 1 ms simulées) par tâche au plus.
    expect(Math.max(...clock.tasks)).toBeLessThan(2 * (LINE_TIME_LIMIT + 1));
    const full_ = slowCalls().filter(
      ([, , limit]) => limit === LINE_TIME_LIMIT,
    );
    expect(full_).toHaveLength(COLD_ATTEMPTS);
    // Comme une ligne trop longue chez Shiki : texte brut, état inchangé.
    expect(tokenize(h, longer, "python")).toEqual(
      full(longer, "python", "slow = 2".length),
    );
  });

  it("gives a slow line a single pass once the highlighter is warmed up", async () => {
    const { hl } = await freshModules();
    await hl.warmUpHighlighter();
    const clock = fakeClock();
    const tokenizeLine = costly("javascript", clock, (line) =>
      line.startsWith("/*") ? 1000 : 1,
    );
    const slow = "/* never ends in time and is the longest line";
    const code = `const a = 1;\n${slow}\nconst b = "x"; */\n`;
    const tokenize = hl.createBlockTokenizer();
    tokenize.subscribe(() => {});
    const h = freshHighlighter();
    tokenize(h, code, "javascript");
    vi.runAllTimers();
    expect(
      tokenizeLine.mock.calls.filter(
        ([line, , limit]) => line === slow && limit === LINE_TIME_LIMIT,
      ),
    ).toHaveLength(1);
    // La ligne suivante n'est pas dans le commentaire qu'elle aurait ouvert.
    const result = tokenize(h, code, "javascript");
    expect(result).toEqual(full(code, "javascript", slow.length));
    expect(result[1]).toEqual([
      { content: slow, offset: 13, color: "", fontStyle: 0 },
    ]);
  });

  it("retries in the background a line stopped by the first compilation of the grammar patterns", async () => {
    const { hl } = await freshModules(); // préchauffage pas fini
    const clock = fakeClock();
    let cold = COLD_ATTEMPTS; // le premier plan, puis deux passes complètes
    costly("javascript", clock, (line) =>
      line.includes("`x`") && cold-- > 0 ? 1000 : 1,
    );
    const code = "const a = `x`;\nlet b;\n";
    const tokenize = hl.createBlockTokenizer();
    tokenize.subscribe(() => {});
    const h = freshHighlighter();
    tokenize(h, code, "javascript");
    vi.runAllTimers();
    expect(tokenize(h, code, "javascript")).toEqual(full(code, "javascript"));
  });

  it("raises at the next call an error met in the background", async () => {
    const { hl } = await freshModules();
    const clock = fakeClock();
    const grammar = core.getLanguage("python");
    vi.spyOn(grammar, "tokenizeLine2").mockImplementation(
      (_line, state, timeLimit = 0) => {
        if (timeLimit === LINE_TIME_LIMIT) throw new SyntaxError("lookbehind");
        clock.tick(timeLimit + 1);
        return {
          tokens: new Uint32Array([0, 0]),
          ruleStack: state!,
          stoppedEarly: true,
        };
      },
    );
    const h = freshHighlighter();
    const tokenize = hl.createBlockTokenizer();
    const onProgress = vi.fn();
    tokenize.subscribe(onProgress);
    const code = pythonBlock(3);
    expect(
      tokenize(h, code, "python").every((l) => l.length === 0 || isPlain(l)),
    ).toBe(true);
    vi.runAllTimers();
    expect(onProgress).toHaveBeenCalledTimes(1);
    expect(() => tokenize(h, code, "python")).toThrow("lookbehind");
  });
});

describe("resolveLanguage", () => {
  it.each([
    ["Python", "python"],
    ["ts", "ts"],
    ["js", "js"],
    [" TS ", "ts"],
    ["c++", "c++"],
    ["shell", "shell"],
    ["ruby", null],
    ["", null],
    ["text", null],
  ])("resolves %j to %j", (lang, expected) => {
    expect(resolveLanguage(highlighter, lang)).toBe(expected);
  });
});

/** Modules neufs : état du chargement, du budget et du cache remis à zéro. */
async function freshModules() {
  vi.resetModules();
  const hl = await import("./highlighter");
  const coreModule = await import("./highlighterCore");
  return {
    hl,
    create: vi.mocked(coreModule.createLibreHighlighter),
    warmUp: vi.mocked(coreModule.createRegexWarmUp),
  };
}

describe("preloadHighlighter", () => {
  it("creates a single highlighter, however many times it is called", async () => {
    const { hl, create } = await freshModules();
    const first = hl.preloadHighlighter();
    expect(hl.preloadHighlighter()).toBe(first);
    await first;
    await hl.preloadHighlighter();
    await hl.warmUpHighlighter();
    expect(create).toHaveBeenCalledTimes(1);
  });

  it("can be retried after a failure, which leaves code plain", async () => {
    const { hl, create } = await freshModules();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    create.mockImplementationOnce(() => {
      throw new Error("boom");
    });
    const { result } = renderHook(() => hl.useHighlighter());
    await act(() => hl.preloadHighlighter());
    expect(result.current).toBeNull();
    expect(error).toHaveBeenCalledWith(
      "Failed to load the code highlighter:",
      expect.any(Error),
    );
    await act(() => hl.preloadHighlighter());
    expect(result.current).not.toBeNull();
    expect(create).toHaveBeenCalledTimes(2);
  });
});

describe("useHighlighter", () => {
  it("is null, then the highlighter once loaded", async () => {
    const { hl } = await freshModules();
    const { result } = renderHook(() => hl.useHighlighter());
    expect(result.current).toBeNull();
    await act(() => hl.preloadHighlighter());
    expect(result.current?.langs.has("python")).toBe(true);
    expect(result.current?.lightToDark.get("#A626A4")).toBe("#C678DD");
    expect(result.current?.core.getLoadedThemes()).toContain("one-light");
  });
});

describe("warmUpHighlighter", () => {
  it("loads the highlighter, then runs the warm-up to its end, once", async () => {
    const { hl, create, warmUp } = await freshModules();
    const done = hl.warmUpHighlighter();
    expect(hl.warmUpHighlighter()).toBe(done);
    await done;
    expect(create).toHaveBeenCalledTimes(1);
    expect(warmUp).toHaveBeenCalledTimes(1);
    const step = warmUp.mock.results[0].value as () => boolean;
    expect(step()).toBe(false);
  });

  it("works in slices of about 8 ms, handing the thread back in between", async () => {
    const { hl, warmUp } = await freshModules();
    await hl.preloadHighlighter();
    const clock = fakeClock();
    let steps = 0;
    warmUp.mockImplementationOnce(() => () => {
      steps += 1;
      clock.tick(3);
      return steps < 10;
    });
    let finished = false;
    const done = hl.warmUpHighlighter().then(() => {
      finished = true;
    });
    // 3 pas de 3 ms : la tranche dépasse 8 ms, la suite attend un temps mort.
    await vi.advanceTimersToNextTimerAsync();
    expect(steps).toBe(3);
    expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersToNextTimerAsync();
    expect(steps).toBe(6);
    await vi.advanceTimersToNextTimerAsync();
    expect(steps).toBe(9);
    expect(finished).toBe(false);
    await vi.advanceTimersToNextTimerAsync();
    await done;
    expect(steps).toBe(10);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("reports once the patterns that do not compile", async () => {
    const { hl, warmUp } = await freshModules();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    warmUp.mockImplementationOnce((onError) => {
      let left = 2;
      return () => {
        onError?.("(?<=x)", new SyntaxError("lookbehind"));
        return left-- > 0;
      };
    });
    await hl.warmUpHighlighter();
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain("3 grammar patterns");
  });
});

describe("scheduleHighlighterWarmUp", () => {
  let visibility: DocumentVisibilityState = "visible";

  function mockVisibility(initial: DocumentVisibilityState) {
    visibility = initial;
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => visibility,
    });
  }

  function fakeTimers() {
    vi.useFakeTimers({
      toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame"],
    });
  }

  afterEach(() => {
    // Revient à la propriété du prototype.
    delete (document as { visibilityState?: unknown }).visibilityState;
  });

  it("waits for the window to be shown, then for its first paint", async () => {
    mockVisibility("hidden");
    fakeTimers();
    const run = vi.fn();
    scheduleHighlighterWarmUp(run);
    await vi.advanceTimersByTimeAsync(4900);
    expect(run).not.toHaveBeenCalled();

    visibility = "visible";
    document.dispatchEvent(new Event("visibilitychange"));
    // Rien de synchrone : après le prochain dessin, puis un temps mort.
    expect(run).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(50);
    expect(run).toHaveBeenCalledTimes(1);
    // Une seule fois : ni le délai de secours ni un nouvel affichage.
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("starts anyway after 5 s if the window never reports being visible", async () => {
    mockVisibility("hidden");
    fakeTimers();
    const run = vi.fn();
    scheduleHighlighterWarmUp(run);
    await vi.advanceTimersByTimeAsync(4990);
    expect(run).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(50);
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("does not start before the first paint when the window is already visible", async () => {
    mockVisibility("visible");
    fakeTimers();
    const run = vi.fn();
    scheduleHighlighterWarmUp(run);
    await vi.advanceTimersByTimeAsync(0);
    expect(run).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(50);
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("loads and warms up the highlighter by default", async () => {
    mockVisibility("visible");
    const { hl, create, warmUp } = await freshModules();
    hl.scheduleHighlighterWarmUp();
    await vi.waitFor(() => expect(warmUp).toHaveBeenCalledTimes(1));
    expect(create).toHaveBeenCalledTimes(1);
  });
});
