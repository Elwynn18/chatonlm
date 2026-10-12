// Barre du mode conversation vocale (propre à ChatonLM), à la place de la zone
// de texte de la conversation de la séance (le brouillon y reste et revient
// à la fin). De gauche à droite : le bouton principal, qui porte
// l'indicateur animé (niveau du micro pendant l'écoute, de la voix pendant
// la lecture ; pulsation pendant la transcription et la réponse ; rien
// d'animé si le système demande moins de mouvement), le libellé de l'étape
// (annoncé, avec l'avis du dernier tour) et l'aide du clavier, puis
// « micro coupé » et End.
//
// Bouton principal et Espace (focus hors d'un champ, et hors d'un autre
// bouton qu'Espace activerait) : en mains libres, « j'ai fini » pendant la
// parole, interrompre la réponse en route ou lue, reprendre après la pause ;
// en push-to-talk, maintenu pour parler (répétitions de la touche ignorées),
// relâché pour envoyer. Échap termine.
//
// Pendant le lancement : « Getting ready… », la boîte d'accord pour les
// fichiers manquants (tailles), puis la progression de leurs
// téléchargements ; la séance démarre quand ils sont installés.

import { MicrophoneIcon, XMarkIcon } from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import {
  type RefObject,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { VoiceModeRequirements } from "@/bindings/VoiceModeRequirements";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import type { VoiceTurnMode } from "@/bindings/VoiceTurnMode";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  speechDownloadsKey,
  useSpeechActions,
  useSpeechDownloads,
  useSpeechModels,
} from "@/hooks/useDictation";
import {
  useVoiceActions,
  useVoiceDownloads,
  useVoiceModels,
  voiceDownloadsKey,
} from "@/hooks/useReadAloud";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import {
  failVoiceModeLaunch,
  stopVoiceMode,
  useVoiceModeLevel,
  useVoiceModeSettings,
  type VoiceModeLaunch,
  type VoiceModeView,
  voiceModeConfirmed,
  voiceModeDownloading,
  voiceModeDownloadsReady,
} from "@/hooks/useVoiceMode";
import { type TFunction, useT } from "@/i18n";
import { formatBytes, formatPercent, languageName } from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import { isActiveSpeechDownload } from "@/lib/dictation";
import { activatedBySpace, isEditable } from "@/lib/editable";
import {
  errorMessage,
  voiceModeEndTurn,
  voiceModeHold,
  voiceModeInterrupt,
  voiceModeMute,
  voiceModeRequirements,
  voiceModeResume,
} from "@/lib/ipc";
import { overlayOpen } from "@/lib/overlay";
import { voiceTitle } from "@/lib/readAloud";
import { isEscapeKey, isVoiceModeSpace } from "@/lib/shortcuts";
import * as streams from "@/lib/streams";
import {
  handsFreeAction,
  helpKey,
  levelSource,
  noticeText,
  activityPercent,
  phaseLabel,
  replyActivity,
  type TalkAction,
} from "@/lib/voiceMode";

const KBD =
  "rounded border border-neutral-300 bg-white px-1 font-sans text-[11px] font-medium text-neutral-700 dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-200";
const ROUND_BUTTON =
  "flex h-9 w-9 flex-none cursor-pointer items-center justify-center rounded-full border border-transparent focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg disabled:cursor-default disabled:opacity-40";

/** Hauteur relative des traits de l'indicateur. */
const BARS = [0.55, 1, 0.75, 0.4];

function report(what: string) {
  return (error: unknown) => console.error(`Failed to ${what}:`, error);
}

/** Commande d'une action du bouton principal en mains libres. */
function run(action: TalkAction) {
  switch (action) {
    case "endTurn":
      voiceModeEndTurn().catch(report("end the voice turn"));
      break;
    case "interrupt":
      voiceModeInterrupt().catch(report("interrupt the reply"));
      break;
    case "resume":
      voiceModeResume().catch(report("resume voice mode"));
      break;
  }
}

/** Flux qui répond dans la conversation (« new » : pas encore créée). */
function useReplyStream(chatId: string): streams.LiveStream | null {
  return useSyncExternalStore(streams.subscribe, () =>
    chatId === streams.NEW_CHAT
      ? streams.pendingNewStream()
      : streams.activeStream(streams.streamsFor(chatId)),
  );
}

