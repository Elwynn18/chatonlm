// Faux moteur de la lecture à voix haute pour le simulateur (tests vitest
// seulement) : catalogue des voix (tts/models.rs : une par langue), leurs
// téléchargements en mémoire et l'état de la lecture (une seule à la fois).
// Comme le moteur, read_aloud (réponse d'une conversation) et
// read_aloud_study (passage d'un cours, nommé par l'état : `source`)
// répondent selon la langue « détectée » du texte (`languages` par réponse,
// `passages` par passage d'un cours, anglais par défaut) : autre langue →
// unsupportedLanguage, rien à lire → nothingToRead, ONNX Runtime absent →
// started puis l'erreur runtimeUnavailable, voix absente → needsVoice (sauf
// avec l'accord ou une voix déjà en téléchargement : la voix est mise en
// file et la lecture passe en downloading), sinon la lecture commence
// (preparing) ; chaque changement émet « read-aloud » après la réponse de
// la commande. read_aloud_stop revient au repos et annule le téléchargement
// attendu par la lecture. Le test
// fait avancer la lecture par play() et end(), un téléchargement par
// progress(), finish() et fail() (« voice-downloads »), et force un état par
// setState(). Une panne d'une commande se force par `failures`.

import type { AppError } from "@/bindings/AppError";
import type { ReadAloudError } from "@/bindings/ReadAloudError";
import type { ReadAloudSource } from "@/bindings/ReadAloudSource";
import type { ReadAloudState } from "@/bindings/ReadAloudState";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { VoiceLanguage } from "@/bindings/VoiceLanguage";
import type { VoiceModelInfo } from "@/bindings/VoiceModelInfo";
import type { Args, Simulator } from "@/test/simulator";

export const VOICE_EN = "pocket-tts-en";
export const VOICE_ES = "pocket-tts-es";
export const VOICE_FR = "pocket-tts-fr";

const MODEL_LICENSE_URL =
  "https://huggingface.co/kyutai/pocket-tts-without-voice-cloning";

const CC_BY_URL = "https://creativecommons.org/licenses/by/4.0/legalcode";
const CC0_URL = "https://creativecommons.org/publicdomain/zero/1.0/legalcode";

/** Catalogue (tts/models.rs), rien d'installé. */
export function voiceCatalog(): VoiceModelInfo[] {
  const voice = (
    id: string,
    name: string,
    language: VoiceLanguage,
    voiceName: string,
    [voiceLicense, voiceLicenseUrl]: [string, string],
    [voiceCredit, voiceSourceUrl]: [string, string],
  ): VoiceModelInfo => ({
    id,
    name,
    language,
    voice: voiceName,
    sizeBytes: 183_500_000,
    license: "CC BY 4.0",
    licenseUrl: MODEL_LICENSE_URL,
    voiceLicense,
    voiceLicenseUrl,
    voiceCredit,
    voiceSourceUrl,
    installed: false,
    runtimeProblem: null,
  });
  return [
    voice(
      VOICE_EN,
      "Pocket TTS · English (Alba)",
      "en",
      "alba",
      ["CC BY 4.0", CC_BY_URL],
      [
        "Alba MacKenna",
        "https://huggingface.co/kyutai/tts-voices/tree/main/alba-mackenna",
      ],
    ),
    voice(
      VOICE_ES,
      "Pocket TTS · Spanish (Lola)",
      "es",
      "lola",
      ["CC0", CC0_URL],
      ["Mozilla Common Voice", "https://commonvoice.mozilla.org/es/datasets"],
    ),
    voice(
      VOICE_FR,
      "Pocket TTS · French (Estelle)",
      "fr",
      "estelle",
      ["CC0", CC0_URL],
      [
        "Kyutai",
        "https://huggingface.co/kyutai/tts-voices/tree/main/unmute-prod-website",
      ],
    ),
  ];
}

/** Ce que le faux moteur « détecte » dans une réponse. */
export type FakeReading = VoiceLanguage | "other" | "empty";

