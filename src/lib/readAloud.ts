// Lecture à voix haute (propre à ChatonLM) : fonctions pures de l'interface
// (bouton des réponses, réglages, ModelHub), testées à part. Celles qui
// produisent du texte prennent la fonction de traduction en premier
// paramètre.

import type { ReadAloudError } from "@/bindings/ReadAloudError";
import type { ReadAloudPhase } from "@/bindings/ReadAloudPhase";
import type { ReadAloudState } from "@/bindings/ReadAloudState";
import type { RuntimeProblemKind } from "@/bindings/RuntimeProblemKind";
import type { VoiceLanguage } from "@/bindings/VoiceLanguage";
import type { VoiceModelInfo } from "@/bindings/VoiceModelInfo";
import type { TFunction, TKey } from "@/i18n";
import { languageName } from "@/i18n/format";

/** Repos, avant que l'état du moteur ne soit lu. */
export const IDLE: ReadAloudState = {
  messageId: null,
  phase: "idle",
  percent: null,
  error: null,
};

/** Étape de la lecture pour la réponse `messageId` (repos si une autre est lue). */
export function phaseFor(
  state: ReadAloudState,
  messageId: number,
): ReadAloudPhase {
  return state.messageId === messageId ? state.phase : "idle";
}

/**
 * Erreur de la lecture qui vient de finir, si c'était celle de `messageId`
 * (le moteur garde la réponse avec l'erreur).
 */
export function errorFor(
  state: ReadAloudState,
  messageId: number,
): ReadAloudError | null {
  return state.phase === "idle" && state.messageId === messageId
    ? state.error
    : null;
}

const ERRORS: Record<ReadAloudError, TKey> = {
  noOutputDevice: "chat.readAloud.error.noOutputDevice",
  runtimeUnavailable: "chat.readAloud.error.runtimeUnavailable",
  downloadFailed: "chat.readAloud.error.downloadFailed",
  engineFailed: "chat.readAloud.error.engineFailed",
};

/** Texte d'une erreur de lecture (code du moteur). */
export function readAloudErrorText(
  t: TFunction,
  error: ReadAloudError,
): string {
  return t(ERRORS[error]);
}

/**
 * Message du dialogue d'accord avant le téléchargement d'une voix, par
 * langue (le nom de la langue s'accorde en français et en espagnol : une
 * phrase par langue).
 */
const NEEDS_VOICE: Record<VoiceLanguage, TKey> = {
  en: "chat.readAloud.needsVoice.en",
  es: "chat.readAloud.needsVoice.es",
  fr: "chat.readAloud.needsVoice.fr",
};

export function needsVoiceText(
  t: TFunction,
  language: VoiceLanguage,
  size: string,
): string {
  return t(NEEDS_VOICE[language], { size });
}

/** Explication d'un ONNX Runtime indisponible pour les voix, par raison. */
export const VOICE_RUNTIME_NOTES: Record<RuntimeProblemKind, TKey> = {
  missing: "hub.voices.runtime.missing",
  incompatible: "hub.voices.runtime.incompatible",
  loadFailed: "hub.voices.runtime.loadFailed",
};

/** Nom d'une voix (identifiant du catalogue en minuscules) : « Estelle ». */
export function voiceName(voice: string): string {
  return voice.charAt(0).toUpperCase() + voice.slice(1);
}

/** Titre d'une voix : « French · Estelle » (langue affichée). */
export function voiceTitle(
  t: TFunction,
  model: Pick<VoiceModelInfo, "language" | "voice">,
): string {
  return t("hub.voices.cardTitle", {
    language: languageName(t.lang, model.language),
    voice: voiceName(model.voice),
  });
}

/**
 * Conversation affichée d'après le chemin : son identifiant (« new » pour
 * une nouvelle conversation), null hors d'une conversation (réglages,
 * ModelHub...).
 */
export function chatOfPath(pathname: string): string | null {
  const match = /^\/c\/([^/]+)\/?$/.exec(pathname);
  return match ? decodeURIComponent(match[1]) : null;
}

/** Identifiant de la page d'une nouvelle conversation (route /c/new). */
export const NEW_CHAT_ID = "new";

/**
 * Passer de `previous` à `next` change-t-il de conversation ? Pas depuis une
 * nouvelle conversation : sa création y mène (la réponse lue est la sienne).
 */
export function leavesChat(previous: string | null, next: string): boolean {
  return previous !== null && previous !== next && previous !== NEW_CHAT_ID;
}
