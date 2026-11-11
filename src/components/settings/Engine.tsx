// Settings › General, propre à ChatonLM : ligne « Engine » (lot L5b, à la
// place de la ligne « Ollama »). Le moteur de texte est `llama-server`
// (llama.cpp) livré avec l'app : sa version, son accélération (carte
// graphique nommée, ou processeur seul), le repli de la session sur le
// processeur après un échec de la carte, son absence (installation
// incomplète : réinstaller), et « Open logs » qui ouvre le dossier de ses
// journaux (échec dit sous la ligne). Rien à régler : « Reset to defaults »
// n'y touche pas.
// Lot L1b : section « NVIDIA acceleration » (pack CUDA facultatif), montrée
// seulement avec une carte NVIDIA : carte incompatible (pilote ou carte trop
// anciens, raison dite), téléchargement après la licence de NVIDIA acceptée
// (lien vers elle, case à cocher), progression et annulation, puis
// interrupteur « Use CUDA », taille, « Remove », et l'échec de CUDA dans la
// session (repli sur Vulkan). État suivi par l'événement `engine-cuda`.

import { CpuChipIcon } from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import type { CudaStatus } from "@/bindings/CudaStatus";
import { Row, Switch } from "@/components/settings/Row";
import { useT } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import {
  accelerationText,
  cudaStatusKey,
  engineStatusKey,
  engineVersionText,
} from "@/lib/engine";
import {
  cudaCancel,
  cudaInstall,
  cudaRemove,
  cudaSetEnabled,
  cudaStatus,
  engineStatus,
  errorMessage,
  listenCudaStatus,
  openEngineLogs,
  openExternal,
} from "@/lib/ipc";

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
      {status?.available && <CudaSection />}
    </Row>
  );
}

/** Suit l'état du pack (événement `engine-cuda`) dans le cache des requêtes. */
function useCudaEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenCudaStatus((status) => {
      queryClient.setQueryData(cudaStatusKey, status);
      // L'accélération montrée peut changer (CUDA en service ou non).
      void queryClient.invalidateQueries({ queryKey: engineStatusKey });
    }).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error: unknown) =>
        console.error("Failed to listen for the NVIDIA pack:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [queryClient]);
}

/**
 * Section « NVIDIA acceleration » : rien sans carte NVIDIA (ni sur un
 * système sans pack).
 */
function CudaSection() {
  const t = useT();
  useCudaEvents();
  const { data: status, error } = useQuery({
    queryKey: cudaStatusKey,
    queryFn: () => cudaStatus(true),
  });
  const titleId = useId();
  if (error) {
    return (
      <p role="alert" className={`mt-3 ${ERROR}`}>
        {t("settings.engine.cuda.statusError", {
          message: errorMessage(error),
        })}
      </p>
    );
  }
  if (
    !status ||
    status.compat === "unsupported" ||
    status.compat === "noNvidia"
  )
    return null;
  return (
    <section
      aria-labelledby={titleId}
      data-testid="engine-cuda"
      className="mt-4 border-t border-neutral-200 pt-3 dark:border-neutral-700"
    >
      <h3
        id={titleId}
        className="text-sm font-medium text-neutral-800 dark:text-neutral-200"
      >
        {t("settings.engine.cuda.title")}
      </h3>
      <CudaBody status={status} />
    </section>
  );
}