export interface FakeTts {
  /** État de la lecture. */
  state: ReadAloudState;
  /** Catalogue, avec les voix installées. */
  voices: VoiceModelInfo[];
  /** Téléchargements, dans l'ordre de demande. */
  downloads: SpeechDownload[];
  /** Langue de chaque réponse, par identifiant (anglais sinon). */
  languages: Map<number, FakeReading>;
  /** Langue de chaque passage d'un cours, par clé (anglais sinon). */
  passages: Map<string, FakeReading>;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Marque des voix du catalogue comme installées. */
  install(...ids: string[]): void;
  /** Change l'état puis émet « read-aloud ». */
  setState(patch: Partial<ReadAloudState>): Promise<void>;
  /** La lecture en cours passe en playing (premier son). */
  play(): Promise<void>;
  /**
   * Fin de la lecture (au repos) ; avec une erreur, le texte lu reste
   * nommé (service.rs `finish`).
   */
  end(error?: ReadAloudError): Promise<void>;
  /** Émet la liste des téléchargements (« voice-downloads »). */
  emitDownloads(): Promise<void>;
  /** Fait avancer un téléchargement (état downloading), puis émet. */
  progress(id: string, completed: number, total?: number): Promise<void>;
  /** Termine un téléchargement : voix installée, puis émet. */
  finish(id: string): Promise<void>;
  /** Fait échouer un téléchargement avec ce message, puis émet. */
  fail(id: string, error: string): Promise<void>;
}

const ACTIVE: SpeechDownload["state"][] = [
  "queued",
  "downloading",
  "verifying",
];

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

const IDLE: ReadAloudState = {
  source: null,
  phase: "idle",
  percent: null,
  error: null,
};

