// « I need you » d'un projet de l'Atelier (propre à ChatonLM) : bandeau
// (annoncé : `role="alert"`) et panneau de la question en attente. Choix de
// la stratégie (votes partagés), questions de cadrage (une réponse libre par
// question ; vide : une hypothèse écrite), approbation du plan (titres, buts,
// mots et ordre modifiables ; approuver, ou refuser avec une note qui dit
// quoi changer), approbation d'une action. Réponse par `workshop_answer`.
// Plan incomplet (titre vide, partie sans titre ou sans mots) : « Approve »
// reste atteignable au clavier (`aria-disabled`), un message dit quoi
// compléter et le champ fautif est marqué ; une note tapée ne part qu'avec
// un refus, ce qui est dit près du bouton.
// Les questions et le plan sont des contenus du modèle, montrés comme tels.

import { ArrowDownIcon, ArrowUpIcon } from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useId, useState } from "react";
import type { PendingQuestion } from "@/bindings/PendingQuestion";
import type { Strategy } from "@/bindings/Strategy";
import type { WorkshopAnswer } from "@/bindings/WorkshopAnswer";
import type { WorkshopPlan } from "@/bindings/WorkshopPlan";
import { Logo } from "@/components/Logo";
import { BUTTON, PRIMARY_BUTTON } from "@/components/study/ui";
import {
  workshopListKey,
  workshopProjectKey,
} from "@/components/workshop/queries";
import { useT } from "@/i18n";
import { errorMessage, workshopAnswer } from "@/lib/ipc";
import {
  moveBrick,
  type PlanEdit,
  planChanged,
  wordsText,
} from "@/lib/workshop";

const FIELD =
  "w-full rounded-lg border border-neutral-300 bg-white px-2 py-1 text-sm text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:placeholder:text-neutral-500";
/** Bouton principal qui reste focalisable quand il est refusé. */
const PRIMARY_ARIA = `${PRIMARY_BUTTON} aria-disabled:cursor-default aria-disabled:opacity-50`;
const ICON_BUTTON =
  "rounded-full p-1 text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-40 dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-white";

export function NeedYou({
  projectId,
  question,
}: {
  projectId: number;
  question: PendingQuestion;
}) {
  const t = useT();
  const client = useQueryClient();
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const send = (answer: WorkshopAnswer) => {
    setSending(true);
    setError(null);
    workshopAnswer(projectId, question.id, answer).then(
      () => {
        void client.invalidateQueries({
          queryKey: workshopProjectKey(projectId),
        });
        void client.invalidateQueries({ queryKey: workshopListKey });
      },
      (cause: unknown) => {
        setSending(false);
        setError(
          t("workshop.needYou.sendError", { message: errorMessage(cause) }),
        );
      },
    );
  };

  const body = question.question;
  return (
    <section
      data-testid="workshop-need-you"
      data-question={body.type}
      className="rounded-2xl border-2 border-accent-fg bg-white p-4 dark:bg-neutral-800/60"
    >
      <div role="alert" className="mb-3 flex items-center gap-2">
        <Logo size={28} className="flex-none" />
        <p className="font-rounded text-base font-semibold text-neutral-900 dark:text-white">
          {t("workshop.needYou.banner")}
        </p>
      </div>
      {body.type === "strategy" && (
        <StrategyChoice
          strategies={body.value}
          sending={sending}
          onSend={send}
        />
      )}
      {body.type === "framing" && (
        <Framing questions={body.value} sending={sending} onSend={send} />
      )}
      {body.type === "approvePlan" && (
        <PlanApproval plan={body.value} sending={sending} onSend={send} />
      )}
      {body.type === "approveAction" && (
        <ActionApproval
          tool={body.value.tool}
          target={body.value.target}
          sending={sending}
          onSend={send}
        />
      )}
      {error && (
        <p
          role="alert"
          data-testid="workshop-answer-error"
          className="mt-3 text-sm text-red-600 dark:text-red-400"
        >
          {error}
        </p>
      )}
    </section>
  );
}

type Send = (answer: WorkshopAnswer) => void;

function Title({ children, hint }: { children: ReactNode; hint: string }) {
  return (
    <div className="mb-3">
      <h3 className="font-medium text-neutral-900 dark:text-white">
        {children}
      </h3>
      <p className="text-sm text-neutral-600 dark:text-neutral-300">{hint}</p>
    </div>
  );
}

