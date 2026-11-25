// Atelier (moteur de tâches longues, propre à ChatonLM) : fonctions pures de
// la page, essayées à part (workshop.test.ts). Le moteur ne produit que des
// codes et des données ; la page les met en mots (src/locales/*/workshop.json)
// et ne montre jamais un texte du moteur tel quel. Les contenus écrits par le
// modèle (titres des parties, questions de cadrage, textes, preuves des juges)
// sont des données, montrés comme tels.

import type { Autonomy } from "@/bindings/Autonomy";
import type { BrickState } from "@/bindings/BrickState";
import type { Check } from "@/bindings/Check";
import type { Phase } from "@/bindings/Phase";
import type { ProjectState } from "@/bindings/ProjectState";
import type { ProjectSummary } from "@/bindings/ProjectSummary";
import type { Strategy } from "@/bindings/Strategy";
import type { WorkshopActivity } from "@/bindings/WorkshopActivity";
import type { WorkshopBrickPlan } from "@/bindings/WorkshopBrickPlan";
import type { WorkshopEvent } from "@/bindings/WorkshopEvent";
import type { WorkshopPlan } from "@/bindings/WorkshopPlan";
import type { TFunction, TKey } from "@/i18n";

/** Longueur maximale de la tâche (MAX_TASK_CHARS du moteur). */
export const MAX_TASK_CHARS = 20_000;
/** Pièces jointes au plus (MAX_ATTACHMENTS du moteur). */
export const MAX_ATTACHMENTS = 10;
/** Longueur du titre tiré de la tâche (MAX_TITLE_CHARS du moteur). */
const MAX_TITLE_CHARS = 80;
/** Longueur d'un nom de dossier (MAX_FOLDER_CHARS du moteur). */
export const MAX_FOLDER_CHARS = 60;

/** Niveaux d'autonomie, dans l'ordre du curseur (Maj+Tab : le suivant). */
export const AUTONOMY_LEVELS: readonly Autonomy[] = [
  "plan",
  "ask",
  "acceptActions",
  "autonomous",
];

/** Niveau suivant (Maj+Tab), le premier après le dernier. */
export function nextAutonomy(level: Autonomy): Autonomy {
  const index = AUTONOMY_LEVELS.indexOf(level);
  return AUTONOMY_LEVELS[(index + 1) % AUTONOMY_LEVELS.length];
}

/** Phases, dans l'ordre de la frise. */
export const PHASES: readonly Phase[] = [
  "collect",
  "strategy",
  "framing",
  "spec",
  "plan",
  "build",
  "assemble",
  "verify",
  "deliver",
];

/** Stratégies, dans l'ordre du choix. */
export const STRATEGIES: readonly Strategy[] = [
  "generic",
  "longForm",
  "research",
  "documents",
  "code",
];

/** Ce que le moteur sait mener aujourd'hui (`Strategy::available`). */
export function strategyAvailable(strategy: Strategy): boolean {
  return strategy === "generic" || strategy === "longForm";
}

/** Le projet est entre les mains du moteur (en file ou au travail). */
export function isActive(state: ProjectState): boolean {
  return state === "queued" || state === "running" || state === "yielding";
}

/** Le projet est fini (livré, échoué ou annulé). */
export function isFinished(state: ProjectState): boolean {
  return state === "done" || state === "failed" || state === "cancelled";
}

/**
 * Pastille de la carte de l'outil : « needsYou » si un projet attend une
 * réponse (prime), « working » si un projet calcule (ou cède la place un
 * instant), rien sinon. Un projet en file seulement n'en donne pas : il ne
 * se passe encore rien.
 */
export function badgeOf(
  projects: readonly ProjectSummary[],
): "needsYou" | "working" | null {
  if (projects.some((project) => project.state === "waiting")) {
    return "needsYou";
  }
  if (
    projects.some(
      (project) => project.state === "running" || project.state === "yielding",
    )
  ) {
    return "working";
  }
  return null;
}

