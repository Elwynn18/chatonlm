// Dictée (étape 17, propre à ChatonLM) : fonctions pures de l'interface
// (réglages, ModelHub, événement « navigate »), testées à part. Celles qui
// produisent du texte prennent la fonction de traduction en premier
// paramètre (textes dans src/locales/<langue>/dictation.json).

import type { DictationPhase } from "@/bindings/DictationPhase";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { TFunction } from "@/i18n";
import { formatBytes, formatSizeValue } from "@/i18n/format";
import { downloadPercent } from "@/lib/hub";

/** Valeur du réglage de langue pour la détection automatique. */
export const AUTO_LANGUAGE = "auto";

/** Mots du dictionnaire, au plus (MAX_DICTIONARY_WORDS du moteur). */
export const MAX_DICTIONARY_WORDS = 200;
/** Longueur d'un mot du dictionnaire (MAX_WORD_CHARS du moteur). */
export const MAX_WORD_CHARS = 64;
/** Longueur des consignes du nettoyage (MAX_PROMPT_CHARS du moteur). */
export const MAX_PROMPT_CHARS = 4000;

/** Téléchargement d'un modèle de transcription en file, en cours ou en vérification. */
export function isActiveSpeechDownload(download: SpeechDownload): boolean {
  return (
    download.state === "queued" ||
    download.state === "downloading" ||
    download.state === "verifying"
  );
}

/** Pourcentage entier ; 0 tant que le total est inconnu. */
export function speechDownloadPercent(download: SpeechDownload): number {
  return downloadPercent(download.completedBytes, download.totalBytes);
}

/**
 * Ligne d'état d'un téléchargement actif : « Waiting… », « 120 MB of
 * 671 MB », « Verifying… ».
 */
export function speechDownloadText(
  t: TFunction,
  download: SpeechDownload,
): string {
  switch (download.state) {
    case "queued":
      return t("dictation.download.waiting");
    case "verifying":
      return t("dictation.download.verifying");
    default:
      return download.totalBytes > 0
        ? t("dictation.download.progress", {
            done: formatBytes(t, download.completedBytes),
            total: formatBytes(t, download.totalBytes),
          })
        : formatBytes(t, download.completedBytes);
  }
}

/** Phase d'une dictée en cours, à annoncer ; null au repos. */
export function phaseText(t: TFunction, phase: DictationPhase): string | null {
  switch (phase) {
    case "recording":
      return t("dictation.phase.recording");
    case "transcribing":
      return t("dictation.phase.transcribing");
    case "polishing":
      return t("dictation.phase.polishing");
    default:
      return null;
  }
}

/**
 * Dictionnaire avec `word` ajouté à la fin (rogné) ; null s'il est vide ou
 * déjà présent sans tenir compte de la casse, comme le moteur.
 */
export function addWord(
  words: readonly string[],
  word: string,
): string[] | null {
  const trimmed = word.trim();
  if (!trimmed) return null;
  const lower = trimmed.toLowerCase();
  if (words.some((seen) => seen.toLowerCase() === lower)) return null;
  return [...words, trimmed];
}

/** Remarque sur un modèle du nettoyage (texte : `polishModelNote`). */
export type PolishModelNote = "bestQuality" | "smallMachines";

/** Modèle Ollama conseillé pour le nettoyage (mesures du contrat 42 § 0). */
export interface PolishModel {
  /** Nom Ollama, tiré tel quel par le téléchargement du ModelHub. */
  model: string;
  /**
   * Taille annoncée par la bibliothèque d'Ollama, en Go (texte :
   * `polishModelSize`).
   */
  sizeGb: number;
  recommended: boolean;
  noteKind: PolishModelNote | null;
}

export const POLISH_MODELS: readonly PolishModel[] = [
  { model: "gemma3:4b", sizeGb: 3.3, recommended: true, noteKind: null },
  {
    model: "qwen3:4b-instruct-2507-q4_K_M",
    sizeGb: 2.5,
    recommended: false,
    noteKind: null,
  },
  { model: "qwen3:8b", sizeGb: 5.2, recommended: false, noteKind: null },
  {
    model: "gemma3:12b",
    sizeGb: 8.1,
    recommended: false,
    noteKind: "bestQuality",
  },
  {
    model: "gemma3:1b",
    sizeGb: 0.8,
    recommended: false,
    noteKind: "smallMachines",
  },
];

/** Taille annoncée d'un modèle du nettoyage : « 3.3 GB », « 3,3 Go ». */
export function polishModelSize(t: TFunction, model: PolishModel): string {
  return formatSizeValue(t.lang, model.sizeGb, 1, "GB");
}

/** Remarque sur un modèle du nettoyage ; null s'il n'en a pas. */
export function polishModelNote(
  t: TFunction,
  model: PolishModel,
): string | null {
  switch (model.noteKind) {
    case "bestQuality":
      return t("dictation.polishModels.bestQuality", {
        size: formatSizeValue(t.lang, 12, 0, "GB"),
      });
    case "smallMachines":
      return t("dictation.polishModels.smallMachines");
    default:
      return null;
  }
}

/**
 * Modèle du cloud d'Ollama (nom en « cloud », comme le moteur) : jamais
 * proposé pour le nettoyage, le texte dicté quitterait la machine.
 */
export function isCloudModel(model: string): boolean {
  return model.trim().toLowerCase().endsWith("cloud");
}

/**
 * L'adresse du serveur Ollama est-elle cette machine (localhost, 127.x,
 * ::1, ou 0.0.0.0 qu'un client joint en local) ? Une adresse illisible
 * compte comme distante.
 */
export function isLocalAddress(address: string): boolean {
  const value = address.trim();
  if (!value) return true;
  let host: string;
  try {
    host = new URL(value.includes("://") ? value : `http://${value}`).hostname;
  } catch {
    return false;
  }
  host = host.replace(/^\[|\]$/g, "").toLowerCase();
  return (
    host === "localhost" ||
    host.endsWith(".localhost") ||
    /^127(\.\d{1,3}){3}$/.test(host) ||
    host === "::1" ||
    host === "0.0.0.0" ||
    host === "::"
  );
}

/**
 * Chemin d'une page demandée par le moteur (événement « navigate »), gardé
 * seulement s'il mène aux réglages, au ModelHub ou à la page Proofread :
 * « /settings », « /settings?section=dictation », « /hub/transcription »,
 * « /proofread?selection=1 » (action « Show » du raccourci de correction)...
 * ; null sinon (autre page, adresse avec un schéma, « //hôte »).
 */
export function navigablePath(path: unknown): string | null {
  if (typeof path !== "string") return null;
  return /^\/(settings|hub|proofread)(\/[\w.$-]+)*\/?([?#][^\s\\]*)?$/.test(
    path,
  )
    ? path
    : null;
}