function CudaBody({ status }: { status: CudaStatus }) {
  const t = useT();
  const [asking, setAsking] = useState(false);
  const [accepted, setAccepted] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const stateId = useId();

  const run = async (action: () => Promise<void>) => {
    setActionError(null);
    try {
      await action();
    } catch (cause) {
      if (mounted.current) setActionError(errorMessage(cause));
    }
  };

  const name = status.gpuName ?? t("settings.engine.cuda.unnamedCard");
  const busy =
    status.phase === "fetching" ||
    status.phase === "downloading" ||
    status.phase === "installing";

  let body: React.ReactNode;
  if (status.compat === "driverTooOld") {
    body = (
      <p className={MUTED} data-testid="engine-cuda-incompatible">
        {t("settings.engine.cuda.driverTooOld", {
          driver: status.driver ?? "?",
          min: String(status.driverMin),
        })}
      </p>
    );
  } else if (status.compat === "computeTooLow") {
    body = (
      <p className={MUTED} data-testid="engine-cuda-incompatible">
        {t("settings.engine.cuda.computeTooLow", {
          name,
          compute: status.compute ?? "?",
          min: status.computeMin,
        })}
      </p>
    );
  } else if (busy) {
    body = (
      <CudaProgress status={status} onCancel={() => void run(cudaCancel)} />
    );
  } else if (status.phase === "installed") {
    body = (
      <>
        <div className="mt-1 flex items-center justify-between gap-3">
          <span
            aria-hidden="true"
            className="text-sm/6 text-neutral-800 dark:text-neutral-200"
          >
            {t("settings.engine.cuda.use")}
          </span>
          <Switch
            checked={status.enabled}
            label={t("settings.engine.cuda.use")}
            describedBy={stateId}
            onChange={(enabled) => void run(() => cudaSetEnabled(enabled))}
          />
        </div>
        <p id={stateId} className={MUTED} data-testid="engine-cuda-state">
          {status.enabled
            ? t("settings.engine.cuda.active", { name })
            : t("settings.engine.cuda.off")}{" "}
          {status.installedBytes !== null &&
            t("settings.engine.cuda.installed", {
              size: formatBytes(t, status.installedBytes),
            })}
        </p>
        <div className="mt-2">
          <button
            type="button"
            className={BUTTON}
            onClick={() => void run(cudaRemove)}
          >
            {t("settings.engine.cuda.remove")}
          </button>
        </div>
      </>
    );
  } else if (asking) {
    body = (
      <div data-testid="engine-cuda-license">
        <p className={MUTED}>{t("settings.engine.cuda.license")}</p>
        <button
          type="button"
          className="mt-1 text-sm font-medium text-accent-fg underline underline-offset-2"
          onClick={() => void run(() => openExternal(status.eulaUrl))}
        >
          {t("settings.engine.cuda.readLicense")}
        </button>
        <label className="mt-2 flex items-start gap-2 text-sm/6 text-neutral-800 dark:text-neutral-200">
          <input
            type="checkbox"
            className="mt-1.5 accent-[var(--color-accent-fg)]"
            checked={accepted}
            onChange={(event) => setAccepted(event.target.checked)}
          />
          {t("settings.engine.cuda.accept")}
        </label>
        <div className="mt-2 flex gap-2">
          <button
            type="button"
            className={BUTTON}
            disabled={!accepted}
            onClick={() =>
              void run(async () => {
                await cudaInstall(true);
                if (mounted.current) {
                  setAsking(false);
                  setAccepted(false);
                }
              })
            }
          >
            {t("settings.engine.cuda.confirm")}
          </button>
          <button
            type="button"
            className={BUTTON}
            onClick={() => {
              setAsking(false);
              setAccepted(false);
            }}
          >
            {t("settings.engine.cuda.back")}
          </button>
        </div>
      </div>
    );
  } else {
    body = (
      <>
        <p className={MUTED} data-testid="engine-cuda-offer">
          {status.stale
            ? t("settings.engine.cuda.stale")
            : t("settings.engine.cuda.offer", {
                name,
                size: formatBytes(t, status.downloadBytes),
              })}
        </p>
        <div className="mt-2">
          <button
            type="button"
            data-testid="engine-cuda-download"
            className={BUTTON}
            onClick={() => setAsking(true)}
          >
            {t("settings.engine.cuda.download")}
          </button>
        </div>
      </>
    );
  }

  return (
    <div className="mt-1">
      {body}
      {status.fallback !== null && (
        <p
          data-testid="engine-cuda-fallback"
          className="mt-1 text-xs break-words text-neutral-600 dark:text-neutral-300"
        >
          {t("settings.engine.cuda.fallback", { reason: status.fallback })}
        </p>
      )}
      {status.error !== null && !busy && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {status.error}
        </p>
      )}
      {actionError && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {t("settings.engine.cuda.actionError", { message: actionError })}
        </p>
      )}
    </div>
  );
}

/** Préparation, téléchargement (jauge) ou installation, et « Cancel ». */
function CudaProgress({
  status,
  onCancel,
}: {
  status: CudaStatus;
  onCancel: () => void;
}) {
  const t = useT();
  const percentage =
    status.totalBytes > 0
      ? Math.min(100, (status.completedBytes / status.totalBytes) * 100)
      : 0;
  const text =
    status.phase === "downloading"
      ? t("settings.engine.cuda.downloading", {
          done: formatBytes(t, status.completedBytes),
          total: formatBytes(t, status.totalBytes),
          percent: formatPercent(t.lang, Math.floor(percentage)),
        })
      : status.phase === "installing"
        ? t("settings.engine.cuda.installing")
        : t("settings.engine.cuda.fetching");
  return (
    <div data-testid="engine-cuda-progress">
      <p className={MUTED}>{text}</p>
      {status.phase === "downloading" && (
        <div
          role="progressbar"
          aria-label={t("settings.engine.cuda.progress")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.floor(percentage)}
          aria-valuetext={text}
          className="relative mt-1 h-1.5 max-w-xs overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
        >
          <div
            className="absolute top-0 left-0 h-full rounded-full bg-accent"
            style={{ width: `${percentage}%` }}
          />
        </div>
      )}
      <div className="mt-2">
        <button type="button" className={BUTTON} onClick={onCancel}>
          {t("settings.engine.cuda.cancel")}
        </button>
      </div>
    </div>
  );
}
