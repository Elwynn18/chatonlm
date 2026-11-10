// Erreur d'une partie d'un cours (onglet Study) avec l'action qui la règle.

import { Link } from "@tanstack/react-router";
import { useId } from "react";
import type { StudyError } from "@/bindings/StudyError";
import { StudyModelSelect } from "@/components/study/StudyHome";
import { LINK_BUTTON } from "@/components/study/ui";
import { useCourseAction } from "@/hooks/useStudy";
import { useT } from "@/i18n";
import { studyRetry } from "@/lib/ipc";

/**
 * Erreur d'une partie du cours (plan, palier, nouvel essai, bilan), avec
 * l'action qui la règle : recherche web impossible (`webSearch`) : « Retry »
 * (avec le web) ou cours sans le web ; modèle à choisir et ModelHub
 * (`noModel`, `modelNotFound`) ; sinon « Retry ». Le message vient du
 * moteur, déjà traduit.
 */
export function StudyErrorNotice({
  courseId,
  error,
}: {
  courseId: string;
  error: StudyError;
}) {
  const t = useT();
  const modelId = useId();
  const action = useCourseAction(courseId);
  const modelProblem =
    error.code === "noModel" || error.code === "modelNotFound";
  return (
    <div
      role="alert"
      data-testid="study-error"
      className="rounded-xl bg-red-50 px-4 py-3 text-sm/6 break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
    >
      <p>{error.message}</p>
      <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-2">
        {modelProblem && (
          <>
            <StudyModelSelect id={modelId} />
            <Link to="/hub" className={LINK_BUTTON}>
              {t("study.error.openHub")}
            </Link>
          </>
        )}
        <button
          type="button"
          className={LINK_BUTTON}
          disabled={action.pending}
          onClick={() => action.run(() => studyRetry(courseId))}
        >
          {t("study.error.retry")}
        </button>
        {error.code === "webSearch" && (
          <button
            type="button"
            className={LINK_BUTTON}
            disabled={action.pending}
            onClick={() => action.run(() => studyRetry(courseId, true))}
          >
            {t("study.error.withoutWeb")}
          </button>
        )}
      </div>
      {action.error && <p className="mt-2">{action.error}</p>}
    </div>
  );
}
