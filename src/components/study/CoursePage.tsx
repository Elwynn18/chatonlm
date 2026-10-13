// Page d'un cours (onglet Study, propre à ChatonLM), selon son état :
// préparation du plan (étapes, « Cancel » supprime le cours), sujet ambigu
// (un bouton par sens, champ libre, « Use the topic as it is »), plan prêt
// (« Start », « New plan », « Delete »), échec (action selon le code de
// l'erreur), puis le lecteur (cours commencé ou fini). Le cours est relu à
// chaque « study-updated » (useStudyEvents, gabarit de l'onglet).
//
// Pendant la préparation et sur le plan, l'écriture du cours est dite en
// texte (GenerationStatus : ce qui s'écrit, paliers prêts, « on peut
// commencer dès que le premier palier est prêt », « Pause » / « Resume ») ;
// la durée du plan est un temps d'étude, pas une attente.

import {
  ArrowPathIcon,
  CheckCircleIcon,
  PauseIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import type { AppError } from "@/bindings/AppError";
import type { Course } from "@/bindings/Course";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CourseReader } from "@/components/study/CourseReader";
import { GenerationStatus } from "@/components/study/GenerationStatus";
import { Sources } from "@/components/study/Sources";
import { StudyErrorNotice } from "@/components/study/StudyErrorNotice";
import {
  BUTTON,
  CARD,
  EYEBROW,
  Markdown,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
  READING,
  TutorBubble,
} from "@/components/study/ui";
import {
  courseKey,
  studyListKey,
  useCourse,
  useCourseAction,
} from "@/hooks/useStudy";
import { useT } from "@/i18n";
import {
  errorMessage,
  studyClarify,
  studyDelete,
  studyReplan,
  studyStart,
} from "@/lib/ipc";
import { levelStartKey, roundMinutes } from "@/lib/study";

export function CoursePage({ courseId }: { courseId: string }) {
  const t = useT();
  const query = useCourse(courseId);
  // Commencé ici : le lecteur ouvre directement le premier palier (le plan
  // vient d'être lu).
  const [startedHere, setStartedHere] = useState(false);

  const loading = (
    <p
      aria-busy="true"
      className={`${READING} p-6 text-sm text-neutral-500 dark:text-neutral-400`}
    >
      {t("study.course.loading")}
    </p>
  );
  let content;
  if (query.isPending) {
    content = loading;
  } else if (query.isError) {
    const missing = (query.error as Partial<AppError>).kind === "notFound";
    content = (
      <div className={`${READING} p-6`}>
        <p
          role="alert"
          className="text-sm text-neutral-700 dark:text-neutral-300"
        >
          {missing
            ? t("study.course.notFound")
            : t("study.course.readError", {
                message: errorMessage(query.error),
              })}
        </p>
        <Link to="/study" className={`${BUTTON} mt-4`}>
          {t("study.course.backHome")}
        </Link>
      </div>
    );
  } else {
    const course = query.data;
    switch (course.status) {
      case "planning":
        content = <Planning course={course} />;
        break;
      case "unclear":
        content = <Unclear course={course} />;
        break;
      case "planned":
        content = (
          <PlanView course={course} onStarted={() => setStartedHere(true)} />
        );
        break;
      case "failed":
        content = <Failed course={course} />;
        break;
      default:
        // Le lecteur reprend la position et le temps passé du cours, puis
        // les enregistre : jamais d'après un cours gardé en cache depuis une
        // visite précédente (study_progress ne le met pas à jour), qui
        // ramènerait en arrière la position et le temps enregistrés.
        if (!query.isFetchedAfterMount) {
          content = loading;
          break;
        }
        return (
          <CourseReader
            course={course}
            initialPosition={startedHere ? levelStartKey(0) : null}
          />
        );
    }
  }
  return (
    <main
      data-testid="study-course"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      {content}
    </main>
  );
}

/** Supprime le cours puis revient à l'accueil. */
function useDeleteCourse(courseId: string) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const remove = () => {
    setError(null);
    studyDelete(courseId).then(
      () => {
        queryClient.removeQueries({ queryKey: courseKey(courseId) });
        void queryClient.invalidateQueries({ queryKey: studyListKey });
        void navigate({ to: "/study" });
      },
      (cause: unknown) => setError(errorMessage(cause)),
    );
  };
  return { remove, error };
}

function ActionError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p
      role="alert"
      className="mt-3 rounded-lg bg-red-50 px-3 py-2 text-sm break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
    >
      {message}
    </p>
  );
}