export function VoiceModeBar({
  chatId,
  view,
}: {
  chatId: string;
  /** Séance ou lancement de cette conversation (useVoiceModeFor). */
  view: VoiceModeView;
}) {
  const t = useT();
  const { settings } = useVoiceModeSettings();
  const turnMode: VoiceTurnMode = settings?.turnMode ?? "handsFree";
  const { state, launch } = view;
  const stream = useReplyStream(chatId);
  const reduced = useReducedMotion();
  const barRef = useRef<HTMLDivElement>(null);
  const talkRef = useRef<HTMLButtonElement>(null);
  // Push-to-talk : touche ou bouton maintenu (montré tout de suite).
  const holdingRef = useRef(false);
  const [holding, setHolding] = useState(false);
  // Fichiers manquants : téléchargements d'avant l'accord (voir LaunchProgress).
  const staleRef = useRef<Record<string, string>>({});
  const queryClient = useQueryClient();
  const speechActions = useSpeechActions();
  const voiceActions = useVoiceActions();

  useVoiceModeLevel(
    barRef,
    launch ? null : levelSource(state.phase, state.muted),
    reduced,
  );

  const startHold = () => {
    if (holdingRef.current) return;
    holdingRef.current = true;
    setHolding(true);
    voiceModeHold(true).catch(report("start talking"));
  };
  const endHold = () => {
    if (!holdingRef.current) return;
    holdingRef.current = false;
    setHolding(false);
    voiceModeHold(false).catch(report("stop talking"));
  };

  const pushToTalk =
    turnMode === "pushToTalk" && launch === null && state.phase !== "paused";
  const action: TalkAction = launch ? null : handsFreeAction(state);

  useVoiceModeKeys({
    talkRef,
    holdingRef,
    onSpace: (event) => {
      if (launch) return;
      if (pushToTalk) {
        if (!event.repeat) startHold();
        return;
      }
      if (!event.repeat) run(action);
    },
    onSpaceUp: endHold,
  });

  // Le focus était dans la zone de texte (ou sur le bouton qui a lancé le
  // mode), qui vient de disparaître : au bouton principal (Espace y agit).
  useEffect(() => {
    const active = document.activeElement;
    if (active === null || active === document.body) talkRef.current?.focus();
  }, []);

  const confirmDownloads = async () => {
    // Téléchargements déjà listés : un état d'avant l'accord (échec ancien)
    // ne compte pas.
    const known = [
      ...(queryClient.getQueryData<SpeechDownload[]>(speechDownloadsKey) ?? []),
      ...(queryClient.getQueryData<SpeechDownload[]>(voiceDownloadsKey) ?? []),
    ];
    staleRef.current = Object.fromEntries(
      known.map((download) => [download.id, JSON.stringify(download)]),
    );
    // Prérequis relus : un fichier a pu s'installer pendant que la boîte
    // était ouverte (téléchargement du ModelHub) ; seul ce qui manque encore
    // est téléchargé, rien du tout : la séance démarre.
    const run = voiceModeConfirmed();
    if (run === null) return;
    let requirements: VoiceModeRequirements;
    try {
      requirements = await voiceModeRequirements();
    } catch (error) {
      failVoiceModeLaunch(errorMessage(error));
      return;
    }
    const missing = voiceModeDownloading(run, requirements);
    if (!missing) return;
    const queued = [
      missing.speech === null
        ? null
        : speechActions.download.mutateAsync(missing.speech),
      missing.voice === null
        ? null
        : voiceActions.download.mutateAsync(missing.voice),
    ];
    Promise.all(queued).catch((error: unknown) =>
      failVoiceModeLaunch(errorMessage(error)),
    );
  };

  const activity = replyActivity(stream);
  const label = launch
    ? t("voiceMode.bar.phase.starting")
    : phaseLabel(t, state, activity);
  // Téléchargement du modèle : pourcentage à côté du libellé, pas annoncé.
  const percent = launch ? null : activityPercent(t, state, activity);
  const paused = launch === null && state.phase === "paused";
  const talkLabel = pushToTalk
    ? t("voiceMode.bar.talk.hold")
    : action === "endTurn"
      ? t("voiceMode.bar.talk.done")
      : action === "interrupt"
        ? t("voiceMode.bar.talk.interrupt")
        : action === "resume"
          ? t("voiceMode.bar.talk.resume")
          : label;
  const vivid = holding || (launch === null && state.phase === "hearing");

  return (
    <div
      ref={barRef}
      data-voice-mode-bar=""
      data-voice-mode-phase={state.phase}
      data-voice-mode-muted={state.muted ? "true" : "false"}
      data-voice-mode-launch={launch?.step}
      className="flex min-h-10 w-full items-center gap-3"
    >
      <button
        ref={talkRef}
        type="button"
        data-voice-mode-talk=""
        aria-label={talkLabel}
        title={talkLabel}
        aria-disabled={(!pushToTalk && action === null) || undefined}
        aria-pressed={pushToTalk ? holding : undefined}
        onClick={() => {
          if (!pushToTalk) run(action);
        }}
        onPointerDown={(event) => {
          if (!pushToTalk || event.button !== 0) return;
          event.currentTarget.setPointerCapture?.(event.pointerId);
          startHold();
        }}
        onPointerUp={() => {
          if (pushToTalk) endHold();
        }}
        onPointerCancel={() => {
          if (pushToTalk) endHold();
        }}
        className={`${ROUND_BUTTON} h-10 w-10 transition-colors ${
          vivid
            ? "bg-neutral-900 text-white dark:bg-white dark:text-neutral-900"
            : "bg-neutral-200 text-neutral-500 dark:bg-neutral-700 dark:text-neutral-300"
        } aria-disabled:cursor-default`}
      >
        <Indicator state={state} launch={launch} vivid={vivid} />
      </button>

      {/* En pause, le libellé (« click to resume ») reprend aussi, comme le
          bouton principal et Espace. */}
      <div
        className={`min-w-0 flex-1 ${paused ? "cursor-pointer" : ""}`}
        onClick={paused ? () => run("resume") : undefined}
      >
        {/* Étape et avis annoncés (téléchargements : l'étape, sans chaque
            pourcentage) ; l'aide ne l'est pas. */}
        <div className="flex min-w-0 items-baseline gap-1.5">
          <div role="status" className="min-w-0">
            {launch?.step === "downloading" ? (
              <span className="sr-only">
                {t("voiceMode.bar.phase.downloading")}
              </span>
            ) : (
              <p
                data-voice-mode-label=""
                className="truncate text-sm/5 text-neutral-800 dark:text-neutral-100"
              >
                {label}
              </p>
            )}
            {state.notice && launch === null && (
              <p
                data-voice-mode-notice={state.notice.kind}
                className="text-xs/5 break-words text-amber-700 dark:text-amber-400"
              >
                {noticeText(t, state.notice)}
              </p>
            )}
          </div>
          {percent !== null && (
            <span
              data-voice-mode-percent=""
              className="flex-none text-sm/5 text-neutral-500 tabular-nums dark:text-neutral-400"
            >
              {percent}
            </span>
          )}
        </div>
        {launch?.step === "downloading" && (
          <LaunchProgress launch={launch} staleRef={staleRef} />
        )}
        {!state.notice && launch === null && (
          <p className="truncate text-xs/5 text-neutral-500 dark:text-neutral-400">
            <Trans
              k={helpKey(turnMode, state.phase)}
              tags={{ key: (chunks) => <kbd className={KBD}>{chunks}</kbd> }}
            />
          </p>
        )}
      </div>

      <button
        type="button"
        data-voice-mode-mute=""
        disabled={launch !== null}
        // Bouton bascule : nom fixe, l'état dit par aria-pressed.
        aria-pressed={state.muted}
        aria-label={t("voiceMode.bar.mute")}
        title={t("voiceMode.bar.mute")}
        onClick={() =>
          voiceModeMute(!state.muted).catch(report("mute the microphone"))
        }
        className={`${ROUND_BUTTON} relative bg-neutral-200 text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300`}
      >
        <MicrophoneIcon aria-hidden="true" className="h-4.5 w-4.5" />
        {state.muted && (
          // Trait barré : micro coupé.
          <span
            aria-hidden="true"
            className="absolute h-0.5 w-5 rotate-45 rounded-full bg-current"
          />
        )}
      </button>
      <button
        type="button"
        data-voice-mode-end=""
        aria-label={t("voiceMode.bar.endLabel")}
        title={t("voiceMode.bar.endLabel")}
        onClick={stopVoiceMode}
        className="flex h-9 flex-none cursor-pointer items-center gap-1 rounded-full bg-neutral-900 px-3 text-sm text-white focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-white dark:text-neutral-900"
      >
        <XMarkIcon aria-hidden="true" className="h-4 w-4" />
        {t("voiceMode.bar.end")}
      </button>

      {launch?.step === "asking" && (
        <DownloadDialog
          requirements={launch.requirements}
          onConfirm={() => void confirmDownloads()}
          onCancel={stopVoiceMode}
        />
      )}
    </div>
  );
}

