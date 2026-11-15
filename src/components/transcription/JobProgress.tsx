// Outil Transcription (propre à ChatonLM) : avancement d'une transcription
// (étapes, octets reçus, son décodé puis transcrit, vitesse, temps
// restant), bouton « Cancel » ; échec ou annulation, avec la raison et ce
// qu'on peut y faire.

import {
  CheckCircleIcon,
  ExclamationTriangleIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { TranscriptionError } from "@/bindings/TranscriptionError";
import type { TranscriptionJob } from "@/bindings/TranscriptionJob";
import type { TranscriptionStage } from "@/bindings/TranscriptionStage";
import { BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import {
  clock,
  remainingMinutes,
  stagePercent,
} from "@/components/transcription/model";
import { transcriptionListKey } from "@/components/transcription/queries";
import { useT } from "@/i18n";
import { formatBytes, formatDecimal, formatPercent } from "@/i18n/format";
import { errorMessage, toolsUpdate, transcriptionCancel } from "@/lib/ipc";

/** Étapes montrées, selon la source. */
type Step = Extract<
  TranscriptionStage,
  "fetching" | "downloading" | "decoding" | "transcribing"
>;
const URL_STEPS: Step[] = [
  "fetching",
  "downloading",
  "decoding",
  "transcribing",
];
const FILE_STEPS: Step[] = ["decoding", "transcribing"];

/** Avancement d'une transcription qui tourne. */
export function JobProgress({ job }: { job: TranscriptionJob }) {
  const t = useT();
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<string | null>(null);
  const steps = job.source === "url" ? URL_STEPS : FILE_STEPS;
  const current = steps.indexOf(job.stage as Step);
  const percent = stagePercent(job);
  const cancel = () => {
    setCancelling(true);
    setCancelError(null);
    transcriptionCancel().catch((cause: unknown) => {
      setCancelling(false);
      setCancelError(errorMessage(cause));
    });
  };
  return (
    <section
      aria-label={t("transcription.job.label")}
      aria-busy="true"
      data-testid="transcription-progress"
      data-stage={job.stage}
      className="rounded-2xl border border-neutral-200 bg-white p-4 dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      {job.stage === "preparing" ? (
        <p className="text-sm text-neutral-600 dark:text-neutral-300">
          {t("transcription.job.preparing")}
        </p>
      ) : (
        <ol className="flex flex-col gap-2">
          {steps.map((step, index) => {
            const state =
              index < current
                ? "done"
                : index === current
                  ? "current"
                  : "pending";
            return (
              <li
                key={step}
                data-step={step}
                data-state={state}
                className="flex flex-col gap-1"
              >
                <span className="flex items-center gap-2 text-sm">
                  <StepMark state={state} />
                  <span
                    className={
                      state === "current"
                        ? "font-medium text-neutral-900 dark:text-white"
                        : "text-neutral-600 dark:text-neutral-300"
                    }
                  >
                    {t(`transcription.job.stages.${step}`)}
                  </span>
                  <span className="sr-only">
                    {t(`transcription.job.steps.${state}`)}
                  </span>
                  {state === "current" && (
                    <span
                      data-testid="transcription-step-detail"
                      className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400"
                    >
                      {stepDetail(t, job)}
                    </span>
                  )}
                </span>
                {state === "current" && percent !== null && (
                  <span
                    role="progressbar"
                    aria-label={t("transcription.job.progress")}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.floor(percent)}
                    aria-valuetext={formatPercent(t.lang, Math.floor(percent))}
                    className="ml-6 block h-1.5 max-w-sm overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
                  >
                    <span
                      className="block h-full rounded-full bg-accent"
                      style={{ width: `${percent}%` }}
                    />
                  </span>
                )}
              </li>
            );
          })}
        </ol>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <button
          type="button"
          className={BUTTON}
          onClick={cancel}
          disabled={cancelling}
          data-testid="transcription-cancel"
        >
          {t("transcription.job.cancel")}
        </button>
        {cancelError && (
          <p role="alert" className="text-sm text-red-600 dark:text-red-400">
            {t("transcription.job.cancelError", { message: cancelError })}
          </p>
        )}
      </div>
    </section>
  );
}

function StepMark({ state }: { state: "done" | "current" | "pending" }) {
  if (state === "done") {
    return (
      <CheckCircleIcon
        aria-hidden="true"
        className="h-4 w-4 flex-none text-accent-fg"
      />
    );
  }
  return (
    <span
      aria-hidden="true"
      className={`mx-0.5 h-3 w-3 flex-none rounded-full border-2 ${
        state === "current"
          ? "border-accent-fg motion-safe:animate-pulse"
          : "border-neutral-300 dark:border-neutral-600"
      }`}
    />
  );
}

/** Détail de l'étape en cours : octets, position, vitesse, temps restant. */
function stepDetail(t: ReturnType<typeof useT>, job: TranscriptionJob) {
  switch (job.stage) {
    case "downloading": {
      const done = formatBytes(t, job.receivedBytes ?? 0);
      return job.totalBytes
        ? t("transcription.job.received", {
            done,
            total: formatBytes(t, job.totalBytes),
          })
        : t("transcription.job.receivedSoFar", { done });
    }
    case "decoding":
    case "transcribing": {
      if (!job.durationS) return clock(job.doneS);
      const long = job.durationS >= 3600;
      const parts = [
        t("transcription.job.position", {
          done: clock(job.doneS, long),
          total: clock(job.durationS, long),
        }),
      ];
      if (job.stage === "transcribing" && job.speed) {
        parts.push(
          t("transcription.job.speed", {
            speed: formatDecimal(t.lang, job.speed, 1),
          }),
        );
      }
      const minutes = remainingMinutes(job);
      if (minutes !== null) {
        parts.push(t("transcription.job.remaining", { count: minutes }));
      }
      return parts.join(" · ");
    }
    default:
      return "";
  }
}

/**
 * Échec d'une transcription : titre d'après son code, message du moteur
 * (déjà traduit), et ce qu'on peut y faire (ModelHub, mise à jour de
 * yt-dlp, nouvel essai).
 */
export function ErrorNotice({
  error,
  onRetry,
}: {
  error: TranscriptionError;
  onRetry?: () => void;
}) {
  const t = useT();
  const client = useQueryClient();
  const [actionError, setActionError] = useState<string | null>(null);
  const [updating, setUpdating] = useState(false);
  const needsModel =
    error.code === "noModel" || error.code === "modelNotInstalled";
  const outdated = error.code === "fetch" && error.fetch === "outdated";
  const update = () => {
    setUpdating(true);
    setActionError(null);
    toolsUpdate().then(
      () => {
        setUpdating(false);
        void client.invalidateQueries({ queryKey: transcriptionListKey });
      },
      (cause: unknown) => {
        setUpdating(false);
        setActionError(errorMessage(cause));
      },
    );
  };
  return (
    <div
      role="alert"
      data-testid="transcription-error"
      data-code={error.code}
      data-fetch={error.fetch ?? undefined}
      className="rounded-2xl bg-red-50 px-4 py-3 text-sm text-red-800 dark:bg-red-950/40 dark:text-red-200"
    >
      <p className="flex items-start gap-2 font-medium">
        <ExclamationTriangleIcon
          aria-hidden="true"
          className="mt-0.5 h-4 w-4 flex-none"
        />
        {t(`transcription.errors.codes.${error.code}`)}
      </p>
      {error.message && (
        <p className="mt-1 pl-6 break-words">{error.message}</p>
      )}
      {(needsModel || outdated || onRetry) && (
        <div className="mt-2 flex flex-wrap gap-2 pl-6">
          {needsModel && (
            <Link to="/hub/transcription" className={BUTTON}>
              {t("transcription.errors.openHub")}
            </Link>
          )}
          {outdated && (
            <button
              type="button"
              className={BUTTON}
              onClick={update}
              disabled={updating}
            >
              {t("transcription.errors.updateTools")}
            </button>
          )}
          {onRetry && (
            <button type="button" className={QUIET_BUTTON} onClick={onRetry}>
              {t("transcription.job.retry")}
            </button>
          )}
        </div>
      )}
      {actionError && (
        <p className="mt-2 pl-6">
          {t("transcription.tools.actionError", { message: actionError })}
        </p>
      )}
    </div>
  );
}