/** Titre tiré de la tâche, comme le moteur (`service::title_of`). */
export function titleOf(task: string): string {
  const line =
    task
      .split("\n")
      .map((part) => part.trim())
      .find((part) => part !== "") ?? "";
  const words = line.split(/\s+/u).filter(Boolean).join(" ");
  const chars = [...words];
  if (chars.length <= MAX_TITLE_CHARS) return words;
  const clipped = chars.slice(0, MAX_TITLE_CHARS).join("");
  const space = clipped.lastIndexOf(" ");
  return space > MAX_TITLE_CHARS / 2
    ? `${clipped.slice(0, space)}…`
    : `${clipped}…`;
}

const FORBIDDEN = new Set(["<", ">", ":", '"', "/", "\\", "|", "?", "*"]);
const RESERVED =
  /^(con|prn|aux|nul|com[0-9¹²³]|lpt[0-9¹²³]|conin\$|conout\$)(\..*)?$/iu;

/**
 * Nom de dossier tiré d'un nom voulu (ou du titre), comme le moteur
 * (`files::folder_name`) : caractères refusés remplacés par « - », espaces
 * réduits, points, espaces et tirets des bords retirés, au plus 60
 * caractères ; vide ou réservé : "" (le moteur prend alors son nom par
 * défaut).
 */
export function folderNameOf(wanted: string): string {
  const replaced = [...wanted]
    .map((c) => {
      // eslint-disable-next-line no-control-regex
      if (/[\u0000-\u001f\u007f-\u009f]/u.test(c) || FORBIDDEN.has(c)) {
        return "-";
      }
      return /\s/u.test(c) ? " " : c;
    })
    .join("");
  const joined = replaced.split(/\s+/u).filter(Boolean).join(" ");
  const clipped = [...joined].slice(0, MAX_FOLDER_CHARS).join("");
  const trimmed = clipped.replace(/^[. -]+|[. -]+$/gu, "");
  if (trimmed === "" || trimmed === "." || trimmed === "..") return "";
  if (RESERVED.test(trimmed)) return "";
  return trimmed;
}

// --- Erreurs, avertissements ------------------------------------------------

/** Codes d'erreur traduits un à un ; les autres passent par un groupe. */
const ERROR_CODES = [
  "strategyNotYet",
  "specUndecided",
  "planUnusable",
  "deliverWriteFailed",
  "folderMissing",
  "noModel",
  "modelNotFound",
  "remoteOff",
  "contextTooSmall",
  "contextExceeded",
  "unreadable",
  "looping",
  "incomplete",
] as const;

const ENGINE_ERRORS = new Set([
  "unavailable",
  "notInstalled",
  "missingFile",
  "startFailed",
  "notReady",
  "crashed",
  "unloaded",
  "closed",
  "loading",
]);

const CONNECTION_ERRORS = new Set([
  "connect",
  "timeout",
  "interrupted",
  "http",
  "malformed",
  "remoteUnavailable",
  "remoteUnreachable",
]);

const ACCOUNT_ERRORS = new Set(["remoteKey", "unauthorized", "remoteLimit"]);

/** Clé du texte d'un code d'erreur du moteur (jamais le code lui-même). */
export function errorKey(code: string | null): TKey {
  if (code && (ERROR_CODES as readonly string[]).includes(code)) {
    return `workshop.errors.${code as (typeof ERROR_CODES)[number]}`;
  }
  if (code && ENGINE_ERRORS.has(code)) return "workshop.errors.engine";
  if (code && CONNECTION_ERRORS.has(code)) return "workshop.errors.connection";
  if (code && ACCOUNT_ERRORS.has(code)) return "workshop.errors.account";
  return "workshop.errors.unknown";
}

