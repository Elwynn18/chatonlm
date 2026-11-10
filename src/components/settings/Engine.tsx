// Settings › General, propre à ChatonLM : ligne « Engine » (lot L5b, à la
// place de la ligne « Ollama »). Le moteur de texte est `llama-server`
// (llama.cpp) livré avec l'app : sa version, son accélération (carte
// graphique nommée, ou processeur seul), le repli de la session sur le
// processeur après un échec de la carte, son absence (installation
// incomplète : réinstaller), et « Open logs » qui ouvre le dossier de ses
// journaux (échec dit sous la ligne). Rien à régler : « Reset to defaults »
// n'y touche pas.

import { CpuChipIcon } from "@heroicons/react/20/solid";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Row } from "@/components/settings/Row";
import { useT } from "@/i18n";
import {
  accelerationText,
  engineStatusKey,
  engineVersionText,
} from "@/lib/engine";
import { engineStatus, errorMessage, openEngineLogs } from "@/lib/ipc";

const BUTTON =
  "flex h-8 flex-none items-center gap-1.5 rounded-lg border border-neutral-300 bg-white px-3 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-600 dark:bg-neutral-800 dark:text-neutral-200 dark:hover:bg-neutral-700";

const ERROR = "text-xs break-words text-red-600 dark:text-red-400";
const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";

/**
 * Ligne « Engine » : version de llama.cpp, accélération, repli de la session,
 * moteur absent, et « Open logs ». Tant que l'état n'est pas lu : la ligne
 * sans détails.
 */
export function EngineRow({ descriptionId }: { descriptionId: string }) {
  const t = useT();
  const { data: status, error } = useQuery({
    queryKey: engineStatusKey,
    queryFn: engineStatus,
  });
  const [openError, setOpenError] = useState<string | null>(null);
  // Une réponse peut arriver après la sortie de la page.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const openLogs = async () => {
    setOpenError(null);
    try {
      await openEngineLogs();
    } catch (cause) {
      if (mounted.current) setOpenError(errorMessage(cause));
    }
  };

  return (
    <Row
      icon={CpuChipIcon}
      label={t("settings.engine.label")}
      description={
        status
          ? engineVersionText(t, status)
          : t("settings.engine.versionUnknown")
      }
      descriptionId={descriptionId}
    >
      {status?.available && (
        <p data-testid="engine-acceleration" className={MUTED}>
          {accelerationText(t, status)}
        </p>
      )}
      {status?.available && status.cpuFallback && (
        <p
          data-testid="engine-fallback"
          className="mt-1 text-xs break-words text-neutral-600 dark:text-neutral-300"
        >
          {t("settings.engine.cpuFallback")}
        </p>
      )}
      {status && !status.available && (
        <p role="alert" className={`mt-1 ${ERROR}`}>
          {t("settings.engine.unavailable")}
        </p>
      )}
      {error && (
        <p role="alert" className={`mt-1 ${ERROR}`}>
          {t("settings.engine.statusError", { message: errorMessage(error) })}
        </p>
      )}
      <div className="mt-3">
        <button
          type="button"
          onClick={() => void openLogs()}
          className={BUTTON}
        >
          {t("settings.engine.openLogs")}
        </button>
      </div>
      {openError && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {t("settings.engine.openLogsError", { message: openError })}
        </p>
      )}
    </Row>
  );
}
