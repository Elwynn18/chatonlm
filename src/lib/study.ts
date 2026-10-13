// Onglet Study (propre à ChatonLM) : fonctions pures du lecteur de cours,
// testées à part. Le moteur calcule les statuts (paliers, cours) ; la page
// choisit et interprète seule la « position » (clé de la carte montrée),
// enregistrée par study_progress :
//
// - `intro` : objectifs et plan, avant le palier 0 ;
// - `L{i}:s{k}` (section k de la leçon), `L{i}:example`, `L{i}:recap` (à
//   retenir et piège), `L{i}:check` (entraînement), puis pour chaque essai j
//   `L{i}:a{j}:explain` (j ≥ 1), `L{i}:a{j}:q{n}` et `L{i}:a{j}:result` ;
//   un palier pas encore prêt n'a qu'une carte d'attente, à la clé
//   `L{i}:s0` (la même que sa première carte une fois prêt) ;
// - `final:q{n}` (bilan ; attente à `final:q0`), puis `end`.

import type { Answered } from "@/bindings/Answered";
import type { Course } from "@/bindings/Course";
import type { Level } from "@/bindings/Level";
import type { ModelInfo } from "@/bindings/ModelInfo";
import type { Question } from "@/bindings/Question";
import type { StudyLevel } from "@/bindings/StudyLevel";

/** Longueur maximale d'un sujet (MAX_TOPIC_CHARS du moteur). */
export const MAX_TOPIC_CHARS = 300;
/** Documents par cours au plus (MAX_DOCUMENTS du moteur). */
export const MAX_DOCUMENTS = 3;
/** Longueur maximale d'une question posée (MAX_QUESTION_CHARS du moteur). */
export const MAX_QUESTION_CHARS = 1000;

export const STUDY_LEVELS: readonly StudyLevel[] = [
  "beginner",
  "intermediate",
  "advanced",
];
export const STUDY_MINUTES = [10, 20, 30] as const;
export type StudyMinutes = (typeof STUDY_MINUTES)[number];
export const DEFAULT_MINUTES: StudyMinutes = 20;

/** Durée estimée d'un palier pas encore écrit (comme le moteur). */
export const UNGENERATED_LEVEL_SECONDS = 240;
/** Durée estimée d'une question du bilan pas encore écrite (comme le moteur). */
export const UNGENERATED_FINAL_QUESTION_SECONDS = 25;
/** Cartes comptées pour un palier pas encore écrit (barre de progression). */
export const UNGENERATED_LEVEL_CARDS = 9;

/**
 * Modèle jugé petit en dessous de ce nombre de paramètres (en milliards) :
 * avertissement discret dans l'en-tête. Banc du contrat 47 (§ 6) : sous
 * ~3,5 milliards (llama3.2:3b, qwen3:1.7b, qwen3.5:2b), la plupart des
 * questions ont une solution que le modèle lui-même contredit ; dès 4
 * milliards (gemma3:4b, qwen3:4b, ministral-3:3b à 3,8), plans et leçons
 * tiennent.
 */
export const SMALL_MODEL_BILLIONS = 3.5;

/** Paramètres d'adresse de /study (préremplissage, « Go further »). */
export interface StudySearch {
  topic?: string;
  level?: StudyLevel;
  minutes?: StudyMinutes;
}

function paramText(value: unknown): string | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  if (typeof value !== "string") return undefined;
  const text = value.trim();
  return text === "" ? undefined : text;
}

/** Paramètres de /study validés ; une valeur inconnue est ignorée. */
export function validateStudySearch(raw: Record<string, unknown>): StudySearch {
  const search: StudySearch = {};
  const topic = paramText(raw.topic);
  if (topic) search.topic = [...topic].slice(0, MAX_TOPIC_CHARS).join("");
  const level = paramText(raw.level);
  if (level && (STUDY_LEVELS as readonly string[]).includes(level)) {
    search.level = level as StudyLevel;
  }
  const minutes = Number(paramText(raw.minutes));
  if ((STUDY_MINUTES as readonly number[]).includes(minutes)) {
    search.minutes = minutes as StudyMinutes;
  }
  return search;
}

/** Nombre de paliers d'une durée (comme le moteur : 2, 4 ou 6). */
export function levelCount(minutes: number): number {
  return minutes <= 10 ? 2 : minutes <= 20 ? 4 : 6;
}

/** Nombre de questions du bilan d'une durée (3, 5 ou 6). */
export function finalCount(minutes: number): number {
  return minutes <= 10 ? 3 : minutes <= 20 ? 5 : 6;
}

