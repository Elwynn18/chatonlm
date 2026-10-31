// Faux moteur de la dictée pour le simulateur (tests vitest seulement) :
// réglages validés comme le moteur (store/dictation.rs), valeurs fixes
// (consignes par défaut, langues), statut (phase, raccourci, dernier
// résultat), catalogue des modèles de transcription (dictation/models.rs) et
// leurs téléchargements en mémoire. Comme le moteur, activer la dictée
// commence la liaison du raccourci (Binding, en tâche de fond : le test la
// termine par shortcutBound(), qui pose `boundShortcut`) et la désactiver le
// délie ; dictation_toggle fait passer de Idle à Recording (dictée activée et
// modèle installé ; sinon le refus devient `lastResult.error`, avec les
// textes du moteur), puis à Transcribing ; dictation_copy_last copie le
// dernier texte (`copied`). Le test fait avancer la dictée par
// complete(), un téléchargement par progress(), finish() et fail(), et force
// un statut par setStatus() : chacun émet l'événement du moteur. Une panne
// d'une commande se force par `failures`.

import type { AppError } from "@/bindings/AppError";
import type { DictationInfo } from "@/bindings/DictationInfo";
import type { DictationLanguage } from "@/bindings/DictationLanguage";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { DictationResult } from "@/bindings/DictationResult";
import type { DictationSettings } from "@/bindings/DictationSettings";
import type { DictationStatus } from "@/bindings/DictationStatus";
import type { ShortcutStatus } from "@/bindings/ShortcutStatus";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { SpeechModelInfo } from "@/bindings/SpeechModelInfo";
import type { Args, Simulator } from "@/test/simulator";

/** Consignes par défaut (début de polish::DEFAULT_PROMPT, assez pour les tests). */
export const DEFAULT_POLISH_PROMPT =
  "You are a dictation cleanup tool, not an assistant. Each user message is a raw speech-to-text transcript between <transcript> tags.\nReply with the cleaned text only, without tags, quotes or comments.";

/** Commande de repli affichée quand le portail manque. */
export const DICTATE_COMMAND = "/usr/bin/chatonlm --dictate";

/** Refus du début d'une dictée (dictation/mod.rs). */
export const DISABLED = "Dictation is turned off in Settings";
export const NO_MODEL = "Choose a speech model in Settings";
export const NOT_INSTALLED = "Speech model not installed";

const lang = (
  code: string,
  name: string,
  parakeet: boolean,
): DictationLanguage => ({ code, name, parakeet });

/** Extrait de languages::LANGUAGES (ordre alphabétique des noms). */
export const LANGUAGES: DictationLanguage[] = [
  lang("ar", "Arabic", false),
  lang("zh", "Chinese", false),
  lang("nl", "Dutch", true),
  lang("en", "English", true),
  lang("fr", "French", true),
  lang("de", "German", true),
  lang("ja", "Japanese", false),
  lang("es", "Spanish", true),
];

/** Catalogue (dictation/models.rs), rien d'installé. */
export function speechCatalog(): SpeechModelInfo[] {
  const whisper = (
    id: string,
    name: string,
    description: string,
    sizeBytes: number,
  ): SpeechModelInfo => ({
    id,
    name,
    engine: "whisper",
    description,
    sizeBytes,
    languages: "99 languages",
    license: "MIT",
    licenseUrl: "https://github.com/openai/whisper/blob/main/LICENSE",
    sourceUrl: "https://huggingface.co/ggerganov/whisper.cpp",
    recommended: false,
    installed: false,
    missingCpuFeatures: [],
    runtimeProblem: null,
  });
  return [
    {
      id: "parakeet-tdt-0.6b-v3",
      name: "Parakeet TDT 0.6B v3",
      engine: "parakeet",
      description:
        "Fast and accurate, with punctuation. Detects the language automatically. Runs well without a GPU.",
      sizeBytes: 670_619_706,
      languages: "25 European languages",
      license: "CC BY 4.0",
      licenseUrl: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
      sourceUrl: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx",
      recommended: true,
      installed: false,
      missingCpuFeatures: [],
      runtimeProblem: null,
    },
    whisper(
      "whisper-tiny",
      "Whisper Tiny",
      "Smallest and fastest Whisper model. Rough accuracy.",
      77_691_713,
    ),
    whisper(
      "whisper-base",
      "Whisper Base",
      "Fast, for clear speech.",
      147_951_465,
    ),
    whisper(
      "whisper-small",
      "Whisper Small",
      "Good balance of speed and accuracy.",
      487_601_967,
    ),
    whisper(
      "whisper-large-v3-turbo",
      "Whisper Large v3 Turbo",
      "Most accurate Whisper model here. Slow without a GPU.",
      574_041_195,
    ),
  ];
}

