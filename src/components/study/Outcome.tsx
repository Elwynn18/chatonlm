// Résultats du lecteur de cours (onglet Study) : résultat d'un essai
// (réussi : célébration brève, étoiles qui apparaissent une à une,
// confettis ; manqué : erreurs expliquées, « Explain differently », « Review
// the lesson », puis « Continue anyway » après deux essais manqués) et fin
// du cours (grande célébration, score du bilan, étoiles et meilleur score
// par palier, temps passé, fiche « Key points » à copier ou enregistrer,
// questions manquées, « Go further », sources).

import {
  ArrowDownTrayIcon,
  CheckCircleIcon,
  CheckIcon,
  ClockIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { Square2StackIcon, TrophyIcon } from "@heroicons/react/24/outline";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import type { Course } from "@/bindings/Course";
import type { Level } from "@/bindings/Level";
import { Sources } from "@/components/study/Sources";
import { CardShell, KeyHint } from "@/components/study/cardKit";
import { Preparing } from "@/components/study/LessonCards";
import { ExpectedDetail } from "@/components/study/QuestionCard";
import { expectedText, levelEyebrow } from "@/components/study/texts";
import { StudyErrorNotice } from "@/components/study/StudyErrorNotice";
import {
  BUTTON,
  Confetti,
  InlineMarkdown,
  Markdown,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
  Stars,
} from "@/components/study/ui";
import { storeRemedyRequested, useCourseAction } from "@/hooks/useStudy";
import { useCardKeys } from "@/hooks/useCardKeys";
import { useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import { copyText } from "@/lib/clipboard";
import {
  errorMessage,
  studyNotes,
  studyRemedy,
  studySaveNotes,
  studySkip,
} from "@/lib/ipc";
import {
  attemptScore,
  bestScore,
  type Card,
  finalKey,
  finalScore,
  levelStars,
  levelStartKey,
  type MissedQuestion,
  missedQuestions,
  questionPrompt,
} from "@/lib/study";

/** Résultat d'un essai de validation. */
export function ResultCard({
  course,
  level,
  attempt,
  next,
  go,
}: {
  course: Course;
  level: Level;
  attempt: number;
  next: Card | null;
  /** `justOpened` : suite ouverte par l'action (voir CourseReader). */
  go: (key: string, justOpened?: boolean) => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const action = useCourseAction(course.id);
  const score = attemptScore(course, level.index, attempt);
  const passMark = level.attempts[attempt]?.passMark ?? 0;
  const passed = score.complete && score.correct >= passMark;
  const latest = attempt === level.attempts.length - 1;
  const review = level.status === "review";
  const stars = passed ? levelStars(course, level.index) : null;
  const nextRef = useRef<HTMLButtonElement>(null);
  const failedLatest = !passed && latest && !review;
  // Nouvel essai demandé : en file (« pending », d'abord), puis écrit.
  const remedyBusy =
    level.remedy === "pending" || level.remedy === "generating";
  const canSkip = failedLatest && level.failedAttempts >= 2;
  const nextLabel =
    next?.kind === "final" || next?.kind === "finalWait"
      ? t("study.result.finalReview")
      : passed || review
        ? t("study.result.nextLevel")
        : t("study.result.seeNewExplanation");

  // Nouvel essai arrivé pendant qu'on attendait : son bouton prend le focus.
  const newer = level.attempts.length;
  useEffect(() => {
    if (!latest && document.activeElement === document.body) {
      nextRef.current?.focus();
    }
  }, [newer, latest]);

  const remedy = () =>
    action.run(async () => {
      await studyRemedy(course.id, level.index);
      storeRemedyRequested(queryClient, course.id, level.index);
    });
  const skip = () =>
    action.run(
      () => studySkip(course.id, level.index),
      // Suite ouverte tout de suite, même encore verrouillée dans le cours
      // lu (statuts relus juste après) : jamais l'introduction entre les deux.
      () =>
        go(
          level.index + 1 < course.levels.length
            ? levelStartKey(level.index + 1)
            : finalKey(0),
          true,
        ),
    );

  useCardKeys({
    onEnter: next
      ? () => go(next.key)
      : failedLatest && !remedyBusy && level.remedy !== "failed"
        ? remedy
        : undefined,
  });

  const missed = missedQuestions(course).filter((item) =>
    level.attempts[attempt]?.questions.some((q) => q.id === item.question.id),
  );

  return (
    <CardShell
      testId="study-result"
      eyebrow={levelEyebrow(t, course, level.index)}
      title={
        passed
          ? t("study.result.passed")
          : review && latest
            ? t("study.result.review")
            : t("study.result.notYet")
      }
      actions={
        <>
          {next && (
            <button
              ref={nextRef}
              type="button"
              className={PRIMARY_BUTTON}
              aria-keyshortcuts="Enter"
              onClick={() => go(next.key)}
            >
              {nextLabel}
            </button>
          )}
          {failedLatest && !remedyBusy && level.remedy !== "failed" && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              aria-keyshortcuts="Enter"
              disabled={action.pending}
              onClick={remedy}
            >
              {t("study.result.explainDifferently")}
            </button>
          )}
          {!passed && (
            <button
              type="button"
              className={BUTTON}
              onClick={() => go(levelStartKey(level.index))}
            >
              {t("study.result.reviewLesson")}
            </button>
          )}
          {canSkip && (
            <button
              type="button"
              className={QUIET_BUTTON}
              disabled={action.pending}
              onClick={skip}
            >
              {t("study.result.continueAnyway")}
            </button>
          )}
        </>
      }
      hint={next && <KeyHint k="study.keys.continue" />}
    >
      {passed && <Confetti />}
      <div
        aria-live="polite"
        className={`flex flex-col items-center gap-2 rounded-2xl px-4 py-5 text-center ${
          passed
            ? "bg-emerald-50 dark:bg-emerald-950/30"
            : "bg-neutral-50 dark:bg-neutral-900/60"
        }`}
      >
        {stars !== null && <Stars count={stars} size="lg" animate />}
        <p
          data-testid="study-score"
          className="font-rounded text-3xl font-semibold text-neutral-900 tabular-nums dark:text-white"
        >
          {t("study.result.score", {
            correct: score.correct,
            total: score.total,
          })}
        </p>
        <p className="text-sm text-neutral-600 dark:text-neutral-300">
          {passed
            ? t("study.result.passedText")
            : review
              ? t("study.result.reviewText")
              : t("study.result.passMark", { count: passMark })}
        </p>
      </div>
      {remedyBusy && latest && (
        <div className="mt-4">
          <Preparing text={t("study.result.preparingRemedy")} />
        </div>
      )}
      {level.remedy === "failed" && level.remedyError && latest && (
        <div className="mt-4">
          <StudyErrorNotice courseId={course.id} error={level.remedyError} />
        </div>
      )}
      {action.error && (
        <p
          role="alert"
          className="mt-3 rounded-lg bg-red-50 px-3 py-2 text-sm break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
        >
          {action.error}
        </p>
      )}
      {!passed && missed.length > 0 && (
        <div className="mt-5">
          <h4 className="mb-2 text-sm font-semibold text-neutral-900 dark:text-white">
            {t("study.result.mistakes")}
          </h4>
          <MissedList items={missed} />
        </div>
      )}
      {canSkip && (
        <p className="mt-4 text-xs text-neutral-500 dark:text-neutral-400">
          {t("study.result.skipHint")}
        </p>
      )}
    </CardShell>
  );
}

/** Questions manquées : énoncé, réponse donnée, attendue, explication. */
function MissedList({ items }: { items: MissedQuestion[] }) {
  const t = useT();
  return (
    <ul className="flex flex-col gap-3" data-testid="study-missed">
      {items.map(({ question, answer }) => {
        const prompt = questionPrompt(question);
        const given = expectedText(t, question, answer.given);
        return (
          <li
            key={question.id}
            className="rounded-xl border border-neutral-200 px-4 py-3 text-sm dark:border-neutral-700"
          >
            <div className="font-medium text-neutral-900 dark:text-white">
              <InlineMarkdown content={prompt} />
            </div>
            {given !== null && (
              <p className="mt-1 flex items-center gap-1.5 text-red-700 dark:text-red-300">
                <XMarkIcon aria-hidden="true" className="h-4 w-4 flex-none" />
                <span>
                  {t("study.result.given")} <InlineMarkdown content={given} />
                </span>
              </p>
            )}
            <div className="mt-1 text-emerald-800 dark:text-emerald-300">
              <ExpectedDetail question={question} value={answer.expected} />
            </div>
            {answer.explanation && (
              <div className="mt-2 text-neutral-700 dark:text-neutral-300">
                <Markdown content={answer.explanation} />
              </div>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** Temps passé, en minutes (une au moins). */
function minutesOf(elapsedMs: number) {
  return Math.max(1, Math.round(elapsedMs / 60000));
}

/** Fin du cours. */
export function EndCard({
  course,
  elapsedMs,
}: {
  course: Course;
  elapsedMs: number;
}) {
  const t = useT();
  const final = finalScore(course);
  const plan = course.plan;
  const missed = missedQuestions(course);
  const totalStars = course.levels.reduce(
    (sum, level) => sum + (levelStars(course, level.index) ?? 0),
    0,
  );
  return (
    <CardShell
      testId="study-end"
      eyebrow={plan?.title ?? course.topic}
      title={t("study.end.title")}
    >
      <Confetti pieces={36} />
      <div className="flex flex-col items-center gap-2 rounded-2xl bg-amber-50 px-4 py-6 text-center dark:bg-amber-950/30">
        <TrophyIcon
          aria-hidden="true"
          className="h-12 w-12 text-amber-500 motion-safe:animate-study-pop"
        />
        {/* Une phrase (« 2 / 3 right answers in the final review »), le
            score en grand au-dessus de sa légende. */}
        <p className="text-sm text-neutral-600 dark:text-neutral-300">
          <Trans
            k="study.end.finalScore"
            params={{ count: final.correct, total: final.total }}
            tags={{
              score: (chunks) => (
                <span
                  data-testid="study-final-score"
                  className="mb-1 block font-rounded text-3xl font-semibold text-neutral-900 tabular-nums dark:text-white"
                >
                  {chunks}
                </span>
              ),
            }}
          />
        </p>
        <p className="mt-1 flex flex-wrap items-center justify-center gap-x-4 gap-y-1 text-sm text-neutral-600 dark:text-neutral-300">
          <span data-testid="study-total-stars">
            {t("study.end.stars", {
              count: totalStars,
              total: course.levels.length * 3,
            })}
          </span>
          <span className="flex items-center gap-1">
            <ClockIcon aria-hidden="true" className="h-4 w-4" />
            {t("study.end.time", { count: minutesOf(elapsedMs) })}
          </span>
        </p>
      </div>

      <h4 className="mt-6 text-sm font-semibold text-neutral-900 dark:text-white">
        {t("study.end.levels")}
      </h4>
      <ul className="mt-2 flex flex-col gap-1.5">
        {course.levels.map((level) => (
          <LevelSummary key={level.index} course={course} level={level} />
        ))}
      </ul>

      <KeyPoints course={course} />

      {missed.length > 0 && (
        <div className="mt-6">
          <h4 className="mb-2 text-sm font-semibold text-neutral-900 dark:text-white">
            {t("study.end.missed")}
          </h4>
          <MissedList items={missed} />
        </div>
      )}

      {plan && plan.further.length > 0 && (
        <div className="mt-6">
          <h4 className="text-sm font-semibold text-neutral-900 dark:text-white">
            {t("study.end.further")}
          </h4>
          <ul className="mt-2 flex flex-wrap gap-2">
            {plan.further.map((topic) => (
              <li key={topic}>
                <Link
                  to="/study"
                  search={{
                    topic,
                    level: course.level,
                    minutes: course.minutes as 10 | 20 | 30,
                  }}
                  className={BUTTON}
                  data-testid="study-further"
                >
                  {topic}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
      <Sources sources={course.sources} />
    </CardShell>
  );
}

function LevelSummary({ course, level }: { course: Course; level: Level }) {
  const t = useT();
  const stars = levelStars(course, level.index);
  const best = bestScore(course, level.index);
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-xl bg-neutral-50 px-3 py-2 text-sm dark:bg-neutral-900/60">
      <span className="min-w-0 flex-1 truncate text-neutral-900 dark:text-white">
        {course.plan?.levels[level.index]?.title ?? ""}
      </span>
      {stars !== null && <Stars count={stars} />}
      <span
        className={
          level.status === "review"
            ? "text-amber-700 dark:text-amber-400"
            : "text-emerald-700 dark:text-emerald-400"
        }
      >
        {level.status === "review"
          ? t("study.path.review")
          : t("study.path.passed")}
      </span>
      {best && (
        <span className="text-neutral-500 tabular-nums dark:text-neutral-400">
          {t("study.end.best", { correct: best.correct, total: best.total })}
        </span>
      )}
    </li>
  );
}

/** Fiche : « Key points » de tous les paliers, Copy et Save as Markdown. */
function KeyPoints({ course }: { course: Course }) {
  const t = useT();
  const notes = useQuery({
    queryKey: ["study", "notes", course.id],
    queryFn: () => studyNotes(course.id),
  });
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  const [save, setSave] = useState<"idle" | "saved" | "failed">("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  useEffect(() => {
    if (copy === "idle") return;
    const timer = setTimeout(() => setCopy("idle"), 2000);
    return () => clearTimeout(timer);
  }, [copy]);
  return (
    <div className="mt-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-sm font-semibold text-neutral-900 dark:text-white">
          {t("study.end.keyPoints")}
        </h4>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            className={BUTTON}
            disabled={notes.data === undefined}
            onClick={() =>
              // Dans le clic : WebKitGTK refuse l'écriture loin du geste.
              copyText(notes.data ?? "").then(
                () => setCopy("copied"),
                () => setCopy("failed"),
              )
            }
          >
            {copy === "copied" ? (
              <CheckIcon aria-hidden="true" className="h-4 w-4" />
            ) : copy === "failed" ? (
              <XMarkIcon aria-hidden="true" className="h-4 w-4" />
            ) : (
              <Square2StackIcon aria-hidden="true" className="h-4 w-4" />
            )}
            <span aria-live="polite">
              {copy === "copied"
                ? t("common.actions.copied")
                : copy === "failed"
                  ? t("study.end.copyFailed")
                  : t("common.actions.copy")}
            </span>
          </button>
          <button
            type="button"
            className={BUTTON}
            onClick={() => {
              setSaveError(null);
              studySaveNotes(course.id).then(
                (saved) => setSave(saved ? "saved" : "idle"),
                (cause: unknown) => {
                  setSave("failed");
                  setSaveError(errorMessage(cause));
                },
              );
            }}
          >
            {save === "saved" ? (
              <CheckCircleIcon aria-hidden="true" className="h-4 w-4" />
            ) : (
              <ArrowDownTrayIcon aria-hidden="true" className="h-4 w-4" />
            )}
            <span aria-live="polite">
              {save === "saved" ? t("study.end.saved") : t("study.end.save")}
            </span>
          </button>
        </div>
      </div>
      {notes.isError && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {errorMessage(notes.error)}
        </p>
      )}
      {saveError && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {saveError}
        </p>
      )}
      <div className="mt-3 flex flex-col gap-3">
        {course.levels.map((level) =>
          level.lesson ? (
            <section
              key={level.index}
              className="rounded-xl border border-neutral-200 px-4 py-3 dark:border-neutral-700"
            >
              <h5 className="text-sm font-medium text-neutral-900 dark:text-white">
                {course.plan?.levels[level.index]?.title ?? ""}
              </h5>
              <ul className="mt-1 list-[disc] pl-5 text-sm">
                {level.lesson.keyPoints.map((point, index) => (
                  <li key={index}>
                    <InlineMarkdown content={point} />
                  </li>
                ))}
              </ul>
            </section>
          ) : null,
        )}
      </div>
    </div>
  );
}
