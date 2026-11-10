// Reste du backend pour le simulateur : réglages (validés comme le moteur,
// échecs forcés par settingsErrors), modèles du moteur (liste du sélecteur,
// détails, réglage Think mémorisé, recommandations compris) en mémoire, et
// une base de conversations tenue à jour par les événements
// rejoués, comme celle du moteur (message enregistré avant la requête, réponse
// créée au premier fragment et superposée à get_chat pendant le flux, réponse
// partielle gardée, modèle installé à la fin de son téléchargement). Pièces
// jointes : sélecteur natif, disque simulé et zone de préparation (validation
// de stage_bytes comme le moteur, fichiers envoyés retirés au message
// enregistré). ModelHub : faux moteur de src/test/hub.ts (backend.hub), qui
// partage la liste des modèles installés. Moteur llama.cpp (état, journaux,
// dossier des modèles et son changement) : faux moteur de src/test/engine.ts
// (backend.engine) ; reprise des modèles d'Ollama : src/test/modelImport.ts
// (backend.modelImport). Dictée et modèles de transcription : faux
// moteur de src/test/dictation.ts (backend.dictation). quit_app répond sans
// rien faire (appels relus par simulator.callsTo). Modèles en mémoire
// (onglet Models, préchargement) : liste du moteur tenue par load_model,
// unload_model et unload_all_models, échecs forcés
// par runningErrors. Favoris des conversations : set_chat_favorite, sans
// toucher à la date d'activité, comme le moteur. Correction (page Proofread)
// et raccourcis du système : faux moteur de src/test/proofread.ts
// (backend.proofread), qui lit les réglages et les modèles installés. Cours
// (onglet Study) : faux moteur de src/test/study.ts (backend.study), qui lit
// les réglages et reprend les fichiers de la zone de préparation. Entrée du
// menu des applications (AppImage) : menu_entry_status et set_menu_entry sur
// menuEntry, échec forcé par menuEntryError (choix enregistré quand même,
// comme le moteur). Croix de la fenêtre : close_button_status et
// set_close_to_tray sur closeButton.
// Lecture à voix haute et voix : faux moteur de src/test/tts.ts
// (backend.tts), réglage autoRead coupé par défaut. Mode conversation
// vocale : faux moteur de src/test/voiceMode.ts (backend.voiceMode), qui lit
// les faux moteurs de la dictée et des voix pour ses prérequis.
// Modèles d'images : faux moteur de src/test/images.ts (backend.images), qui
// lit le réglage showUncensored.
// Mises à jour de ChatonLM : update_status et app_versions sur updates et
// versions, set_auto_download change updates.autoDownload ; check_for_updates,
// download_update et restart_to_update répondent sans rien changer (le test
// émet « update-status »), échecs forcés par updateErrors.
// Système : app_platform répond `platform` (« linux » par défaut) ; un essai
// d'un écran sous Windows appelle aussi setPlatform (src/lib/platform.ts), que
// main.tsx appelle dans l'app.
// Easy Link : faux moteur de src/test/easyLink.ts (backend.easyLink) : deux
// bots sans jeton, aucun compte lié, pas de code d'appairage. Une
// conversation venue d'un bot ou de l'extension VS Code porte son origine
// (`source`, rendue par list_chats ; null pour une conversation de l'app).
// Pont de l'extension VS Code : faux moteur de src/test/bridge.ts
// (backend.bridge) : pont coupé, aucun éditeur appairé, pas de code.
// Contexte (propre à ChatonLM) : les événements context et compacted
// rejoués mettent à jour la conversation enregistrée ; compact_chat la
// compacte selon backend.compaction.
// Images créées (propre à ChatonLM) : imageReady rejoué ajoute la pièce
// jointe à la réponse enregistrée (imageProgress n'enregistre rien) ;
// save_image note la pièce jointe dans imageSaves et répond imageSave
// (vrai : écrite, faux : annulé, ou l'erreur renvoyée).
// Pour les tests seulement.

import type { AppVersions } from "@/bindings/AppVersions";
import type { AppError } from "@/bindings/AppError";
import type { AttachmentKind } from "@/bindings/AttachmentKind";
import type { Chat } from "@/bindings/Chat";
import type { CloseButtonStatus } from "@/bindings/CloseButtonStatus";
import type { Compaction } from "@/bindings/Compaction";
import type { ContextUsage } from "@/bindings/ContextUsage";
import type { MenuEntryStatus } from "@/bindings/MenuEntryStatus";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { ChatInfo } from "@/bindings/ChatInfo";
import type { ChatModel } from "@/bindings/ChatModel";
import type { ChatSource } from "@/bindings/ChatSource";
import type { Message } from "@/bindings/Message";
import type { ModelDetails } from "@/bindings/ModelDetails";
import type { EngineRunningModel } from "@/bindings/EngineRunningModel";
import type { MachineMemory } from "@/bindings/MachineMemory";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import type { StagedFile } from "@/bindings/StagedFile";
import type { StageResult } from "@/bindings/StageResult";
import type { Theme } from "@/bindings/Theme";
import type { Think } from "@/bindings/Think";
import type { ThinkingOptions } from "@/bindings/ThinkingOptions";
import type { UpdateStatus } from "@/bindings/UpdateStatus";
import { isRemoteModel, sameModel } from "@/lib/modelNames";
import type { Platform } from "@/lib/platform";
import { autoModelOf } from "@/test/autoModel";
import { type FakeBridge, fakeBridge } from "@/test/bridge";
import { type FakeDictation, fakeDictation } from "@/test/dictation";
import { type FakeEasyLink, fakeEasyLink } from "@/test/easyLink";
import { type FakeEngine, fakeEngine } from "@/test/engine";
import { canonicalModel, type FakeHub, fakeHub } from "@/test/hub";
import { type FakeImages, fakeImages } from "@/test/images";
import { type FakeModelImport, fakeModelImport } from "@/test/modelImport";
import { type FakeProofread, fakeProofread } from "@/test/proofread";
import { installRemote } from "@/test/remote";
import {
  type Args,
  type RawBody,
  type Sequence,
  SEQUENCE_TIME,
  type Simulator,
} from "@/test/simulator";
import { type FakeStudy, fakeStudy } from "@/test/study";
import { type FakeTts, fakeTts } from "@/test/tts";
import { type FakeVoiceMode, fakeVoiceMode } from "@/test/voiceMode";
import { installWebSearch } from "@/test/webSearch";

