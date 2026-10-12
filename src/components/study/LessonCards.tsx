// Cartes de leçon du lecteur de cours (onglet Study) : introduction,
// sections (accroche du tuteur en tête de la première, « I don't
// understand » sous chacune), exemple résolu (« Try it yourself first » : le
// problème, puis la solution révélée étape par étape, ou d'un coup), « À
// retenir » (cartes révélées une à une, puis le piège fréquent), explication
// autrement d'un nouvel essai, attentes (palier ou bilan en préparation, ou
// en échec). Entrée : action principale (étape suivante, puis « Continue »).

import {
  ArrowPathIcon,
  CheckCircleIcon,
  ExclamationTriangleIcon,
  LightBulbIcon,
} from "@heroicons/react/20/solid";
import { useState } from "react";
import type { Course } from "@/bindings/Course";
import type { Level } from "@/bindings/Level";
import { StudyErrorNotice } from "@/components/study/StudyErrorNotice";
import { CardShell, KeyHint } from "@/components/study/cardKit";
import { ExplainBlock } from "@/components/study/Tutor";
import {
  BUTTON,
  Markdown,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
  TutorBubble,
} from "@/components/study/ui";
import { levelEyebrow } from "@/components/study/texts";
import { useCardKeys } from "@/hooks/useCardKeys";
import { useT } from "@/i18n";
import { roundMinutes, secondsLeft, solutionSteps } from "@/lib/study";

/** Bouton « Continue » d'une carte (Entrée aussi). */
export function ContinueButton({
  onClick,
  label,
  primary = true,
}: {
  onClick: () => void;
  label?: string;
  primary?: boolean;
}) {
  const t = useT();
  return (
    <button
      type="button"
      className={primary ? PRIMARY_BUTTON : BUTTON}
      aria-keyshortcuts={primary ? "Enter" : undefined}
      onClick={onClick}
    >
      {label ?? t("study.card.continue")}
    </button>
  );
}

/** Introduction : objectifs, parcours, durée. */
export function IntroCard({
  course,
  onNext,
}: {
  course: Course;
  onNext: (() => void) | null;
}) {
  const t = useT();
  const plan = course.plan;
  useCardKeys({ onEnter: onNext ?? undefined });
  const started = Object.keys(course.answers).length > 0;
  return (
    <CardShell
      testId="study-intro"
      eyebrow={t("study.intro.eyebrow")}
      title={plan?.title ?? course.topic}
      actions={
        onNext && (
          <ContinueButton
            onClick={onNext}
            label={started ? t("study.intro.resume") : t("study.intro.start")}
          />
        )
      }
      hint={onNext && <KeyHint k="study.keys.continue" />}
    >
      {plan && <Markdown content={plan.summary} />}
      {plan && (
        <ul className="mt-4 space-y-1.5">
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
      )}
      <p className="mt-4 text-sm text-neutral-500 dark:text-neutral-400">
        {course.status === "finished"
          ? t("study.intro.finished")
          : t("study.intro.timeLeft", {
              count: roundMinutes(secondsLeft(course)),
            })}
      </p>
    </CardShell>
  );
}

/** Section d'une leçon (l'accroche du tuteur en tête de la première). */
export function SectionCard({
  course,
  level,
  section,
  onNext,
}: {
  course: Course;
  level: Level;
  section: number;
  onNext: (() => void) | null;
}) {
  const t = useT();
  const lesson = level.lesson;
  useCardKeys({ onEnter: onNext ?? undefined });
  if (!lesson) return null;
  const part = lesson.sections[section];
  return (
    <CardShell
      testId="study-section"
      eyebrow={levelEyebrow(t, course, level.index)}
      title={part?.heading ?? ""}
      actions={onNext && <ContinueButton onClick={onNext} />}
      hint={onNext && <KeyHint k="study.keys.continue" />}
    >
      {section === 0 && lesson.hook && (
        <div className="mb-5">
          <TutorBubble testId="study-hook">
            <Markdown content={lesson.hook} />
          </TutorBubble>
        </div>
      )}
      <p className="mb-2 text-xs text-neutral-500">
        {t("study.card.part", {
          current: section + 1,
          total: lesson.sections.length,
        })}
      </p>
      <Markdown content={part?.body ?? ""} />
      <ExplainBlock course={course} level={level} section={section} />
    </CardShell>
  );
}