const WARNING_CODES = [
  "strategyNotYet",
  "planBestOfThree",
  "specBestOfThree",
  "registryCondensed",
  "registryConflict",
  "judgeUnavailable",
  "attachmentsCut",
  "lengthCapped",
  "modelRetry",
  "directFailed",
  "documentCriteriaRemoved",
  "looping",
  "briefTooLong",
  "proofreadRejected",
  "surgeryNotFound",
  "surgeryRejected",
] as const;

/** Clé du texte d'un avertissement du moteur (`planDefect.<code>`… groupés). */
export function warningKey(code: string): TKey {
  if ((WARNING_CODES as readonly string[]).includes(code)) {
    return `workshop.warnings.${code as (typeof WARNING_CODES)[number]}`;
  }
  if (code.startsWith("planDefect.")) return "workshop.warnings.planDefect";
  if (code.startsWith("specDefect.")) return "workshop.warnings.specDefect";
  return "workshop.warnings.generic";
}

const MODEL_WARNINGS = [
  "tooSmall",
  "small",
  "smallContext",
  "noThinking",
  "unknownSize",
] as const;

/** Clé du texte d'un avertissement du modèle (`ModelWarning`). */
export function modelWarningKey(code: string): TKey | null {
  return (MODEL_WARNINGS as readonly string[]).includes(code)
    ? `workshop.modelWarnings.${code as (typeof MODEL_WARNINGS)[number]}`
    : null;
}

const NOT_CHECKED = ["factJudge", "formJudge", "sources", "documents"] as const;

/** Clé du texte d'un point non vérifié du rapport. */
export function notCheckedKey(code: string): TKey {
  return (NOT_CHECKED as readonly string[]).includes(code)
    ? `workshop.report.notCheckedCodes.${code as (typeof NOT_CHECKED)[number]}`
    : "workshop.report.notCheckedCodes.generic";
}

// --- Vérifications -----------------------------------------------------------

/** Contrôles par programme nommés par un code (sinon : un critère « C2 »). */
const PROGRAM_CHECKS = [
  "empty",
  "length",
  "internalWord",
  "engineWord",
  "outsideRegistry",
  "language",
  "register",
  "goal",
  "forbidden",
  "registryConflict",
  "copiedRemark",
  "contradiction",
  "contradictions",
] as const;

type ProgramCheck = (typeof PROGRAM_CHECKS)[number];

/** Identifiant d'un critère de la spécification (« C1 »…). */
export function isCriterionId(text: string): boolean {
  return /^C\d+$/u.test(text);
}

/** Mesure d'un contrôle par programme : « 3239/3300-3700 words », « 950/1000 ». */
export interface Measured {
  value: number;
  min: number | null;
  max: number | null;
  unit: string;
}

/** Relit la preuve d'un contrôle mesuré (`measure::read_evidence`, `length`). */
export function readMeasure(evidence: string): Measured | null {
  const full = /^(\d+)\/(\d*)-(\d*) ([a-z@0-9]+)$/u.exec(evidence);
  if (full) {
    return {
      value: Number(full[1]),
      min: full[2] === "" ? null : Number(full[2]),
      max: full[3] === "" ? null : Number(full[3]),
      unit: full[4],
    };
  }
  const length = /^(\d+)\/(\d+)$/u.exec(evidence);
  if (length) {
    const target = Number(length[2]);
    return {
      value: Number(length[1]),
      min: target,
      max: target,
      unit: "words",
    };
  }
  return null;
}

/**
 * Ce qu'une vérification examine, en clair : l'affirmation jugée, le critère
 * de la spécification (son texte quand `criteria` le connaît), ou le
 * contrôle par programme.
 */
export function checkSubject(
  t: TFunction,
  check: Check,
  criteria?: ReadonlyMap<string, string>,
): string {
  if (check.method === "factJudge") return check.criterion;
  if (isCriterionId(check.criterion)) {
    const text = criteria?.get(check.criterion);
    return text
      ? t("workshop.checks.criterionText", { id: check.criterion, text })
      : t("workshop.checks.criterion", { id: check.criterion });
  }
  if ((PROGRAM_CHECKS as readonly string[]).includes(check.criterion)) {
    return t(`workshop.checks.codes.${check.criterion as ProgramCheck}`);
  }
  return t("workshop.checks.codes.generic");
}