/** Carte du lecteur. */
export type Card =
  | { key: "intro"; kind: "intro" }
  | { key: string; kind: "section"; level: number; section: number }
  | { key: string; kind: "example"; level: number }
  | { key: string; kind: "recap"; level: number }
  | { key: string; kind: "check"; level: number; question: Question }
  | { key: string; kind: "explain"; level: number; attempt: number }
  | {
      key: string;
      kind: "question";
      level: number;
      attempt: number;
      index: number;
      total: number;
      question: Question;
    }
  | { key: string; kind: "result"; level: number; attempt: number }
  /** Palier pas encore prêt (en attente, en cours d'écriture ou en échec). */
  | { key: string; kind: "levelWait"; level: number }
  | {
      key: string;
      kind: "final";
      index: number;
      total: number;
      question: Question;
    }
  /** Bilan pas encore prêt. */
  | { key: string; kind: "finalWait" }
  | { key: "end"; kind: "end" };

export const INTRO = "intro";
export const END = "end";

export const sectionKey = (level: number, section: number) =>
  `L${level}:s${section}`;

/**
 * Clés des textes d'un cours lus à voix haute (le moteur relit le texte en
 * base : src-tauri/src/study/passage.rs) : section d'une leçon (la clé de sa
 * carte : titre, accroche du palier en tête de la première, texte), énoncé
 * et solution de l'exemple, « à retenir » et piège, explication autrement
 * d'un essai, explication plus simple et réponse du tuteur (par leur rang
 * dans le palier).
 */
export const passageKey = {
  section: sectionKey,
  example: (level: number) => `L${level}:example`,
  solution: (level: number) => `L${level}:solution`,
  recap: (level: number) => `L${level}:recap`,
  remedy: (level: number, attempt: number) => `L${level}:a${attempt}:explain`,
  explain: (level: number, index: number) => `L${level}:explain${index}`,
  ask: (level: number, index: number) => `L${level}:ask${index}`,
};
export const levelStartKey = (level: number) => sectionKey(level, 0);
export const finalKey = (index: number) => `final:q${index}`;

/** Le palier est-il fini (réussi, ou passé par « Continue anyway ») ? */
export function levelDone(level: Level): boolean {
  return level.status === "passed" || level.status === "review";
}

/** Cartes d'un palier prêt, dans l'ordre. */
function levelCards(level: Level): Card[] {
  const i = level.index;
  const lesson = level.lesson;
  if (level.state !== "ready" || !lesson) {
    return [{ key: levelStartKey(i), kind: "levelWait", level: i }];
  }
  const cards: Card[] = lesson.sections.map((_, k) => ({
    key: sectionKey(i, k),
    kind: "section",
    level: i,
    section: k,
  }));
  cards.push({ key: `L${i}:example`, kind: "example", level: i });
  cards.push({ key: `L${i}:recap`, kind: "recap", level: i });
  if (level.check) {
    cards.push({
      key: `L${i}:check`,
      kind: "check",
      level: i,
      question: level.check,
    });
  }
  level.attempts.forEach((attempt, j) => {
    if (j > 0 && attempt.explanation !== null) {
      cards.push({
        key: `L${i}:a${j}:explain`,
        kind: "explain",
        level: i,
        attempt: j,
      });
    }
    attempt.questions.forEach((question, n) => {
      cards.push({
        key: `L${i}:a${j}:q${n}`,
        kind: "question",
        level: i,
        attempt: j,
        index: n,
        total: attempt.questions.length,
        question,
      });
    });
    cards.push({
      key: `L${i}:a${j}:result`,
      kind: "result",
      level: i,
      attempt: j,
    });
  });
  return cards;
}

/** Toutes les cartes du cours, dans l'ordre (attentes comprises). */
export function cardsOf(course: Course): Card[] {
  const cards: Card[] = [{ key: INTRO, kind: "intro" }];
  for (const level of course.levels) cards.push(...levelCards(level));
  const quiz = course.finalQuiz;
  if (quiz && quiz.state === "ready" && quiz.questions.length > 0) {
    quiz.questions.forEach((question, n) => {
      cards.push({
        key: finalKey(n),
        kind: "final",
        index: n,
        total: quiz.questions.length,
        question,
      });
    });
  } else {
    cards.push({ key: finalKey(0), kind: "finalWait" });
  }
  cards.push({ key: END, kind: "end" });
  return cards;
}

/** Les paliers sont-ils tous finis (bilan ouvert) ? */
export function allLevelsDone(course: Course): boolean {
  return course.levels.length > 0 && course.levels.every(levelDone);
}

/** Palier d'une carte, ou null (intro, bilan, fin). */
export function levelOfCard(card: Card): number | null {
  return "level" in card ? card.level : null;
}

/**
 * Carte accessible : palier non verrouillé ; bilan une fois les paliers
 * finis ; fin une fois le cours fini.
 */
export function reachable(course: Course, card: Card): boolean {
  const level = levelOfCard(card);
  if (level !== null) {
    return course.levels[level]?.status !== "locked";
  }
  if (card.kind === "final" || card.kind === "finalWait") {
    return allLevelsDone(course);
  }
  if (card.kind === "end") return course.status === "finished";
  return true;
}

