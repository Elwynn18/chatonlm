// Barre de la séance vocale, à la place de la zone de saisie : état (écoute,
// réflexion, parole, coupé), niveau du micro, couper, reprendre une séance en
// pause, terminer, où part ce qui est dit (modèle de cette machine, d'une
// autre, du cloud) et ce qui est partagé avec le modèle (passage de l'éditeur,
// dernière commande échouée, après filtrage hors machine). L'audio est dans
// l'app.
import {
  MicrophoneIcon,
  PlayIcon,
  StopIcon,
  SpeakerXMarkIcon,
} from "@heroicons/react/20/solid";
import {
  CloudIcon,
  ComputerDesktopIcon,
  GlobeAltIcon,
  QuestionMarkCircleIcon,
} from "@heroicons/react/24/outline";
import { useT } from "@/i18n";
import type { VoiceView } from "../shared/protocol";
import { sharedItems, voiceDestination, voicePhaseLabel } from "./labels";

const PLACES = {
  local: ComputerDesktopIcon,
  cloud: CloudIcon,
  remote: GlobeAltIcon,
  unknown: QuestionMarkCircleIcon,
} as const;

export function VoiceBar({
  voice,
  level,
  onMute,
  onStop,
  onResume,
}: {
  voice: VoiceView;
  level: number;
  onMute: (muted: boolean) => void;
  onStop: () => void;
  onResume: () => void;
}) {
  const t = useT();
  const items = sharedItems(t, voice);
  const percent = voice.muted ? 0 : Math.round(level * 100);
  // Hors de cette machine (ou sans le savoir) : marqué en roux.
  const away = voice.destination !== "local";
  const Place = PLACES[voice.destination];
  return (
    <section
      aria-label={t(
        voice.profile === "duck" ? "vscode.voice.duck" : "vscode.voice.tour",
      )}
      data-testid="voice"
      data-phase={voice.phase}
      className="px-3 pt-2 pb-3"
    >
      <div className="flex items-center gap-2 rounded-3xl bg-white py-2 pr-2 pl-4 ring-[1.5px] ring-neutral-300 ring-inset dark:bg-neutral-800 dark:ring-white/10">
        <div className="min-w-0 flex-1">
          <p
            className="text-sm text-neutral-800 dark:text-neutral-100"
            role="status"
            data-testid="voice-phase"
          >
            {voicePhaseLabel(t, voice)}
          </p>
          <div
            role="meter"
            aria-label={t("vscode.voice.level")}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
            className="mt-1 h-1 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
          >
            <div
              className="h-full rounded-full bg-accent"
              style={{ width: `${percent}%` }}
            />
          </div>
        </div>
        {voice.phase === "paused" && (
          <button
            type="button"
            aria-label={t("vscode.voice.resume")}
            title={t("vscode.voice.resume")}
            onClick={onResume}
            className="flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-accent text-neutral-900"
          >
            <PlayIcon className="h-4 w-4" />
          </button>
        )}
        <button
          type="button"
          aria-label={t(
            voice.muted ? "vscode.voice.unmute" : "vscode.voice.mute",
          )}
          title={t(voice.muted ? "vscode.voice.unmute" : "vscode.voice.mute")}
          aria-pressed={voice.muted}
          onClick={() => onMute(!voice.muted)}
          className={
            voice.muted
              ? "flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-accent text-neutral-900"
              : "flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-700 hover:bg-neutral-200 dark:text-neutral-200 dark:hover:bg-neutral-700/75"
          }
        >
          {voice.muted ? (
            <SpeakerXMarkIcon className="h-4 w-4" />
          ) : (
            <MicrophoneIcon className="h-4 w-4" />
          )}
        </button>
        <button
          type="button"
          aria-label={t("vscode.voice.stop")}
          title={t("vscode.voice.stop")}
          onClick={onStop}
          className="flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-black text-white dark:bg-white dark:text-black"
        >
          <StopIcon className="h-3.5 w-3.5" />
        </button>
      </div>
      {voice.notice && (
        <p
          className="pt-2 text-xs text-neutral-800 dark:text-neutral-200"
          role="alert"
          data-testid="voice-notice"
        >
          {voice.notice}
        </p>
      )}
      <p
        className={
          away
            ? "flex items-start gap-1 pt-2 text-xs font-medium text-neutral-800 dark:text-neutral-100"
            : "flex items-start gap-1 pt-2 text-xs text-neutral-500 dark:text-neutral-400"
        }
        data-testid="voice-destination"
        data-destination={voice.destination}
      >
        <Place
          className={
            away
              ? "mt-px h-3.5 w-3.5 shrink-0 text-accent-fg"
              : "mt-px h-3.5 w-3.5 shrink-0"
          }
          aria-hidden="true"
        />
        <span>{voiceDestination(t, voice)}</span>
      </p>
      <p
        className="pt-1 text-xs text-neutral-500 dark:text-neutral-400"
        data-testid="voice-shared"
      >
        {items.length > 0
          ? t("vscode.voice.shared", { items: items.join(" · ") })
          : t("vscode.voice.sharedNothing")}
      </p>
      {voice.blocked.length > 0 && (
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          {t("vscode.voice.blocked", { count: voice.blocked.length })}
        </p>
      )}
      {voice.profile === "duck" && voice.noTerminalError && (
        <p
          className="text-xs text-neutral-500 dark:text-neutral-400"
          data-testid="voice-no-terminal"
        >
          {t("vscode.voice.noTerminalError")}
        </p>
      )}
    </section>
  );
}
