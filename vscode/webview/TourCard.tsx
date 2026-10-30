// Carte de la visite guidée parlée, en haut du panneau : où elle en est,
// segments (le segment en cours mis en avant ; un clic relit à partir de
// lui), contrôles (précédent, pause/reprise, suivant, arrêt), « Ask about
// this part » tapée ou à la voix, et le reçu de ce qui est parti au modèle.
// La lecture et le surlignage de l'éditeur sont faits par l'hôte.
import {
  BackwardIcon,
  ForwardIcon,
  MicrophoneIcon,
  PauseIcon,
  PlayIcon,
  StopIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import type { ReactNode } from "react";
import { useT } from "@/i18n";
import type { TourView } from "../shared/protocol";
import { linesLabel, tourProgress, tourTitle } from "./labels";
import { ReceiptView } from "./ReceiptView";

const ICON =
  "flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-700 hover:bg-neutral-200 disabled:cursor-default disabled:opacity-50 dark:text-neutral-200 dark:hover:bg-neutral-700/75";
const PILL =
  "cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-xs text-neutral-700 hover:bg-neutral-100 disabled:cursor-default disabled:opacity-50 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

function IconButton({
  label,
  onClick,
  disabled = false,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
      className={ICON}
    >
      {children}
    </button>
  );
}

export function TourCard({
  tour,
  voiceOn,
  onCommand,
  onGoto,
  onAsk,
  onClose,
}: {
  tour: TourView;
  /** Une séance vocale est en cours (pas de seconde question à la voix). */
  voiceOn: boolean;
  onCommand: (
    command: "previous" | "next" | "pause" | "resume" | "stop",
  ) => void;
  onGoto: (index: number) => void;
  onAsk: (voice: boolean) => void;
  /** Ferme la carte d'une visite finie. */
  onClose: () => void;
}) {
  const t = useT();
  const { status, segments, current } = tour;
  const running = status === "playing" || status === "paused";
  const ended = status === "ended";
  const hasCurrent = current >= 0 && current < segments.length;

  return (
    <section
      aria-label={t("vscode.tour.label")}
      data-testid="tour"
      data-status={status}
      className="mx-3 mt-2 shrink-0 rounded-2xl bg-neutral-100 px-3 py-2 text-sm dark:bg-neutral-800"
    >
      <div className="flex items-center gap-1">
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-neutral-900 dark:text-white">
            {tourTitle(t, tour)}
          </p>
          <p
            className="text-xs text-neutral-600 dark:text-neutral-300"
            role="status"
            data-testid="tour-progress"
          >
            {tourProgress(t, tour)}
          </p>
        </div>
        {running && (
          <>
            <IconButton
              label={t("vscode.tour.previous")}
              disabled={current <= 0}
              onClick={() => onCommand("previous")}
            >
              <BackwardIcon className="h-4 w-4" />
            </IconButton>
            {status === "playing" ? (
              <IconButton
                label={t("vscode.tour.pause")}
                onClick={() => onCommand("pause")}
              >
                <PauseIcon className="h-4 w-4" />
              </IconButton>
            ) : (
              <IconButton
                label={t("vscode.tour.resume")}
                onClick={() => onCommand("resume")}
              >
                <PlayIcon className="h-4 w-4" />
              </IconButton>
            )}
            <IconButton
              label={t("vscode.tour.next")}
              disabled={current >= segments.length - 1}
              onClick={() => onCommand("next")}
            >
              <ForwardIcon className="h-4 w-4" />
            </IconButton>
          </>
        )}
        {ended ? (
          <IconButton label={t("vscode.tour.close")} onClick={onClose}>
            <XMarkIcon className="h-4 w-4" />
          </IconButton>
        ) : (
          <IconButton
            label={t("vscode.tour.stop")}
            onClick={() => onCommand("stop")}
          >
            <StopIcon className="h-4 w-4" />
          </IconButton>
        )}
      </div>
      {segments.length > 0 && (
        <ol
          aria-label={t("vscode.tour.segments")}
          className="mt-2 max-h-48 space-y-1 overflow-y-auto"
        >
          {segments.map((segment, index) => {
            const active = index === current && !ended;
            return (
              <li key={index}>
                <button
                  type="button"
                  aria-current={active ? "step" : undefined}
                  title={t("vscode.tour.readFromHere")}
                  onClick={() => onGoto(index)}
                  className={
                    active
                      ? "w-full cursor-pointer rounded-xl bg-accent px-2 py-1 text-left text-xs text-neutral-900"
                      : "w-full cursor-pointer rounded-xl px-2 py-1 text-left text-xs text-neutral-700 hover:bg-neutral-200 dark:text-neutral-200 dark:hover:bg-neutral-700/75"
                  }
                >
                  <span className="font-medium">
                    {linesLabel(t, segment.lines)}
                  </span>
                  {" · "}
                  {segment.text}
                </button>
              </li>
            );
          })}
        </ol>
      )}
      {running && hasCurrent && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          <button type="button" className={PILL} onClick={() => onAsk(false)}>
            {t("vscode.tour.ask")}
          </button>
          <button
            type="button"
            className={`${PILL} inline-flex items-center gap-1`}
            disabled={voiceOn}
            onClick={() => onAsk(true)}
          >
            <MicrophoneIcon className="h-3.5 w-3.5" aria-hidden="true" />
            {t("vscode.tour.askVoice")}
          </button>
        </div>
      )}
      {tour.receipt && (
        <div className="flex flex-col">
          <ReceiptView receipt={tour.receipt} />
        </div>
      )}
    </section>
  );
}