/**
 * Exemple résolu : le problème d'abord (« Try it yourself first »), puis la
 * solution révélée étape par étape (« Show the next step »), ou en entier.
 */
export function ExampleCard({
  course,
  level,
  onNext,
}: {
  course: Course;
  level: Level;
  onNext: (() => void) | null;
}) {
  const t = useT();
  const [shown, setShown] = useState(0);
  const example = level.lesson?.example;
  const steps = example ? solutionSteps(example.solution) : [];
  const all = shown >= steps.length;
  const revealNext = () =>
    setShown((count) => Math.min(count + 1, steps.length));
  useCardKeys({ onEnter: all ? (onNext ?? undefined) : revealNext });
  if (!example) return null;
  return (
    <CardShell
      testId="study-example"
      eyebrow={levelEyebrow(t, course, level.index)}
      title={t("study.example.title")}
      actions={
        <>
          {!all && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              aria-keyshortcuts="Enter"
              onClick={revealNext}
            >
              {shown === 0
                ? t("study.example.show")
                : t("study.example.nextStep")}
            </button>
          )}
          {!all && steps.length > 1 && (
            <button
              type="button"
              className={QUIET_BUTTON}
              onClick={() => setShown(steps.length)}
            >
              {t("study.example.showAll")}
            </button>
          )}
          {onNext && <ContinueButton onClick={onNext} primary={all} />}
        </>
      }
      hint={<KeyHint k={all ? "study.keys.continue" : "study.keys.reveal"} />}
    >
      <div className="rounded-xl border border-dashed border-neutral-300 px-4 py-3 dark:border-neutral-600">
        <p className="mb-1 flex items-center gap-1.5 text-xs font-medium text-neutral-500 dark:text-neutral-400">
          <LightBulbIcon aria-hidden="true" className="h-3.5 w-3.5" />
          {t("study.example.tryFirst")}
        </p>
        <Markdown content={example.problem} />
      </div>
      {shown > 0 && (
        <div className="mt-4" data-testid="study-solution">
          <p className="mb-2 text-sm font-semibold text-neutral-900 dark:text-white">
            {t("study.example.solution")}
            {steps.length > 1 && (
              <span className="ml-2 font-normal text-neutral-500 dark:text-neutral-400">
                {t("study.example.stepCount", {
                  current: Math.min(shown, steps.length),
                  total: steps.length,
                })}
              </span>
            )}
          </p>
          <ol className="flex flex-col gap-2" aria-live="polite">
            {steps.slice(0, shown).map((step, index) => (
              <li
                key={index}
                data-testid="study-solution-step"
                className="rounded-xl bg-neutral-50 px-4 py-2 motion-safe:animate-study-reveal dark:bg-neutral-900/60"
              >
                <Markdown content={step} />
              </li>
            ))}
          </ol>
        </div>
      )}
    </CardShell>
  );
}

/**
 * À retenir : une carte par point, révélées une à une (clic ou Entrée),
 * puis le piège fréquent.
 */