/** Réponse enregistrée d'une question, ou null. */
export function answerOf(course: Course, question: Question): Answered | null {
  return course.answers[question.id] ?? null;
}

/** Bonnes réponses d'un essai, et s'il est complet. */
export function attemptScore(course: Course, level: number, attempt: number) {
  const questions = course.levels[level]?.attempts[attempt]?.questions ?? [];
  const answers = questions.map((question) => answerOf(course, question));
  return {
    correct: answers.filter((answer) => answer?.correct === true).length,
    total: questions.length,
    complete: answers.every((answer) => answer !== null),
  };
}

/** Meilleur score des essais complets d'un palier ; null sans essai complet. */
export function bestScore(
  course: Course,
  level: number,
): { correct: number; total: number } | null {
  let best: { correct: number; total: number } | null = null;
  const attempts = course.levels[level]?.attempts ?? [];
  attempts.forEach((_, j) => {
    const score = attemptScore(course, level, j);
    if (!score.complete) return;
    if (!best || score.correct / score.total > best.correct / best.total) {
      best = { correct: score.correct, total: score.total };
    }
  });
  return best;
}

/** Bonnes réponses du bilan. */
export function finalScore(course: Course) {
  const questions = course.finalQuiz?.questions ?? [];
  const answers = questions.map((question) => answerOf(course, question));
  return {
    correct: answers.filter((answer) => answer?.correct === true).length,
    total: questions.length,
    answered: answers.filter((answer) => answer !== null).length,
  };
}

/**
 * Carte qui suit `key` pour « Continue » ; null s'il n'y en a pas : question
 * sans réponse, essai manqué sans nouvel essai (« Explain differently » ou
 * « Continue anyway » d'abord), carte suivante verrouillée, fin du cours.
 */
export function nextCard(course: Course, key: string): Card | null {
  const cards = cardsOf(course);
  const index = cards.findIndex((card) => card.key === key);
  if (index === -1 || index === cards.length - 1) return null;
  const card = cards[index];
  if (
    (card.kind === "question" ||
      card.kind === "check" ||
      card.kind === "final") &&
    !answerOf(course, card.question)
  ) {
    return null;
  }
  if (card.kind === "levelWait" || card.kind === "finalWait") return null;
  const next = cards[index + 1];
  return reachable(course, next) ? next : null;
}

/**
 * Carte montrée pour une position enregistrée : la sienne si elle existe et
 * est accessible, sinon l'introduction (la fin pour un cours fini).
 */
export function resolvePosition(course: Course, position: string): string {
  const cards = cardsOf(course);
  const card = cards.find((c) => c.key === position);
  if (card && reachable(course, card)) return card.key;
  // Carte disparue (palier refait, essai effacé) : début de son palier.
  const level = /^L(\d+):/.exec(position);
  const start = level
    ? cards.find((c) => c.key === levelStartKey(Number(level[1])))
    : undefined;
  if (start && reachable(course, start)) return start.key;
  return course.status === "finished" ? END : INTRO;
}

/** Carte d'une clé (null : clé inconnue du cours tel qu'il est). */
export function cardOf(course: Course, key: string): Card | null {
  return cardsOf(course).find((card) => card.key === key) ?? null;
}

/** Cartes comptées pour un palier (premier essai seulement). */
function nominalCards(level: Level): number {
  if (level.state !== "ready" || !level.lesson) return UNGENERATED_LEVEL_CARDS;
  const first = level.attempts[0]?.questions.length ?? 0;
  return level.lesson.sections.length + 2 + (level.check ? 1 : 0) + first + 1;
}

/** Rang d'une clé dans les cartes comptées de son palier. */
function rankInLevel(level: Level, key: string): number {
  const lesson = level.lesson;
  const nominal = nominalCards(level);
  if (!lesson) return 0;
  const sections = lesson.sections.length;
  const rest = key.slice(`L${level.index}:`.length);
  const section = /^s(\d+)$/.exec(rest);
  if (section) return Math.min(Number(section[1]), sections - 1);
  if (rest === "example") return sections;
  if (rest === "recap") return sections + 1;
  const check = level.check ? 1 : 0;
  if (rest === "check") return sections + 2;
  const question = /^a0:q(\d+)$/.exec(rest);
  if (question) return sections + 2 + check + Number(question[1]);
  // Résultat du premier essai, ou essais suivants : la dernière carte.
  return nominal - 1;
}

/**
 * Avancement du cours (barre de progression) : cartes faites sur cartes
 * comptées. Un palier fini compte toutes ses cartes ; le palier en cours,
 * celles avant la carte montrée (si elle est à lui) ; le bilan, ses
 * réponses. Un palier pas encore écrit compte pour UNGENERATED_LEVEL_CARDS.
 */