/** Préparation du plan : étapes, et « Cancel » (le cours est supprimé). */
function Planning({ course }: { course: Course }) {
  const t = useT();
  const { remove, error } = useDeleteCourse(course.id);
  const documents = course.sources.filter((s) => s.kind === "document");
  const webDone = course.sources.some((s) => s.kind === "web");
  const steps: { key: string; text: string; done: boolean }[] = [];
  if (documents.length > 0) {
    steps.push({
      key: "documents",
      text: t("study.planning.documents", { count: documents.length }),
      done: true,
    });
  }
  if (course.web) {
    steps.push({ key: "web", text: t("study.planning.web"), done: webDone });
  }
  steps.push({ key: "plan", text: t("study.planning.plan"), done: false });
  return (
    <div className={`${READING} flex flex-col gap-5 p-6`}>
      <TutorBubble testId="study-planning">
        <p className="font-medium text-neutral-900 dark:text-white">
          {t("study.planning.title")}
        </p>
        <p className="mt-0.5 text-neutral-600 dark:text-neutral-300">
          {course.topic || documents.map((d) => d.title).join(", ")}
        </p>
      </TutorBubble>
      <ol aria-label={t("study.planning.steps")} className="ml-11 space-y-2">
        {steps.map((step) => (
          <li
            key={step.key}
            // En pause : plus rien n'avance (dit par l'état, dessous).
            aria-busy={(!step.done && !course.paused) || undefined}
            className="flex items-center gap-2 text-sm text-neutral-700 dark:text-neutral-300"
          >
            {step.done ? (
              <CheckCircleIcon
                aria-hidden="true"
                className="h-4 w-4 text-emerald-600 dark:text-emerald-400"
              />
            ) : course.paused ? (
              <PauseIcon
                aria-hidden="true"
                className="h-4 w-4 text-neutral-500"
              />
            ) : (
              <ArrowPathIcon
                aria-hidden="true"
                className="h-4 w-4 text-neutral-500 motion-safe:animate-spin"
              />
            )}
            <span>{step.text}</span>
            {step.done && (
              <span className="sr-only">{t("study.planning.done")}</span>
            )}
          </li>
        ))}
      </ol>
      <div className="ml-11">
        <GenerationStatus course={course} hint omitPlan />
      </div>
      <div className="ml-11">
        <button type="button" className={BUTTON} onClick={remove}>
          {t("common.actions.cancel")}
        </button>
        <ActionError message={error} />
      </div>
    </div>
  );
}

/** Sujet ambigu : un sens à choisir, ou le sien, ou le sujet tel quel. */
function Unclear({ course }: { course: Course }) {
  const t = useT();
  const fieldId = useId();
  const [meaning, setMeaning] = useState("");
  const action = useCourseAction(course.id);
  const clarify = course.clarify;
  const choose = (choice: string | null) =>
    action.run(() => studyClarify(course.id, choice));
  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    if (meaning.trim()) choose(meaning.trim());
  };
  return (
    <div className={`${READING} flex flex-col gap-4 p-6`}>
      <TutorBubble testId="study-unclear">
        <p className="text-neutral-900 dark:text-white">
          {clarify?.question ?? t("study.unclear.fallback")}
        </p>
      </TutorBubble>
      <div
        role="group"
        aria-label={t("study.unclear.options")}
        className="ml-11 flex flex-wrap gap-2"
      >
        {(clarify?.options ?? []).map((option) => (
          <button
            key={option}
            type="button"
            className={BUTTON}
            disabled={action.pending}
            onClick={() => choose(option)}
          >
            {option}
          </button>
        ))}
      </div>
      <form
        onSubmit={onSubmit}
        className="ml-11 flex flex-wrap items-end gap-2"
      >
        <div className="min-w-48 flex-1">
          <label
            htmlFor={fieldId}
            className="mb-1 block text-sm text-neutral-600 dark:text-neutral-300"
          >
            {t("study.unclear.custom")}
          </label>
          <input
            id={fieldId}
            value={meaning}
            maxLength={300}
            onChange={(event) => setMeaning(event.target.value)}
            className="h-9 w-full rounded-lg border border-neutral-300 bg-white px-3 text-sm outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
          />
        </div>
        <button
          type="submit"
          className={PRIMARY_BUTTON}
          disabled={action.pending || !meaning.trim()}
        >
          {t("study.unclear.useMeaning")}
        </button>
      </form>
      <div className="ml-11">
        <button
          type="button"
          className={QUIET_BUTTON}
          disabled={action.pending}
          onClick={() => choose(null)}
        >
          {t("study.unclear.asIs")}
        </button>
        <ActionError message={action.error} />
      </div>
    </div>
  );
}

