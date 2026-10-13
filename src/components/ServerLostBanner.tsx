import { useState } from "react";
import {
  BANNER_BOX as BOX,
  BANNER_ERROR as ERROR,
  BANNER_PRIMARY as PRIMARY,
  BANNER_SECONDARY as SECONDARY,
} from "@/components/bannerStyles";
import { useT } from "@/i18n";
import { errorMessage, openBuiltinLog } from "@/lib/ipc";

/**
 * Ollama ne répond plus pendant la session : bandeau au-dessus de la zone de
 * saisie (désactivée), avec Retry. La vérification automatique continue ; le
 * bandeau disparaît dès qu'Ollama répond. Pendant que l'Ollama intégré
 * démarre (`starting`), il le dit, sans Retry. L'intégré en échec
 * (`failed`) : il le dit, avec son journal ; Retry le relance (`onRetry`
 * rejette si la relance échoue).
 */
export function ServerLostBanner({
  onRetry,
  starting = false,
  failed = false,
}: {
  onRetry: () => Promise<unknown>;
  starting?: boolean;
  failed?: boolean;
}) {
  const t = useT();
  const [retrying, setRetrying] = useState(false);
  // Échec de la relance de l'intégré, et de l'ouverture de son journal.
  const [retryError, setRetryError] = useState<string | null>(null);
  const [logError, setLogError] = useState<string | null>(null);

  const retry = async () => {
    setRetrying(true);
    setRetryError(null);
    setLogError(null);
    try {
      await onRetry();
    } catch (cause) {
      setRetryError(errorMessage(cause));
    } finally {
      setRetrying(false);
    }
  };

  const openLog = async () => {
    setLogError(null);
    try {
      await openBuiltinLog();
    } catch (cause) {
      setLogError(errorMessage(cause));
    }
  };

  if (starting) {
    return (
      <div role="status" className={`${BOX} flex items-center gap-3`}>
        {t("chat.advice.builtinStarting.title")}
      </div>
    );
  }

  return (
    <div role="alert" className={BOX}>
      <div className="flex items-center justify-between gap-3">
        <span>
          {failed ? t("chat.server.builtinFailed") : t("chat.server.lost")}
        </span>
        <div className="flex flex-none items-center gap-2">
          {failed && (
            <button
              type="button"
              onClick={() => void openLog()}
              className={SECONDARY}
            >
              {t("settings.ollama.openLog")}
            </button>
          )}
          <button
            type="button"
            onClick={() => void retry()}
            disabled={retrying}
            className={PRIMARY}
          >
            {retrying ? t("chat.actions.checking") : t("common.actions.retry")}
          </button>
        </div>
      </div>
      {retryError && (
        <p className={ERROR}>
          {t("chat.server.startFailed", { message: retryError })}
        </p>
      )}
      {logError && (
        <p className={ERROR}>
          {t("settings.ollama.openLogError", { message: logError })}
        </p>
      )}
    </div>
  );
}