export function courseProgress(
  course: Course,
  key: string,
): { done: number; total: number } {
  let total = 0;
  let done = 0;
  for (const level of course.levels) {
    const nominal = nominalCards(level);
    total += nominal;
    if (levelDone(level)) done += nominal;
    else if (level.status === "current" && key.startsWith(`L${level.index}:`)) {
      done += Math.min(rankInLevel(level, key), nominal);
    }
  }
  const quiz = course.finalQuiz;
  const finalTotal =
    quiz && quiz.state === "ready" && quiz.questions.length > 0
      ? quiz.questions.length
      : finalCount(course.minutes);
  total += finalTotal;
  done += Math.min(finalScore(course).answered, finalTotal);
  if (course.status === "finished") done = total;
  return { done: Math.min(done, total), total };
}

/**
 * Temps restant estimé, en secondes : paliers non finis (durée du moteur,
 * UNGENERATED_LEVEL_SECONDS s'ils ne sont pas écrits) et bilan s'il reste
 * des réponses à donner ; 0 pour un cours fini.
 */
export function secondsLeft(course: Course): number {
  if (course.status === "finished") return 0;
  let seconds = 0;
  for (const level of course.levels) {
    if (levelDone(level)) continue;
    seconds +=
      level.state === "ready" && level.estimatedSeconds > 0
        ? level.estimatedSeconds
        : UNGENERATED_LEVEL_SECONDS;
  }
  const quiz = course.finalQuiz;
  if (quiz && quiz.state === "ready" && quiz.questions.length > 0) {
    if (finalScore(course).answered < quiz.questions.length) {
      seconds += quiz.estimatedSeconds;
    }
  } else {
    seconds += finalCount(course.minutes) * UNGENERATED_FINAL_QUESTION_SECONDS;
  }
  return seconds;
}

/** Partie d'un cours écrite par le modèle (génération en arrière-plan). */
export type CoursePart =
  | { kind: "plan" }
  /** Palier : sa leçon d'abord, puis ses questions (`stage`). */
  | { kind: "level"; level: number; stage: "lesson" | "questions" }
  /** Nouvel essai demandé (« Explain differently »). */
  | { kind: "remedy"; level: number }
  | { kind: "final" };

/** Où en est l'écriture d'un cours. */
export interface Generation {
  /**
   * Partie en cours d'écriture : celle que le moteur dit « generating »,
   * sinon la première qui attend (elle part aussitôt la précédente écrite) ;
   * null si rien ne s'écrit (tout est écrit, pause, échec, ou tour d'un
   * autre cours : `waiting`).
   */
  writing: CoursePart | null;
  /** Des parties restent à écrire, mais la file écrit un autre cours. */
  waiting: boolean;
  /** Des parties restent à écrire (en cours, en file, ou retenues par la pause). */
  pending: boolean;
  /**
   * La pause retient des parties à écrire (un cours en pause dont tout est
   * écrit, ou qu'un échec arrête, n'a rien à reprendre).
   */
  paused: boolean;
  /** Première partie en échec (elle attend « Retry »). */
  failed: CoursePart | null;
  /** Paliers écrits, sur ceux du plan (0 avant le plan). */
  ready: number;
  total: number;
}

const levelPart = (level: Level): CoursePart => ({
  kind: "level",
  level: level.index,
  stage: level.lesson ? "questions" : "lesson",
});

/**
 * Où en est l'écriture d'un cours, d'après ce que le moteur en dit (états
 * des parties, pause, file occupée par un autre cours). Comme la file du
 * moteur : le plan ; avant « Start », le premier palier seulement ; ensuite
 * les nouveaux essais demandés, les paliers dans l'ordre, puis le bilan (une
 * fois tous les paliers écrits : un palier en échec le retient).
 */
export function generationOf(course: Course): Generation {
  const levels = course.levels;
  const quiz = course.finalQuiz;
  // Parties que la file écrit pour ce cours, dans son ordre, et leur état.
  const parts: [CoursePart, Level["state"]][] = [];
  if (course.status === "planning") {
    // Le moteur ne dit pas si le plan attend son tour ou s'écrit.
    parts.push([{ kind: "plan" }, "pending"]);
  } else if (course.status === "planned") {
    if (levels.length > 0) parts.push([levelPart(levels[0]), levels[0].state]);
  } else if (course.status === "active" || course.status === "finished") {
    for (const level of levels) {
      if (level.remedy) {
        parts.push([{ kind: "remedy", level: level.index }, level.remedy]);
      }
    }
    for (const level of levels) parts.push([levelPart(level), level.state]);
    const held =
      quiz?.state === "pending" &&
      levels.some((level) => level.state === "failed");
    if (quiz && !held) parts.push([{ kind: "final" }, quiz.state]);
  }
  const first = (state: Level["state"]) =>
    parts.find(([, partState]) => partState === state)?.[0] ?? null;
  const generating = first("generating");
  const queued = first("pending");
  const pending = generating !== null || queued !== null;
  const active = pending && !course.paused;
  const waiting = active && course.queueBusy && generating === null;
  return {
    writing: active && !waiting ? (generating ?? queued) : null,
    waiting,
    pending,
    paused: pending && course.paused,
    failed: first("failed"),
    ready: levels.filter((level) => level.state === "ready").length,
    total: course.plan?.levels.length ?? 0,
  };
}