/**
 * Traits de l'indicateur : hauteur selon le niveau (variable CSS
 * `--voice-level` de la barre) pendant l'écoute et la lecture ; pulsation
 * pendant le lancement, la transcription et la réponse ; bas et pâles en
 * pause ou micro coupé.
 */
function Indicator({
  state,
  launch,
  vivid,
}: {
  state: VoiceModeState;
  launch: VoiceModeLaunch | null;
  vivid: boolean;
}) {
  const waiting =
    launch !== null ||
    state.phase === "starting" ||
    state.phase === "transcribing" ||
    state.phase === "responding";
  const quiet = state.phase === "paused" || (state.muted && !waiting);
  const color = vivid
    ? "bg-white dark:bg-neutral-900"
    : quiet
      ? "bg-neutral-300 dark:bg-neutral-500"
      : state.phase === "speaking"
        ? "bg-accent-fg"
        : "bg-neutral-500 dark:bg-neutral-300";
  return (
    <span aria-hidden="true" className="flex h-6 items-center gap-[3px]">
      {BARS.map((scale, index) => (
        <span
          key={index}
          className={`w-[3px] rounded-full ${color} transition-[height] duration-100 motion-reduce:transition-none ${
            waiting ? "motion-safe:animate-pulse" : ""
          }`}
          style={{
            height: quiet
              ? "4px"
              : waiting
                ? `${6 + 8 * scale}px`
                : `calc(5px + var(--voice-level, 0) * ${18 * scale}px)`,
          }}
        />
      ))}
    </span>
  );
}

