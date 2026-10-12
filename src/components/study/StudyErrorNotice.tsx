// Erreur d'une partie d'un cours (onglet Study) avec l'action qui la règle.

import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import type { StudyError } from "@/bindings/StudyError";
import { StudyModelSelect } from "@/components/study/StudyHome";
import { LINK_BUTTON } from "@/components/study/ui";
import { useAccount, useSignIn } from "@/hooks/useAccount";
import { useCourseAction } from "@/hooks/useStudy";
import { useT } from "@/i18n";
import { errorMessage, studyRetry } from "@/lib/ipc";

/**
 * Erreur d'une partie du cours (plan, palier, nouvel essai, bilan), avec
 * l'action qui la règle : connexion au compte ou cours sans le web
 * (`signedOut` ; compte relié depuis : « Retry », avec le web, comme le
 * chat), modèle à choisir et ModelHub (`noModel`, `modelNotFound`), sinon
 * « Retry ». Le message vient du moteur, déjà traduit.
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
  const { isAuthenticated } = useAccount();
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
        {error.code === "signedOut" ? (
          <>
            {isAuthenticated ? (
              <>
                <span>{t("study.error.signedIn")}</span>
                <button
                  type="button"
                  className={LINK_BUTTON}
                  disabled={action.pending}
                  onClick={() => action.run(() => studyRetry(courseId))}
                >
                  {t("study.error.retry")}
                </button>
              </>
            ) : (
              <SignInButton />
            )}
            <button
              type="button"
              className={LINK_BUTTON}
              disabled={action.pending}
              onClick={() => action.run(() => studyRetry(courseId, true))}
            >
              {t("study.error.withoutWeb")}
            </button>
          </>
        ) : (
          <>
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
          </>
        )}
      </div>
      {action.error && <p className="mt-2">{action.error}</p>}
    </div>
  );
}

/** Connexion au compte ollama.com (page ouverte dans le navigateur). */
function SignInButton() {
  const t = useT();
  const { isAuthenticated, refresh } = useAccount();
  const signIn = useSignIn({ isAuthenticated, refresh });
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <button
        type="button"
        className={LINK_BUTTON}
        disabled={signIn.starting}
        onClick={() =>
          signIn
            .start()
            .catch((cause: unknown) => setError(errorMessage(cause)))
        }
      >
        {t("chat.webSearch.signIn")}
      </button>
      {error && <span>{error}</span>}
    </>
  );
}