/** Unité d'une mesure, en clair (« mots », « titres de niveau 2 »…). */
function unitText(t: TFunction, unit: string, count: number): string {
  const level = /^headings@(\d)$/u.exec(unit);
  if (level) {
    return t("workshop.checks.units.headingsLevel", {
      count,
      level: Number(level[1]),
    });
  }
  switch (unit) {
    case "words":
    case "headings":
    case "sections":
    case "tables":
    case "items":
      return t(`workshop.checks.units.${unit}`, { count });
    default:
      return t("workshop.checks.units.generic", { count });
  }
}

/** Mesure en clair : « 3 239 mots (visé : 3 300 à 3 700) ». */
export function measureText(t: TFunction, measure: Measured): string {
  const value = unitText(t, measure.unit, measure.value);
  const { min, max } = measure;
  if (min !== null && max !== null && min === max) {
    return t("workshop.checks.measure.exact", {
      value,
      target: min,
    });
  }
  if (min !== null && max !== null) {
    return t("workshop.checks.measure.between", {
      value,
      min,
      max,
    });
  }
  if (min !== null) {
    return t("workshop.checks.measure.atLeast", {
      value,
      min,
    });
  }
  if (max !== null) {
    return t("workshop.checks.measure.atMost", {
      value,
      max,
    });
  }
  return value;
}

/**
 * Détail d'une vérification à montrer sous son sujet : la mesure en clair
 * pour un contrôle par programme, la preuve citée pour un juge (contenu du
 * modèle) ; rien pour une preuve de programme illisible (code interne).
 */
export function checkDetail(t: TFunction, check: Check): string | null {
  if (check.method === "program") {
    const measure = readMeasure(check.evidence);
    return measure ? measureText(t, measure) : null;
  }
  if (check.evidence === "unavailable" || check.evidence === "") return null;
  return check.evidence;
}

/** Une affirmation jugée douteuse (jamais bloquante). */
export function isDoubtful(check: Check): boolean {
  return check.method === "factJudge" && !check.passed && !check.blocking;
}

/** Un échec qui compte, resté après les corrections (« à revoir »). */
export function isToReview(check: Check): boolean {
  return !check.passed && check.blocking;
}

/** Un signalement : échec non bloquant d'un contrôle par programme ou de forme. */
export function isSignal(check: Check): boolean {
  return !check.passed && !check.blocking && check.method !== "factJudge";
}

// --- Plan et parties -------------------------------------------------------

/** Parent d'une sous-partie (`parent * 100 + k`), ou null. */
export function parentOf(id: number): number | null {
  return id >= 100 ? Math.floor(id / 100) : null;
}

/** Une partie est finie (avec ou sans points à revoir). */
export function isPartDone(state: BrickState): boolean {
  return state === "done" || state === "toReview";
}

/** Une partie est au travail. */
export function isPartBusy(state: BrickState): boolean {
  return state === "running" || state === "checking" || state === "fixing";
}

/** Plan modifiable par l'utilisateur : titre, but, mots, ordre. */
export interface PlanEdit {
  title: string;
  bricks: WorkshopBrickPlan[];
}

/** Le plan modifié diffère-t-il du plan proposé ? */
export function planChanged(original: WorkshopPlan, edited: PlanEdit): boolean {
  if (original.bricks.length !== edited.bricks.length) return true;
  return original.bricks.some((brick, index) => {
    const other = edited.bricks[index];
    return (
      brick.id !== other.id ||
      brick.title !== other.title ||
      brick.goal !== other.goal ||
      brick.words !== other.words
    );
  });
}

