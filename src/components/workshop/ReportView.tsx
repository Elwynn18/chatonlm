// Rapport de vérification d'un projet de l'Atelier (propre à ChatonLM), mis
// en forme et traduit par la page depuis `Report` : comment le livrable a
// été produit, sa longueur, les vérifications d'ensemble, ce qui reste « à
// revoir », les affirmations douteuses, les signalements, ce qui n'a PAS été
// vérifié, les avertissements du projet, la relecture de la langue, puis les
// parties. Jamais de « vérifié » qui n'en est pas un : chaque point dit sa
// méthode. Boutons « Open the deliverable » et « Open the folder ».

import { DocumentTextIcon, FolderOpenIcon } from "@heroicons/react/20/solid";
import { type ReactNode, useId } from "react";
import type { Check } from "@/bindings/Check";
import type { Report } from "@/bindings/Report";
import { BUTTON, PRIMARY_BUTTON } from "@/components/study/ui";
import { CheckRow } from "@/components/workshop/PartPanel";
import { PartMark } from "@/components/workshop/ui";
import { useT } from "@/i18n";
import {
  isDoubtful,
  isSignal,
  isToReview,
  notCheckedKey,
  warningKey,
  wordsText,
} from "@/lib/workshop";

export function ReportView({
  report,
  criteria,
  onOpenFile,
  onOpenFolder,
}: {
  report: Report;
  /** Critères de la spécification (identifiant → texte). */
  criteria: ReadonlyMap<string, string>;
  onOpenFile: () => void;
  onOpenFolder: () => void;
}) {
  const t = useT();
  const headingId = useId();
  const all: Check[] = [
    ...report.checks,
    ...report.bricks.flatMap((brick) => brick.checks),
  ];
  const toReview = all.filter(isToReview);
  const doubtful = all.filter(isDoubtful);
  const signals = all.filter(isSignal);
  // Un même avertissement noté plusieurs fois n'est dit qu'une fois.
  const warnings = [
    ...new Set(report.warnings.map((code) => warningKey(code))),
  ];
  const notChecked = [...new Set(report.notChecked.map(notCheckedKey))];
  return (
    <section
      aria-labelledby={headingId}
      data-testid="workshop-report"
      className="flex flex-col gap-4 rounded-2xl border border-neutral-200 bg-white p-5 dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h3
            id={headingId}
            className="font-rounded text-lg font-medium text-neutral-900 dark:text-white"
          >
            {t("workshop.report.title")}
          </h3>
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            {t(`workshop.report.route.${report.route}`)}{" "}
            {report.targetWords !== null
              ? t("workshop.report.wordsTarget", {
                  words: wordsText(t, report.words),
                  target: wordsText(t, report.targetWords),
                })
              : t("workshop.report.words", {
                  words: wordsText(t, report.words),
                })}
          </p>
          <p
            data-testid="workshop-report-file"
            className="mt-1 font-mono text-xs break-all text-neutral-600 dark:text-neutral-300"
          >
            {t("workshop.report.file", { file: report.file })}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            className={PRIMARY_BUTTON}
            onClick={onOpenFile}
            data-testid="workshop-report-open-file"
          >
            <DocumentTextIcon aria-hidden="true" className="h-4 w-4" />
            {t("workshop.report.openFile")}
          </button>
          <button
            type="button"
            className={BUTTON}
            onClick={onOpenFolder}
            data-testid="workshop-report-open"
          >
            <FolderOpenIcon aria-hidden="true" className="h-4 w-4" />
            {t("workshop.report.openFolder")}
          </button>
        </div>
      </div>

      <Block
        title={t("workshop.report.criteria")}
        testId="workshop-report-checks"
      >
        {report.checks.length === 0 ? (
          // Rien de noté : ni « tout est tenu », ni le contraire.
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            {t("workshop.report.noChecks")}
          </p>
        ) : report.checks.every((check) => check.passed) ? (
          <p className="text-sm text-green-800 dark:text-green-400">
            {t("workshop.report.allGood")}
          </p>
        ) : null}
        {report.checks.length > 0 && (
          <ul className="flex flex-col gap-1.5">
            {report.checks.map((check, index) => (
              <CheckRow key={index} check={check} criteria={criteria} />
            ))}
          </ul>
        )}
        {report.judgedOnOutline && (
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            {t("workshop.report.outline")}
          </p>
        )}
      </Block>

      {(toReview.length > 0 || report.toReview > 0) && (
        <Block
          title={t("workshop.report.toReviewCount", {
            count: Math.max(report.toReview, toReview.length),
          })}
          testId="workshop-report-review"
        >
          <ul className="flex flex-col gap-1.5">
            {toReview.map((check, index) => (
              <CheckRow key={index} check={check} criteria={criteria} />
            ))}
          </ul>
        </Block>
      )}

      {(doubtful.length > 0 || report.doubtful > 0) && (
        <Block
          title={t("workshop.report.doubtfulCount", {
            count: Math.max(report.doubtful, doubtful.length),
          })}
          testId="workshop-report-doubtful"
        >
          <ul className="flex flex-col gap-1.5">
            {doubtful.map((check, index) => (
              <CheckRow key={index} check={check} criteria={criteria} />
            ))}
          </ul>
        </Block>
      )}

      {signals.length > 0 && (
        <Block
          title={t("workshop.report.signals")}
          testId="workshop-report-signals"
        >
          <ul className="flex flex-col gap-1.5">
            {signals.map((check, index) => (
              <CheckRow key={index} check={check} criteria={criteria} />
            ))}
          </ul>
        </Block>
      )}

      {notChecked.length > 0 && (
        <Block
          title={t("workshop.report.notChecked")}
          testId="workshop-report-not-checked"
        >
          <ul className="flex flex-col gap-1 text-sm text-neutral-700 dark:text-neutral-200">
            {notChecked.map((key) => (
              <li key={key}>{t(key)}</li>
            ))}
          </ul>
        </Block>
      )}

      {warnings.length > 0 && (
        <Block
          title={t("workshop.report.warnings")}
          testId="workshop-report-warnings"
        >
          <ul className="flex flex-col gap-1 text-sm text-neutral-700 dark:text-neutral-200">
            {warnings.map((key) => (
              <li key={key}>{t(key)}</li>
            ))}
          </ul>
        </Block>
      )}

      {report.proofreadBlocks > 0 && (
        <p
          data-testid="workshop-report-proofread"
          className="text-sm text-neutral-600 dark:text-neutral-300"
        >
          {t("workshop.report.proofread", {
            blocks: report.proofreadBlocks,
            changed: report.proofreadChanged,
            rejected: report.proofreadRejected,
          })}
        </p>
      )}

      {report.bricks.length > 0 && (
        <Block
          title={t("workshop.report.parts")}
          testId="workshop-report-parts"
        >
          <ul className="flex flex-col gap-1">
            {report.bricks.map((brick) => (
              <li
                key={brick.id}
                data-testid="workshop-report-part"
                data-state={brick.state}
                className="flex items-center gap-2 text-sm"
              >
                <PartMark state={brick.state} />
                <span className="min-w-0 flex-1 truncate text-neutral-800 dark:text-neutral-100">
                  {brick.id === 0 ? t("workshop.plan.direct") : brick.title}
                </span>
                <span className="flex-none text-xs text-neutral-500 dark:text-neutral-400">
                  {t(`workshop.partStates.${brick.state}`)}
                  {brick.fixed && ` · ${t("workshop.report.fixed")}`}
                  {` · ${wordsText(t, brick.words)}`}
                </span>
              </li>
            ))}
          </ul>
        </Block>
      )}
    </section>
  );
}

function Block({
  title,
  testId,
  children,
}: {
  title: string;
  testId: string;
  children: ReactNode;
}) {
  return (
    <div data-testid={testId} className="flex flex-col gap-1.5">
      <h4 className="text-sm font-medium text-neutral-700 dark:text-neutral-300">
        {title}
      </h4>
      {children}
    </div>
  );
}