/**
 * Touches de la barre (écouteur de la fenêtre, tant qu'elle est montée) :
 * Espace, sauf dans un champ ou sur un autre bouton qu'elle activerait ;
 * Échap termine, sauf dans un champ ou quand une boîte ou un menu l'a déjà
 * pris. La fenêtre perd le focus en push-to-talk : la parole est envoyée.
 */
function useVoiceModeKeys({
  talkRef,
  holdingRef,
  onSpace,
  onSpaceUp,
}: {
  talkRef: RefObject<HTMLButtonElement | null>;
  holdingRef: RefObject<boolean>;
  onSpace: (event: KeyboardEvent) => void;
  onSpaceUp: () => void;
}) {
  const ours = (target: EventTarget | null) =>
    target instanceof Node && talkRef.current?.contains(target) === true;
  const keyDown = useEffectEvent((event: KeyboardEvent) => {
    if (event.defaultPrevented || overlayOpen()) return;
    if (isEditable(event.target)) return;
    if (isEscapeKey(event)) {
      event.preventDefault();
      stopVoiceMode();
      return;
    }
    if (!isVoiceModeSpace(event)) return;
    if (!ours(event.target) && activatedBySpace(event.target)) return;
    // Ni défilement de la page, ni clic du bouton principal au relâché.
    event.preventDefault();
    onSpace(event);
  });
  const keyUp = useEffectEvent((event: KeyboardEvent) => {
    if (event.key !== " ") return;
    if (holdingRef.current) {
      event.preventDefault();
      onSpaceUp();
    } else if (ours(event.target)) {
      event.preventDefault();
    }
  });
  const blur = useEffectEvent(() => onSpaceUp());
  useEffect(() => {
    const down = (event: KeyboardEvent) => keyDown(event);
    const up = (event: KeyboardEvent) => keyUp(event);
    const lost = () => blur();
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", lost);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", lost);
    };
  }, []);
}

/**
 * Boîte d'accord : fichiers manquants (modèle de transcription, voix), avec
 * leur nom et leur taille.
 */