/** Déplace la partie `index` d'un cran (`delta` : -1 vers le haut, 1 vers le bas). */
export function moveBrick<T>(
  items: readonly T[],
  index: number,
  delta: number,
) {
  const target = index + delta;
  if (target < 0 || target >= items.length) return [...items];
  const next = [...items];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

// --- Événements ----------------------------------------------------------------

/** État de chaque phase dans la frise. */
export type PhaseMark = "done" | "current" | "pending";

/**
 * État des phases, relu des événements (`phaseStarted`, `phaseDone`), avec
 * la phase du projet (`ProjectSummary.phase`) pour une page qui n'aurait pas
 * encore lu le journal. Une phase sautée (cadrage en autonomie, plan d'une
 * version d'un seul jet) qui précède la phase en cours compte comme faite.
 */
export function phaseMarks(
  events: readonly WorkshopEvent[],
  current: Phase | null,
  finished: boolean,
): Record<Phase, PhaseMark> {
  const done = new Set<Phase>();
  let latest: Phase | null = current;
  for (const event of events) {
    const kind = event.kind;
    if (kind.type === "phaseStarted") latest = kind.phase;
    if (kind.type === "phaseDone") done.add(kind.phase);
  }
  const at = latest === null ? -1 : PHASES.indexOf(latest);
  const marks = {} as Record<Phase, PhaseMark>;
  PHASES.forEach((phase, index) => {
    if (done.has(phase) || index < at || (finished && index <= at)) {
      marks[phase] = "done";
    } else if (index === at) {
      marks[phase] = "current";
    } else {
      marks[phase] = "pending";
    }
  });
  return marks;
}

/** Dernière activité connue (null : aucune). */
export function lastActivity(
  events: readonly WorkshopEvent[],
): WorkshopActivity | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const kind = events[i].kind;
    if (kind.type === "activity") return kind.activity;
  }
  return null;
}

/** Dernier plan retenu (`planReady`), ou null. */
export function lastPlan(
  events: readonly WorkshopEvent[],
): WorkshopPlan | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const kind = events[i].kind;
    if (kind.type === "planReady") return kind.plan;
  }
  return null;
}

/** La carte est prêtée à un autre moteur en ce moment ; à qui, sinon null. */
export function gpuLentTo(events: readonly WorkshopEvent[]): string | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const kind = events[i].kind;
    if (kind.type === "gpuBack") return null;
    if (kind.type === "gpuLent") return kind.to;
  }
  return null;
}

/**
 * Numéro d'un projet lu dans l'adresse (`/workshop/<id>`) ; null s'il n'en
 * est pas un (entier positif écrit en chiffres).
 */
export function projectIdOf(param: string): number | null {
  if (!/^\d{1,15}$/u.test(param)) return null;
  const id = Number(param);
  return id > 0 ? id : null;
}

/**
 * Temps de travail (ms) : somme des périodes où le projet travaille, lues
 * dans le journal. Une période commence à chaque lancement (le moteur note
 * le modèle retenu : `modelWarnings`) et à chaque réponse de l'utilisateur ;
 * elle finit à une question (attente de l'utilisateur), à la fin du projet,
 * ou au dernier événement avant le lancement suivant (pause, fermeture de
 * l'app : rien n'est noté à l'arrêt). Céder la place un instant
 * (`yielding`) compte comme du travail, comme l'état « running ». La
 * période en cours court jusqu'à `now` tant que le projet travaille.
 */
