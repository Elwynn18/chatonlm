// Reste du backend pour le simulateur : réglages (validés comme le moteur,
// échecs forcés par settingsErrors), modèles (détails, réglage
// Think mémorisé, recommandations et registre compris) et état d'Ollama en
// mémoire, et une base de conversations tenue à jour par les événements
// rejoués, comme celle du moteur (message enregistré avant la requête, réponse
// créée au premier fragment et superposée à get_chat pendant le flux, réponse
// partielle gardée, modèle installé à la fin de son téléchargement). Pièces
// jointes : sélecteur natif, disque simulé et zone de préparation (validation
// de stage_bytes comme le moteur, fichiers envoyés retirés au message
// enregistré). ModelHub : faux moteur de src/test/hub.ts (backend.hub), qui
// partage la liste des modèles installés. Dossier des modèles : emplacement,
// sélecteur et vérification simulés (change_models_location reste au test) ;
// dossier .ollama caché au service (get_ollama_home, fix_ollama_home reste
// au test).
// Compte ollama.com et état du nuage du serveur (recherche web) : en mémoire,
// échecs forcés par accountErrors. Dictée et modèles de transcription : faux
// moteur de src/test/dictation.ts (backend.dictation). quit_app répond sans
// rien faire (appels relus par simulator.callsTo). Modèles en mémoire
// (onglet Models, préchargement) : liste tenue par load_model,
// unload_model et unload_all_models, comme le faux serveur, échecs forcés
// par runningErrors. Favoris des conversations : set_chat_favorite, sans
// toucher à la date d'activité, comme le moteur. Correction (page Proofread)
// et raccourcis du système : faux moteur de src/test/proofread.ts
// (backend.proofread), qui lit les réglages et les modèles installés. Cours
// (onglet Study) : faux moteur de src/test/study.ts (backend.study), qui lit
// les réglages et reprend les fichiers de la zone de préparation. Entrée du
// menu des applications (AppImage) : menu_entry_status et set_menu_entry sur
// menuEntry, échec forcé par menuEntryError (choix enregistré quand même,
// comme le moteur). Ollama qui sert (runtime : celui de la machine ou
// l'intégré) : ollama_runtime, set_ollama_source (qui change l'adresse
// effective et l'état du serveur), start_ollama_service (issue
// serviceStart, service démarré), restart_builtin_ollama (l'intégré prend
// builtinState), open_builtin_folder et open_builtin_log, échecs forcés par
// runtimeErrors ; get_models_location donne le dossier de l'intégré quand il
// sert ; change_builtin_models_dir reste au test.
// Lecture à voix haute et voix : faux moteur de src/test/tts.ts
// (backend.tts), réglage autoRead coupé par défaut. Mode conversation
// vocale : faux moteur de src/test/voiceMode.ts (backend.voiceMode), qui lit
// les faux moteurs de la dictée et des voix pour ses prérequis.
// Mises à jour de ChatonLM : update_status et app_versions sur updates et
// versions, set_auto_download change updates.autoDownload ; check_for_updates,
// download_update et restart_to_update répondent sans rien changer (le test
// émet « update-status »), échecs forcés par updateErrors.
// Système : app_platform répond `platform` (« linux » par défaut) ; un essai
// d'un écran sous Windows appelle aussi setPlatform (src/lib/platform.ts), que
// main.tsx appelle dans l'app.
// Easy Link : faux moteur de src/test/easyLink.ts (backend.easyLink) : deux
// bots sans jeton, aucun compte lié, pas de code d'appairage. Une
// conversation venue d'un bot porte sa messagerie (`source`, rendue par
// list_chats ; null pour une conversation de l'app).
// Pour les tests seulement.