/** Minutes arrondies, une au moins (« About 3 min left »). */
export function roundMinutes(seconds: number): number {
  return Math.max(1, Math.round(seconds / 60));
}

/** Question manquée (fin du cours) : essais de validation et bilan. */
export interface MissedQuestion {
  question: Question;
  answer: Answered;
  /** Palier (null : bilan). */
  level: number | null;
}

/** Questions manquées des essais de validation et du bilan, dans l'ordre. */
export function missedQuestions(course: Course): MissedQuestion[] {
  const missed: MissedQuestion[] = [];
  for (const level of course.levels) {
    for (const attempt of level.attempts) {
      for (const question of attempt.questions) {
        const answer = answerOf(course, question);
        if (answer && !answer.correct) {
          missed.push({ question, answer, level: level.index });
        }
      }
    }
  }
  for (const question of course.finalQuiz?.questions ?? []) {
    const answer = answerOf(course, question);
    if (answer && !answer.correct)
      missed.push({ question, answer, level: null });
  }
  return missed;
}

/**
 * Nombre de paramètres en milliards d'une taille d'Ollama (« 751.63M »,
 * « 8.0B », « 1.5T ») ; null si illisible.
 */
export function parameterBillions(size: string): number | null {
  const match = /^\s*([\d.]+)\s*([KMBT])\s*$/i.exec(size);
  if (!match) return null;
  const value = Number(match[1]);
  if (!Number.isFinite(value)) return null;
  const scale = { K: 1e-6, M: 1e-3, B: 1, T: 1e3 }[
    match[2].toUpperCase() as "K" | "M" | "B" | "T"
  ];
  return value * scale;
}

/** Le modèle est-il petit (avertissement) ? Faux si sa taille est inconnue. */
export function isSmallModel(model: ModelInfo | undefined): boolean {
  if (!model) return false;
  const billions = parameterBillions(model.parameterSize);
  return billions !== null && billions < SMALL_MODEL_BILLIONS;
}

/**
 * Segments d'un texte Markdown d'une ligne : formules (`\(…\)`, `\[…\]`,
 * `$$…$$`, `$…$`), code en ligne (accents graves) et texte autour. Un
 * segment ouvert jusqu'à la fin du texte a `closed: false`. Dans une
 * formule, `\x` est une commande (jamais un fermant) ; dans du code, la
 * barre oblique est un caractère comme un autre.
 */
interface Segment {
  kind: "text" | "math" | "code";
  /** Contenu, sans les délimiteurs. */
  body: string;
  /** Délimiteur ouvrant (vide pour du texte). */
  open: string;
  closed: boolean;
}

/**
 * `singleDollar` : `$…$` est une formule (sinon du texte, comme dans le
 * rendu, réglé `singleDollarTextMath: false` comme l'officiel).
 */
function segmentsOf(text: string, singleDollar: boolean): Segment[] {
  const segments: Segment[] = [];
  let plain = "";
  let i = 0;
  const flush = () => {
    if (plain) {
      segments.push({ kind: "text", body: plain, open: "", closed: true });
    }
    plain = "";
  };
  while (i < text.length) {
    const c = text[i];
    let open: string | null = null;
    let close = "";
    let kind: "math" | "code" = "math";
    if (c === "\\" && (text[i + 1] === "(" || text[i + 1] === "[")) {
      open = text.slice(i, i + 2);
      close = text[i + 1] === "(" ? "\\)" : "\\]";
    } else if (c === "\\") {
      // Caractère échappé (« \$ », « \* ») : du texte.
      plain += text.slice(i, i + 2);
      i += 2;
      continue;
    } else if (c === "$" && (singleDollar || text[i + 1] === "$")) {
      open = text[i + 1] === "$" ? "$$" : "$";
      close = open;
    } else if (c === "`") {
      let n = 1;
      while (text[i + n] === "`") n += 1;
      open = "`".repeat(n);
      close = open;
      kind = "code";
    }
    if (open === null) {
      plain += c;
      i += 1;
      continue;
    }
    flush();
    let j = i + open.length;
    let end = -1;
    while (j < text.length) {
      if (text.startsWith(close, j)) {
        end = j;
        break;
      }
      j += kind === "math" && text[j] === "\\" ? 2 : 1;
    }
    if (end === -1) {
      segments.push({
        kind,
        body: text.slice(i + open.length),
        open,
        closed: false,
      });
      return segments;
    }
    segments.push({
      kind,
      body: text.slice(i + open.length, end),
      open,
      closed: true,
    });
    i = end + close.length;
  }
  flush();
  return segments;
}