/** Crans de la longueur de contexte (CONTEXT_LENGTHS du moteur) ; 0 = défaut. */
const CONTEXT_LENGTHS = [0, 4096, 8192, 16384, 32768, 65536, 131072, 262144];

const THEMES: readonly Theme[] = ["system", "light", "dark"];

/** Durées avant déchargement acceptées (KEEP_ALIVES du moteur), en secondes. */
const KEEP_ALIVES = [0, 60, 300, 900, 1800, 3600, 14400, -1];
/** `MAX_SYSTEM_PROMPT_CHARS` du moteur (store/settings.rs). */
const MAX_SYSTEM_PROMPT_CHARS = 10_000;

/** Taille d'un modèle du moteur : 1 Gio. */
export const RUNNING_SIZE = 1024 ** 3;

/**
 * Modèle prêt dans le moteur : 1 Gio, tout sur la carte (28 couches sur
 * 28), contexte 4096, déchargé dans 5 min (à partir de maintenant).
 */
export function runningModel(
  id: string,
  fields: Partial<EngineRunningModel> = {},
): EngineRunningModel {
  return {
    id,
    state: "ready",
    sizeBytes: RUNNING_SIZE,
    vramBytes: RUNNING_SIZE,
    contextLength: 4096,
    expiresAt: Date.now() + 300_000,
    kept: false,
    inUse: false,
    gpuLayers: 28,
    totalLayers: 28,
    cpuFallback: false,
    notice: null,
    cpuOnly: false,
    paramsLabel: "1B",
    ...fields,
  };
}

/** Longueur de l'extrait de l'historique (EXCERPT_CHARS du store). */
const EXCERPT_CHARS = 100;

/** Taille maximale d'un fichier : 10 Mio (MAX_FILE_SIZE du moteur). */
export const MAX_FILE_SIZE = 10 * 1024 * 1024;

/** Extensions acceptées par le moteur (attachments/mod.rs, sans docx). */
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp"];
// prettier-ignore
const TEXT_EXTENSIONS = [
  "pdf", "txt", "md", "csv", "json", "xml", "html", "htm", "js", "jsx", "ts",
  "tsx", "py", "java", "cpp", "c", "cc", "h", "cs", "php", "rb", "go", "rs",
  "swift", "kt", "scala", "sh", "bat", "yaml", "yml", "toml", "ini", "cfg",
  "conf", "log", "rtf",
];

/** Refus officiels, montrés tels quels par l'interface. */
const FILE_TYPE_NOT_SUPPORTED = "File type not supported";
const FILE_TOO_LARGE = "File too large";
const ERROR_READING_FILE = "Error reading file";

/**
 * Fichier du disque simulé (stage_paths) ; champ absent : déduit du nom
 * (sorte, type MIME), ou 1 octet.
 */
export interface FakeFile {
  size?: number;
  kind?: AttachmentKind;
  mime?: string;
}

/** Fichier du disque simulé, ou refus renvoyé tel quel par stage_paths. */
type FakeEntry = FakeFile | { error: string };

/**
 * Conversation enregistrée : celle de get_chat, plus son rang de favori et
 * la messagerie d'où elle vient (list_chats seulement ; absents : non
 * favorite, conversation de l'app). Contexte, compactages et réponse d'un
 * bot en cours facultatifs (absents : taille inconnue, aucun compactage,
 * aucune réponse d'Easy Link en cours).
 */
export type StoredChat = Omit<
  Chat,
  "context" | "compactions" | "replyRunning" | "ephemeral"
> & {
  context?: ContextUsage | null;
  compactions?: Compaction[];
  replyRunning?: number | null;
  /** Conversation éphémère (absente : non). */
  ephemeral?: boolean;
  favorite?: boolean;
  source?: ChatSource | null;
};

/**
 * Clé d'un ancien nom d'Ollama (`ollama_key` du moteur, sans les hôtes) :
 * sans la casse, `:latest` ajouté s'il n'a pas d'étiquette.
 */
function ollamaKey(name: string): string {
  const key = name.trim().toLowerCase();
  return key.split("/").at(-1)?.includes(":") ? key : `${key}:latest`;
}

/**
 * Modèle du sélecteur (list_models) : installé (origine Hugging Face), ou
 * distant pour un identifiant `@remote/…` ; `installedAt` donne sa
 * version (bandeau des mises à jour).
 */
export function modelInfo(
  id: string,
  fields: Partial<ChatModel> = {},
): ChatModel {
  const remote = isRemoteModel(id);
  return {
    id,
    displayName: id,
    origin: remote ? "remote" : id.startsWith("@ollama/") ? "ollama" : "hf",
    provider: remote ? (id.split("/")[1] ?? null) : null,
    sizeBytes: remote ? null : 1,
    paramsLabel: "1B",
    quant: "Q4_K_M",
    contextMax: null,
    capabilities: ["completion"],
    installedAt: remote ? null : 1_700_000_000_000,
    ...fields,
  };
}