import type { AccountStatus } from "@/bindings/AccountStatus";
import type { AppVersions } from "@/bindings/AppVersions";
import type { AppError } from "@/bindings/AppError";
import type { AttachmentKind } from "@/bindings/AttachmentKind";
import type { Chat } from "@/bindings/Chat";
import type { CloudStatus } from "@/bindings/CloudStatus";
import type { FolderCheck } from "@/bindings/FolderCheck";
import type { HiddenHome } from "@/bindings/HiddenHome";
import type { MenuEntryStatus } from "@/bindings/MenuEntryStatus";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { ChatInfo } from "@/bindings/ChatInfo";
import type { EasyLinkPlatform } from "@/bindings/EasyLinkPlatform";
import type { Message } from "@/bindings/Message";
import type { ModelDetails } from "@/bindings/ModelDetails";
import type { ModelInfo } from "@/bindings/ModelInfo";
import type { ModelsLocation } from "@/bindings/ModelsLocation";
import type { RunningModel } from "@/bindings/RunningModel";
import type { ServerState } from "@/bindings/ServerState";
import type { ServerStatus } from "@/bindings/ServerStatus";
import type { ServiceInfo } from "@/bindings/ServiceInfo";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import type { StagedFile } from "@/bindings/StagedFile";
import type { StageResult } from "@/bindings/StageResult";
import type { Theme } from "@/bindings/Theme";
import type { Think } from "@/bindings/Think";
import type { ThinkingOptions } from "@/bindings/ThinkingOptions";
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { ServiceStart } from "@/bindings/ServiceStart";
import type { UpdateStatus } from "@/bindings/UpdateStatus";
import type { Platform } from "@/lib/platform";
import { type FakeDictation, fakeDictation } from "@/test/dictation";
import { type FakeEasyLink, fakeEasyLink } from "@/test/easyLink";
import { canonicalModel, type FakeHub, fakeHub } from "@/test/hub";
import { type FakeProofread, fakeProofread } from "@/test/proofread";
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

/** Adresse des séquences (normalisée par les tests Rust). */
export const SERVER_URL = "http://127.0.0.1:11434";

/** Adresse privée de l'Ollama intégré (port libre choisi par le moteur). */
export const BUILTIN_URL = "http://127.0.0.1:47811";

/** Dossier des modèles de l'Ollama intégré, par défaut. */
export const BUILTIN_MODELS_DIR =
  "/home/user/.local/share/chatonlm/ollama/models";

/** Journal de l'Ollama intégré. */
export const OLLAMA_LOG = "/home/user/.local/share/chatonlm/logs/ollama.log";

/** Version de l'Ollama de la machine (séquences) et de l'intégré. */
export const MACHINE_VERSION = "0.35.1";
export const BUILTIN_VERSION = "0.40.0";

/**
 * Celui de la machine sert, l'intégré est disponible (paquets) et arrêté ;
 * adresse, version servie et état de l'intégré relus par ollama_runtime
 * (voir FakeBackend.runtime).
 */
export function machineRuntime(
  fields: Partial<OllamaRuntime> = {},
): OllamaRuntime {
  return {
    source: "auto",
    kind: "machine",
    url: SERVER_URL,
    local: true,
    machineUrl: SERVER_URL,
    machineLocal: true,
    explicitAddress: false,
    servedVersion: MACHINE_VERSION,
    builtinAvailable: true,
    builtinState: "stopped",
    builtinVersion: BUILTIN_VERSION,
    builtinModelsDir: BUILTIN_MODELS_DIR,
    builtinModelsDirIsDefault: true,
    builtinModelsDirMissing: false,
    builtinModels: null,
    logPath: OLLAMA_LOG,
    builtinFailure: null,
    ...fields,
  };
}

/** État de l'intégré qui sert, d'après celui du serveur. */
const BUILTIN_STATES: Partial<
  Record<ServerState, OllamaRuntime["builtinState"]>
> = {
  running: "running",
  builtinStarting: "starting",
  builtinFailed: "failed",
};

/** Adresse de cette machine (bouclage). */
const LOOPBACK = /^https?:\/\/(localhost|127\.[\d.]+|\[::1\])(:\d+)?\/?$/;

/** Crans de la longueur de contexte (CONTEXT_LENGTHS du moteur) ; 0 = défaut. */
const CONTEXT_LENGTHS = [0, 4096, 8192, 16384, 32768, 65536, 131072, 262144];

const THEMES: readonly Theme[] = ["system", "light", "dark"];