/** Plan prêt : à lire avant de commencer. */
function PlanView({
  course,
  onStarted,
}: {
  course: Course;
  onStarted: () => void;
}) {
  const t = useT();
  const action = useCourseAction(course.id);
  const deletion = useDeleteCourse(course.id);
  const [confirming, setConfirming] = useState(false);
  const startRef = useRef<HTMLButtonElement>(null);
  const plan = course.plan;
  // Plan arrivé : « Start » prend le focus (Entrée commence).
  useEffect(() => {
    if (document.activeElement === document.body) startRef.current?.focus();
  }, [plan?.title]);
  if (!plan) return null;
  return (
    <div className={`${READING} flex flex-col gap-4 p-6`}>
      <article
        data-testid="study-plan"
        aria-labelledby="study-plan-title"
        className={`${CARD} motion-safe:animate-study-card-in`}
      >
        <p className={EYEBROW}>{t("study.plan.eyebrow")}</p>
        <h2
          id="study-plan-title"
          className="mt-1 font-rounded text-2xl font-semibold text-neutral-900 dark:text-white"
        >
          {plan.title}
        </h2>
        <p className="mt-2 flex flex-wrap gap-2 text-xs">
          <span className="rounded-full bg-neutral-100 px-2.5 py-1 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">
            {t(`study.levels.${course.level}`)}
          </span>
          <span className="rounded-full bg-neutral-100 px-2.5 py-1 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">
            {t("study.plan.duration", {
              count: roundMinutes(course.estimatedSeconds),
            })}
          </span>
          <span className="rounded-full bg-neutral-100 px-2.5 py-1 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">
            {t("study.plan.levelCount", { count: plan.levels.length })}
          </span>
        </p>
        <div className="mt-4">
          <Markdown content={plan.summary} />
        </div>
        <h3 className="mt-5 text-sm font-semibold text-neutral-900 dark:text-white">
          {t("study.plan.objectives")}
        </h3>
        <ul className="mt-2 space-y-1.5">
          {plan.objectives.map((objective) => (
            <li
              key={objective}
              className="flex items-start gap-2 text-sm text-neutral-700 dark:text-neutral-300"
            >
              <CheckCircleIcon
                aria-hidden="true"
                className="mt-0.5 h-4 w-4 flex-none text-neutral-500"
              />
              {objective}
            </li>
          ))}
        </ul>
        <h3 className="mt-5 text-sm font-semibold text-neutral-900 dark:text-white">
          {t("study.plan.levels")}
        </h3>
        <ol className="mt-3 space-y-3">
          {plan.levels.map((level, index) => (
            <li key={index} className="flex gap-3">
              <span
                aria-hidden="true"
                className="flex h-7 w-7 flex-none items-center justify-center rounded-full border border-neutral-300 text-xs font-semibold text-neutral-700 dark:border-neutral-600 dark:text-neutral-200"
              >
                {index + 1}
              </span>
              <div className="min-w-0">
                <p className="text-sm font-medium text-neutral-900 dark:text-white">
                  <span className="sr-only">
                    {t("study.path.levelNumber", { number: index + 1 })}{" "}
                  </span>
                  {level.title}
                </p>
                <p className="text-sm text-neutral-600 dark:text-neutral-400">
                  {level.goal}
                </p>
                {level.points.length > 0 && (
                  <ul
                    aria-label={t("study.plan.points")}
                    className="mt-1.5 flex flex-wrap gap-1.5"
                  >
                    {level.points.map((point) => (
                      <li
                        key={point}
                        className="rounded-full bg-neutral-100 px-2 py-0.5 text-xs text-neutral-600 dark:bg-neutral-700/70 dark:text-neutral-300"
                      >
                        {point}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </li>
          ))}
        </ol>
        {plan.scope && (
          <p className="mt-5 rounded-xl bg-neutral-50 px-3 py-2 text-sm text-neutral-600 dark:bg-neutral-900/60 dark:text-neutral-400">
            {t("study.plan.scope", { scope: plan.scope })}
          </p>
        )}
        <Sources sources={course.sources} />
      </article>
      <GenerationStatus course={course} hint />
      <div className="flex flex-wrap items-center gap-2">
        <button
          ref={startRef}
          type="button"
          className={PRIMARY_BUTTON}
          disabled={action.pending}
          onClick={() => action.run(() => studyStart(course.id), onStarted)}
        >
          {t("study.plan.start")}
        </button>
        <button
          type="button"
          className={BUTTON}
          disabled={action.pending}
          onClick={() => action.run(() => studyReplan(course.id))}
        >
          {t("study.plan.newPlan")}
        </button>
        <button
          type="button"
          className={QUIET_BUTTON}
          onClick={() => setConfirming(true)}
        >
          {t("common.actions.delete")}
        </button>
      </div>
      <ActionError message={action.error ?? deletion.error} />
      {confirming && (
        <ConfirmDialog
          title={t("study.delete.title", { title: plan.title })}
          message={t("study.delete.message")}
          confirmLabel={t("common.actions.delete")}
          onConfirm={() => {
            setConfirming(false);
            deletion.remove();
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </div>
  );
}

/** Le plan n'a pas pu être écrit. */
function Failed({ course }: { course: Course }) {
  const t = useT();
  const deletion = useDeleteCourse(course.id);
  return (
    <div className={`${READING} flex flex-col gap-4 p-6`}>
      <h2 className="font-rounded text-xl font-semibold text-neutral-900 dark:text-white">
        {course.topic || t("study.failed.untitled")}
      </h2>
      <StudyErrorNotice
        courseId={course.id}
        error={
          course.error ?? { code: "failed", message: t("study.failed.unknown") }
        }
      />
      <div>
        <button
          type="button"
          className={QUIET_BUTTON}
          onClick={deletion.remove}
        >
          {t("study.failed.delete")}
        </button>
        <ActionError message={deletion.error} />
      </div>
    </div>
  );
}