function StrategyChoice({
  strategies,
  sending,
  onSend,
}: {
  strategies: Strategy[];
  sending: boolean;
  onSend: Send;
}) {
  const t = useT();
  const name = useId();
  const [choice, setChoice] = useState<Strategy | null>(strategies[0] ?? null);
  return (
    <form
      data-testid="workshop-question-strategy"
      onSubmit={(event) => {
        event.preventDefault();
        if (choice) onSend({ type: "strategy", value: choice });
      }}
    >
      <Title hint={t("workshop.needYou.strategy.hint")}>
        {t("workshop.needYou.strategy.title")}
      </Title>
      <fieldset className="mb-3 flex flex-col gap-1.5">
        <legend className="sr-only">
          {t("workshop.needYou.strategy.title")}
        </legend>
        {strategies.map((strategy) => (
          <label
            key={strategy}
            className="flex items-center gap-2 text-sm text-neutral-800 dark:text-neutral-100"
          >
            <input
              type="radio"
              name={name}
              value={strategy}
              checked={choice === strategy}
              onChange={() => setChoice(strategy)}
              className="h-4 w-4 accent-accent-fg"
            />
            {t(`workshop.strategies.${strategy}`)}
          </label>
        ))}
      </fieldset>
      <button
        type="submit"
        className={PRIMARY_BUTTON}
        disabled={sending || choice === null}
        data-testid="workshop-answer-strategy"
      >
        {t("workshop.needYou.strategy.submit")}
      </button>
    </form>
  );
}

function Framing({
  questions,
  sending,
  onSend,
}: {
  questions: string[];
  sending: boolean;
  onSend: Send;
}) {
  const t = useT();
  const baseId = useId();
  const [answers, setAnswers] = useState<string[]>(() =>
    questions.map(() => ""),
  );
  return (
    <form
      data-testid="workshop-question-framing"
      onSubmit={(event) => {
        event.preventDefault();
        onSend({
          type: "framing",
          value: answers.map((answer) => answer.trim()),
        });
      }}
    >
      <Title hint={t("workshop.needYou.framing.hint")}>
        {t("workshop.needYou.framing.title")}
      </Title>
      <ol className="mb-3 flex flex-col gap-3">
        {questions.map((question, index) => (
          <li key={index} className="flex flex-col gap-1">
            <label
              htmlFor={`${baseId}-${index}`}
              className="text-sm font-medium text-neutral-800 dark:text-neutral-100"
            >
              {index + 1}. {question}
            </label>
            <textarea
              id={`${baseId}-${index}`}
              rows={2}
              value={answers[index]}
              aria-description={t("workshop.needYou.framing.answer", {
                number: index + 1,
              })}
              data-testid="workshop-framing-answer"
              onChange={(event) => {
                const value = event.target.value;
                setAnswers((current) =>
                  current.map((answer, i) => (i === index ? value : answer)),
                );
              }}
              className={`${FIELD} resize-y`}
            />
          </li>
        ))}
      </ol>
      <button
        type="submit"
        className={PRIMARY_BUTTON}
        disabled={sending}
        data-testid="workshop-answer-framing"
      >
        {t("workshop.needYou.framing.submit")}
      </button>
    </form>
  );
}

