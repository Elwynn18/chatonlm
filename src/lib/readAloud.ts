// Lecture à voix haute (propre à ChatonLM) : fonctions pures de l'interface
// (bouton des réponses et des textes d'un cours, réglages, ModelHub),
// testées à part. Celles qui produisent du texte prennent la fonction de
// traduction en premier paramètre.

import type { ReadAloudError } from "@/bindings/ReadAloudError";
import type { ReadAloudPhase } from "@/bindings/ReadAloudPhase";
import type { ReadAloudSource } from "@/bindings/ReadAloudSource";
import type { ReadAloudState } from "@/bindings/ReadAloudState";
import type { RuntimeProblemKind } from "@/bindings/RuntimeProblemKind";
import type { VoiceLanguage } from "@/bindings/VoiceLanguage";
import type { VoiceModelInfo } from "@/bindings/VoiceModelInfo";
import type { TFunction, TKey } from "@/i18n";
import { languageName } from "@/i18n/format";

/** Repos, avant que l'état du moteur ne soit lu. */
export const IDLE: ReadAloudState = {
  source: null,
  phase: "idle",
  percent: null,
  error: null,
};

/**
 * Texte qu'un bouton fait lire : la réponse d'une conversation, ou un
 * passage d'un cours (onglet Study ; clés : src-tauri/src/study/passage.rs).
 * Le moteur relit le texte en base dans les deux cas.
 */
export type ReadAloudTarget =
  | { kind: "message"; chatId: string; messageId: number }
  | { kind: "study"; courseId: string; passage: string };

/** Nom du texte dans l'état de la lecture (ReadAloudState.source). */
export function sourceOf(target: ReadAloudTarget): ReadAloudSource {
  return target.kind === "message"
    ? { kind: "message", messageId: target.messageId }
    : { kind: "study", courseId: target.courseId, passage: target.passage };
}

/** Les deux noms désignent-ils le même texte ? */
export function sameSource(
  a: ReadAloudSource | null,
  b: ReadAloudSource | null,
): boolean {
  if (a === null || b === null) return false;
  if (a.kind === "message") {
    return b.kind === "message" && a.messageId === b.messageId;
  }
  return (
    b.kind === "study" && a.courseId === b.courseId && a.passage === b.passage
  );
}

/**
 * Textes de cours montrés à l'écran avec leur bouton de lecture (nombre de
 * boutons par texte). Le chat garde le bouton d'une réponse lue sous les
 * yeux ; une carte de cours, elle, cède la place à la suivante : la lecture
 * d'un texte qui n'est plus montré est arrêtée (son « Stop » aurait disparu
 * avec lui), voir useReadAloud.ts.
 */
const shownPassages = new Map<string, number>();

const passageId = (courseId: string, passage: string) =>
  `${courseId}\n${passage}`;

/** Inscrit un texte de cours montré ; rend la fonction qui le retire. */
export function showStudyPassage(courseId: string, passage: string) {
  const id = passageId(courseId, passage);
  shownPassages.set(id, (shownPassages.get(id) ?? 0) + 1);
  return () => {
    const count = (shownPassages.get(id) ?? 1) - 1;
    if (count > 0) shownPassages.set(id, count);
    else shownPassages.delete(id);
  };
}

/**
 * L'état nomme-t-il un texte de cours en cours de lecture (ou de
 * préparation) qui n'est plus montré ?
 */
export function readsHiddenPassage(state: ReadAloudState): boolean {
  const source = state.source;
  return (
    state.phase !== "idle" &&
    source?.kind === "study" &&
    !shownPassages.has(passageId(source.courseId, source.passage))
  );
}

/** Étape de la lecture pour le texte `source` (repos si un autre est lu). */
export function phaseFor(
  state: ReadAloudState,
  source: ReadAloudSource,
): ReadAloudPhase {
  return sameSource(state.source, source) ? state.phase : "idle";
}

/**
 * Erreur de la lecture qui vient de finir, si c'était celle de `source` (le
 * moteur garde le texte avec l'erreur).
 */
export function errorFor(
  state: ReadAloudState,
  source: ReadAloudSource,
): ReadAloudError | null {
  return state.phase === "idle" && sameSource(state.source, source)
    ? state.error
    : null;
}

const ERRORS: Record<ReadAloudError, TKey> = {
  noOutputDevice: "chat.readAloud.error.noOutputDevice",
  runtimeUnavailable: "chat.readAloud.error.runtimeUnavailable",
  downloadFailed: "chat.readAloud.error.downloadFailed",
  engineFailed: "chat.readAloud.error.engineFailed",
};

/** Un texte d'un cours n'est pas une « réponse » : sa propre phrase. */
const STUDY_ERRORS: Partial<Record<ReadAloudError, TKey>> = {
  engineFailed: "study.readAloud.engineFailed",
};

/** Texte d'une erreur de lecture (code du moteur), selon ce qui était lu. */
export function readAloudErrorText(
  t: TFunction,
  error: ReadAloudError,
  kind: ReadAloudSource["kind"] = "message",
): string {
  return t((kind === "study" && STUDY_ERRORS[error]) || ERRORS[error]);
}

/** Raison d'un texte que le moteur refuse de lire (bouton inactif). */
export type ReadAloudBlocked = "unsupportedLanguage" | "nothingToRead";

export function readAloudBlockedText(
  t: TFunction,
  blocked: ReadAloudBlocked,
  kind: ReadAloudSource["kind"] = "message",
): string {
  if (blocked === "unsupportedLanguage") return t("chat.readAloud.unsupported");
  return kind === "study"
    ? t("study.readAloud.nothingToRead")
    : t("chat.readAloud.nothingToRead");
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
