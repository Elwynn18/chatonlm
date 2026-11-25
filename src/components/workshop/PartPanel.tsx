// Panneau d'une partie d'un projet de l'Atelier (propre à ChatonLM) : la
// partie cliquée dans le plan, ou celle au travail. État, essai, mots écrits
// et visés, vérifications (tenues ou non, avec leur méthode : mesurée par le
// programme, jugée sur la forme, jugée sur l'exactitude ; affirmations
// douteuses à part), puis l'aperçu du texte sauvé (rendu Markdown du chat).
// Version d'un seul jet abandonnée (`directFailed`) : la partie 0 disparaît
// avant que la liste ne passe au plan ; son « introuvable » passager est tu.

import {
  CheckCircleIcon,
  ExclamationTriangleIcon,
  XCircleIcon,
} from "@heroicons/react/20/solid";
import { useId } from "react";
import type { Check } from "@/bindings/Check";
import { Markdown } from "@/components/study/ui";
import { useWorkshopPart } from "@/components/workshop/queries";
import { useT } from "@/i18n";
import type { BrickState } from "@/bindings/BrickState";
import { errorMessage, isAppError } from "@/lib/ipc";
import {
  checkDetail,
  checkSubject,
  isDoubtful,
  wordsText,
} from "@/lib/workshop";

export function PartPanel({
  projectId,
  partId,
  criteria,
}: {
  projectId: number;
  /** Partie montrée (null : aucune encore). */
  partId: number | null;
  /** Critères de la spécification (identifiant → texte). */
  criteria: ReadonlyMap<string, string>;
}) {
  const t = useT();
  const headingId = useId();
  const part = useWorkshopPart(projectId, partId);
  const data = part.data;
  // La version d'un seul jet vient d'être abandonnée : la liste des parties
  // va passer au plan, rien à signaler.
  const directGone =
    partId === 0 && isAppError(part.error) && part.error.kind === "notFound";
  return (
    <section
      aria-labelledby={headingId}
      data-testid="workshop-part-panel"
      data-part={partId ?? ""}
      className="min-w-0 rounded-2xl border border-neutral-200 bg-white p-4 dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <h3
        id={headingId}
        className="mb-2 text-xs font-medium tracking-wide text-neutral-500 uppercase dark:text-neutral-400"
      >
        {t("workshop.part.label")}
      </h3>
      {part.isError && !directGone && (
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          {t("workshop.part.loadError", { message: errorMessage(part.error) })}
        </p>
      )}
      {data && (
        <div className="flex flex-col gap-3">
          <div>
            <p
              data-testid="workshop-part-title"
              className="font-medium text-neutral-900 dark:text-white"
            >
              {data.id === 0 ? t("workshop.plan.direct") : data.title}
            </p>
            <p className="flex flex-wrap gap-x-3 text-sm text-neutral-600 dark:text-neutral-300">
              <span data-testid="workshop-part-state">
                {t(`workshop.partStates.${data.state}`)}
              </span>
              <span>
                {t("workshop.part.attempt", { number: data.attempt + 1 })}
              </span>
              <span className="tabular-nums">
                {data.targetWords !== null
                  ? t("workshop.part.wordsOf", {
                      words: data.words,
                      target: data.targetWords,
                    })
                  : wordsText(t, data.words)}
              </span>
            </p>
          </div>
          <Checks checks={data.checks} criteria={criteria} state={data.state} />
          <div>
            <h4 className="mb-1 text-sm font-medium text-neutral-700 dark:text-neutral-300">
              {t("workshop.part.preview")}
            </h4>
            {data.text.trim() === "" ? (
              <p className="text-sm text-neutral-500 dark:text-neutral-400">
                {t("workshop.part.noText")}
              </p>
            ) : (
              <div
                data-testid="workshop-part-text"
                className="max-h-96 overflow-y-auto rounded-xl border border-neutral-200 bg-neutral-50 px-3 py-2 dark:border-neutral-700 dark:bg-neutral-900"
              >
                <Markdown content={data.text} />
              </div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

/** Parties pas encore finies : leurs vérifications peuvent encore venir. */
const UNFINISHED: readonly BrickState[] = [
  "pending",
  "running",
  "checking",
  "fixing",
];

/** Vérifications d'une partie : décompte, puis une ligne par vérification. */
export function Checks({
  checks,
  criteria,
  state,
}: {
  checks: readonly Check[];
  criteria: ReadonlyMap<string, string>;
  /** État de la partie : sans vérification, une partie finie (ou refusée,
   * à revoir) n'en a pas eu de propre ; rien n'affirme qu'elle a été
   * vérifiée autrement. « Pas encore » seulement avant la fin. */
  state: BrickState;
}) {
  const t = useT();
  if (checks.length === 0) {
    return (
      <p className="text-sm text-neutral-500 dark:text-neutral-400">
        {UNFINISHED.includes(state)
          ? t("workshop.part.noChecks")
          : t("workshop.part.noOwnCheck")}
      </p>
    );
  }
  const passed = checks.filter((check) => check.passed).length;
  return (
    <div data-testid="workshop-checks">
      <p className="mb-1 text-sm font-medium text-neutral-700 dark:text-neutral-300">
        {t("workshop.part.held", { passed, count: checks.length })}
      </p>
      <ul className="flex flex-col gap-1.5">
        {checks.map((check, index) => (
          <CheckRow key={index} check={check} criteria={criteria} />
        ))}
      </ul>
    </div>
  );
}

export function CheckRow({
  check,
  criteria,
}: {
  check: Check;
  criteria: ReadonlyMap<string, string>;
}) {
  const t = useT();
  const doubtful = isDoubtful(check);
  const detail = checkDetail(t, check);
  const status = check.passed
    ? t("workshop.part.passed")
    : doubtful
      ? t("workshop.part.doubtful")
      : t("workshop.part.failed");
  return (
    <li
      data-testid="workshop-check"
      data-passed={check.passed}
      data-method={check.method}
      className="flex items-start gap-2 text-sm"
    >
      {check.passed ? (
        <CheckCircleIcon
          aria-hidden="true"
          className="mt-0.5 h-4 w-4 flex-none text-green-700 dark:text-green-400"
        />
      ) : doubtful ? (
        <ExclamationTriangleIcon
          aria-hidden="true"
          className="mt-0.5 h-4 w-4 flex-none text-accent-fg"
        />
      ) : (
        <XCircleIcon
          aria-hidden="true"
          className="mt-0.5 h-4 w-4 flex-none text-red-600 dark:text-red-400"
        />
      )}
      <span className="flex min-w-0 flex-col">
        <span className="text-neutral-800 dark:text-neutral-100">
          <span className="sr-only">{status} · </span>
          {checkSubject(t, check, criteria)}
        </span>
        <span className="text-xs text-neutral-500 dark:text-neutral-400">
          {t(`workshop.methods.${check.method}`)}
          {detail !== null && ` · ${detail}`}
        </span>
      </span>
    </li>
  );
}