/**
 * Détails d'un modèle qui sait raisonner ; `thinking` : objet de /api/show
 * (null : pas de métadonnées).
 */
export function thinkingModel(
  model: string,
  thinking: ThinkingOptions | null,
): ModelDetails {
  return {
    model,
    capabilities: ["completion", "thinking"],
    thinking,
    alwaysThinks: false,
  };
}

export interface FakeBackend {
  settings: Settings;
  /**
   * Échecs forcés de get_settings et update_settings : la commande renvoie
   * cette erreur tant qu'elle est posée (null : réponse normale).
   */
  settingsErrors: { get: AppError | null; update: AppError | null };
  /**
   * Réponses de model_details par modèle : détails, ou erreur renvoyée. Un
   * modèle installé absent d'ici n'a que la capacité completion (pas de
   * bouton Think, comme les séquences enregistrées sans think) ; un modèle
   * non installé donne l'erreur d'un modèle absent.
   */
  details: Map<string, ModelDetails | AppError>;
  /** Réglages Think mémorisés (model_prefs), par modèle. */
  thinkPrefs: Map<string, Think>;
  /**
   * Modèles installés (list_models), du plus récent au plus ancien ; un
   * téléchargement terminé (download done) y ajoute le modèle en tête.
   */
  models: string[];
  /**
   * Table des noms repris d'Ollama (ancien nom → identifiant) : comme le
   * moteur (`store::model_import::renamed`), update_settings y traduit
   * selectedModel, proofreadModel et studyModel (sans la casse, `latest`
   * pour un nom sans étiquette) ; vide par défaut.
   */
  renamed: Map<string, string>;
  /** Réponse de recommended_models. */
  recommended: RecommendedModel[];
  /**
   * Modèles installés dont Hugging Face a une version plus récente
   * (outdated_models, comparés sans la casse) ; un modèle téléchargé de
   * nouveau par hub.finish() en sort.
   */
  outdated: Set<string>;
  /** outdated_models rejette cette erreur. */
  outdatedError: AppError | null;
  /** Conversations enregistrées, par identifiant. */
  chats: Map<string, StoredChat>;
  /**
   * Issue de compact_chat (propre à ChatonLM) : résumé et jetons comptés
   * ensuite (taille gardée, si elle est connue) ; `error` : refus renvoyé.
   */
  compaction: { summary: string; used: number; error: AppError | null };
  /**
   * Disque lu par stage_paths, par chemin : un chemin absent d'ici est
   * illisible (« Error reading file »).
   */
  files: Map<string, FakeEntry>;
  /**
   * Zone de préparation : fichiers préparés, ni retirés (unstage) ni partis
   * avec un message enregistré.
   */
  staged: Map<string, StagedFile>;
  /** Réponses des prochains pick_files ; plus rien : annulé (liste vide). */
  picks: string[][];
  /**
   * Image du presse-papiers lue par stage_clipboard_image (préparée sous le
   * nom image.png, taille limitée comme stage_bytes) ; null : aucune.
   */
  clipboardImage: { size: number } | null;
  /**
   * Modèles du moteur (running_models) ; load_model y ajoute un modèle
   * installé (contexte et durée des réglages), unload_model et
   * unload_all_models l'en retirent.
   */
  running: EngineRunningModel[];
  /**
   * Mémoire de la machine (machine_memory, jauge de la VRAM) ; null :
   * illisible. Par défaut, une carte de 24 Go dont la mémoire libre est
   * inconnue (pas d'autres usages).
   */
  machine: MachineMemory | null;
  /**
   * Échecs forcés de running_models, load_model (par modèle) et
   * unload_model / unload_all_models (null : réponse normale).
   */
  runningErrors: {
    list: AppError | null;
    load: Map<string, AppError>;
    unload: AppError | null;
  };
  /** Réponse de app_platform : « linux » par défaut. */
  platform: Platform;
  /**
   * Entrée du menu des applications (menu_entry_status) : `available` faux
   * par défaut (pas d'AppImage) ; set_menu_entry change `enabled`.
   */
  menuEntry: MenuEntryStatus;
  /**
   * Croix de la fenêtre (close_button_status) : zone de notification présente
   * et réglage « masquer » par défaut ; set_close_to_tray change le réglage.
   */
  closeButton: CloseButtonStatus;
  /**
   * Échec forcé de set_menu_entry (fichier impossible à écrire) : le choix
   * est enregistré, puis cette erreur renvoyée (null : réponse normale).
   */
  menuEntryError: AppError | null;
  /**
   * Réponse de update_status : à jour, Auto-download activé, AppImage, sans
   * échec gardé.
   */
  updates: UpdateStatus;
  /** Réponse de app_versions : ChatonLM 0.9.0, sans moteur livré. */
  versions: AppVersions;
  /**
   * Échecs forcés de check_for_updates, download_update, restart_to_update
   * et set_auto_download (null : réponse normale).
   */
  updateErrors: {
    check: AppError | null;
    download: AppError | null;
    restart: AppError | null;
    autoDownload: AppError | null;
  };
  /** Enregistre une séquence complète sans la livrer (conversation existante). */
  record(sequence: Sequence): void;
  /** Faux moteur du ModelHub (commandes hub_*). */
  hub: FakeHub;
  /** Faux moteur llama.cpp (engine_*, pick_models_folder, change_engine_models_dir). */
  engine: FakeEngine;
  /** Faux moteur de la reprise des modèles d'Ollama (model_import_*). */
  modelImport: FakeModelImport;
  /** Faux moteur de la dictée (commandes de la dictée et speech_*). */
  dictation: FakeDictation;
  /** Faux moteur de la correction (proofread_*) et des raccourcis (shortcuts_*). */
  proofread: FakeProofread;
  /** Faux moteur des cours (study_*). */
  study: FakeStudy;
  /** Faux moteur de la lecture à voix haute (read_aloud*, voice_*). */
  tts: FakeTts;
  /** Faux moteur du mode conversation vocale (voice_mode_*). */
  voiceMode: FakeVoiceMode;
  /** Faux moteur d'Easy Link (easy_link_*). */
  easyLink: FakeEasyLink;
  /** Faux moteur du pont de l'extension VS Code (bridge_*). */
  bridge: FakeBridge;
  /** Faux moteur des modèles d'images (image_*). */
  images: FakeImages;
  /** Pièces jointes demandées à save_image, dans l'ordre. */
  imageSaves: number[];
  /** Réponse de save_image (vrai par défaut), ou l'erreur renvoyée. */
  imageSave: boolean | AppError;
}