/** Durées avant déchargement acceptées (KEEP_ALIVES du moteur), en secondes. */
const KEEP_ALIVES = [0, 60, 300, 900, 1800, 3600, 14400, -1];

/** Taille d'un modèle en mémoire, comme le faux serveur : 1 Gio. */
export const RUNNING_SIZE = 1024 ** 3;

/**
 * Modèle en mémoire comme le faux serveur le donne : 1 Gio, tout en VRAM,
 * contexte 4096, déchargé dans 5 min (à partir de maintenant).
 */
export function runningModel(
  name: string,
  fields: Partial<RunningModel> = {},
): RunningModel {
  return {
    name,
    size: RUNNING_SIZE,
    sizeVram: RUNNING_SIZE,
    contextLength: 4096,
    expiresAt: Date.now() + 300_000,
    kept: false,
    parameterSize: "1B",
    quantizationLevel: "Q4_K_M",
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
 * favorite, conversation de l'app).
 */
export type StoredChat = Chat & {
  favorite?: boolean;
  source?: EasyLinkPlatform | null;
};

export function modelInfo(model: string): ModelInfo {
  return {
    model,
    digest: `digest-${model}`,
    size: 1,
    parameterSize: "1B",
    quantizationLevel: "Q4_K_M",
    contextLength: null,
    capabilities: ["completion"],
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
  /** Réponse de recommended_models. */
  recommended: string[];
  /** Modèles présents au registre (model_exists_upstream). */
  upstream: Set<string>;
  /**
   * Modèles installés dont le registre a une version plus récente
   * (outdated_models, comparés sans la casse, `:latest` implicite) ; un
   * modèle téléchargé de nouveau par hub.finish() en sort.
   */
  outdated: Set<string>;
  /** outdated_models rejette cette erreur (Ollama injoignable). */
  outdatedError: AppError | null;
  /** Conversations enregistrées, par identifiant. */
  chats: Map<string, StoredChat>;
  /** État renvoyé par server_status (celui de la cible). */
  serverState: ServerState;
  /** Service systemd d'Ollama renvoyé par server_status (null : aucun). */
  service: ServiceInfo | null;
  /**
   * Ollama qui sert (ollama_runtime). Déduits à chaque lecture : `url`
   * (l'intégré : BUILTIN_URL ; sinon l'adresse effective des réglages),
   * `local`, `explicitAddress` (adresse réglée), `machineUrl` (l'adresse
   * réglée, sinon celle de runtime.machineUrl : OLLAMA_HOST simulé) et
   * `machineLocal`, `servedVersion` (rendue seulement si serverState vaut
   * running ; l'intégré : builtinVersion) et, quand l'intégré sert,
   * `builtinState` (d'après serverState).
   */
  runtime: OllamaRuntime;
  /**
   * État que prend le serveur quand l'intégré est choisi (set_ollama_source)
   * ou relancé (restart_builtin_ollama) : running par défaut.
   */
  builtinState: ServerState;
  /** Issue de start_ollama_service (started : serverState passe à running). */
  serviceStart: ServiceStart;
  /**
   * Échecs forcés de set_ollama_source, start_ollama_service,
   * restart_builtin_ollama, open_builtin_folder et open_builtin_log (null :
   * réponse normale).
   */
  runtimeErrors: {
    setSource: AppError | null;
    start: AppError | null;
    restart: AppError | null;
    openFolder: AppError | null;
    openLog: AppError | null;
  };
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
  /** Réponse de get_models_location (service système par défaut). */
  modelsLocation: ModelsLocation;
  /** Réponses des prochains pick_models_folder ; plus rien : annulé (null). */
  folderPicks: (string | null)[];
  /**
   * Réponses de check_models_folder par chemin, sur une vérification sans
   * problème (DEFAULT_FOLDER_CHECK).
   */
  folderChecks: Map<string, Partial<FolderCheck>>;
  /** Réponse de get_ollama_home : dossier .ollama caché au service, ou null. */
  ollamaHome: HiddenHome | null;
  /**
   * Compte du serveur (account_status) ; délié par account_sign_out.
   * account_sign_in n'y touche pas (la connexion se fait dans le
   * navigateur) : le test le relie lui-même.
   */
  account: AccountStatus;
  /**
   * Échecs forcés de account_status, account_sign_in et account_sign_out
   * (null : réponse normale).
   */
  accountErrors: {
    status: AppError | null;
    signIn: AppError | null;
    signOut: AppError | null;
  };
  /** Réponse de cloud_status (null : le serveur n'en dit rien). */
  cloud: CloudStatus | null;
  /**
   * Modèles en mémoire (running_models) ; load_model y ajoute un modèle
   * installé (contexte et durée des réglages), unload_model et
   * unload_all_models l'en retirent.
   */
  running: RunningModel[];
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
   * Échec forcé de set_menu_entry (fichier impossible à écrire) : le choix
   * est enregistré, puis cette erreur renvoyée (null : réponse normale).
   */
  menuEntryError: AppError | null;
  /**
   * Réponse de update_status : à jour, Auto-download activé, AppImage, sans
   * échec gardé.
   */
  updates: UpdateStatus;
  /** Réponse de app_versions : ChatonLM 0.9.0, sans Ollama intégré. */
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
}

/** Service système de la plupart des machines, pkexec présent. */
export const SYSTEM_SERVICE: Extract<
  ModelsLocation,
  { kind: "systemService" }
> = {
  kind: "systemService",
  dir: "/var/lib/ollama",
  user: "ollama",
  group: "ollama",
  running: true,
  managed: false,
  canElevate: true,
  envFile: null,
};

/** Vérification d'un dossier vide admis, sur le même disque. */
export function defaultFolderCheck(path: string): FolderCheck {
  return {
    path,
    problem: null,
    old: "/var/lib/ollama",
    hasModels: false,
    sameDrive: true,
    models: { count: 2, bytes: 2 * 1024 ** 3 },
    freeBytes: 100 * 1024 ** 3,
    commands: `sudo mkdir -p '${path}'`,
    repliesRunning: false,
    downloadsRunning: false,
  };
}

/**
 * Gestionnaires de account_status, account_sign_in, account_sign_out,
 * cloud_status, get_settings, update_settings, list_models,
 * recommended_models, model_exists_upstream, model_details, get_think_pref,
 * set_think_pref, server_status, get_chat, list_chats, set_chat_favorite,
 * pick_files,
 * stage_paths, stage_bytes, stage_clipboard_image, unstage, les commandes
 * hub_* (src/test/hub.ts), get_models_location, pick_models_folder,
 * check_models_folder, get_ollama_home, get_dictation_settings, update_dictation_settings,
 * dictation_info, dictation_status, dictation_toggle, dictation_cancel,
 * dictation_copy_last,
 * dictation_bind_shortcut, dictation_configure_shortcut, speech_models,
 * speech_downloads, speech_download, speech_cancel_download,
 * speech_dismiss_download, speech_delete_model (src/test/dictation.ts),
 * running_models, load_model, unload_model, unload_all_models, quit_app,
 * proofread_run, proofread_cancel, proofread_last_selection et shortcuts_*
 * (src/test/proofread.ts), study_* (src/test/study.ts), read_aloud,
 * read_aloud_stop, read_aloud_state et voice_* (src/test/tts.ts),
 * voice_mode_*, get_voice_mode_settings et update_voice_mode_settings
 * (src/test/voiceMode.ts), easy_link_* (src/test/easyLink.ts),
 * ollama_runtime,
 * set_ollama_source, start_ollama_service, restart_builtin_ollama,
 * open_builtin_folder, open_builtin_log, menu_entry_status,
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
    upstream = [],
    files = {},
  }: {
    selectedModel?: string;
    models?: string[];
    details?: (ModelDetails | [string, AppError])[];
    thinkPrefs?: Record<string, Think>;
    /** Modèles recommandés par le serveur. */
    recommended?: string[];
    /** Modèles présents au registre. */
    upstream?: string[];
    /** Disque lu par stage_paths, par chemin. */
    files?: Record<string, FakeEntry>;
  } = {},
): FakeBackend {
  const backend: Omit<
    FakeBackend,
    | "hub"
    | "dictation"
    | "proofread"
    | "study"
    | "tts"
    | "voiceMode"
    | "easyLink"
  > & {
    hub?: FakeHub;
    dictation?: FakeDictation;
    proofread?: FakeProofread;
    study?: FakeStudy;
    tts?: FakeTts;
    voiceMode?: FakeVoiceMode;
    easyLink?: FakeEasyLink;
  } = {
    settings: {
      serverUrl: "",
      effectiveServerUrl: SERVER_URL,
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
      studyModel: "",
      autoRead: false,
      onboardingDone: true,
    },
    settingsErrors: { get: null, update: null },
    details: new Map<string, ModelDetails | AppError>(
      details.map((entry) =>
        Array.isArray(entry) ? entry : [entry.model, entry],
      ),
    ),
    thinkPrefs: new Map(Object.entries(thinkPrefs)),
    models: [...models],
    recommended: [...recommended],
    upstream: new Set(upstream),
    outdated: new Set(),
    outdatedError: null,
    chats: new Map(),
    serverState: "running",
    service: null,
    runtime: machineRuntime(),
    builtinState: "running",
    serviceStart: { kind: "started" },
    runtimeErrors: {
      setSource: null,
      start: null,
      restart: null,
      openFolder: null,
      openLog: null,
    },
    files: new Map(Object.entries(files)),
    staged: new Map(),
    picks: [],
    clipboardImage: null,
    modelsLocation: { ...SYSTEM_SERVICE },
    folderPicks: [],
    folderChecks: new Map(),
    ollamaHome: null,
    account: { state: "signedOut" },
    accountErrors: { status: null, signIn: null, signOut: null },
    cloud: { disabled: false, source: "none" },
    running: [],
    runningErrors: { list: null, load: new Map(), unload: null },
    platform: "linux",
    menuEntry: { available: false, enabled: true },
    menuEntryError: null,
    updates: {
      state: { state: "upToDate" },
      autoDownload: true,
      checkedAt: SEQUENCE_TIME,
      installer: "appImage",
      lastError: null,
    },
    versions: { chatonlm: "0.9.0", ollama: null },
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
      };
      for (const event of sequence.events) {
        if (event.type === "chatCreated") context.chatId = event.chatId;
        apply(event, context, SEQUENCE_TIME);
      }
    },
  };

  backend.hub = fakeHub(simulator, backend);
  backend.dictation = fakeDictation(simulator);
  backend.proofread = fakeProofread(simulator, backend);
  backend.study = fakeStudy(simulator, backend);
  backend.tts = fakeTts(simulator);
  backend.voiceMode = fakeVoiceMode(simulator, {
    dictation: backend.dictation,
    tts: backend.tts,
  });
  backend.easyLink = fakeEasyLink(simulator);
  simulator.handle("quit_app", () => null);
  simulator.handle("app_platform", () => backend.platform);
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
    { chatId, model }: { chatId: string | null; model: string },
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
      case "loading":
      case "loaded":
        // Chargement du modèle : rien d'enregistré.
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
      { chatId: replay.chatId, model: replay.request?.model ?? "" },
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

  simulator.handle("get_models_location", (): ModelsLocation =>
    backend.runtime.kind === "builtin"
      ? {
          kind: "builtin",
          dir: backend.runtime.builtinModelsDir,
          isDefault: backend.runtime.builtinModelsDirIsDefault,
        }
      : backend.modelsLocation,
  );
  simulator.handle(
    "pick_models_folder",
    () => backend.folderPicks.shift() ?? null,
  );
  simulator.handle("get_ollama_home", () => backend.ollamaHome);
  simulator.handle("check_models_folder", (args: Args) => {
    const path = args.path as string;
    return { ...defaultFolderCheck(path), ...backend.folderChecks.get(path) };
  });

  simulator.handle("account_status", () => {
    if (backend.accountErrors.status) throw backend.accountErrors.status;
    return backend.account;
  });
  simulator.handle("account_sign_in", () => {
    if (backend.accountErrors.signIn) throw backend.accountErrors.signIn;
    return null;
  });
  simulator.handle("account_sign_out", () => {
    if (backend.accountErrors.signOut) throw backend.accountErrors.signOut;
    backend.account = { state: "signedOut" };
    return null;
  });
  simulator.handle("cloud_status", () => backend.cloud);

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
    const serverUrl =
      patch.serverUrl === undefined ? undefined : checkAddress(patch.serverUrl);
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
    const settings: Settings = {
      ...backend.settings,
      ...patch,
      ...(serverUrl !== undefined && { serverUrl }),
    };
    settings.effectiveServerUrl = settings.serverUrl || SERVER_URL;
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
    backend.settings = settings;
    return settings;
  });
  simulator.handle("list_models", () => backend.models.map(modelInfo));
  simulator.handle("running_models", () => {
    if (backend.runningErrors.list) throw backend.runningErrors.list;
    return backend.running;
  });
  // Comme le moteur et Ollama : modèle inconnu → 404 « not found » ; déjà
  // en mémoire, il y reste (même contexte), sa durée repart.
  simulator.handle("load_model", (args: Args) => {
    const model = args.model as string;
    const error = backend.runningErrors.load.get(model);
    if (error) throw error;
    if (!backend.models.some((name) => sameRunningName(name, model))) {
      throw { kind: "notFound", message: `model '${model}' not found` };
    }
    const { contextLength, keepAlive } = backend.settings;
    const seconds = keepAlive === 0 ? 300 : keepAlive;
    const loaded = runningModel(withLatest(model), {
      contextLength: contextLength || 4096,
      expiresAt: seconds < 0 ? null : Date.now() + seconds * 1000,
      kept: seconds < 0,
    });
    backend.running = [
      ...backend.running.filter((m) => !sameRunningName(m.name, model)),
      loaded,
    ];
    return null;
  });
  simulator.handle("unload_model", (args: Args) => {
    if (backend.runningErrors.unload) throw backend.runningErrors.unload;
    const model = args.model as string;
    backend.running = backend.running.filter(
      (m) => !sameRunningName(m.name, model),
    );
    return null;
  });
  simulator.handle("unload_all_models", () => {
    if (backend.runningErrors.unload) throw backend.runningErrors.unload;
    backend.running = [];
    return null;
  });
  simulator.handle("recommended_models", () => backend.recommended);
  simulator.handle("model_exists_upstream", (args: Args) =>
    backend.upstream.has(args.model as string),
  );
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
  simulator.handle("server_status", (): ServerStatus => {
    const builtin = backend.runtime.kind === "builtin";
    return {
      url: builtin ? BUILTIN_URL : SERVER_URL,
      state: backend.serverState,
      version:
        backend.serverState === "running"
          ? builtin
            ? backend.runtime.builtinVersion
            : MACHINE_VERSION
          : null,
      alternativeUrl: null,
      hostEnv: null,
      httpStatus: null,
      detail: null,
      service: builtin ? null : backend.service,
      builtin,
      builtinAvailable: backend.runtime.builtinAvailable,
      explicitAddress: !builtin && backend.settings.serverUrl !== "",
      logPath: builtin ? OLLAMA_LOG : null,
    };
  });
  const runtime = (): OllamaRuntime => {
    const current = backend.runtime;
    const builtin = current.kind === "builtin";
    const url = builtin ? BUILTIN_URL : backend.settings.effectiveServerUrl;
    const running = backend.serverState === "running";
    // Adresse que reprend le choix automatique : celle des réglages, sinon
    // runtime.machineUrl (OLLAMA_HOST ou l'adresse par défaut).
    const machineUrl = backend.settings.serverUrl || current.machineUrl;
    return {
      ...current,
      url,
      local: builtin || LOOPBACK.test(url),
      machineUrl,
      machineLocal: LOOPBACK.test(machineUrl),
      explicitAddress: backend.settings.serverUrl !== "",
      servedVersion: !running
        ? null
        : builtin
          ? current.builtinVersion
          : current.servedVersion,
      builtinState: builtin
        ? (BUILTIN_STATES[backend.serverState] ?? "stopped")
        : current.builtinState,
    };
  };
  simulator.handle("ollama_runtime", runtime);
  // État de la machine, retrouvé en quittant l'intégré.
  let machineState: ServerState = "running";
  simulator.handle("set_ollama_source", (args: Args) => {
    if (backend.runtimeErrors.setSource) throw backend.runtimeErrors.setSource;
    const source = args.source as OllamaRuntime["source"];
    const wasBuiltin = backend.runtime.kind === "builtin";
    backend.runtime.source = source;
    if (source === "builtin") {
      if (!wasBuiltin) machineState = backend.serverState;
      backend.runtime.kind = "builtin";
      backend.serverState = backend.builtinState;
      backend.settings.effectiveServerUrl = BUILTIN_URL;
    } else {
      backend.runtime.kind = "machine";
      if (wasBuiltin) backend.serverState = machineState;
      backend.settings.effectiveServerUrl =
        backend.settings.serverUrl || SERVER_URL;
    }
    return runtime();
  });
  simulator.handle("start_ollama_service", (): ServiceStart => {
    if (backend.runtimeErrors.start) throw backend.runtimeErrors.start;
    if (backend.serviceStart.kind === "started")
      backend.serverState = "running";
    return backend.serviceStart;
  });
  simulator.handle("restart_builtin_ollama", () => {
    if (backend.runtimeErrors.restart) throw backend.runtimeErrors.restart;
    backend.serverState = backend.builtinState;
    return null;
  });
  simulator.handle("open_builtin_folder", () => {
    if (backend.runtimeErrors.openFolder) {
      throw backend.runtimeErrors.openFolder;
    }
    return null;
  });
  simulator.handle("open_builtin_log", () => {
    if (backend.runtimeErrors.openLog) throw backend.runtimeErrors.openLog;
    return null;
  });
  simulator.handle("get_chat", (args: Args): Chat => {
    const chat = backend.chats.get(args.id as string);
    if (!chat) throw { kind: "notFound", message: "chat not found" };
    // Sans le favori ni la messagerie, que seul list_chats donne.
    const { id, title, createdAt, updatedAt, messages } = chat;
    return { id, title, createdAt, updatedAt, messages };
  });
  simulator.handle("list_chats", (): ChatInfo[] =>
    [...backend.chats.values()]
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

  return backend as FakeBackend;
}

