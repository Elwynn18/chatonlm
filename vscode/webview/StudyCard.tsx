// Carte du cours de l'Étude demandé depuis VS Code (lot 4a) : sujet, où en
// est la préparation, fichiers partis et ce qui a été retiré avant l'envoi,
// « Open in ChatonLM » (le cours vit dans l'onglet Study de l'app).
import { XMarkIcon } from "@heroicons/react/20/solid";
import { useT } from "@/i18n";
import type { StudyView } from "../shared/protocol";
import { blockedLabel, studyProgress } from "./labels";

const PILL =
  "cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-xs text-neutral-700 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

export function StudyCard({
  study,
  onOpen,
  onClose,
}: {
  study: StudyView;
  onOpen: () => void;
  onClose: () => void;
}) {
  const t = useT();
  return (
    <section
      aria-label={t("vscode.study.label")}
      data-testid="study"
      data-status={study.status}
      className="mx-3 mt-2 shrink-0 rounded-2xl bg-neutral-100 px-3 py-2 text-sm dark:bg-neutral-800"
    >
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-neutral-900 dark:text-white">
            {t("vscode.study.title", { topic: study.topic })}
          </p>
          <p
            role="status"
            data-testid="study-progress"
            className={
              study.status === "failed"
                ? "text-xs text-red-600 dark:text-red-400"
                : "text-xs text-neutral-600 dark:text-neutral-300"
            }
          >
            {studyProgress(t, study)}
          </p>
        </div>
        <button
          type="button"
          aria-label={t("vscode.study.close")}
          title={t("vscode.study.close")}
          onClick={onClose}
          className="flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-700 hover:bg-neutral-200 dark:text-neutral-200 dark:hover:bg-neutral-700/75"
        >
          <XMarkIcon className="h-4 w-4" />
        </button>
      </div>
      {study.files.length > 0 && (
        <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-300">
          {t("vscode.study.files", { files: study.files.join(", ") })}
        </p>
      )}
      {study.files.length === 0 && study.kind === "project" && (
        <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-300">
          {t("vscode.study.noFiles")}
        </p>
      )}
      {study.blocked.length > 0 && (
        <ul
          aria-label={t("vscode.study.blocked")}
          className="mt-1 text-xs text-neutral-600 dark:text-neutral-300"
        >
          {study.blocked.map((blocked) => (
            <li key={`${blocked.reason}:${blocked.label}`}>
              {blockedLabel(t, blocked)}
            </li>
          ))}
        </ul>
      )}
      {study.courseId && (
        <div className="mt-2">
          <button type="button" className={PILL} onClick={onOpen}>
            {t("vscode.study.open")}
          </button>
        </div>
      )}
    </section>
  );
}