export function elapsedMs(
  events: readonly WorkshopEvent[],
  working: boolean,
  now: number,
): number {
  let total = 0;
  /** Début de la période en cours (null : pas de travail). */
  let start: number | null = null;
  /** Dernier événement lu (fin d'une période interrompue sans trace). */
  let previous: number | null = null;
  /** Question en attente ou projet fini : rien ne reprend sans lancement. */
  let stopped = false;
  for (const event of events) {
    const at = event.atMs;
    switch (event.kind.type) {
      case "modelWarnings":
        // Nouveau lancement : la période d'avant (pause) a fini au dernier
        // événement noté.
        if (start !== null && previous !== null) total += previous - start;
        start = at;
        stopped = false;
        break;
      case "questionAsked":
      case "finished":
        if (start !== null) total += at - start;
        start = null;
        stopped = true;
        break;
      case "answerReceived":
        start = at;
        stopped = false;
        break;
      default:
        // Journal d'avant un lancement noté : le travail court depuis là.
        if (!stopped) start ??= at;
    }
    previous = at;
  }
  if (start !== null && previous !== null) {
    total += (working ? Math.max(now, previous) : previous) - start;
  }
  return Math.max(0, total);
}

/** Le journal couvre-t-il plusieurs jours (heure locale) ? */
export function spansDays(events: readonly WorkshopEvent[]): boolean {
  if (events.length < 2) return false;
  const day = (ms: number) => new Date(ms).toDateString();
  return day(events[0].atMs) !== day(events[events.length - 1].atMs);
}

/** Durée en clair, à la minute (« 2 h 14 min ») ; pas de fausse précision. */
export function durationText(t: TFunction, ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return t("workshop.gauge.underMinute");
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) return t("workshop.gauge.minutes", { count: rest });
  return t("workshop.gauge.hours", { hours, minutes: rest });
}

/** Titre d'une partie connue du plan, sinon null. */
function titleIn(plan: WorkshopPlan | null, id: number): string | null {
  return plan?.bricks.find((brick) => brick.id === id)?.title ?? null;
}

/** Nom d'une partie dans une phrase : son titre, ou « partie N ». */
export function partName(
  t: TFunction,
  plan: WorkshopPlan | null,
  id: number,
  titles?: ReadonlyMap<number, string>,
): string {
  if (id === 0) return t("workshop.plan.direct");
  const title = titles?.get(id) ?? titleIn(plan, id);
  return title ?? t("workshop.plan.partNumber", { number: id });
}

/** Activité en clair (ligne d'activité, journal). */
export function activityText(
  t: TFunction,
  activity: WorkshopActivity,
  name: (id: number) => string,
): string {
  switch (activity.code) {
    case "choosingStrategy":
      return t("workshop.activity.choosingStrategy", { draw: activity.draw });
    case "writingSpec":
      return t("workshop.activity.writingSpec", { attempt: activity.attempt });
    case "writingDirect":
      return t("workshop.activity.writingDirect");
    case "checkingDirect":
      return t("workshop.activity.checkingDirect");
    case "writingPlan":
      return t("workshop.activity.writingPlan", { attempt: activity.attempt });
    case "judgingPlan":
      return t("workshop.activity.judgingPlan", { attempt: activity.attempt });
    case "writingBrick":
      return t("workshop.activity.writingBrick", { part: name(activity.id) });
    case "checkingBrick":
      return t("workshop.activity.checkingBrick", { part: name(activity.id) });
    case "fixingBrick":
      return t("workshop.activity.fixingBrick", { part: name(activity.id) });
    case "splittingBrick":
      return t("workshop.activity.splittingBrick", { part: name(activity.id) });
    case "summarizingBrick":
      return t("workshop.activity.summarizingBrick", {
        part: name(activity.id),
      });
    case "fixingSentences":
      return t("workshop.activity.fixingSentences", {
        part: name(activity.id),
      });
    case "assembling":
      return t("workshop.activity.assembling");
    case "checkingWhole":
      return t("workshop.activity.checkingWhole");
    case "proofreading":
      return t("workshop.activity.proofreading", {
        number: activity.block + 1,
      });
    case "delivering":
      return t("workshop.activity.delivering");
  }
}

/** Moteur à qui la carte est prêtée, en clair. */
export function engineName(t: TFunction, code: string): string {
  switch (code) {
    case "image":
    case "images":
      return t("workshop.engines.image");
    case "transcription":
    case "whisper":
      return t("workshop.engines.transcription");
    default:
      return t("workshop.engines.generic");
  }
}

