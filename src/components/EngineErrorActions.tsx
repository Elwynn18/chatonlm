// Actions sous une panne du moteur dans le chat (propre à ChatonLM ; codes
// `engineFailed` et `engineUnavailable`, remplacent le bandeau « serveur
// perdu » de l'Ollama intégré) : Retry (panne du chargement ou du
// processus, pas pour un moteur absent de l'installation), « Open logs »
// (journaux de llama.cpp, échec dit dessous) et « Engine settings » (écran
// du moteur, Settings › General : version, accélération, repli). Le texte de
// l'erreur, déjà traduit par le moteur, est au-dessus (ErrorMessage).

import { useState } from "react";
import { useT } from "@/i18n";
import type { EngineErrorKind } from "@/lib/engine";
import { errorMessage, openEngineLogs } from "@/lib/ipc";

const PRIMARY =
  "rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";
const SECONDARY =
  "rounded-full border border-neutral-300 px-3 py-1 text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

export function EngineErrorActions({
  kind,
  onOpenSettings,
  onRetry,
  retryDisabled = false,
}: {
  kind: EngineErrorKind;
  /** Ouvre l'écran du moteur (Settings › General). */
  onOpenSettings: () => void;
  /** Relance du dernier message ; absente : rien à relancer. */
  onRetry?: () => void;
  retryDisabled?: boolean;
}) {
  const t = useT();
  const [logsError, setLogsError] = useState<string | null>(null);
  const openLogs = async () => {
    setLogsError(null);
    try {
      await openEngineLogs();
    } catch (cause) {
      setLogsError(errorMessage(cause));
    }
  };
  const retry = kind === "engineFailed" ? onRetry : undefined;
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        {retry && (
          <button
            type="button"
            onClick={retry}
            disabled={retryDisabled}
            className={PRIMARY}
          >
            {t("common.actions.retry")}
          </button>
        )}
        <button
          type="button"
          onClick={() => void openLogs()}
          className={retry ? SECONDARY : PRIMARY}
        >
          {t("chat.engine.openLogs")}
        </button>
        <button type="button" onClick={onOpenSettings} className={SECONDARY}>
          {t("chat.engine.openSettings")}
        </button>
      </div>
      {logsError && (
        <p className="text-xs break-words text-red-600 dark:text-red-400">
          {t("chat.engine.openLogsError", { message: logsError })}
        </p>
      )}
    </div>
  );
}