/** Nom avec « :latest » s'il n'a pas d'étiquette, comme /api/ps. */
function withLatest(name: string): string {
  const last = name.slice(name.lastIndexOf("/") + 1);
  return last.includes(":") ? name : `${name}:latest`;
}

function sameRunningName(a: string, b: string): boolean {
  return withLatest(a).toLowerCase() === withLatest(b).toLowerCase();
}

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

/**
 * Adresse réglée, version simplifiée d'address::normalize du moteur : rognée,
 * vide = automatique ; avec « :// », URL lisible, schéma http ou https, sans
 * identifiants, requête ni fragment ; sinon, hôte simple (nom ou IPv4, ou
 * IPv6 entre crochets), port et chemin facultatifs. Pas d'autre
 * normalisation (schéma ajouté, port par défaut, /api retiré...).
 */
function checkAddress(input: string): string {
  const value = input.trim();
  if (value === "") return "";
  if (!value.includes("://")) {
    if (!SIMPLE_HOST.test(value)) {
      throw invalid(`invalid server address: ${value}`);
    }
    return value;
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw invalid(`invalid server address: ${value}`);
  }
  const scheme = url.protocol.slice(0, -1);
  if (scheme !== "http" && scheme !== "https") {
    throw invalid(`invalid server address: unsupported scheme ${scheme}`);
  }
  if (url.username || url.password) {
    throw invalid("invalid server address: credentials are not supported");
  }
  if (url.search || url.hash || value.includes("?") || value.includes("#")) {
    throw invalid("invalid server address: query strings are not supported");
  }
  return value;
}

/** Hôte simple, port et chemin facultatifs (adresse sans schéma). */
const SIMPLE_HOST = /^(\[[0-9a-fA-F:.]+\]|[A-Za-z0-9.-]+)(:\d{1,5})?(\/\S*)?$/;

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}