/**
 * Gestionnaires de get_settings, update_settings, list_models,
 * recommended_models, outdated_models, model_details, get_think_pref,
 * set_think_pref, get_chat, list_chats, set_chat_favorite,
 * compact_chat (backend.compaction),
 * pick_files,
 * stage_paths, stage_bytes, stage_clipboard_image, unstage, les commandes
 * hub_* (src/test/hub.ts), engine_status, open_engine_logs,
 * engine_models_dir, pick_models_folder, change_engine_models_dir
 * (src/test/engine.ts), model_import_* (src/test/modelImport.ts),
 * get_dictation_settings, update_dictation_settings,
 * dictation_info, dictation_status, dictation_toggle, dictation_cancel,
 * dictation_copy_last,
 * dictation_bind_shortcut, dictation_configure_shortcut, speech_models,
 * speech_downloads, speech_download, speech_cancel_download,
 * speech_dismiss_download, speech_delete_model (src/test/dictation.ts),
 * running_models, machine_memory, load_model, unload_model,
 * unload_all_models, quit_app,
 * proofread_run, proofread_cancel, proofread_last_selection et shortcuts_*
 * (src/test/proofread.ts), study_* (src/test/study.ts), read_aloud,
 * read_aloud_stop, read_aloud_state et voice_* (src/test/tts.ts),
 * voice_mode_*, get_voice_mode_settings et update_voice_mode_settings
 * (src/test/voiceMode.ts), easy_link_* (src/test/easyLink.ts), bridge_*
 * (src/test/bridge.ts), image_*
 * (src/test/images.ts), save_image (backend.imageSave), menu_entry_status,
 * set_menu_entry, update_status, app_versions, check_for_updates,
 * download_update, restart_to_update et set_auto_download,
 * branchés sur le simulateur ; un send_message qui joint un
 * fichier non préparé fait échouer le test, sauf pour une séquence refusée.
 */