/**
 * Prompt d'une question à trou coupé à son trou (`___`), si chaque moitié
 * peut être rendue seule : trou hors d'une formule (`\(…\)`, `\[…\]`,
 * `$$…$$`, `$…$`) et d'un code en ligne, gras et italique équilibrés de
 * chaque côté ; null sinon (l'énoncé est alors montré en entier, le champ
 * dessous).
 */
export function splitBlank(prompt: string): [string, string] | null {
  const at = prompt.indexOf("___");
  if (at === -1) return null;
  let end = at;
  while (prompt[end] === "_") end += 1;
  const before = prompt.slice(0, at);
  const after = prompt.slice(end);
  // Trou dans une formule ou un code : la moitié d'avant le laisse ouvert.
  if (segmentsOf(before, true).some((segment) => !segment.closed)) return null;
  const balanced = (text: string) => {
    const bold = text.match(/\*\*/g)?.length ?? 0;
    const single = text.replace(/\*\*/g, "").match(/\*/g)?.length ?? 0;
    return bold % 2 === 0 && single % 2 === 0;
  };
  return balanced(before) && balanced(after) ? [before, after] : null;
}

/** Délimiteur fermant d'un segment. */
const CLOSING: Record<string, string> = { "\\(": "\\)", "\\[": "\\]" };

/**
 * Énoncé d'une question tel qu'il est montré en entier (titre de la carte,
 * questions manquées) : un trou dans une formule (`\frac{___}{4}`), que KaTeX
 * refuserait (`_` y est un indice), devient un blanc souligné.
 */
export function questionPrompt(question: Question): string {
  const body = question.body;
  if (body.type === "trueFalse") return body.statement;
  if (body.type !== "blank") return body.prompt;
  return segmentsOf(body.prompt, false)
    .map((segment) => {
      if (segment.kind === "text") return segment.body;
      if (!segment.closed) return segment.open + segment.body;
      const close = CLOSING[segment.open] ?? segment.open;
      const inner =
        segment.kind === "math"
          ? segment.body.replace(/_{3,}/g, String.raw`\underline{\qquad}`)
          : segment.body;
      return segment.open + inner + close;
    })
    .join("");
}

/** Commandes LaTeX lues comme un symbole. */
const LATEX_SYMBOLS: Record<string, string> = {
  times: "×",
  cdot: "·",
  div: "÷",
  pm: "±",
  mp: "∓",
  le: "≤",
  leq: "≤",
  ge: "≥",
  geq: "≥",
  ne: "≠",
  neq: "≠",
  approx: "≈",
  equiv: "≡",
  sim: "∼",
  propto: "∝",
  to: "→",
  rightarrow: "→",
  leftarrow: "←",
  Rightarrow: "⇒",
  Leftrightarrow: "⇔",
  leftrightarrow: "↔",
  infty: "∞",
  circ: "°",
  degree: "°",
  sum: "∑",
  prod: "∏",
  int: "∫",
  partial: "∂",
  nabla: "∇",
  in: "∈",
  notin: "∉",
  subset: "⊂",
  cup: "∪",
  cap: "∩",
  emptyset: "∅",
  forall: "∀",
  exists: "∃",
  neg: "¬",
  ldots: "…",
  cdots: "⋯",
  dots: "…",
  percent: "%",
  alpha: "α",
  beta: "β",
  gamma: "γ",
  delta: "δ",
  epsilon: "ε",
  varepsilon: "ε",
  theta: "θ",
  lambda: "λ",
  mu: "μ",
  pi: "π",
  rho: "ρ",
  sigma: "σ",
  tau: "τ",
  phi: "φ",
  varphi: "φ",
  omega: "ω",
  Gamma: "Γ",
  Delta: "Δ",
  Theta: "Θ",
  Lambda: "Λ",
  Pi: "Π",
  Sigma: "Σ",
  Phi: "Φ",
  Omega: "Ω",
};
/** Commandes dont seul l'argument se lit (texte, police). */
const LATEX_WRAPPERS = new Set([
  "text",
  "textrm",
  "textbf",
  "textit",
  "mathrm",
  "mathbf",
  "mathit",
  "mathsf",
  "mathtt",
  "mathcal",
  "mathbb",
  "operatorname",
  "boldsymbol",
  "displaystyle",
  "left",
  "right",
]);
/** Espaces de LaTeX. */
const LATEX_SPACES = new Set([",", ";", ":", "!", " ", "quad", "qquad"]);
const SUPERSCRIPT: Record<string, string> = {
  "0": "⁰",
  "1": "¹",
  "2": "²",
  "3": "³",
  "4": "⁴",
  "5": "⁵",
  "6": "⁶",
  "7": "⁷",
  "8": "⁸",
  "9": "⁹",
  "+": "⁺",
  "-": "⁻",
  "=": "⁼",
  "(": "⁽",
  ")": "⁾",
  n: "ⁿ",
  i: "ⁱ",
};
const SUBSCRIPT: Record<string, string> = {
  "0": "₀",
  "1": "₁",
  "2": "₂",
  "3": "₃",
  "4": "₄",
  "5": "₅",
  "6": "₆",
  "7": "₇",
  "8": "₈",
  "9": "₉",
  "+": "₊",
  "-": "₋",
  "=": "₌",
  "(": "₍",
  ")": "₎",
  a: "ₐ",
  e: "ₑ",
  o: "ₒ",
  x: "ₓ",
  h: "ₕ",
  k: "ₖ",
  l: "ₗ",
  m: "ₘ",
  n: "ₙ",
  p: "ₚ",
  s: "ₛ",
  t: "ₜ",
};

