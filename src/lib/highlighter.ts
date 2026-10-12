/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/lib/highlighter.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: the highlighter (highlighterCore.ts) is loaded in a
 * separate chunk once the window is shown, then warmed up in the background;
 * code blocks re-render when it is ready; fence languages are checked
 * (case-insensitive) instead of throwing; code blocks are tokenized
 * incrementally, line by line, with the light theme only (dark colors from a
 * table): the grammar state is kept after each line and an edited text
 * resumes at its longest common prefix; tokenizing during one task is capped
 * by a time budget shared by all blocks, the remaining lines are shown plain
 * then colored during idle time (subscribe/getVersion); long or slow lines
 * stay plain.
 */

import { useSyncExternalStore } from "react";
import type { Grammar, HighlighterCore, ThemedToken } from "shiki/core";

export interface Highlighter {
  core: HighlighterCore;
  lightToDark: ReadonlyMap<string, string>;
  langs: ReadonlySet<string>;
}

type CoreModule = typeof import("./highlighterCore");

let ready: Highlighter | null = null;
let coreModule: CoreModule | null = null;
let loading: Promise<void> | null = null;
let warming: Promise<void> | null = null;
let warmedUp = false;
const listeners = new Set<() => void>();

/**
 * Charge le morceau du surligneur et crée l'unique highlighter (idempotent ;
 * relançable après un échec). Ne rejette jamais : un échec laisse le code
 * en texte brut.
 */
export function preloadHighlighter(): Promise<void> {
  loading ??= import("./highlighterCore")
    .then((m) => {
      const core = m.createLibreHighlighter();
      coreModule = m;
      ready = {
        core,
        lightToDark: m.LIGHT_TO_DARK,
        langs: new Set(core.getLoadedLanguages()),
      };
      listeners.forEach((listener) => listener());
    })
    .catch((error: unknown) => {
      console.error("Failed to load the code highlighter:", error);
      loading = null; // un appel suivant réessaiera
    });
  return loading;
}

/**
 * Charge le surligneur puis compile d'avance, par petites tranches pendant
 * les temps morts, les expressions de ses grammaires (idempotent). Sans cela,
 * le premier bloc d'un langage bloque l'interface le temps de les compiler
 * (jusqu'à 1 s pour cpp dans WebKitGTK) ; ensuite, il ne coûte que sa
 * tokenisation.
 */
export function warmUpHighlighter(): Promise<void> {
  warming ??= preloadHighlighter().then(() => {
    if (!coreModule) {
      warming = null; // chargement en échec : on pourra réessayer
      return;
    }
    let failures = 0;
    let firstError: unknown;
    const step = coreModule.createRegexWarmUp((_source, error) => {
      failures += 1;
      firstError ??= error;
    });
    return runInSlices(step).then(() => {
      warmedUp = true;
      if (failures > 0) {
        console.warn(
          `Code highlighter: ${failures} grammar patterns do not compile here, some code stays plain.`,
          firstError,
        );
      }
    });
  });
  return warming;
}

// Durée d'une tranche de travail sans requestIdleCallback (absent de WebKit
// tant que la préférence expérimentale n'est pas activée).
const SLICE_MS = 8;

function whenIdle(callback: (deadline?: IdleDeadline) => void): void {
  if (typeof requestIdleCallback === "function") requestIdleCallback(callback);
  else setTimeout(callback, 0);
}

/** Appelle step jusqu'à ce qu'il renvoie false, sans monopoliser le fil. */
function runInSlices(step: () => boolean): Promise<void> {
  return new Promise((resolve) => {
    const slice = (deadline?: IdleDeadline) => {
      const end = performance.now() + SLICE_MS;
      do {
        if (!step()) {
          resolve();
          return;
        }
      } while (
        deadline ? deadline.timeRemaining() > 1 : performance.now() < end
      );
      whenIdle(slice);
    };
    whenIdle(slice);
  });
}

// Si la fenêtre ne devient jamais « visible » (fenêtre réduite au démarrage,
// état de visibilité non transmis), le surligneur est chargé quand même.
const SHOWN_FALLBACK_MS = 5000;

/**
 * Charge et préchauffe le surligneur après l'affichage de la fenêtre (créée
 * masquée, montrée par le frontend) et son premier dessin, pendant un temps
 * mort : le morceau de 2 Mo et la création du highlighter ne retardent ni
 * l'apparition de la fenêtre ni le premier rendu. Appelé une fois au
 * démarrage.
 */
