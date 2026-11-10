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

/**
 * Modèle conseillé pour le nettoyage (mesures du contrat 42 § 0, faites avec
 * les mêmes modèles chez Ollama : gemma3:4b, qwen3:4b-instruct-2507,
 * qwen3:8b, gemma3:12b, gemma3:1b).
 */
export interface PolishModel {
  /**
   * Identifiant de Hugging Face (`auteur/dépôt:QUANT`), téléchargé tel quel
   * par le ModelHub (base locale des modèles). Le nettoyage passe encore par
   * Ollama jusqu'au passage complet à llama-server.
   */
  model: string;
  /**
   * Taille du téléchargement en Go, projecteur de vision compris quand le
   * dépôt en a un (fiche de Hugging Face, nov. 2026 ; texte :
   * `polishModelSize`).
   */
  sizeGb: number;
  recommended: boolean;
  noteKind: PolishModelNote | null;
}

/**
 * GGUF officiels de ggml-org (l'organisation de llama.cpp) pour Gemma 3 et
 * de Qwen pour Qwen3 8B ; Qwen3-4B-Instruct-2507 n'a de GGUF que chez les
 * quantificateurs : celui d'unsloth. Q4_K_M partout, comme chez Ollama.
 */
export const POLISH_MODELS: readonly PolishModel[] = [
  {
    model: "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M",
    sizeGb: 3.3,
    recommended: true,
    noteKind: null,
  },
  {
    model: "unsloth/Qwen3-4B-Instruct-2507-GGUF:Q4_K_M",
    sizeGb: 2.5,
    recommended: false,
    noteKind: null,
  },
  {
    model: "Qwen/Qwen3-8B-GGUF:Q4_K_M",
    sizeGb: 5.0,
    recommended: false,
    noteKind: null,
  },
  {
    model: "ggml-org/gemma-3-12b-it-GGUF:Q4_K_M",
    sizeGb: 8.2,
    recommended: false,
    noteKind: "bestQuality",
  },
  {
    model: "ggml-org/gemma-3-1b-it-GGUF:Q4_K_M",
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

/** Identifiant d'une conversation ou d'un cours (UUID). */
const UUID = "[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}";

/**
 * Chemin d'une page demandée par le moteur (événement « navigate »), gardé
 * seulement s'il mène aux réglages, au ModelHub, à la page Proofread, ou à
 * une conversation ou un cours désigné par son identifiant (`app.show` du
 * lien VS Code) : « /settings », « /settings?section=dictation »,
 * « /hub/transcription », « /proofread?selection=1 » (action « Show » du
 * raccourci de correction), « /study/<id> », « /c/<id> »... ; null sinon
 * (autre page, « /c/new », adresse avec un schéma, « //hôte »).
 */
export function navigablePath(path: unknown): string | null {
  if (typeof path !== "string") return null;
  const page =
    /^\/(settings|hub|proofread)(\/[\w.$-]+)*\/?([?#][^\s\\]*)?$/.test(path);
  const item = new RegExp(`^/(study|c)/${UUID}$`).test(path);
  return page || item ? path : null;
}
