// Écriture d'un cours (onglet Study) : ce que le modèle écrit en ce moment
// (plan, palier n sur m et son étape, nouvel essai, bilan), combien de
// paliers sont prêts, et le bouton « Pause » / « Resume » du cours. Les
// durées montrées ailleurs sont des temps d'étude : c'est ici que
// l'apprenant voit où en est l'écriture, qui se poursuit pendant qu'il
// étudie (il commence dès que le premier palier est prêt).
//
// L'état est dit en texte, pas par la seule roue, dans une région
// `role="status"` : annoncée quand la partie écrite change, qu'un palier
// devient prêt ou que l'écriture s'arrête, pas à chaque étape d'une partie
// (l'étape, « Step 1 of 2: the lesson », est hors de la région). Une seule
// région par page : dans le lecteur, celle de l'en-tête, ou celle de la
// carte d'attente qui la remplace (`live` faux pour un second état montré
// en même temps). Le moteur
// ne dit rien de l'avancement à l'intérieur d'une requête (réponses en JSON,
// sans flux) : entre deux étapes, la roue seule tourne.
//
// Pause : la partie en cours est arrêtée, plus rien n'est écrit pour ce
// cours (les autres continuent) jusqu'à « Resume » ; enregistré par le
// moteur. Une partie en échec se relance d'ici (« Retry ») quand plus rien
// d'autre ne s'écrit.

import {
  ArrowPathIcon,
  ClockIcon,
  ExclamationTriangleIcon,
  PauseIcon,
  PlayIcon,
} from "@heroicons/react/20/solid";
import type { Course } from "@/bindings/Course";
import { BUTTON, PRIMARY_BUTTON } from "@/components/study/ui";
import { useCourseAction } from "@/hooks/useStudy";
import { type TFunction, useT } from "@/i18n";
import { studyPause, studyRetry } from "@/lib/ipc";
import { type CoursePart, type Generation, generationOf } from "@/lib/study";

/** Bouton discret de la ligne d'état du lecteur. */
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1 rounded-full border border-neutral-300 px-2.5 py-0.5 text-xs font-medium text-neutral-700 outline-none hover:bg-neutral-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

/** Ce qui s'écrit : « Writing level 2 of 4… ». */
function writingText(t: TFunction, part: CoursePart, total: number): string {
  switch (part.kind) {
    case "plan":
      return t("study.generation.plan");
    case "level":
      return t("study.generation.level", { number: part.level + 1, total });
    case "remedy":
      return t("study.generation.remedy", { number: part.level + 1 });
    case "final":
      return t("study.generation.final");
  }
}

/** Ce qui n'a pas pu être écrit. */
function failedText(t: TFunction, part: CoursePart): string {
  switch (part.kind) {
    case "level":
      return t("study.generation.failed.level", { number: part.level + 1 });
    case "remedy":
      return t("study.generation.failed.remedy", { number: part.level + 1 });
    default:
      return t("study.generation.failed.final");
  }
}

type State = "paused" | "waiting" | "writing" | "failed" | "idle";

function stateOf(generation: Generation): State {
  if (generation.paused) return "paused";
  if (generation.waiting) return "waiting";
  if (generation.writing) return "writing";
  return generation.failed ? "failed" : "idle";
}

/** Tout est-il écrit (paliers et bilan d'un cours commencé) ? */
function complete(course: Course, generation: Generation): boolean {
  return (
    (course.status === "active" || course.status === "finished") &&
    !generation.pending &&
    !generation.failed &&
    course.finalQuiz?.state === "ready"
  );
}