export function fakeBackend(
  simulator: Simulator,
  {
    selectedModel = "qwen3:0.6b",
    models = ["qwen3:0.6b", "qwen3.5:0.8b"],
    details = [],
    thinkPrefs = {},
    recommended = [],
    files = {},
  }: {
    selectedModel?: string;
    models?: string[];
    details?: (ModelDetails | [string, AppError])[];
    thinkPrefs?: Record<string, Think>;
    /**
     * Modèles recommandés : un identifiant seul, ou une entrée dont la
     * mémoire et la pastille sont absentes par défaut.
     */
    recommended?: (
      string | (Pick<RecommendedModel, "model"> & Partial<RecommendedModel>)
    )[];
    /** Disque lu par stage_paths, par chemin. */
    files?: Record<string, FakeEntry>;
  } = {},
): FakeBackend {
  const backend: Omit<
    FakeBackend,
    | "hub"
    | "engine"
    | "modelImport"
    | "dictation"
    | "proofread"
    | "study"
    | "tts"
    | "voiceMode"
    | "easyLink"
    | "bridge"
    | "images"
  > & {
    hub?: FakeHub;
    engine?: FakeEngine;
    modelImport?: FakeModelImport;
    dictation?: FakeDictation;
    proofread?: FakeProofread;
    study?: FakeStudy;
    tts?: FakeTts;
    voiceMode?: FakeVoiceMode;
    easyLink?: FakeEasyLink;
    bridge?: FakeBridge;
    images?: FakeImages;
  } = {
    settings: {
      formerServerUrl: "",
      contextLength: 0,
      theme: "system",
      selectedModel,
      windowWidth: null,
      windowHeight: null,
      webSearchEnabled: false,
      keepAlive: 0,
      preloadModel: false,
      language: "system",
      effectiveLanguage: "en",
      proofreadModel: "",
      proofreadShortcut: false,
      proofreadPaste: true,
      readAloudShortcut: false,
      proofreadKeepOrder: false,
      proofreadFormatting: true,
      proofreadDigits: false,
      studyModel: "",
      autoRead: false,
      onboardingDone: true,
      showUncensored: false,
      systemPrompt: "",
      remoteModels: false,
    },
    settingsErrors: { get: null, update: null },
    details: new Map<string, ModelDetails | AppError>(
      details.map((entry) =>
        Array.isArray(entry) ? entry : [entry.model, entry],
      ),
    ),
    thinkPrefs: new Map(Object.entries(thinkPrefs)),
    models: [...models],
    renamed: new Map(),
    recommended: recommended.map((entry) =>
      typeof entry === "string"
        ? {
            model: entry,
            displayName: entry,
            sizeBytes: null,
            vramBytes: null,
            fit: null,
          }
        : {
            displayName: entry.model,
            sizeBytes: null,
            vramBytes: null,
            fit: null,
            ...entry,
          },
    ),
    outdated: new Set(),
    outdatedError: null,
    chats: new Map(),
    compaction: {
      summary: "The user asked for a greeting.",
      used: 200,
      error: null,
    },
    files: new Map(Object.entries(files)),
    staged: new Map(),
    picks: [],
    clipboardImage: null,
    running: [],
    machine: {
      gpus: [
        {
          name: "NVIDIA GeForce RTX 3090",
          vramTotal: 24 * 1024 ** 3,
          vramFree: null,
        },
      ],
      ramTotal: 64 * 1024 ** 3,
      ramAvailable: 48 * 1024 ** 3,
    },
    runningErrors: { list: null, load: new Map(), unload: null },
    platform: "linux",
    menuEntry: { available: false, enabled: true },
    closeButton: { trayAvailable: true, closeToTray: true },
    menuEntryError: null,
    updates: {
      state: { state: "upToDate" },
      autoDownload: true,
      checkedAt: SEQUENCE_TIME,
      installer: "appImage",
      lastError: null,
    },
    versions: { chatonlm: "0.9.0", llamaCpp: null },
    imageSaves: [],
    imageSave: true,
    updateErrors: {
      check: null,
      download: null,
      restart: null,
      autoDownload: null,
    },
    record(sequence) {
      const context = {
        chatId: sequence.request.chatId ?? null,
        model: sequence.request.model,
        ephemeral: sequence.request.ephemeral === true,
      };
      for (const event of sequence.events) {
        if (event.type === "chatCreated") context.chatId = event.chatId;
        apply(event, context, SEQUENCE_TIME);
      }
    },
  };

  backend.hub = fakeHub(simulator, backend);
  backend.engine = fakeEngine(simulator);
  // Changement de dossier accepté : le moteur décharge les modèles.
  backend.engine.onAccepted = () => {
    backend.running = [];
  };
  backend.modelImport = fakeModelImport(simulator);
  backend.dictation = fakeDictation(simulator);
  backend.proofread = fakeProofread(simulator, backend);
  backend.study = fakeStudy(simulator, backend);
  backend.tts = fakeTts(simulator);
  backend.voiceMode = fakeVoiceMode(simulator, {
    dictation: backend.dictation,
    tts: backend.tts,
  });
  backend.easyLink = fakeEasyLink(simulator);
  backend.bridge = fakeBridge(simulator);
  backend.images = fakeImages(simulator, () => backend.settings.showUncensored);
  installRemote(simulator);
  installWebSearch(simulator);
  simulator.handle("quit_app", () => null);
  simulator.handle("save_image", (args: Args) => {
    backend.imageSaves.push(args.attachmentId as number);
    if (typeof backend.imageSave !== "boolean") throw backend.imageSave;
    return backend.imageSave;
  });
  simulator.handle("app_platform", () => backend.platform);
  simulator.handle("close_button_status", () => ({ ...backend.closeButton }));
  simulator.handle("set_close_to_tray", (args: Args) => {
    backend.closeButton.closeToTray = args.enabled as boolean;
    return { ...backend.closeButton };
  });
  simulator.handle("menu_entry_status", () => ({ ...backend.menuEntry }));
  simulator.handle("set_menu_entry", (args: Args) => {
    backend.menuEntry.enabled = args.enabled as boolean;
    if (backend.menuEntryError) throw backend.menuEntryError;
    return { ...backend.menuEntry };
  });
  simulator.handle("update_status", () => backend.updates);
  simulator.handle("app_versions", () => backend.versions);
  simulator.handle("check_for_updates", () => {
    if (backend.updateErrors.check) throw backend.updateErrors.check;
    return null;
  });
  simulator.handle("download_update", () => {
    if (backend.updateErrors.download) throw backend.updateErrors.download;
    return null;
  });
  simulator.handle("restart_to_update", () => {
    if (backend.updateErrors.restart) throw backend.updateErrors.restart;
    return null;
  });
  simulator.handle("set_auto_download", (args: Args) => {
    if (backend.updateErrors.autoDownload) {
      throw backend.updateErrors.autoDownload;
    }
    backend.updates = {
      ...backend.updates,
      autoDownload: args.enabled as boolean,
    };
    return null;
  });

  /** Applique un événement à la base, comme le moteur l'a enregistré. */
  function apply(
    event: ChatEvent,
    {
      chatId,
      model,
      ephemeral = false,
    }: { chatId: string | null; model: string; ephemeral?: boolean },
    now: number,
  ) {
    if (event.type === "download") {
      // Modèle téléchargé : installé, le plus récent.
      if (event.done && !backend.models.includes(model)) {
        backend.models.unshift(model);
      }
      return;
    }
    if (event.type === "chatCreated") {
      backend.chats.set(event.chatId, {
        id: event.chatId,
        title: "",
        createdAt: now,
        updatedAt: now,
        messages: [],
        ...(ephemeral && { ephemeral }),
      });
      return;
    }
    const chat = chatId ? backend.chats.get(chatId) : undefined;
    if (!chat) return;
    const reply = chat.messages.at(-1);
    const answering = reply?.role === "assistant" ? reply : null;
    switch (event.type) {
      case "userMessage": {
        // Message modifié (même id) : il remplace l'ancien, les suivants
        // disparaissent, comme la transaction du moteur.
        const edited = chat.messages.findIndex(
          (m) => m.id === event.message.id,
        );
        if (edited !== -1) chat.messages.splice(edited);
        chat.messages.push(event.message);
        chat.updatedAt = Math.max(chat.updatedAt, event.message.updatedAt);
        break;
      }
      case "replyStarted":
        chat.messages.push(emptyReply(event.messageId, event.createdAt, model));
        break;
      case "thinking":
        if (!answering) break;
        answering.thinking += event.text;
        answering.thinkingStartedAt ??= now;
        break;
      case "content":
        if (!answering) break;
        answering.content += event.text;
        if (answering.thinkingStartedAt !== null) {
          answering.thinkingEndedAt ??= now;
        }
        break;
      case "toolCalls": {
        // Fin d'un tour d'outils : appels enregistrés, raisonnement clos.
        const target = chat.messages.find((m) => m.id === event.messageId);
        if (!target) break;
        target.toolCalls = event.toolCalls.map((call) => ({ ...call }));
        if (target.thinkingStartedAt !== null) target.thinkingEndedAt ??= now;
        break;
      }
      case "toolResult": {
        const target = chat.messages.find((m) => m.id === event.messageId);
        const call = target?.toolCalls[event.index];
        if (!call) break;
        call.result = event.result;
        call.error = event.error;
        break;
      }
      case "imageReady": {
        // Image enregistrée en pièce jointe de la réponse.
        const target = chat.messages.find((m) => m.id === event.messageId);
        target?.attachments.push({ ...event.attachment });
        break;
      }
      case "imageProgress":
        // Avancement d'une image : rien d'enregistré.
        break;
      case "loading":
      case "loaded":
      case "compacting":
        // Chargement du modèle, compactage commencé : rien d'enregistré.
        break;
      case "context":
        chat.context = { used: event.used, size: event.size };
        break;
      case "compacted":
        chat.compactions = [...(chat.compactions ?? []), event.compaction];
        chat.context = event.context;
        break;
      default:
        // Fin, quelle qu'elle soit : raisonnement clos, réponse partielle
        // gardée.
        if (answering && answering.thinkingStartedAt !== null) {
          answering.thinkingEndedAt ??= now;
        }
    }
  }

  simulator.onEvent((event, replay) => {
    // Message enregistré : ses fichiers quittent la zone de préparation.
    if (event.type === "userMessage") {
      for (const id of replay.request?.attachmentIds ?? []) {
        backend.staged.delete(id);
      }
    }
    apply(
      event,
      {
        chatId: replay.chatId,
        model: replay.request?.model ?? "",
        ephemeral: replay.request?.ephemeral === true,
      },
      Date.now(),
    );
  });

  // Le moteur refuse d'emblée un fichier qui n'est plus préparé : seule une
  // séquence refusée peut le rejouer.
  simulator.onSend((request, replay) => {
    if (replay.sequence.error !== null) return;
    const missing = (request.attachmentIds ?? []).filter(
      (id) => !backend.staged.has(id),
    );
    if (missing.length > 0) {
      throw simulator.fail(
        `send_message for "${replay.name}": ${missing.join(", ")} not staged (staged: ${[...backend.staged.keys()].join(", ") || "none"})`,
      );
    }
  });

  /** Dernier identifiant de préparation donné (jamais réutilisé). */
  let lastStagedId = 0;

  /** Met un fichier accepté dans la zone de préparation. */
  function stage(filename: string, file: FakeFile, size: number): StageResult {
    const extension = extensionOf(filename);
    lastStagedId += 1;
    const staged: StagedFile = {
      id: `staged-${lastStagedId}`,
      filename,
      kind:
        file.kind ?? (IMAGE_EXTENSIONS.includes(extension) ? "image" : "text"),
      mime: file.mime ?? mimeOf(extension),
      size,
    };
    backend.staged.set(staged.id, staged);
    return { status: "staged", file: staged };
  }

  simulator.handle("pick_files", () => backend.picks.shift() ?? []);
  simulator.handle("stage_paths", (args: Args) =>
    (args.paths as string[]).map((path) => {
      // Comme le moteur : « \ » sépare aussi les dossiers d'un chemin Windows.
      const filename = path.slice(
        Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1,
      );
      const entry = backend.files.get(path);
      if (!entry) return rejected(filename, ERROR_READING_FILE);
      if ("error" in entry) return rejected(filename, entry.error);
      return stage(filename, entry, entry.size ?? 1);
    }),
  );
  // Comme la commande du moteur (mêmes erreurs) : en-têtes illisibles
  // refusés, puis extension, taille déclarée, taille reçue. Seul écart : le
  // corps est contrôlé en premier (le moteur, après les refus), car des
  // arguments JSON perdent ici leurs en-têtes.
  simulator.handle("stage_bytes", (args: Args) => {
    const { bytes, headers = {} } = args as Partial<RawBody>;
    if (!Array.isArray(bytes)) {
      throw invalid("the file must be sent as raw bytes");
    }
    const encoded = headers["x-filename"];
    if (encoded === undefined) throw invalid("missing x-filename header");
    let filename: string;
    try {
      filename = decodeURIComponent(encoded);
    } catch {
      throw invalid("invalid x-filename header");
    }
    const declared = headers["x-size"];
    if (declared !== undefined && !/^\d+$/.test(declared)) {
      throw invalid("invalid x-size header");
    }
    if (!isAccepted(extensionOf(filename))) {
      return rejected(filename, FILE_TYPE_NOT_SUPPORTED);
    }
    if (Number(declared ?? 0) > MAX_FILE_SIZE || bytes.length > MAX_FILE_SIZE) {
      return rejected(filename, FILE_TOO_LARGE);
    }
    return stage(filename, {}, bytes.length);
  });
  simulator.handle("stage_clipboard_image", () => {
    const image = backend.clipboardImage;
    if (!image) return null;
    if (image.size > MAX_FILE_SIZE)
      return rejected("image.png", FILE_TOO_LARGE);
    return stage("image.png", {}, image.size);
  });
  simulator.handle("unstage", (args: Args) => {
    for (const id of args.ids as string[]) backend.staged.delete(id);
    return null;
  });

  simulator.handle("ui_language", () => backend.settings.effectiveLanguage);
  simulator.handle("get_settings", () => {
    if (backend.settingsErrors.get) throw backend.settingsErrors.get;
    return backend.settings;
  });
  // Comme le moteur : tout le correctif est validé avant d'écrire quoi que ce
  // soit, et la réponse est un nouvel objet (le cache change de référence).
  simulator.handle("update_settings", (args: Args) => {
    if (backend.settingsErrors.update) throw backend.settingsErrors.update;
    const patch = args.patch as SettingsPatch;
    if (
      patch.contextLength !== undefined &&
      !CONTEXT_LENGTHS.includes(patch.contextLength)
    ) {
      throw invalid(`unsupported context length: ${patch.contextLength}`);
    }
    if (patch.theme !== undefined && !THEMES.includes(patch.theme)) {
      throw invalid(`unknown theme: ${String(patch.theme)}`);
    }
    if (
      patch.language !== undefined &&
      !["system", "en", "es", "fr"].includes(patch.language)
    ) {
      throw invalid(`unknown language: ${String(patch.language)}`);
    }
    if (
      patch.keepAlive !== undefined &&
      !KEEP_ALIVES.includes(patch.keepAlive)
    ) {
      throw invalid(`unsupported keep alive: ${patch.keepAlive}`);
    }
    if (
      patch.systemPrompt !== undefined &&
      [...patch.systemPrompt].length > MAX_SYSTEM_PROMPT_CHARS
    ) {
      throw invalid(
        `the system prompt holds at most ${MAX_SYSTEM_PROMPT_CHARS} characters`,
      );
    }
    const settings: Settings = { ...backend.settings, ...patch };
    // Ancien nom d'un modèle repris : son nouvel identifiant.
    for (const key of [
      "selectedModel",
      "proofreadModel",
      "studyModel",
    ] as const) {
      const value = patch[key];
      if (value === undefined || value === null) continue;
      const renamed = [...backend.renamed].find(
        ([name]) => ollamaKey(name) === ollamaKey(value),
      );
      if (renamed) settings[key] = renamed[1];
    }
    // Langue du bureau des essais : l'anglais.
    settings.effectiveLanguage =
      settings.language === "system" ? "en" : settings.language;
    // Appliqué ensuite (desktop::apply_proofread_shortcut) : liaison
    // commencée ou raccourci délié.
    if (
      patch.proofreadShortcut !== undefined &&
      patch.proofreadShortcut !== backend.settings.proofreadShortcut
    ) {
      backend.proofread?.applyShortcut(patch.proofreadShortcut);
    }
    // De même pour desktop::apply_read_aloud_shortcut.
    if (
      patch.readAloudShortcut !== undefined &&
      patch.readAloudShortcut !== backend.settings.readAloudShortcut
    ) {
      backend.proofread?.applyReadAloudShortcut(patch.readAloudShortcut);
    }
    backend.settings = settings;
    return settings;
  });
  // Comme le moteur : les modèles distants seulement si le réglage les
  // permet.
  simulator.handle("list_models", () =>
    backend.models
      .filter((id) => backend.settings.remoteModels || !isRemoteModel(id))
      .map((id) => modelInfo(id)),
  );
  simulator.handle("machine_memory", () => backend.machine);
  simulator.handle("auto_model", () => {
    if (backend.runningErrors.list)
      return backend.settings.selectedModel || null;
    return (
      autoModelOf("", backend.settings.selectedModel, backend.running) || null
    );
  });
  simulator.handle("running_models", () => {
    if (backend.runningErrors.list) throw backend.runningErrors.list;
    return backend.running;
  });
  // Comme le moteur : modèle inconnu → « not found » ; distant : rien à
  // charger ; déjà chargé, il y reste (même contexte), sa durée repart.
  simulator.handle("load_model", (args: Args) => {
    const model = args.model as string;
    const error = backend.runningErrors.load.get(model);
    if (error) throw error;
    const installed = backend.models.find((id) => sameModel(id, model));
    if (installed === undefined) {
      throw {
        kind: "notFound",
        message: `the model ${model} isn't installed`,
      };
    }
    if (isRemoteModel(installed)) return null;
    const { contextLength, keepAlive } = backend.settings;
    const seconds = keepAlive === 0 ? 300 : keepAlive;
    const loaded = runningModel(installed, {
      contextLength: contextLength || 4096,
      expiresAt: seconds < 0 ? null : Date.now() + seconds * 1000,
      kept: seconds < 0,
    });
    backend.running = [
      loaded,
      ...backend.running.filter((m) => !sameModel(m.id, model)),
    ];
    return null;
  });
  simulator.handle("unload_model", (args: Args) => {
    if (backend.runningErrors.unload) throw backend.runningErrors.unload;
    const model = args.model as string;
    backend.running = backend.running.filter((m) => !sameModel(m.id, model));
    return null;
  });
  simulator.handle("unload_all_models", () => {
    if (backend.runningErrors.unload) throw backend.runningErrors.unload;
    backend.running = [];
    return null;
  });
  simulator.handle("recommended_models", () => backend.recommended);
  simulator.handle("outdated_models", (args: Args) => {
    if (backend.outdatedError) throw backend.outdatedError;
    const stale = [...backend.outdated].map(canonicalModel);
    return (args.models as string[]).filter((model) =>
      stale.includes(canonicalModel(model)),
    );
  });
  simulator.handle("model_details", (args: Args) => {
    const model = args.model as string;
    const found = backend.details.get(model);
    if (found && "kind" in found) throw found;
    if (found) return found;
    if (backend.models.includes(model)) {
      return {
        model,
        capabilities: ["completion"],
        thinking: null,
        alwaysThinks: false,
      };
    }
    throw { kind: "notFound", message: `model '${model}' not found` };
  });
  simulator.handle(
    "get_think_pref",
    (args: Args) => backend.thinkPrefs.get(args.model as string) ?? null,
  );
  simulator.handle("set_think_pref", (args: Args) => {
    backend.thinkPrefs.set(args.model as string, args.think as Think);
    return null;
  });
  simulator.handle("get_chat", (args: Args): Chat => {
    const chat = backend.chats.get(args.id as string);
    if (!chat) throw { kind: "notFound", message: "chat not found" };
    // Sans le favori ni la messagerie, que seul list_chats donne.
    const { id, title, createdAt, updatedAt, messages } = chat;
    return {
      id,
      title,
      createdAt,
      updatedAt,
      messages,
      context: chat.context ?? null,
      compactions: chat.compactions ?? [],
      replyRunning: chat.replyRunning ?? null,
      ephemeral: chat.ephemeral ?? false,
    };
  });
  // Compactage manuel : tout ce qui suit le dernier compactage, résumé par
  // backend.compaction ; refus comme le moteur quand il n'y a rien à
  // compacter.
  simulator.handle("compact_chat", (args: Args): Compaction => {
    const chat = backend.chats.get(args.chatId as string);
    if (!chat) throw { kind: "notFound", message: "chat not found" };
    if (backend.compaction.error) throw backend.compaction.error;
    const covered = chat.compactions?.at(-1)?.throughMessageId ?? 0;
    const last = chat.messages.at(-1);
    if (!last || last.id <= covered) {
      throw invalid("there is nothing new to compact in this chat");
    }
    const compaction: Compaction = {
      id: Math.max(0, ...(chat.compactions ?? []).map((c) => c.id)) + 1,
      throughMessageId: last.id,
      summary: backend.compaction.summary,
      automatic: false,
      createdAt: Date.now(),
    };
    chat.compactions = [...(chat.compactions ?? []), compaction];
    if (chat.context) {
      chat.context = { used: backend.compaction.used, size: chat.context.size };
    }
    return compaction;
  });
  simulator.handle("list_chats", (): ChatInfo[] =>
    [...backend.chats.values()]
      .filter((chat) => chat.ephemeral !== true)
      .sort((a, b) => b.updatedAt - a.updatedAt)
      .map((chat) => ({
        id: chat.id,
        title: chat.title,
        userExcerpt: excerpt(
          chat.messages.find((m) => m.role === "user")?.content ?? "",
        ),
        createdAt: chat.createdAt,
        updatedAt: chat.updatedAt,
        favorite: chat.favorite ?? false,
        source: chat.source ?? null,
      })),
  );
  // Date d'activité inchangée, comme le moteur.
  simulator.handle("set_chat_favorite", (args: Args) => {
    const chat = backend.chats.get(args.id as string);
    if (!chat) {
      throw { kind: "notFound", message: `chat not found: ${args.id}` };
    }
    chat.favorite = args.favorite as boolean;
    return null;
  });

  // Conversation éphémère quittée : effacée seulement si elle l'est encore.
  simulator.handle("forget_chat", (args: Args) => {
    const chat = backend.chats.get(args.id as string);
    if (chat?.ephemeral === true) backend.chats.delete(chat.id);
    return null;
  });

  simulator.handle("keep_chat", (args: Args) => {
    const chat = backend.chats.get(args.id as string);
    if (!chat) {
      throw { kind: "notFound", message: `chat not found: ${args.id}` };
    }
    delete chat.ephemeral;
    return null;
  });

  return backend as FakeBackend;
}

/** Nom avec « :latest » s'il n'a pas d'étiquette, comme /api/ps. */
function emptyReply(id: number, at: number, model: string): Message {
  return {
    id,
    role: "assistant",
    content: "",
    thinking: "",
    model,
    createdAt: at,
    updatedAt: at,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

function excerpt(content: string) {
  return content.split(/\s+/).filter(Boolean).join(" ").slice(0, EXCERPT_CHARS);
}

/**
 * Extension au sens de l'officiel : `nom.toLowerCase().split(".").pop()` ;
 * un nom sans point donne le nom entier.
 */
function extensionOf(filename: string) {
  return filename.toLowerCase().split(".").pop() ?? "";
}

function isAccepted(extension: string) {
  return (
    IMAGE_EXTENSIONS.includes(extension) || TEXT_EXTENSIONS.includes(extension)
  );
}

/** Type MIME par défaut d'un fichier préparé. */
function mimeOf(extension: string) {
  switch (extension) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "webp":
      return "image/webp";
    case "pdf":
      return "application/pdf";
    default:
      return "text/plain";
  }
}

function rejected(filename: string, error: string): StageResult {
  return { status: "rejected", filename, error };
}

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}