/** Gestionnaires des commandes de la lecture à voix haute et des voix. */
export function fakeTts(simulator: Simulator): FakeTts {
  const tts: FakeTts = {
    state: { ...IDLE },
    voices: voiceCatalog(),
    downloads: [],
    languages: new Map(),
    passages: new Map(),
    failures: new Map(),
    install(...ids) {
      for (const id of ids) voice(id).installed = true;
    },
    async setState(patch) {
      tts.state = { ...tts.state, ...patch };
      await simulator.emitEvent("read-aloud", tts.state);
    },
    play: () => tts.setState({ phase: "playing", percent: null }),
    end: (error) =>
      tts.setState(
        error ? { phase: "idle", percent: null, error } : { ...IDLE },
      ),
    emitDownloads: () => simulator.emitEvent("voice-downloads", tts.downloads),
    async progress(id, completed, total) {
      const download = entry(id);
      download.state = "downloading";
      download.completedBytes = completed;
      if (total !== undefined) download.totalBytes = total;
      await tts.emitDownloads();
    },
    async finish(id) {
      const download = entry(id);
      download.state = "done";
      download.completedBytes = download.totalBytes;
      voice(id).installed = true;
      await tts.emitDownloads();
    },
    async fail(id, error) {
      const download = entry(id);
      download.state = "failed";
      download.error = error;
      await tts.emitDownloads();
    },
  };

  function voice(id: string): VoiceModelInfo {
    const found = tts.voices.find((v) => v.id === id);
    if (!found) throw simulator.fail(`no voice "${id}"`);
    return found;
  }

  /** Voix du catalogue, ou le refus du moteur pour un identifiant inconnu. */
  function known(id: string): VoiceModelInfo {
    const found = tts.voices.find((v) => v.id === id);
    if (!found) throw invalid(`unknown voice: ${id}`);
    return found;
  }

  function entry(id: string): SpeechDownload {
    const found = tts.downloads.find((d) => d.id === id);
    if (!found) throw simulator.fail(`no voice download of "${id}"`);
    return found;
  }

  function failure(command: string) {
    const error = tts.failures.get(command);
    if (error) throw error;
  }

  /** Émis par le moteur après la réponse de la commande (état d'alors). */
  function emitLater(event: "read-aloud" | "voice-downloads") {
    setTimeout(() => {
      if (simulator.disposed) return;
      void (event === "read-aloud"
        ? simulator.emitEvent(event, tts.state)
        : tts.emitDownloads());
    }, 0);
  }

  /** Met une voix en file (sans effet si elle y est déjà). */
  function enqueue(found: VoiceModelInfo) {
    const existing = tts.downloads.find((d) => d.id === found.id);
    if (existing && ACTIVE.includes(existing.state)) return;
    if (existing) tts.downloads.splice(tts.downloads.indexOf(existing), 1);
    tts.downloads.push({
      id: found.id,
      name: found.name,
      state: "queued",
      completedBytes: 0,
      totalBytes: found.sizeBytes,
      error: null,
    });
  }

  simulator.handle("read_aloud_state", () => {
    failure("read_aloud_state");
    return tts.state;
  });
  /** Langue « détectée » du texte que nomme `source`. */
  function readingOf(source: ReadAloudSource | null): FakeReading {
    if (source?.kind === "study")
      return tts.passages.get(source.passage) ?? "en";
    const message = source?.kind === "message" ? source.messageId : 0;
    return tts.languages.get(message) ?? "en";
  }

  simulator.handle("read_aloud", (args: Args) => {
    failure("read_aloud");
    const messageId = args.messageId as number;
    if (typeof args.chatId !== "string" || typeof messageId !== "number") {
      throw simulator.fail(`read_aloud: bad arguments ${JSON.stringify(args)}`);
    }
    return read({ kind: "message", messageId }, args.download === true);
  });
  simulator.handle("read_aloud_study", (args: Args) => {
    failure("read_aloud_study");
    const { courseId, passage } = args;
    if (typeof courseId !== "string" || typeof passage !== "string") {
      throw simulator.fail(
        `read_aloud_study: bad arguments ${JSON.stringify(args)}`,
      );
    }
    return read({ kind: "study", courseId, passage }, args.download === true);
  });

  /** Lecture de `source`, comme le moteur (service.rs `read`). */
  function read(source: ReadAloudSource, download: boolean) {
    const reading = readingOf(source);
    if (reading === "other") return { kind: "unsupportedLanguage" };
    if (reading === "empty") return { kind: "nothingToRead" };
    const found = tts.voices.find((v) => v.language === reading);
    if (!found) throw simulator.fail(`no voice for "${reading}"`);
    // Sans ONNX Runtime : ni dialogue ni téléchargement, l'erreur aussitôt.
    if (found.runtimeProblem !== null) {
      tts.state = {
        source,
        phase: "idle",
        percent: null,
        error: "runtimeUnavailable",
      };
      emitLater("read-aloud");
      return { kind: "started" };
    }
    const downloading = tts.downloads.some(
      (d) => d.id === found.id && ACTIVE.includes(d.state),
    );
    if (!found.installed && !download && !downloading) {
      return {
        kind: "needsVoice",
        modelId: found.id,
        language: reading,
        bytes: found.sizeBytes,
      };
    }
    // Une seule lecture : la précédente s'arrête, celle-ci commence.
    if (found.installed) {
      tts.state = { source, phase: "preparing", percent: null, error: null };
    } else {
      enqueue(found);
      emitLater("voice-downloads");
      tts.state = { source, phase: "downloading", percent: 0, error: null };
    }
    emitLater("read-aloud");
    return { kind: "started" };
  }
  simulator.handle("read_aloud_stop", () => {
    failure("read_aloud_stop");
    // Téléchargement attendu par la lecture : annulé avec elle.
    if (tts.state.phase === "downloading") {
      const reading = readingOf(tts.state.source);
      const voice = tts.voices.find((v) => v.language === reading);
      const download = tts.downloads.find((d) => d.id === voice?.id);
      if (download && ACTIVE.includes(download.state)) {
        download.state = "cancelled";
        emitLater("voice-downloads");
      }
    }
    if (tts.state.phase !== "idle") {
      tts.state = { ...IDLE };
      emitLater("read-aloud");
    }
    return null;
  });

  simulator.handle("voice_models", () => {
    failure("voice_models");
    return tts.voices;
  });
  simulator.handle("voice_downloads", () => {
    failure("voice_downloads");
    return tts.downloads;
  });
  simulator.handle("voice_download", (args: Args) => {
    failure("voice_download");
    enqueue(known(args.id as string));
    return null;
  });
  simulator.handle("voice_cancel_download", (args: Args) => {
    failure("voice_cancel_download");
    const download = tts.downloads.find((d) => d.id === args.id);
    if (download && ACTIVE.includes(download.state)) {
      download.state = "cancelled";
    }
    return null;
  });
  simulator.handle("voice_dismiss_download", (args: Args) => {
    failure("voice_dismiss_download");
    const index = tts.downloads.findIndex(
      (d) => d.id === args.id && !ACTIVE.includes(d.state),
    );
    if (index !== -1) tts.downloads.splice(index, 1);
    return null;
  });
  simulator.handle("voice_delete_model", (args: Args) => {
    failure("voice_delete_model");
    const id = args.id as string;
    const found = known(id);
    if (tts.downloads.some((d) => d.id === id && ACTIVE.includes(d.state))) {
      throw invalid("This voice is downloading: cancel the download first");
    }
    found.installed = false;
    return null;
  });

  return tts;
}