/** Ligne du journal : un événement en clair (null : rien à dire). */
export function eventText(
  t: TFunction,
  event: WorkshopEvent,
  name: (id: number) => string,
): string {
  const kind = event.kind;
  switch (kind.type) {
    case "phaseStarted":
      return t("workshop.log.phaseStarted", {
        phase: t(`workshop.phases.${kind.phase}`),
      });
    case "phaseDone":
      return t("workshop.log.phaseDone", {
        phase: t(`workshop.phases.${kind.phase}`),
      });
    case "strategyChosen":
      return kind.unanimous
        ? t("workshop.log.strategyChosenUnanimous", {
            strategy: t(`workshop.strategies.${kind.strategy}`),
          })
        : t("workshop.log.strategyChosen", {
            strategy: t(`workshop.strategies.${kind.strategy}`),
          });
    case "planReady":
      return t("workshop.log.planReady", { count: kind.plan.bricks.length });
    case "brick":
      return t("workshop.log.brick", {
        part: name(kind.id),
        state: t(`workshop.partStates.${kind.state}`),
      });
    case "brickChecks": {
      const failed = kind.checks.filter((check) => !check.passed).length;
      return failed === 0
        ? t("workshop.log.checksPassed", {
            part: name(kind.id),
            count: kind.checks.length,
          })
        : t("workshop.log.checksFailed", {
            part: name(kind.id),
            count: failed,
            total: kind.checks.length,
          });
    }
    case "activity":
      return activityText(t, kind.activity, name);
    case "questionAsked":
      return t(`workshop.log.question.${kind.question.type}`);
    case "answerReceived":
      return t(`workshop.log.answer.${kind.answer.type}`);
    case "gpuLent":
      return t("workshop.log.gpuLent", { engine: engineName(t, kind.to) });
    case "gpuBack":
      return t("workshop.log.gpuBack");
    case "yielding":
      return t("workshop.log.yielding");
    case "resumed":
      return t("workshop.log.resumed");
    case "modelWarnings": {
      const texts = kind.warnings.flatMap((code) => {
        const key = modelWarningKey(code);
        return key ? [t(key)] : [];
      });
      return texts.length === 0
        ? t("workshop.log.model", { model: kind.model })
        : t("workshop.log.modelWarnings", {
            model: kind.model,
            warnings: texts.join(" "),
          });
    }
    case "warning":
      return t(warningKey(kind.code));
    case "error":
      return t(errorKey(kind.code));
    case "finished":
      return kind.state === "done"
        ? t("workshop.log.finishedDone")
        : t("workshop.log.finishedFailed");
  }
}

/** Mots en clair (« 48 200 mots »). */
export function wordsText(t: TFunction, words: number): string {
  return t("workshop.words", { count: words });
}

/** Phrase de l'activité, selon l'état du projet et le dernier événement. */
export function activityLine(
  t: TFunction,
  project: ProjectSummary,
  events: readonly WorkshopEvent[],
  name: (id: number) => string,
): string {
  switch (project.state) {
    case "queued":
      return project.position !== null
        ? t("workshop.activity.queued", { position: project.position })
        : t("workshop.activity.queuedNext");
    case "waiting":
      return t("workshop.activity.waiting");
    case "yielding":
      return t("workshop.activity.yielding");
    case "paused":
      return t("workshop.activity.paused");
    case "done":
      return t("workshop.activity.done");
    case "failed":
      return t(errorKey(project.error));
    case "cancelled":
      return t("workshop.activity.cancelled");
    case "running": {
      const lent = gpuLentTo(events);
      if (lent !== null) {
        return t("workshop.activity.gpuLent", { engine: engineName(t, lent) });
      }
      const activity = lastActivity(events);
      return activity
        ? activityText(t, activity, name)
        : t("workshop.activity.starting");
    }
  }
}