/** Formule LaTeX lue en texte (« \frac{1}{2} » → « 1/2 », « CO_2 » → « CO₂ »). */
function latexText(latex: string): string {
  let i = 0;
  /** Argument suivant : groupe entre accolades, ou un caractère. */
  const argument = (): string => {
    while (latex[i] === " ") i += 1;
    if (latex[i] === "{") {
      let depth = 0;
      const start = i + 1;
      for (; i < latex.length; i += 1) {
        if (latex[i] === "{") depth += 1;
        else if (latex[i] === "}" && --depth === 0) break;
        else if (latex[i] === "\\") i += 1;
      }
      const inner = latex.slice(start, i);
      i += 1;
      return latexText(inner);
    }
    if (latex[i] === "\\") return command();
    const char = latex[i] ?? "";
    i += 1;
    return char;
  };
  /** Commande à `i` (barre oblique comprise). */
  const command = (): string => {
    i += 1;
    const name = /^[A-Za-z]+/.exec(latex.slice(i))?.[0] ?? latex[i] ?? "";
    i += Math.max(name.length, 1);
    if (name === "frac" || name === "dfrac" || name === "tfrac") {
      const top = argument();
      const bottom = argument();
      const wrap = (part: string) =>
        /^(?:\d+(?:[.,]\d+)?|\p{L})$/u.test(part) ? part : `(${part})`;
      return `${wrap(top)}/${wrap(bottom)}`;
    }
    if (name === "sqrt") {
      if (latex[i] === "[") {
        const close = latex.indexOf("]", i);
        i = close === -1 ? latex.length : close + 1;
      }
      const inner = argument();
      return inner.length > 1 ? `√(${inner})` : `√${inner}`;
    }
    if (LATEX_WRAPPERS.has(name))
      return name === "left" || name === "right" ? "" : argument();
    if (LATEX_SPACES.has(name)) return " ";
    if (name === "\\") return " ";
    if ("{}$%&#_".includes(name)) return name;
    return LATEX_SYMBOLS[name] ?? name;
  };
  /** Exposant ou indice : caractères Unicode s'ils existent tous. */
  const script = (table: Record<string, string>, mark: string): string => {
    const value = argument();
    const chars = [...value];
    if (value === "°") return "°";
    return chars.length > 0 && chars.every((char) => char in table)
      ? chars.map((char) => table[char]).join("")
      : `${mark}(${value})`;
  };
  let out = "";
  while (i < latex.length) {
    const c = latex[i];
    if (c === "\\") out += command();
    else if (c === "^") {
      i += 1;
      out += script(SUPERSCRIPT, "^");
    } else if (c === "_") {
      i += 1;
      out += script(SUBSCRIPT, "_");
    } else if (c === "{" || c === "}") i += 1;
    else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/** Marques Markdown d'un texte d'une ligne retirées (gras, italique, liens). */
function markdownText(text: string): string {
  return text
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__|~~)(.+?)\1/g, "$2")
    .replace(/\*(\S(?:.*?\S)?)\*/g, "$1")
    .replace(/(^|[^\p{L}\p{N}])_(\S(?:.*?\S)?)_(?![\p{L}\p{N}])/gu, "$1$2")
    .replace(/\\([\\`*_{}[\]()#+\-.!$~|])/g, "$1");
}

/**
 * Texte d'un Markdown d'une ligne là où le Markdown ne peut pas être rendu
 * (option d'une liste déroulante native, nom accessible) : marques retirées,
 * code en ligne gardé tel quel, formules lues en texte (« \(CO_2\) » →
 * « CO₂ », « \(\frac{1}{2}\) » → « 1/2 »).
 */
export function plainInline(markdown: string): string {
  return segmentsOf(markdown, false)
    .map((segment) =>
      !segment.closed
        ? markdownText(segment.open + segment.body)
        : segment.kind === "math"
          ? latexText(segment.body)
          : segment.kind === "code"
            ? segment.body
            : markdownText(segment.body),
    )
    .join("")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Temps passé, compté seulement quand la page est vue et la fenêtre active
 * (`setActive`) : `base` (temps déjà enregistré) plus les périodes actives.
 */
export class StudyClock {
  private total: number;
  private since: number | null = null;

  constructor(
    base: number,
    private readonly now: () => number = () => performance.now(),
  ) {
    this.total = Math.max(0, base);
  }

  setActive(active: boolean) {
    if (active && this.since === null) {
      this.since = this.now();
    } else if (!active && this.since !== null) {
      this.total += this.now() - this.since;
      this.since = null;
    }
  }

  /** Temps passé en millisecondes (entier). */
  elapsed(): number {
    const running = this.since === null ? 0 : this.now() - this.since;
    return Math.round(this.total + running);
  }
}

/**
 * Étoiles d'un palier fini : 3 = premier essai sans faute, 2 = premier essai
 * réussi avec des fautes, 1 = réussi après « Explain differently », 0 = à
 * revoir ; null pour un palier pas encore fini.
 */
export function levelStars(course: Course, index: number): number | null {
  const level = course.levels[index];
  if (!level || !levelDone(level)) return null;
  if (level.status === "review") return 0;
  for (let j = 0; j < level.attempts.length; j += 1) {
    const score = attemptScore(course, index, j);
    if (!score.complete || score.correct < level.attempts[j].passMark) continue;
    if (j > 0) return 1;
    return score.correct === score.total ? 3 : 2;
  }
  return 0;
}

/**
 * Bonnes réponses d'affilée, en finissant par la dernière donnée (toutes
 * les questions, entraînement compris), dans l'ordre des réponses.
 */
export function currentStreak(course: Course): number {
  const answers = Object.entries(course.answers).sort(
    ([idA, a], [idB, b]) => a.at - b.at || idA.localeCompare(idB),
  );
  let streak = 0;
  for (let i = answers.length - 1; i >= 0 && answers[i][1].correct; i -= 1) {
    streak += 1;
  }
  return streak;
}

/** Série montrée (petite flamme) à partir de ce nombre de bonnes réponses. */
export const STREAK_SHOWN = 3;

/**
 * Indice stable dans [0, count) tiré d'un texte (identifiant d'une
 * question) : variante d'un message choisie sans hasard pendant le rendu.
 */
export function variantOf(seed: string, count: number): number {
  let hash = 5381;
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 33 + seed.charCodeAt(i)) >>> 0;
  }
  return count > 0 ? hash % count : 0;
}

const LIST_ITEM = /^ {0,3}(?:\d+[.)]|[-*+])\s+/;
const FENCE = /^ {0,3}(```|~~~)/;

/**
 * Étapes d'une solution (exemple résolu) à révéler une à une : chaque
 * élément de liste, chaque paragraphe et chaque bloc de code ou de formule
 * (jamais coupé) ; la suite indentée d'un élément de liste lui reste.
 * Une seule étape si le texte n'en a pas plusieurs.
 */
export function solutionSteps(markdown: string): string[] {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  const steps: string[][] = [];
  let current: string[] | null = null;
  let fence: string | null = null;
  let math = false;
  let blank = false;
  // Bloc tout juste fermé : la ligne suivante commence une étape.
  let closed = false;
  for (const line of lines) {
    const inBlock = fence !== null || math;
    if (!inBlock && line.trim() === "") {
      blank = true;
      continue;
    }
    const startsItem = !inBlock && LIST_ITEM.test(line);
    // Bloc de code ou de formule : une étape à lui, même collé au texte.
    const startsBlock = !inBlock && (FENCE.test(line) || line.trim() === "$$");
    const indented = /^\s{2,}\S/.test(line);
    // `blank` ne vaut jamais vrai dans un bloc (lignes vides gardées).
    const startsParagraph = (blank || closed) && !indented;
    if (current === null || startsItem || startsBlock || startsParagraph) {
      current = [];
      steps.push(current);
    } else if (blank) {
      current.push("");
    }
    blank = false;
    closed = false;
    current.push(line);
    const fenceMatch = FENCE.exec(line);
    if (fenceMatch) {
      if (fence === null) fence = fenceMatch[1];
      else if (line.trim().startsWith(fence)) {
        fence = null;
        closed = true;
      }
    } else if (fence === null && line.trim() === "$$") {
      math = !math;
      closed = !math;
    }
  }
  const texts = steps
    .map((step) => step.join("\n").trim())
    .filter((step) => step !== "");
  return texts.length > 0 ? texts : [markdown.trim()];
}