export const PARAKEET = "parakeet-tdt-0.6b-v3";
export const WHISPER_TINY = "whisper-tiny";

export interface FakeDictation {
  settings: DictationSettings;
  info: DictationInfo;
  status: DictationStatus;
  /** Catalogue, avec les modèles installés. */
  models: SpeechModelInfo[];
  /** Téléchargements, dans l'ordre de demande. */
  downloads: SpeechDownload[];
  /** Raccourci au bout de la liaison (portail qui lie Ctrl+Alt+D). */
  boundShortcut: ShortcutStatus;
  /** Le portail sait ouvrir sa boîte de réglage (version 2), une fois lié. */
  configurable: boolean;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Marque des modèles du catalogue comme installés. */
  install(...ids: string[]): void;
  /** Change le statut puis émet « dictation-status ». */
  setStatus(patch: Partial<DictationStatus>): Promise<void>;
  /** Fin de la liaison par le bureau : `boundShortcut`, puis émission. */
  shortcutBound(): Promise<void>;
  /**
   * Fin de la dictée en cours : résultat (copié s'il a un texte et que la
   * copie automatique est active, sauf `copied` donné), retour au repos,
   * émission.
   */
  complete(result: Partial<DictationResult>): Promise<void>;
  /** Émet la liste des téléchargements (« speech-downloads »). */
  emitDownloads(): Promise<void>;
  /** Fait avancer un téléchargement (état downloading), puis émet. */
  progress(id: string, completed: number, total?: number): Promise<void>;
  /** Termine un téléchargement : modèle installé, puis émet. */
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

/** Réglages par défaut (migrations 0006, 0007 et 0031). */
export function defaultDictationSettings(): DictationSettings {
  return {
    enabled: false,
    mode: "toggle",
    speechModel: "",
    language: "auto",
    polishModel: "",
    polishPrompt: "",
    dictionary: [],
    notifyText: true,
    autoCopy: true,
    autoPaste: true,
  };
}

/** Dictionnaire nettoyé comme clean_dictionary du moteur. */
function cleanDictionary(words: string[]): string[] {
  const kept: string[] = [];
  for (const raw of words) {
    const word = raw.trim();
    if (!word) continue;
    if ([...word].length > 64 || /\p{Cc}/u.test(word)) {
      throw invalid(`invalid dictionary word: ${word}`);
    }
    if (!kept.some((seen) => seen.toLowerCase() === word.toLowerCase())) {
      kept.push(word);
    }
  }
  if (kept.length > 200) {
    throw invalid("the dictionary holds at most 200 words");
  }
  return kept;
}

/** Gestionnaires des commandes de la dictée et des modèles de transcription. */
export function fakeDictation(simulator: Simulator): FakeDictation {
  const dictation: FakeDictation = {
    settings: defaultDictationSettings(),
    info: { defaultPolishPrompt: DEFAULT_POLISH_PROMPT, languages: LANGUAGES },
    status: {
      phase: "idle",
      shortcut: { state: "disabled" },
      canConfigureShortcut: true,
      editor: false,
      trayAvailable: true,
      command: DICTATE_COMMAND,
      lastResult: null,
    },
    models: speechCatalog(),
    downloads: [],
    boundShortcut: { state: "bound", trigger: "Ctrl+Alt+D" },
    configurable: true,
    failures: new Map(),
    install(...ids) {
      for (const id of ids) model(id).installed = true;
    },
    async setStatus(patch) {
      dictation.status = { ...dictation.status, ...patch };
      await simulator.emitEvent("dictation-status", dictation.status);
    },
    async shortcutBound() {
      await dictation.setStatus({
        shortcut: dictation.boundShortcut,
        canConfigureShortcut:
          dictation.configurable && dictation.boundShortcut.state === "bound",
      });
    },
    async complete(result) {
      dictation.status = {
        ...dictation.status,
        phase: "idle",
        lastResult: {
          at: Date.now(),
          text: "",
          raw: "",
          polished: false,
          // Copié s'il y a un texte et que la copie automatique est active.
          copied:
            result.text !== undefined &&
            result.text !== "" &&
            dictation.settings.autoCopy,
          error: null,
          copyFailed: false,
          pasted: false,
          pasteError: null,
          ...result,
        },
      };
      await simulator.emitEvent("dictation-status", dictation.status);
    },
    emitDownloads: () =>
      simulator.emitEvent("speech-downloads", dictation.downloads),
    async progress(id, completed, total) {
      const download = entry(id);
      download.state = "downloading";
      download.completedBytes = completed;
      if (total !== undefined) download.totalBytes = total;
      await dictation.emitDownloads();
    },
    async finish(id) {
      const download = entry(id);
      download.state = "done";
      download.completedBytes = download.totalBytes;
      model(id).installed = true;
      await dictation.emitDownloads();
    },
    async fail(id, error) {
      const download = entry(id);
      download.state = "failed";
      download.error = error;
      await dictation.emitDownloads();
    },
  };

  function model(id: string): SpeechModelInfo {
    const found = dictation.models.find((m) => m.id === id);
    if (!found) throw simulator.fail(`no speech model "${id}"`);
    return found;
  }

  /** Modèle du catalogue, ou le refus du moteur pour un identifiant inconnu. */
  function known(id: string): SpeechModelInfo {
    const found = dictation.models.find((m) => m.id === id);
    if (!found) throw invalid(`unknown speech model: ${id}`);
    return found;
  }

  function entry(id: string): SpeechDownload {
    const found = dictation.downloads.find((d) => d.id === id);
    if (!found) throw simulator.fail(`no speech download of "${id}"`);
    return found;
  }

  function failure(command: string) {
    const error = dictation.failures.get(command);
    if (error) throw error;
  }

  function installedModel(id: string) {
    return dictation.models.some((m) => m.id === id && m.installed);
  }

  /** Liaison commencée (desktop/shortcut.rs, start) : Binding. */
  function startBinding() {
    dictation.status = {
      ...dictation.status,
      shortcut: { state: "binding" },
      canConfigureShortcut: false,
    };
  }

  simulator.handle("get_dictation_settings", () => {
    failure("get_dictation_settings");
    return dictation.settings;
  });
  // Comme le moteur : tout est validé avant d'écrire, réponse neuve.
  simulator.handle("update_dictation_settings", (args: Args) => {
    failure("update_dictation_settings");
    const patch = args.patch as DictationPatch;
    if (
      patch.speechModel !== undefined &&
      patch.speechModel !== "" &&
      !dictation.models.some((m) => m.id === patch.speechModel)
    ) {
      throw invalid(`unknown speech model: ${patch.speechModel}`);
    }
    if (
      patch.language !== undefined &&
      patch.language !== "auto" &&
      !dictation.info.languages.some((l) => l.code === patch.language)
    ) {
      throw invalid(`unsupported language: ${patch.language}`);
    }
    if (
      patch.polishModel !== undefined &&
      (patch.polishModel.trim() !== patch.polishModel ||
        patch.polishModel.length > 256)
    ) {
      throw invalid(`invalid model name: ${patch.polishModel}`);
    }
    if (
      patch.polishPrompt !== undefined &&
      [...patch.polishPrompt].length > 4000
    ) {
      throw invalid("the instructions hold at most 4000 characters");
    }
    const dictionary =
      patch.dictionary === undefined
        ? undefined
        : cleanDictionary(patch.dictionary);
    const settings: DictationSettings = { ...dictation.settings, ...patch };
    if (dictionary !== undefined) settings.dictionary = dictionary;
    if (patch.polishPrompt !== undefined) {
      const prompt = patch.polishPrompt.trim();
      settings.polishPrompt =
        prompt === DEFAULT_POLISH_PROMPT.trim() ? "" : prompt;
    }
    const wanted = dictation.settings.enabled;
    dictation.settings = settings;
    // Appliqué ensuite (desktop::apply) : liaison commencée ou raccourci délié.
    if (settings.enabled && !wanted) startBinding();
    if (!settings.enabled && wanted) {
      dictation.status = {
        ...dictation.status,
        shortcut: { state: "disabled" },
        canConfigureShortcut: false,
      };
    }
    return settings;
  });
  simulator.handle("dictation_info", () => {
    failure("dictation_info");
    return dictation.info;
  });
  simulator.handle("dictation_status", () => {
    failure("dictation_status");
    return dictation.status;
  });
  simulator.handle("dictation_toggle", () => {
    failure("dictation_toggle");
    const { phase } = dictation.status;
    if (phase === "idle") {
      // Refus (notification d'échec), dans l'ordre du moteur : il devient
      // le dernier résultat.
      const { enabled, speechModel } = dictation.settings;
      const refused = !enabled
        ? DISABLED
        : !speechModel
          ? NO_MODEL
          : !installedModel(speechModel)
            ? NOT_INSTALLED
            : null;
      dictation.status = refused
        ? {
            ...dictation.status,
            lastResult: {
              at: Date.now(),
              text: "",
              raw: "",
              polished: false,
              copied: false,
              error: refused,
              copyFailed: false,
              pasted: false,
              pasteError: null,
            },
          }
        : { ...dictation.status, phase: "recording" };
    } else if (phase === "recording") {
      dictation.status = { ...dictation.status, phase: "transcribing" };
    }
    return null;
  });
  // Copie du dernier texte (copie automatique coupée, ou à refaire) :
  // `copied` passe à vrai, l'erreur reste (comme le moteur).
  simulator.handle("dictation_copy_last", () => {
    failure("dictation_copy_last");
    const last = dictation.status.lastResult;
    if (!last?.text) throw invalid("Nothing to copy");
    dictation.status = {
      ...dictation.status,
      lastResult: { ...last, copied: true },
    };
    return null;
  });
  simulator.handle("dictation_cancel", () => {
    failure("dictation_cancel");
    // Comme le moteur : pas la dictée de l'éditeur.
    if (!dictation.status.editor) {
      dictation.status = { ...dictation.status, phase: "idle" };
    }
    return null;
  });
  simulator.handle("dictation_stop_editor", () => {
    failure("dictation_stop_editor");
    // Statut envoyé après la réponse, comme l'événement du moteur.
    if (dictation.status.editor) {
      setTimeout(() => {
        if (!simulator.disposed) {
          void dictation.setStatus({ phase: "idle", editor: false });
        }
      }, 0);
    }
    return null;
  });
  simulator.handle("dictation_bind_shortcut", () => {
    failure("dictation_bind_shortcut");
    if (!dictation.settings.enabled) throw invalid("Turn on dictation first");
    startBinding();
    return null;
  });
  simulator.handle("dictation_configure_shortcut", () => {
    failure("dictation_configure_shortcut");
    if (dictation.status.shortcut.state !== "bound") {
      throw invalid("The shortcut is not set yet");
    }
    return null;
  });

  simulator.handle("speech_models", () => {
    failure("speech_models");
    return dictation.models;
  });
  simulator.handle("speech_downloads", () => {
    failure("speech_downloads");
    return dictation.downloads;
  });
  simulator.handle("speech_download", (args: Args) => {
    failure("speech_download");
    const id = args.id as string;
    const found = known(id);
    const existing = dictation.downloads.find((d) => d.id === id);
    if (existing && ACTIVE.includes(existing.state)) return null;
    if (existing)
      dictation.downloads.splice(dictation.downloads.indexOf(existing), 1);
    dictation.downloads.push({
      id,
      name: found.name,
      state: "queued",
      completedBytes: 0,
      totalBytes: found.sizeBytes,
      error: null,
    });
    return null;
  });
  simulator.handle("speech_cancel_download", (args: Args) => {
    failure("speech_cancel_download");
    const download = dictation.downloads.find((d) => d.id === args.id);
    if (download && ACTIVE.includes(download.state)) {
      download.state = "cancelled";
    }
    return null;
  });
  simulator.handle("speech_dismiss_download", (args: Args) => {
    failure("speech_dismiss_download");
    const index = dictation.downloads.findIndex(
      (d) => d.id === args.id && !ACTIVE.includes(d.state),
    );
    if (index !== -1) dictation.downloads.splice(index, 1);
    return null;
  });
  simulator.handle("speech_delete_model", (args: Args) => {
    failure("speech_delete_model");
    const id = args.id as string;
    const found = known(id);
    if (
      dictation.downloads.some((d) => d.id === id && ACTIVE.includes(d.state))
    ) {
      throw invalid("This model is downloading: cancel the download first");
    }
    found.installed = false;
    return null;
  });

  return dictation;
}
