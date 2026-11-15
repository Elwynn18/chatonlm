// Outil Transcription (propre à ChatonLM) : fonctions pures de la page
// (horodatage, paragraphes, passages reçus en delta, progression, conseils
// du choix du modèle), essayées à part (model.test.ts). Celles qui
// produisent du texte reçoivent `t`.

import type { SpeechEngine } from "@/bindings/SpeechEngine";
import type { ToolsStatus } from "@/bindings/ToolsStatus";
import type { ToolStatus } from "@/bindings/ToolStatus";
import type { TranscriptionJob } from "@/bindings/TranscriptionJob";
import type { TranscriptionSegment } from "@/bindings/TranscriptionSegment";
import type { TranscriptionStage } from "@/bindings/TranscriptionStage";
import type { TranscriptionStatus } from "@/bindings/TranscriptionStatus";

/** Pause qui ouvre un nouveau paragraphe (comme paragraphs.rs du moteur). */
const PARAGRAPH_PAUSE_S = 1.5;
/** Durée au-delà de laquelle un paragraphe s'arrête au passage suivant. */
const PARAGRAPH_SPAN_S = 60;

/**
 * Whisper sur processeur : environ 55 min de calcul par heure de son
 * (rapport 106, lot 2) ; avertissement au-delà de 10 min de son.
 */
const WHISPER_MINUTES_PER_HOUR = 55;
export const WHISPER_WARNING_S = 10 * 60;

export interface Paragraph {
  start: number;
  end: number;
  text: string;
}

/**
 * Paragraphes des passages, à la règle du moteur (paragraphs.rs) : nouveau
 * après une pause de plus de 1,5 s, ou dès qu'il couvre 60 s.
 */
export function paragraphs(segments: readonly TranscriptionSegment[]) {
  const result: Paragraph[] = [];
  for (const segment of segments) {
    const text = segment.text.trim();
    if (text === "") continue;
    const last = result.at(-1);
    if (
      last &&
      segment.start - last.end <= PARAGRAPH_PAUSE_S &&
      segment.start - last.start < PARAGRAPH_SPAN_S
    ) {
      last.text += ` ${text}`;
      last.end = Math.max(last.end, segment.end);
    } else {
      result.push({ start: segment.start, end: segment.end, text });
    }
  }
  return result;
}

/**
 * Position dans le son : « 1:05 », « 12:30 », ou « 1:02:03 » si `long`
 * (texte d'une heure ou plus : même forme pour tous ses horodatages).
 */