function PlanApproval({
  plan,
  sending,
  onSend,
}: {
  plan: WorkshopPlan;
  sending: boolean;
  onSend: Send;
}) {
  const t = useT();
  const ids = {
    title: useId(),
    note: useId(),
    base: useId(),
    problem: useId(),
    noteIgnored: useId(),
  };
  const [edit, setEdit] = useState<PlanEdit>(() => ({
    title: plan.title,
    bricks: plan.bricks.map((brick) => ({ ...brick })),
  }));
  const [note, setNote] = useState("");
  const changed = edit.title !== plan.title || planChanged(plan, edit);
  const titleMissing = edit.title.trim() === "";
  const untitled = (brick: PlanEdit["bricks"][0]) => brick.title.trim() === "";
  const wordless = (brick: PlanEdit["bricks"][0]) => brick.words < 1;
  // Premier manque, nommé dans le message (partie : son rang, son titre).
  const problemIndex = edit.bricks.findIndex(
    (brick) => untitled(brick) || wordless(brick),
  );
  const problem = titleMissing
    ? t("workshop.needYou.plan.missingDocumentTitle")
    : problemIndex === -1
      ? null
      : untitled(edit.bricks[problemIndex])
        ? t("workshop.needYou.plan.missingPartTitle", {
            number: problemIndex + 1,
          })
        : t("workshop.needYou.plan.missingPartWords", {
            number: problemIndex + 1,
            title: edit.bricks[problemIndex].title.trim(),
          });
  const noteTyped = note.trim() !== "";
  const refused = sending || problem !== null;
  const total = edit.bricks.reduce((sum, brick) => sum + brick.words, 0);

  const update = (index: number, patch: Partial<PlanEdit["bricks"][0]>) =>
    setEdit((current) => ({
      ...current,
      bricks: current.bricks.map((brick, i) =>
        i === index ? { ...brick, ...patch } : brick,
      ),
    }));
  const move = (index: number, delta: number) =>
    setEdit((current) => ({
      ...current,
      bricks: moveBrick(current.bricks, index, delta),
    }));

  const approve = () => {
    if (refused) return;
    onSend({
      type: "plan",
      value: {
        approved: true,
        edited: changed
          ? {
              title: edit.title.trim(),
              bricks: edit.bricks.map((brick) => ({
                ...brick,
                title: brick.title.trim(),
                goal: brick.goal.trim(),
              })),
            }
          : null,
        // La note ne sert qu'à un refus : jamais envoyée avec un accord.
        note: null,
      },
    });
  };
  const refuse = () =>
    onSend({
      type: "plan",
      value: { approved: false, edited: null, note: note.trim() },
    });

  return (
    <div data-testid="workshop-question-plan">
      <Title hint={t("workshop.needYou.plan.hint")}>
        {t("workshop.needYou.plan.title")}
      </Title>
      <div className="mb-3 flex flex-col gap-1">
        <label
          htmlFor={ids.title}
          className="text-sm font-medium text-neutral-700 dark:text-neutral-300"
        >
          {t("workshop.needYou.plan.documentTitle")}
        </label>
        <input
          id={ids.title}
          type="text"
          value={edit.title}
          aria-invalid={titleMissing || undefined}
          aria-describedby={titleMissing ? ids.problem : undefined}
          data-testid="workshop-plan-document-title"
          onChange={(event) =>
            setEdit((current) => ({ ...current, title: event.target.value }))
          }
          className={FIELD}
        />
      </div>
      <ol className="flex flex-col gap-2">
        {edit.bricks.map((brick, index) => {
          const number = index + 1;
          const titleId = `${ids.base}-title-${brick.id}`;
          return (
            <li
              key={brick.id}
              data-testid="workshop-plan-part"
              data-part={brick.id}
              className="flex gap-2 rounded-xl border border-neutral-200 p-2 dark:border-neutral-700"
            >
              <span
                aria-hidden="true"
                className="w-6 flex-none pt-1 text-right text-sm text-neutral-500 tabular-nums dark:text-neutral-400"
              >
                {number}
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                <input
                  id={titleId}
                  type="text"
                  value={brick.title}
                  aria-label={t("workshop.needYou.plan.partTitle", { number })}
                  aria-invalid={untitled(brick) || undefined}
                  aria-describedby={
                    index === problemIndex && !titleMissing
                      ? ids.problem
                      : undefined
                  }
                  data-testid="workshop-plan-part-title"
                  onChange={(event) =>
                    update(index, { title: event.target.value })
                  }
                  className={`${FIELD} font-medium`}
                />
                <textarea
                  rows={2}
                  value={brick.goal}
                  aria-label={t("workshop.needYou.plan.partGoal", { number })}
                  data-testid="workshop-plan-part-goal"
                  onChange={(event) =>
                    update(index, { goal: event.target.value })
                  }
                  className={`${FIELD} resize-y`}
                />
                <div className="flex flex-wrap items-center gap-2 text-xs text-neutral-500 dark:text-neutral-400">
                  <input
                    type="number"
                    min={1}
                    step={50}
                    value={brick.words > 0 ? brick.words : ""}
                    aria-label={t("workshop.needYou.plan.partWords", {
                      number,
                    })}
                    aria-invalid={wordless(brick) || undefined}
                    aria-describedby={
                      index === problemIndex &&
                      !titleMissing &&
                      !untitled(brick)
                        ? ids.problem
                        : undefined
                    }
                    data-testid="workshop-plan-part-words"
                    onChange={(event) => {
                      const words = Number.parseInt(event.target.value, 10);
                      update(index, {
                        words: Number.isFinite(words) ? Math.max(0, words) : 0,
                      });
                    }}
                    className={`${FIELD} w-24 tabular-nums`}
                  />
                  {(brick.kind === "intro" || brick.kind === "conclusion") && (
                    <span>{t("workshop.plan.writtenLast")}</span>
                  )}
                </div>
              </div>
              <div className="flex flex-none flex-col">
                <button
                  type="button"
                  className={ICON_BUTTON}
                  disabled={index === 0}
                  aria-label={t("workshop.needYou.plan.moveUp", {
                    title: brick.title,
                  })}
                  title={t("workshop.needYou.plan.moveUp", {
                    title: brick.title,
                  })}
                  data-testid="workshop-plan-part-up"
                  onClick={() => move(index, -1)}
                >
                  <ArrowUpIcon aria-hidden="true" className="h-4 w-4" />
                </button>
                <button
                  type="button"
                  className={ICON_BUTTON}
                  disabled={index === edit.bricks.length - 1}
                  aria-label={t("workshop.needYou.plan.moveDown", {
                    title: brick.title,
                  })}
                  title={t("workshop.needYou.plan.moveDown", {
                    title: brick.title,
                  })}
                  data-testid="workshop-plan-part-down"
                  onClick={() => move(index, 1)}
                >
                  <ArrowDownIcon aria-hidden="true" className="h-4 w-4" />
                </button>
              </div>
            </li>
          );
        })}
      </ol>
      <p className="mt-2 text-sm text-neutral-600 tabular-nums dark:text-neutral-300">
        {t("workshop.needYou.plan.total", { words: wordsText(t, total) })}
      </p>
      {problem !== null && (
        <p
          id={ids.problem}
          role="alert"
          data-testid="workshop-plan-problem"
          className="mt-2 text-sm text-red-600 dark:text-red-400"
        >
          {problem}
        </p>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2">
        <button
          type="button"
          className={PRIMARY_ARIA}
          aria-disabled={refused || undefined}
          aria-describedby={
            [
              problem !== null ? ids.problem : null,
              noteTyped ? ids.noteIgnored : null,
            ]
              .filter((id) => id !== null)
              .join(" ") || undefined
          }
          data-testid="workshop-plan-approve"
          onClick={approve}
        >
          {changed
            ? t("workshop.needYou.plan.approveEdited")
            : t("workshop.needYou.plan.approve")}
        </button>
        {noteTyped && (
          <p
            id={ids.noteIgnored}
            data-testid="workshop-plan-note-ignored"
            className="text-sm text-neutral-600 dark:text-neutral-300"
          >
            {t("workshop.needYou.plan.noteIgnored")}
          </p>
        )}
      </div>
      <div className="mt-4 flex flex-col gap-1.5 border-t border-neutral-200 pt-3 dark:border-neutral-700">
        <label
          htmlFor={ids.note}
          className="text-sm font-medium text-neutral-700 dark:text-neutral-300"
        >
          {t("workshop.needYou.plan.note")}
        </label>
        <textarea
          id={ids.note}
          rows={2}
          value={note}
          placeholder={t("workshop.needYou.plan.notePlaceholder")}
          data-testid="workshop-plan-note"
          onChange={(event) => setNote(event.target.value)}
          className={`${FIELD} resize-y`}
        />
        <div>
          <button
            type="button"
            className={BUTTON}
            disabled={sending || note.trim() === ""}
            data-testid="workshop-plan-refuse"
            onClick={refuse}
          >
            {t("workshop.needYou.plan.refuse")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Outils connus d'une action à permettre (lots 4-5) ; les autres : texte commun. */
const ACTION_TOOLS = ["webSearch", "webFetch", "readFile"] as const;

function ActionApproval({
  tool,
  target,
  sending,
  onSend,
}: {
  tool: string;
  target: string;
  sending: boolean;
  onSend: Send;
}) {
  const t = useT();
  const toolText = (ACTION_TOOLS as readonly string[]).includes(tool)
    ? t(
        `workshop.needYou.action.tools.${tool as (typeof ACTION_TOOLS)[number]}`,
      )
    : t("workshop.needYou.action.tools.generic");
  return (
    <div data-testid="workshop-question-action" data-tool={tool}>
      <h3 className="mb-1 font-medium text-neutral-900 dark:text-white">
        {t("workshop.needYou.action.title")}
      </h3>
      <p className="text-sm text-neutral-700 dark:text-neutral-200">
        {toolText}
      </p>
      <p className="mt-1 mb-3 font-mono text-xs break-all text-neutral-600 dark:text-neutral-300">
        {target}
      </p>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className={PRIMARY_BUTTON}
          disabled={sending}
          data-testid="workshop-action-allow"
          onClick={() => onSend({ type: "action", value: true })}
        >
          {t("workshop.needYou.action.allow")}
        </button>
        <button
          type="button"
          className={BUTTON}
          disabled={sending}
          data-testid="workshop-action-deny"
          onClick={() => onSend({ type: "action", value: false })}
        >
          {t("workshop.needYou.action.deny")}
        </button>
      </div>
    </div>
  );
}