export function RecapCard({
  course,
  level,
  onNext,
}: {
  course: Course;
  level: Level;
  onNext: (() => void) | null;
}) {
  const t = useT();
  const points = level.lesson?.keyPoints ?? [];
  const [shown, setShown] = useState(1);
  const all = shown >= points.length;
  const revealNext = () =>
    setShown((count) => Math.min(count + 1, points.length));
  useCardKeys({ onEnter: all ? (onNext ?? undefined) : revealNext });
  if (!level.lesson) return null;
  return (
    <CardShell
      testId="study-recap"
      eyebrow={levelEyebrow(t, course, level.index)}
      title={t("study.recap.title")}
      actions={
        <>
          {!all && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              aria-keyshortcuts="Enter"
              onClick={revealNext}
            >
              {t("study.recap.next")}
            </button>
          )}
          {onNext && <ContinueButton onClick={onNext} primary={all} />}
        </>
      }
      hint={<KeyHint k={all ? "study.keys.continue" : "study.keys.reveal"} />}
    >
      <ol className="grid gap-2 sm:grid-cols-2" aria-live="polite">
        {points.map((point, index) =>
          index < shown ? (
            <li
              key={index}
              data-testid="study-key-point"
              className="flex gap-2.5 rounded-xl border border-neutral-200 bg-neutral-50 px-3 py-2.5 text-sm motion-safe:animate-study-pop dark:border-neutral-700 dark:bg-neutral-900/60"
            >
              <span
                aria-hidden="true"
                className="flex h-5 w-5 flex-none items-center justify-center rounded-full bg-neutral-900 text-[11px] font-semibold text-white dark:bg-white dark:text-neutral-900"
              >
                {index + 1}
              </span>
              <div className="min-w-0 flex-1">
                <Markdown content={point} />
              </div>
            </li>
          ) : (
            <li key={index} aria-hidden="true">
              <button
                type="button"
                tabIndex={-1}
                className="h-full min-h-12 w-full rounded-xl border-2 border-dashed border-neutral-200 text-neutral-300 hover:border-neutral-300 hover:text-neutral-400 dark:border-neutral-700 dark:text-neutral-600"
                onClick={revealNext}
              >
                ?
              </button>
            </li>
          ),
        )}
      </ol>
      {all && level.lesson.pitfall && (
        <div
          data-testid="study-pitfall"
          className="mt-4 flex gap-2.5 rounded-xl bg-amber-50 px-4 py-3 text-sm text-amber-900 motion-safe:animate-study-reveal dark:bg-amber-950/40 dark:text-amber-100"
        >
          <ExclamationTriangleIcon
            aria-hidden="true"
            className="mt-0.5 h-4 w-4 flex-none text-amber-600 dark:text-amber-400"
          />
          <div className="min-w-0 flex-1">
            <p className="font-semibold">{t("study.recap.pitfall")}</p>
            <Markdown content={level.lesson.pitfall} />
          </div>
        </div>
      )}
    </CardShell>
  );
}

/** Explication autrement (nouvel essai), dite par le tuteur. */
export function RemedyCard({
  course,
  level,
  attempt,
  onNext,
}: {
  course: Course;
  level: Level;
  attempt: number;
  onNext: (() => void) | null;
}) {
  const t = useT();
  useCardKeys({ onEnter: onNext ?? undefined });
  const explanation = level.attempts[attempt]?.explanation ?? "";
  return (
    <CardShell
      testId="study-remedy"
      eyebrow={levelEyebrow(t, course, level.index)}
      title={t("study.remedy.title")}
      actions={
        onNext && (
          <ContinueButton onClick={onNext} label={t("study.remedy.try")} />
        )
      }
      hint={onNext && <KeyHint k="study.keys.continue" />}
    >
      <TutorBubble testId="study-remedy-text">
        <Markdown content={explanation} />
      </TutorBubble>
    </CardShell>
  );
}

/** Palier pas encore prêt : préparation en cours, ou échec et « Retry ». */
export function LevelWaitCard({
  course,
  level,
}: {
  course: Course;
  level: Level;
}) {
  const t = useT();
  const title = course.plan?.levels[level.index]?.title ?? "";
  return (
    <CardShell
      testId="study-level-wait"
      eyebrow={t("study.path.levelNumber", { number: level.index + 1 })}
      title={title}
    >
      {level.state === "failed" && level.error ? (
        <StudyErrorNotice courseId={course.id} error={level.error} />
      ) : (
        <Preparing text={t("study.wait.level")} />
      )}
    </CardShell>
  );
}

/** Bilan pas encore prêt. */
export function FinalWaitCard({ course }: { course: Course }) {
  const t = useT();
  const quiz = course.finalQuiz;
  return (
    <CardShell
      testId="study-final-wait"
      eyebrow={t("study.final.eyebrow")}
      title={t("study.final.title")}
    >
      {quiz?.state === "failed" && quiz.error ? (
        <StudyErrorNotice courseId={course.id} error={quiz.error} />
      ) : (
        <Preparing text={t("study.wait.final")} />
      )}
    </CardShell>
  );
}

/** Le tuteur prépare la suite. */
export function Preparing({ text }: { text: string }) {
  return (
    <p
      role="status"
      aria-busy="true"
      className="flex items-center gap-2 text-sm text-neutral-600 dark:text-neutral-300"
    >
      <ArrowPathIcon
        aria-hidden="true"
        className="h-4 w-4 text-neutral-500 motion-safe:animate-spin"
      />
      {text}
    </p>
  );
}