export function scheduleHighlighterWarmUp(
  run: () => void = () => void warmUpHighlighter(),
): void {
  let started = false;
  function start() {
    if (started) return;
    started = true;
    document.removeEventListener("visibilitychange", onVisibilityChange);
    clearTimeout(fallback);
    whenIdle(() => run());
  }
  function onVisibilityChange() {
    // Après le prochain dessin : requestAnimationFrame passe juste avant, le
    // setTimeout juste après.
    if (document.visibilityState === "visible") {
      requestAnimationFrame(() => setTimeout(start, 0));
    }
  }
  const fallback = setTimeout(start, SHOWN_FALLBACK_MS);
  document.addEventListener("visibilitychange", onVisibilityChange);
  onVisibilityChange();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** null jusqu'à ce que le highlighter soit prêt ; le composant est alors rendu de nouveau. */
export function useHighlighter(): Highlighter | null {
  return useSyncExternalStore(subscribe, () => ready);
}

/**
 * Nom ou alias d'une grammaire chargée pour le langage d'une clôture de code
 * (« Python » → « python »), ou null : bloc en texte brut. Évite l'exception
 * de Shiki (et son console.error) à chaque fragment d'un bloc ```ruby.
 */
export function resolveLanguage(h: Highlighter, lang: string): string | null {
  const name = lang.trim().toLowerCase();
  return name && h.langs.has(name) ? name : null;
}

const THEME = "one-light";

// Une ligne de cette longueur reste en texte brut (comme l'option
// tokenizeMaxLineLength de Shiki) : le moteur JavaScript est quadratique sur
// une ligne. Mesuré dans WebKitGTK 2.52 (une passe, expressions compilées) :
// TypeScript compacté sur une ligne 60 ms à 500 caractères, 240 ms à 1000 ;
// C++ 410 ms à 500, 1,2 s à 1000.
const MAX_LINE = 500;
// Temps de tokenisation accordé au premier plan pendant une tâche (un rendu
// React), tous blocs confondus, mesuré avec performance.now : au-delà, les
// lignes restantes sont rendues en texte brut, sans toucher à l'état de la
// grammaire, puis colorées pendant les temps morts. Une réponse rouverte avec
// beaucoup de code, ou de nombreuses lignes arrivées d'un coup, ne fige donc
// pas l'interface plus de quelques images. Une ligne courante prend 1 à 5 ms
// dans WebKitGTK, 10 à 60 ms en C++ (grammaire très lourde).
const TASK_BUDGET = 40;
// Temps accordé à une ligne par la tâche de fond. Au-delà, elle reste en
// texte brut et l'état de la grammaire ne change pas (comme une ligne trop
// longue), au lieu de la coupure de Shiki qui garde un état pris au milieu
// de la ligne et fausse les lignes suivantes. Au premier plan, une ligne n'a
// que ce qui reste du budget : si elle ne finit pas, elle passe à la tâche de
// fond, sans être tenue pour trop lente.
const LINE_TIME_LIMIT = 300;
// Tant que le préchauffage n'est pas fini, un dépassement peut venir de la
// compilation des expressions (jusqu'à 1 s pour le premier bloc C++ dans
// WebKitGTK) : la tâche de fond retente la ligne, une passe par tranche, les
// expressions déjà compilées restant en cache.
const COLD_ATTEMPTS = 3;
// Temps accordé à la dernière ligne pendant le flux : elle est recalculée à
// chaque fragment (au plus toutes les 16 ms). Si elle dépasse, elle reste en
// texte brut tant qu'elle s'allonge ; terminée, elle est retentée avec le
// budget ; restée la dernière, la tâche de fond la colore avec
// LINE_TIME_LIMIT une fois le texte stable depuis TAIL_SETTLE_MS.
const TAIL_TIME_LIMIT = 10;
const TAIL_SETTLE_MS = 300;
// Secours : la fenêtre du budget se ferme d'elle-même après ce délai si la
// minuterie de fin de tâche n'est pas servie (fenêtre masquée, minuterie
// perdue).
const BUDGET_WINDOW_MAX = 1000;

// Métadonnées d'un jeton de vscode-textmate (EncodedTokenMetadata) : style
// de police sur les bits 11 à 14, indice de couleur sur les bits 15 à 23.
const FONT_STYLE_MASK = 0x7800;
const FONT_STYLE_OFFSET = 11;
const FOREGROUND_MASK = 0xff8000;
const FOREGROUND_OFFSET = 15;

type RuleStack = ReturnType<Grammar["tokenizeLine2"]>["ruleStack"];

let budgetSpent = 0;
let budgetOpenedAt = 0;
let budgetTimer: ReturnType<typeof setTimeout> | undefined;

/** Ouvre au besoin la fenêtre de la tâche en cours ; elle se ferme à la tâche suivante. */
function taskBudget(): void {
  const now = performance.now();
  if (budgetTimer !== undefined && now - budgetOpenedAt < BUDGET_WINDOW_MAX) {
    return;
  }
  clearTimeout(budgetTimer);
  budgetSpent = 0;
  budgetOpenedAt = now;
  budgetTimer = setTimeout(() => {
    budgetTimer = undefined;
  }, 0);
}

/** Temps qui reste au premier plan pour la tâche en cours. */
function budgetLeft(): number {
  taskBudget();
  return TASK_BUDGET - budgetSpent;
}

/**
 * Après une tranche de la tâche de fond : le rendu qu'elle provoque (dans la
 * même tâche) ne tokenise rien de plus, la tranche reste courte.
 */
function exhaustBudget(): void {
  taskBudget();
  budgetSpent = Math.max(budgetSpent, TASK_BUDGET);
}

/**
 * Lignes terminées d'un bloc, déjà tokenisées. Les tableaux n'appartiennent
 * qu'à un tokenizer : l'avancement d'un autre bloc est repris par copie.
 */
interface Progress {
  owner: Highlighter;
  lang: string;
  /** Texte des lignes terminées, chacune suivie de son saut de ligne. */
  done: string;
  /** Fin de chaque ligne terminée dans done, saut de ligne compris. */
  ends: number[];
  lines: ThemedToken[][];
  /** État de la grammaire après chaque ligne terminée. */
  states: (RuleStack | null)[];
}

function stateAfter(p: Progress): RuleStack | null {
  return p.states.length > 0 ? p.states[p.states.length - 1] : null;
}

/**
 * Nombre de lignes terminées de p que text reprend à l'identique, sauts de
 * ligne compris. Si text prolonge p.done, toutes ; sinon le plus long préfixe
 * commun aligné sur une fin de ligne, par dichotomie (si text commence par
 * les k premières lignes, il commence aussi par les k - 1 premières).
 */
function sharedLines(p: Progress, text: string): number {
  if (text.startsWith(p.done)) return p.lines.length;
  let lo = 0;
  let hi = p.lines.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (text.startsWith(p.done.slice(0, p.ends[mid - 1]))) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

function truncate(p: Progress, count: number): void {
  p.done = p.done.slice(0, count > 0 ? p.ends[count - 1] : 0);
  p.ends.length = count;
  p.lines.length = count;
  p.states.length = count;
}

// Avancement des derniers blocs, un par tokenizer : un bloc remonté (en
// rouvrant une conversation, ou après un rendu de secours) reprend les lignes
// déjà faites au lieu de tout recalculer. Le plus récent en tête ; un
// avancement n'y entre qu'avec sa première ligne terminée (jamais vide), et
// un tokenizer qui reprend plus haut dans son bloc garde sa place.
const RECENT_MAX = 64;
const recent: Progress[] = [];

/** Met en tête l'avancement d'un bloc qui vient de terminer une ligne. */
function remember(p: Progress): void {
  const index = recent.indexOf(p);
  if (index === 0) return;
  if (index > 0) recent.splice(index, 1);
  recent.unshift(p);
  if (recent.length > RECENT_MAX) recent.length = RECENT_MAX;
}

/**
 * Nouvel avancement pour un bloc : copie des lignes communes de l'avancement
 * déjà calculé qui en partage le plus avec text. Une copie qui reprend tout
 * l'original prend sa place dans le cache (bloc remonté : l'original ne
 * servira plus), sans en chasser un autre.
 */
function resume(owner: Highlighter, lang: string, text: string): Progress {
  let best = -1;
  let count = 0;
  for (let i = 0; i < recent.length; i++) {
    const r = recent[i];
    if (r.owner !== owner || r.lang !== lang || r.lines.length <= count) {
      continue;
    }
    const shared = sharedLines(r, text);
    if (shared > count) {
      best = i;
      count = shared;
    }
  }
  if (best < 0) {
    return { owner, lang, done: "", ends: [], lines: [], states: [] };
  }
  const known = recent[best];
  const copy: Progress = {
    owner,
    lang,
    done: known.done.slice(0, known.ends[count - 1]),
    ends: known.ends.slice(0, count),
    lines: known.lines.slice(0, count),
    states: known.states.slice(0, count),
  };
  if (count === known.lines.length) recent[best] = copy;
  return copy;
}

/** Ligne qui commence à start et finit au saut de ligne nl (\n ou \r\n), comme splitLines de Shiki. */
function lineAt(text: string, start: number, nl: number): string {
  return text.slice(
    start,
    nl > start && text.charCodeAt(nl - 1) === 13 ? nl - 1 : nl,
  );
}

function plainLine(line: string, offset: number): ThemedToken[] {
  return line === ""
    ? []
    : [{ content: line, offset, color: "", fontStyle: 0 }];
}

/**
 * Une ligne, comme la boucle de Shiki (codeToTokensBase) mais en une seule
 * passe (Shiki 3.14 tokenise tout deux fois : les jetons, puis l'état).
 * Renvoie null si la ligne n'est pas finie dans le temps imparti. Le temps
 * passé est décompté du budget de la tâche.
 */
function tokenizeLine(
  grammar: Grammar,
  colorMap: string[],
  line: string,
  offset: number,
  state: RuleStack | null,
  timeLimit: number,
): { tokens: ThemedToken[]; state: RuleStack | null } | null {
  if (line === "") return { tokens: [], state };
  if (line.length >= MAX_LINE)
    return { tokens: plainLine(line, offset), state };
  const started = performance.now();
  let result: ReturnType<Grammar["tokenizeLine2"]>;
  try {
    result = grammar.tokenizeLine2(line, state, timeLimit);
  } finally {
    budgetSpent += performance.now() - started;
  }
  if (result.stoppedEarly) return null;
  const raw = result.tokens;
  const count = raw.length / 2;
  const tokens: ThemedToken[] = [];
  for (let j = 0; j < count; j++) {
    const start = raw[2 * j];
    const end = j + 1 < count ? raw[2 * j + 2] : line.length;
    if (start === end) continue;
    const metadata = raw[2 * j + 1];
    tokens.push({
      content: line.substring(start, end),
      offset: offset + start,
      color: colorMap[(metadata & FOREGROUND_MASK) >>> FOREGROUND_OFFSET],
      fontStyle: (metadata & FONT_STYLE_MASK) >>> FONT_STYLE_OFFSET,
    });
  }
  return { tokens, state: result.ruleStack };
}

/**
 * Tokenizer d'un bloc de code (un par bloc : useState(createBlockTokenizer)),
 * avec un abonnement pour useSyncExternalStore : getVersion change à chaque
 * progrès fait en tâche de fond.
 */
export interface BlockTokenizer {
  (h: Highlighter, text: string, lang: string): ThemedToken[][];
  /** onProgress est appelé à chaque ligne colorée en tâche de fond. */
  subscribe(onProgress: () => void): () => void;
  getVersion(): number;
}

/** Résultat d'une unité de travail de la tâche de fond. */
type Unit = "progress" | "retry" | "wait" | "idle";

/**
 * Tokenizer d'un bloc de code. Mêmes lignes, une fois tout coloré, que
 * h.core.codeToTokensBase(text, { lang, theme: "one-light",
 * tokenizeMaxLineLength: 500 }) (hors lignes trop lentes, laissées en texte
 * brut), couleurs de one-light ; couleur sombre =
 * h.lightToDark.get(token.color).
 *
 * Le texte d'un bloc se termine toujours par "\n" (ajouté par
 * mdast-util-to-hast, même pour une clôture pas encore fermée pendant le
 * flux) : ce "\n" final est mis de côté et la dernière ligne est donc
 * considérée comme pouvant encore s'allonger. Les lignes qui la précèdent
 * sont gardées avec l'état de la grammaire après chacune : une mise à jour ne
 * tokenise que les nouvelles lignes terminées et la dernière ligne, et un
 * texte modifié avant sa fin reprend au plus long préfixe commun.
 *
 * Un appel ne dépasse pas le budget de la tâche (TASK_BUDGET, partagé par
 * tous les blocs) : les lignes qui n'ont pas eu le temps sont rendues en
 * texte brut (color ""), puis colorées pendant les temps morts, sur le
 * dernier texte reçu, tant que quelqu'un est abonné ; à chaque ligne
 * colorée, la version change et les abonnés sont prévenus, et un nouvel
 * appel avec le même texte rend les lignes colorées.
 */
export function createBlockTokenizer(): BlockTokenizer {
  let p: Progress | null = null;
  // Dernière demande : la tâche de fond travaille sur ce texte.
  let owner: Highlighter | null = null;
  let lang = "";
  let code: string | null = null;
  let text = "";
  let finalNewline = false;
  // Premier calcul de ce bloc (ou nouveau langage) : sa dernière ligne n'est
  // pas en train de s'allonger, elle a le budget au lieu de TAIL_TIME_LIMIT.
  let fresh = false;
  let failure: { error: unknown } | null = null;

  // Dernier résultat rendu, pour un même appel.
  let result: ThemedToken[][] = [];
  let resultCode: string | null = null;
  let resultVersion = -1;

  // Dernière ligne colorée (elle peut encore s'allonger).
  let last: { start: number; line: string; tokens: ThemedToken[] } | null =
    null;
  // Début de la ligne qui n'a pas fini dans slowLimit ms au premier plan :
  // le premier plan ne la retente qu'avec plus de temps (pas à chaque
  // fragment du flux).
  let slowAt = -1;
  let slowLimit = 0;
  // Passes complètes ratées sur la prochaine ligne tant que le préchauffage
  // n'est pas fini.
  let coldTries = 0;

  let version = 0;
  const subscribers = new Set<() => void>();
  let running = false;
  // Le texte n'a pas changé depuis TAIL_SETTLE_MS.
  let settled = false;
  let settleTimer: ReturnType<typeof setTimeout> | undefined;

  function forgetLine(): void {
    last = null;
    slowAt = -1;
    slowLimit = 0;
    coldTries = 0;
  }

  function request(h: Highlighter, newCode: string, newLang: string): void {
    const final = /\r?\n$/.exec(newCode);
    const newText = final ? newCode.slice(0, final.index) : newCode;
    if (!p || p.owner !== h || p.lang !== newLang) {
      p = resume(h, newLang, newText);
      fresh = true;
      failure = null;
      forgetLine();
    } else {
      fresh = false;
      const count = sharedLines(p, newText);
      if (count < p.lines.length) {
        truncate(p, count);
        forgetLine();
      }
    }
    owner = h;
    lang = newLang;
    code = newCode;
    text = newText;
    finalNewline = !!final;
    settled = false;
    clearTimeout(settleTimer);
    settleTimer = undefined;
  }

  function grammarAndColors(h: Highlighter) {
    return {
      grammar: h.core.getLanguage(lang),
      colorMap: h.core.setTheme(THEME).colorMap,
    };
  }

  function pushLine(
    prog: Progress,
    tokens: ThemedToken[],
    state: RuleStack | null,
    end: number,
  ): void {
    prog.lines.push(tokens);
    prog.states.push(state);
    prog.ends.push(end);
    prog.done = text.slice(0, end);
    coldTries = 0;
    remember(prog);
  }

  /** La dernière ligne est vide ou déjà calculée pour le texte actuel. */
  function lastLineReady(prog: Progress): boolean {
    const start = prog.done.length;
    const line = text.slice(start);
    return line === "" || (last?.start === start && last.line === line);
  }

  function pending(): boolean {
    return (
      !failure &&
      !!p &&
      (text.indexOf("\n", p.done.length) >= 0 || !lastLineReady(p))
    );
  }

  /** Une passe complète ratée : true si la ligne doit être retentée. */
  function retryCold(): boolean {
    coldTries += 1;
    if (!warmedUp && coldTries < COLD_ATTEMPTS) return true;
    coldTries = 0;
    return false;
  }

  /** Premier plan : lignes en attente puis dernière ligne, dans le budget. */
  function foreground(h: Highlighter, prog: Progress): void {
    let tools: ReturnType<typeof grammarAndColors> | null = null;
    for (;;) {
      const start = prog.done.length;
      const nl = text.indexOf("\n", start);
      if (nl < 0 && lastLineReady(prog)) return;
      const limit = Math.floor(
        Math.min(
          nl >= 0 || fresh ? LINE_TIME_LIMIT : TAIL_TIME_LIMIT,
          budgetLeft(),
        ),
      );
      if (limit < 1 || (slowAt === start && limit <= slowLimit)) return;
      tools ??= grammarAndColors(h);
      const line = nl >= 0 ? lineAt(text, start, nl) : text.slice(start);
      const state = stateAfter(prog);
      const r = tokenizeLine(
        tools.grammar,
        tools.colorMap,
        line,
        start,
        state,
        limit,
      );
      if (!r) {
        // Pas fini dans ce temps : la tâche de fond la reprendra. Une ligne
        // terminée n'est essayée qu'une fois au premier plan ; la dernière
        // ligne, de nouveau quand elle a plus de temps (une fois terminée).
        slowAt = start;
        slowLimit = nl >= 0 ? Infinity : limit;
        return;
      }
      if (nl < 0) {
        last = { start, line, tokens: r.tokens };
        return;
      }
      pushLine(prog, r.tokens, r.state, nl + 1);
    }
  }

  /** Tâche de fond : une ligne terminée, ou la dernière ligne. */
  function backgroundUnit(h: Highlighter, prog: Progress): Unit {
    const start = prog.done.length;
    const nl = text.indexOf("\n", start);
    if (nl < 0 && lastLineReady(prog)) return "idle";
    const { grammar, colorMap } = grammarAndColors(h);
    const line = nl >= 0 ? lineAt(text, start, nl) : text.slice(start);
    const state = stateAfter(prog);
    if (nl >= 0) {
      const r = tokenizeLine(
        grammar,
        colorMap,
        line,
        start,
        state,
        LINE_TIME_LIMIT,
      );
      if (!r && retryCold()) return "retry";
      // Trop lente : texte brut, état de la grammaire inchangé.
      pushLine(
        prog,
        r ? r.tokens : plainLine(line, start),
        r?.state ?? state,
        nl + 1,
      );
      return "progress";
    }
    // Dernière ligne : d'abord la limite courte, si elle n'a pas déjà
    // échoué au premier plan.
    if (slowAt !== start || slowLimit < TAIL_TIME_LIMIT) {
      const r = tokenizeLine(
        grammar,
        colorMap,
        line,
        start,
        state,
        TAIL_TIME_LIMIT,
      );
      if (r) {
        last = { start, line, tokens: r.tokens };
        return "progress";
      }
      slowAt = start;
      slowLimit = TAIL_TIME_LIMIT;
    }
    if (!settled) {
      settleTimer ??= setTimeout(() => {
        settleTimer = undefined;
        settled = true;
        resumeWork();
      }, TAIL_SETTLE_MS);
      return "wait";
    }
    const r = tokenizeLine(
      grammar,
      colorMap,
      line,
      start,
      state,
      LINE_TIME_LIMIT,
    );
    if (!r && retryCold()) return "retry";
    last = { start, line, tokens: r ? r.tokens : plainLine(line, start) };
    coldTries = 0;
    return "progress";
  }

  function notify(): void {
    version += 1;
    subscribers.forEach((onProgress) => onProgress());
  }

  function step(): boolean {
    if (subscribers.size === 0 || !p || !owner || failure) {
      running = false; // abandonné ; un abonnement le relancera
      return false;
    }
    let unit: Unit;
    try {
      unit = backgroundUnit(owner, p);
    } catch (error) {
      // Rendu suivant : l'appel relance l'erreur (le bloc reste en texte brut).
      failure = { error };
      unit = "progress";
    } finally {
      exhaustBudget();
    }
    if (unit === "progress") notify();
    if (!failure && (unit === "progress" || unit === "retry")) return true;
    running = false;
    return false;
  }

  function resumeWork(): void {
    if (running || subscribers.size === 0 || !pending()) return;
    running = true;
    void runInSlices(step);
  }

  function build(prog: Progress): ThemedToken[][] {
    const out = prog.lines.slice();
    let start = prog.done.length;
    for (let nl = text.indexOf("\n", start); nl >= 0;) {
      out.push(plainLine(lineAt(text, start, nl), start));
      start = nl + 1;
      nl = text.indexOf("\n", start);
    }
    const line = text.slice(start);
    out.push(
      last?.start === start && last.line === line && start === prog.done.length
        ? last.tokens
        : plainLine(line, start),
    );
    if (finalNewline) out.push([]);
    return out;
  }

  const tokenize = ((h: Highlighter, newCode: string, newLang: string) => {
    if (
      h === owner &&
      newLang === lang &&
      newCode === resultCode &&
      version === resultVersion
    ) {
      return result;
    }
    if (h !== owner || newLang !== lang || newCode !== code) {
      request(h, newCode, newLang);
    }
    if (failure) throw failure.error;
    const prog = p as Progress;
    try {
      foreground(h, prog);
    } catch (error) {
      failure = { error };
      throw error;
    }
    result = build(prog);
    resultCode = newCode;
    resultVersion = version;
    resumeWork();
    return result;
  }) as BlockTokenizer;

  tokenize.subscribe = (onProgress) => {
    subscribers.add(onProgress);
    resumeWork();
    return () => {
      subscribers.delete(onProgress);
    };
  };
  tokenize.getVersion = () => version;
  return tokenize;
}