function DownloadDialog({
  requirements,
  onConfirm,
  onCancel,
}: {
  requirements: VoiceModeRequirements;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const speechModels = useSpeechModels();
  const voices = useVoiceModels();
  const speechName =
    speechModels.data?.find((model) => model.id === requirements.speechModel)
      ?.name ?? requirements.speechModel;
  const voice = voices.data?.find(
    (model) => model.id === requirements.voiceModel,
  );
  const voiceName = voice
    ? voiceTitle(t, voice)
    : languageName(t.lang, requirements.voiceLanguage);
  // Fichiers manquants : un ou deux (titre et phrase au singulier ou au
  // pluriel).
  const count =
    Number(!requirements.speechModelInstalled) +
    Number(!requirements.voiceInstalled);
  return (
    <ConfirmDialog
      title={t("voiceMode.download.title", { count })}
      message={
        <>
          <p>{t("voiceMode.download.intro", { count })}</p>
          <ul data-voice-mode-downloads="" className="space-y-1 font-medium">
            {!requirements.speechModelInstalled && (
              <li>
                {t("voiceMode.download.speechModel", {
                  name: speechName,
                  size: formatBytes(t, requirements.speechModelBytes),
                })}
              </li>
            )}
            {!requirements.voiceInstalled && (
              <li>
                {t("voiceMode.download.voice", {
                  name: voiceName,
                  size: formatBytes(t, requirements.voiceBytes),
                })}
              </li>
            )}
          </ul>
        </>
      }
      confirmLabel={t("voiceMode.download.confirm")}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}

/** Fichiers attendus et leur téléchargement (absent : pas encore listé). */
function expectedDownloads(
  launch: Extract<VoiceModeLaunch, { step: "downloading" }>,
  speechDownloads: readonly SpeechDownload[],
  voiceDownloads: readonly SpeechDownload[],
): { id: string; entry: SpeechDownload | undefined }[] {
  const items: { id: string; entry: SpeechDownload | undefined }[] = [];
  if (launch.speech !== null) {
    const id = launch.speech;
    items.push({ id, entry: speechDownloads.find((d) => d.id === id) });
  }
  if (launch.voice !== null) {
    const id = launch.voice;
    items.push({ id, entry: voiceDownloads.find((d) => d.id === id) });
  }
  return items;
}

/** Pourcentage des téléchargements attendus, null tant que la taille manque. */
function progressPercent(entries: (SpeechDownload | undefined)[]) {
  let total = 0;
  let completed = 0;
  for (const entry of entries) {
    if (!entry || entry.totalBytes <= 0) return null;
    total += entry.totalBytes;
    completed +=
      entry.state === "done" ? entry.totalBytes : entry.completedBytes;
  }
  return total > 0
    ? Math.min(100, Math.floor((completed / total) * 100))
    : null;
}

function progressText(t: TFunction, percent: number | null) {
  return percent === null
    ? t("voiceMode.bar.phase.downloading")
    : t("voiceMode.bar.phase.downloadingProgress", {
        percent: formatPercent(t.lang, percent),
      });
}

/**
 * Téléchargements des fichiers manquants : progression, puis démarrage
 * quand tous ont fini ; échec ou arrêt (ModelHub) : le lancement échoue.
 * Un état déjà listé avant l'accord (`staleRef`) ne compte que s'il a changé
 * ou si le téléchargement a été vu en cours depuis.
 */
function LaunchProgress({
  launch,
  staleRef,
}: {
  launch: Extract<VoiceModeLaunch, { step: "downloading" }>;
  staleRef: RefObject<Record<string, string>>;
}) {
  const t = useT();
  const { downloads: speechDownloads } = useSpeechDownloads();
  const { downloads: voiceDownloads } = useVoiceDownloads();
  const seenRef = useRef(new Set<string>());
  const expected = expectedDownloads(launch, speechDownloads, voiceDownloads);

  useEffect(() => {
    const seen = seenRef.current;
    const items = expectedDownloads(launch, speechDownloads, voiceDownloads);
    for (const { id, entry } of items) {
      if (entry && isActiveSpeechDownload(entry)) seen.add(id);
    }
    const current = items.map(({ id, entry }) =>
      entry && (seen.has(id) || JSON.stringify(entry) !== staleRef.current[id])
        ? entry
        : null,
    );
    const ended = current.find(
      (entry) => entry?.state === "failed" || entry?.state === "cancelled",
    );
    if (ended) {
      failVoiceModeLaunch(
        ended.state === "failed"
          ? t("voiceMode.download.failed")
          : t("voiceMode.download.cancelled"),
      );
      return;
    }
    if (current.every((entry) => entry?.state === "done")) {
      voiceModeDownloadsReady();
    }
  }, [launch, speechDownloads, voiceDownloads, staleRef, t]);

  return (
    <p
      data-voice-mode-label=""
      className="truncate text-sm/5 text-neutral-800 tabular-nums dark:text-neutral-100"
    >
      {progressText(t, progressPercent(expected.map((item) => item.entry)))}
    </p>
  );
}