export function clock(seconds: number, long = false): string {
  const total = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor(total / 60) % 60;
  const rest = String(total % 60).padStart(2, "0");
  if (long || hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${rest}`;
  }
  return `${Math.floor(total / 60)}:${rest}`;
}

/** Vrai si le son dure une heure ou plus (horodatage « h:mm:ss »). */
export function isLong(segments: readonly TranscriptionSegment[]): boolean {
  return (segments.at(-1)?.end ?? 0) >= 3600;
}

/**
 * Texte à copier : les paragraphes séparés par une ligne vide, avec leur
 * horodatage « [m:ss] » en tête si `timestamps`.
 */
export function copyText(
  segments: readonly TranscriptionSegment[],
  timestamps: boolean,
): string {
  const long = isLong(segments);
  return paragraphs(segments)
    .map((p) => (timestamps ? `[${clock(p.start, long)}] ${p.text}` : p.text))
    .join("\n\n");
}

/** Étape finie (plus rien ne change). */
export function isFinished(stage: TranscriptionStage): boolean {
  return stage === "done" || stage === "failed" || stage === "cancelled";
}

/** Une transcription tourne (« Transcribe » grisé). */
export function isRunning(job: TranscriptionJob | null | undefined): boolean {
  return job !== null && job !== undefined && !isFinished(job.stage);
}

/**
 * État reçu par l'événement, joint à celui que la page connaît : les
 * passages arrivent en delta (`segmentsFrom` = nombre déjà reçu). `null` :
 * il en manque (événement perdu, page rechargée), relire l'état complet.
 */
export function mergeStatus(
  known: TranscriptionStatus | undefined,
  event: TranscriptionStatus,
): TranscriptionStatus | null {
  const job = event.job;
  if (!job) return event;
  if (job.segmentsFrom === 0) return event;
  const previous = known?.job;
  if (!previous || previous.id !== job.id) return null;
  if (job.segmentsFrom !== previous.segments.length) return null;
  return {
    job: {
      ...job,
      segments: [...previous.segments, ...job.segments],
      segmentsFrom: 0,
    },
  };
}

/** Avancement de l'étape en cours, de 0 à 100 ; null : inconnu. */
export function stagePercent(job: TranscriptionJob): number | null {
  if (job.stage === "downloading") {
    if (!job.totalBytes || job.receivedBytes === null) return null;
    return Math.min(100, (job.receivedBytes / job.totalBytes) * 100);
  }
  if (job.stage === "decoding" || job.stage === "transcribing") {
    if (!job.durationS) return null;
    return Math.min(100, (job.doneS / job.durationS) * 100);
  }
  return null;
}

/**
 * Minutes restantes d'une transcription (son restant ÷ vitesse mesurée) ;
 * null tant que la vitesse ou la durée n'est pas connue.
 */
export function remainingMinutes(job: TranscriptionJob): number | null {
  if (job.stage !== "transcribing" || !job.speed || !job.durationS) {
    return null;
  }
  const seconds = Math.max(0, job.durationS - job.doneS) / job.speed;
  return Math.ceil(seconds / 60);
}

/** Minutes de calcul de Whisper sur processeur pour `durationS` de son. */
export function whisperMinutes(durationS: number): number {
  return Math.max(1, Math.ceil((durationS * WHISPER_MINUTES_PER_HOUR) / 3600));
}

/** Conseil d'un moteur pour la source et la langue choisies. */
export type EngineAdvice =
  | { kind: "whisperSlow" }
  | { kind: "whisperSlowFile"; minutes: number }
  | { kind: "parakeetLanguage" }
  | { kind: "parakeetGuesses" }
  | { kind: "twoLanguages" };

/**
 * Conseils sous le choix du modèle : Whisper lent sur un long son (durée du
 * fichier connue, sinon adresse : durée encore inconnue) ; Parakeet ne
 * connaît pas la langue demandée ; langue choisie qu'il connaît : il ne
 * l'impose pas, il devine celle de chaque passage (plusieurs voix, mots
 * d'une autre langue : des passages sortent dans une autre langue, retour de
 * recette de la 0.10.0) ; en détection automatique, il garde une seule
 * langue par passage (vidéo qui mélange deux langues).
 */
export function engineAdvice({
  engine,
  source,
  durationS,
  language,
  parakeetLanguages,
}: {
  engine: SpeechEngine | null;
  source: "url" | "file" | null;
  durationS: number | null;
  language: string;
  /** Codes que Parakeet connaît. */
  parakeetLanguages: readonly string[];
}): EngineAdvice[] {
  const advice: EngineAdvice[] = [];
  if (engine === "whisper") {
    if (source === "file" && durationS !== null) {
      if (durationS >= WHISPER_WARNING_S) {
        advice.push({
          kind: "whisperSlowFile",
          minutes: whisperMinutes(durationS),
        });
      }
    } else if (source === "url") {
      advice.push({ kind: "whisperSlow" });
    }
  } else if (engine === "parakeet") {
    if (language === "auto") {
      advice.push({ kind: "twoLanguages" });
    } else if (parakeetLanguages.includes(language)) {
      advice.push({ kind: "parakeetGuesses" });
    } else {
      advice.push({ kind: "parakeetLanguage" }, { kind: "twoLanguages" });
    }
  }
  return advice;
}

/** Outil en cours de relevé, de téléchargement ou d'installation. */
export function isToolBusy(tool: ToolStatus): boolean {
  return (
    tool.phase === "fetching" ||
    tool.phase === "downloading" ||
    tool.phase === "verifying" ||
    tool.phase === "installing"
  );
}

/** Les outils s'installent (progression montrée, « Cancel »). */
export function toolsBusy(status: ToolsStatus | undefined): boolean {
  return status?.tools.some(isToolBusy) ?? false;
}

/** Octets à télécharger pour les outils absents (consentement). */
export function toolsDownloadBytes(status: ToolsStatus): number {
  return status.tools
    .filter((tool) => tool.version === null)
    .reduce((sum, tool) => sum + tool.downloadBytes, 0);
}

/** Adresse plausible d'une page web (le moteur vérifie le reste). */
export function isWebAddress(value: string): boolean {
  const trimmed = value.trim();
  if (!/^https?:\/\//i.test(trimmed)) return false;
  try {
    return new URL(trimmed).host !== "";
  } catch {
    return false;
  }
}

/** Texte en octets UTF-8, pour l'enregistrement. */
export function utf8(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}
