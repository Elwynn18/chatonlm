// Mode conversation vocale (propre à ChatonLM) : fonctions pures de
// l'interface (barre de la zone de saisie, raccourcis), testées à part.
// Celles qui produisent du texte prennent la fonction de traduction en
// premier paramètre.

import type { VoiceLanguage } from "@/bindings/VoiceLanguage";
import type { VoiceModeNotice } from "@/bindings/VoiceModeNotice";
import type { VoiceModePhase } from "@/bindings/VoiceModePhase";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import type { VoiceTurnMode } from "@/bindings/VoiceTurnMode";
import type { TFunction, TKey } from "@/i18n";
import { formatPercent } from "@/i18n/format";
import { readAloudErrorText } from "@/lib/readAloud";
import type { LiveStream } from "@/lib/streams";

/** Pas de séance (avant que l'état du moteur ne soit lu). */
export const VOICE_MODE_OFF: VoiceModeState = {
  phase: "off",
  muted: false,
  turn: 0,
  notice: null,
};

/**
 * Ce que fait le modèle pendant Responding (rien n'est encore lu), d'après
 * le flux de la réponse : téléchargement ou chargement du modèle, recherche
 * web (un appel d'outil attend son résultat), sinon réflexion.
 */
export type ReplyActivity =
  | { kind: "downloadingModel"; percent: number | null }
  | { kind: "loadingModel" }
  | { kind: "searching" }
  | { kind: "thinking" };

export function replyActivity(stream: LiveStream | null): ReplyActivity {
  if (!stream) return { kind: "thinking" };
  const download = stream.download;
  if (download && !download.done) {
    return {
      kind: "downloadingModel",
      percent:
        download.total > 0
          ? Math.min(
              100,
              Math.floor((download.completed / download.total) * 100),
            )
          : null,
    };
  }
  if (stream.loading) return { kind: "loadingModel" };
  const last = stream.replies.at(-1);
  if (
    last?.toolCalls.some((call) => call.result === null && call.error === null)
  ) {
    return { kind: "searching" };
  }
  return { kind: "thinking" };
}

/** Micro coupé : le libellé le dit tant que l'on attendrait la parole. */
const MUTED_PHASES: readonly VoiceModePhase[] = [
  "starting",
  "listening",
  "hearing",
  "paused",
];

/**
 * Libellé de la barre (annoncé par sa région d'état) : étape de la séance ;
 * Responding d'après ce que fait le modèle (`activity`). Jamais de
 * pourcentage (il serait annoncé à chaque point) : voir activityPercent.
 */
export function phaseLabel(
  t: TFunction,
  state: Pick<VoiceModeState, "phase" | "muted">,
  activity: ReplyActivity,
): string {
  if (state.muted && MUTED_PHASES.includes(state.phase)) {
    return t("voiceMode.bar.phase.muted");
  }
  switch (state.phase) {
    case "off":
    case "starting":
      return t("voiceMode.bar.phase.starting");
    case "listening":
    case "hearing":
      return t("voiceMode.bar.phase.listening");
    case "transcribing":
      return t("voiceMode.bar.phase.transcribing");
    case "responding":
      return activityLabel(t, activity);
    case "speaking":
      return t("voiceMode.bar.phase.speaking");
    case "paused":
      return t("voiceMode.bar.phase.paused");
  }
}

function activityLabel(t: TFunction, activity: ReplyActivity): string {
  switch (activity.kind) {
    case "downloadingModel":
      return t("voiceMode.bar.phase.downloadingModel");
    case "loadingModel":
      return t("voiceMode.bar.phase.loadingModel");
    case "searching":
      return t("voiceMode.bar.phase.searching");
    case "thinking":
      return t("voiceMode.bar.phase.thinking");
  }
}

/**
 * Pourcentage du téléchargement du modèle, montré à côté du libellé (hors de
 * la région annoncée) ; null sinon.
 */
export function activityPercent(
  t: TFunction,
  state: Pick<VoiceModeState, "phase">,
  activity: ReplyActivity,
): string | null {
  if (state.phase !== "responding") return null;
  if (activity.kind !== "downloadingModel" || activity.percent === null) {
    return null;
  }
  return formatPercent(t.lang, activity.percent);
}

/** Avis d'une voix absente, par langue (accord du nom de la langue). */
const MISSING_VOICE: Record<VoiceLanguage, TKey> = {
  en: "voiceMode.notice.missingVoice.en",
  es: "voiceMode.notice.missingVoice.es",
  fr: "voiceMode.notice.missingVoice.fr",
};

/** Texte de l'avis du dernier tour (message du moteur déjà traduit). */
export function noticeText(t: TFunction, notice: VoiceModeNotice): string {
  switch (notice.kind) {
    case "unsupportedLanguage":
      return t("voiceMode.notice.unsupportedLanguage");
    case "missingVoice":
      return t(MISSING_VOICE[notice.language]);
    case "readFailed":
      return readAloudErrorText(t, notice.error);
    case "failed":
      return notice.message;
  }
}

/** La réponse est en route ou lue : Espace (ou le bouton) l'interrompt. */
export function canInterrupt(phase: VoiceModePhase): boolean {
  return phase === "responding" || phase === "speaking";
}

/**
 * Action du bouton principal de la barre (et d'Espace) en mains libres :
 * « j'ai fini » pendant la parole, interrompre la réponse, reprendre après
 * la pause ; rien sinon (écoute, transcription, démarrage).
 */
export type TalkAction = "endTurn" | "interrupt" | "resume" | null;

export function handsFreeAction(
  state: Pick<VoiceModeState, "phase" | "muted">,
): TalkAction {
  if (canInterrupt(state.phase)) return "interrupt";
  if (state.phase === "paused") return "resume";
  if (state.phase === "hearing" && !state.muted) return "endTurn";
  return null;
}

/** Clé de l'aide sous le libellé (touches selon le mode et l'étape). */
export function helpKey(turnMode: VoiceTurnMode, phase: VoiceModePhase): TKey {
  if (phase === "paused") return "voiceMode.bar.help.paused";
  if (turnMode === "pushToTalk") return "voiceMode.bar.help.pushToTalk";
  return canInterrupt(phase)
    ? "voiceMode.bar.help.interrupt"
    : "voiceMode.bar.help.handsFree";
}

/**
 * Niveau montré par l'indicateur : celui du micro pendant l'écoute, de la
 * voix pendant la lecture, aucun sinon.
 */
export function levelSource(
  phase: VoiceModePhase,
  muted: boolean,
): "input" | "output" | null {
  if (phase === "speaking") return "output";
  if (!muted && (phase === "listening" || phase === "hearing")) return "input";
  return null;
}