export function GenerationStatus({
  course,
  compact = false,
  hint = false,
  omitPlan = false,
  emphasis = false,
  live = true,
}: {
  course: Course;
  /** Une ligne (en-tête du lecteur) : rien de visible quand rien ne reste. */
  compact?: boolean;
  /** Avec la phrase qui dit quand commencer, ou ce que la pause retient. */
  hint?: boolean;
  /** L'écriture du plan est déjà dite par la page (étapes de la préparation). */
  omitPlan?: boolean;
  /** « Resume » en bouton principal (attente d'un palier en pause). */
  emphasis?: boolean;
  /** Faux : pas de région d'annonces (une autre dit déjà l'état). */
  live?: boolean;
}) {
  const t = useT();
  const action = useCourseAction(course.id);
  const generation = generationOf(course);
  const { writing, paused, pending, failed, ready, total } = generation;
  const state = stateOf(generation);
  const what =
    state === "paused"
      ? t("study.generation.paused")
      : state === "waiting"
        ? t("study.generation.waiting")
        : state === "writing" && writing
          ? omitPlan && writing.kind === "plan"
            ? null
            : writingText(t, writing, total)
          : state === "failed" && failed
            ? failedText(t, failed)
            : null;
  const done = complete(course, generation);
  const count =
    total > 0 && !done
      ? t("study.generation.ready", { count: ready, total })
      : null;
  const stage =
    !compact && state === "writing" && writing?.kind === "level"
      ? t(`study.generation.stage.${writing.stage}`)
      : null;
  const planned = course.status === "planning" || course.status === "planned";
  const hintText = !hint
    ? null
    : paused
      ? t("study.generation.hint.paused")
      : planned && course.levels[0]?.state === "ready"
        ? t("study.generation.hint.start")
        : planned
          ? t("study.generation.hint.first")
          : null;
  // Rien à mettre en pause, ni à reprendre, quand plus rien ne reste à écrire.
  const canPause = pending;
  const Icon =
    state === "paused"
      ? PauseIcon
      : state === "waiting"
        ? ClockIcon
        : state === "failed"
          ? ExclamationTriangleIcon
          : ArrowPathIcon;
  const toggleTitle = paused
    ? t("study.generation.resumeTitle")
    : t("study.generation.pauseTitle");

  const status = (
    <p
      role={live ? "status" : undefined}
      data-testid="study-generation"
      data-state={state}
      className={`flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-0.5 ${
        compact ? "text-xs" : "text-sm"
      } ${
        state === "failed"
          ? "text-red-600 dark:text-red-400"
          : compact
            ? "text-neutral-500 dark:text-neutral-400"
            : "text-neutral-700 dark:text-neutral-300"
      }`}
    >
      {what && (
        <span className="flex items-center gap-1.5">
          <Icon
            aria-hidden="true"
            className={`h-4 w-4 flex-none ${
              state === "writing" ? "motion-safe:animate-spin" : ""
            }`}
          />
          {what}
        </span>
      )}
      {what && count && <span aria-hidden="true">·</span>}
      {count && (what || !compact) && <span>{count}</span>}
      {done && (
        <span className="sr-only">{t("study.generation.complete")}</span>
      )}
    </p>
  );
  const controls = (
    <>
      {state === "failed" && (
        <button
          type="button"
          className={compact ? SMALL_BUTTON : BUTTON}
          disabled={action.pending}
          onClick={() => action.run(() => studyRetry(course.id))}
        >
          {t("study.error.retry")}
        </button>
      )}
      {canPause && (
        <button
          type="button"
          data-testid="study-pause"
          className={
            compact
              ? SMALL_BUTTON
              : paused && emphasis
                ? PRIMARY_BUTTON
                : BUTTON
          }
          disabled={action.pending}
          aria-label={toggleTitle}
          title={toggleTitle}
          onClick={() => action.run(() => studyPause(course.id, !paused))}
        >
          {paused ? (
            <PlayIcon aria-hidden="true" className="h-4 w-4" />
          ) : (
            <PauseIcon aria-hidden="true" className="h-4 w-4" />
          )}
          {paused ? t("study.generation.resume") : t("study.generation.pause")}
        </button>
      )}
    </>
  );
  const error = action.error && (
    <p role="alert" className="text-xs text-red-600 dark:text-red-400">
      {action.error}
    </p>
  );

  if (compact) {
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        {status}
        {controls}
        {error}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-1.5 rounded-xl bg-neutral-50 px-4 py-3 dark:bg-neutral-900/60">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {status}
        {controls}
      </div>
      {stage && (
        <p
          data-testid="study-generation-stage"
          className="text-xs text-neutral-500 dark:text-neutral-400"
        >
          {stage}
        </p>
      )}
      {hintText && (
        <p
          data-testid="study-generation-hint"
          className="text-sm text-neutral-600 dark:text-neutral-400"
        >
          {hintText}
        </p>
      )}
      {error}
    </div>
  );
}
