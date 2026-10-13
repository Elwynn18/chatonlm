// Appels au backend Rust. Le reste du frontend passe par ces fonctions plutôt
// que par invoke(), pour garder un seul endroit où noms de commandes et types
// (générés depuis Rust dans src/bindings) se rencontrent.
import { type Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AccountStatus } from "@/bindings/AccountStatus";
import type { AppError } from "@/bindings/AppError";
import type { Chat } from "@/bindings/Chat";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { ChatInfo } from "@/bindings/ChatInfo";
import type { CloudStatus } from "@/bindings/CloudStatus";
import type { DatabaseProblem } from "@/bindings/DatabaseProblem";
import type { DictationInfo } from "@/bindings/DictationInfo";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { DictationSettings } from "@/bindings/DictationSettings";
import type { DictationStatus } from "@/bindings/DictationStatus";
import type { FolderCheck } from "@/bindings/FolderCheck";
import type { HiddenHome } from "@/bindings/HiddenHome";
import type { HomeFixOutcome } from "@/bindings/HomeFixOutcome";
import type { HubDownload } from "@/bindings/HubDownload";
import type { HubDownloadRequest } from "@/bindings/HubDownloadRequest";
import type { HubFilters } from "@/bindings/HubFilters";
import type { HubHardware } from "@/bindings/HubHardware";
import type { HubHome } from "@/bindings/HubHome";
import type { HubInstalled } from "@/bindings/HubInstalled";
import type { HubList } from "@/bindings/HubList";
import type { HubReadme } from "@/bindings/HubReadme";
import type { HubRepoResult } from "@/bindings/HubRepoResult";
import type { HubScope } from "@/bindings/HubScope";
import type { HubSearch } from "@/bindings/HubSearch";
import type { MenuEntryStatus } from "@/bindings/MenuEntryStatus";
import type { ModelDetails } from "@/bindings/ModelDetails";
import type { ModelInfo } from "@/bindings/ModelInfo";
import type { ModelsLocation } from "@/bindings/ModelsLocation";
import type { ProofreadEvent } from "@/bindings/ProofreadEvent";
import type { ProofreadMode } from "@/bindings/ProofreadMode";
import type { MoveEvent } from "@/bindings/MoveEvent";
import type { Lang } from "@/bindings/Lang";
import type { MoveOutcome } from "@/bindings/MoveOutcome";
import type { RunningModel } from "@/bindings/RunningModel";
import type { SelectionResult } from "@/bindings/SelectionResult";
import type { SendRequest } from "@/bindings/SendRequest";
import type { ServerStatus } from "@/bindings/ServerStatus";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import type { ShortcutsStatus } from "@/bindings/ShortcutsStatus";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { SpeechModelInfo } from "@/bindings/SpeechModelInfo";
import type { StageResult } from "@/bindings/StageResult";
import type { Think } from "@/bindings/Think";
import type { AnswerValue } from "@/bindings/AnswerValue";
import type { Answered } from "@/bindings/Answered";
import type { Course } from "@/bindings/Course";
import type { CourseSummary } from "@/bindings/CourseSummary";
import type { NewCourse } from "@/bindings/NewCourse";
import type { StudyStreamEvent } from "@/bindings/StudyStreamEvent";
import type { StudyUpdated } from "@/bindings/StudyUpdated";
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { OllamaSource } from "@/bindings/OllamaSource";
import type { OllamaTarget } from "@/bindings/OllamaTarget";
import type { ServiceStart } from "@/bindings/ServiceStart";
import type { AppVersions } from "@/bindings/AppVersions";
import type { UpdateStatus } from "@/bindings/UpdateStatus";
import type { ReadAloudOutcome } from "@/bindings/ReadAloudOutcome";
import type { ReadAloudState } from "@/bindings/ReadAloudState";
import type { VoiceModelInfo } from "@/bindings/VoiceModelInfo";
import type { VoiceModeInfo } from "@/bindings/VoiceModeInfo";
import type { VoiceModeLevel } from "@/bindings/VoiceModeLevel";
import type { VoiceModeRequirements } from "@/bindings/VoiceModeRequirements";
import type { VoiceModeSettings } from "@/bindings/VoiceModeSettings";
import type { VoiceModeSettingsPatch } from "@/bindings/VoiceModeSettingsPatch";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import type { VoiceModeUtterance } from "@/bindings/VoiceModeUtterance";
import type { Platform } from "@/bindings/Platform";
import type { ShortcutKeys } from "@/bindings/ShortcutKeys";

/** Problème rencontré à l'ouverture de la base au démarrage, ou null. */
export function databaseProblem(): Promise<DatabaseProblem | null> {
  return invoke<DatabaseProblem | null>("database_problem");
}

/**
 * Nouvel essai d'ouverture de la base ; avec `fresh`, l'ancien fichier est mis
 * de côté et une base neuve créée. Renvoie le problème restant, ou null.
 */
export function recoverDatabase(
  fresh: boolean,
): Promise<DatabaseProblem | null> {
  return invoke<DatabaseProblem | null>("recover_database", { fresh });
}

/**
 * Langue de l'interface (réglée, sinon celle du bureau), lue avant le premier
 * affichage ; répond aussi quand la base est illisible.
 */
export function uiLanguage(): Promise<Lang> {
  return invoke<Lang>("ui_language");
}

/**
 * Système sur lequel tourne l'app, lu avant le premier affichage comme la
 * langue (src/lib/platform.ts).
 */
export function appPlatform(): Promise<Platform> {
  return invoke<Platform>("app_platform");
}

export function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export function updateSettings(patch: SettingsPatch): Promise<Settings> {
  return invoke<Settings>("update_settings", { patch });
}

/**
 * Entrée du menu des applications d'une AppImage (Settings › General) :
 * `available` seulement sous AppImage ; réglage enregistré.
 */
export function menuEntryStatus(): Promise<MenuEntryStatus> {
  return invoke<MenuEntryStatus>("menu_entry_status");
}

/**
 * Montre ou retire ChatonLM du menu des applications (fichier .desktop et
 * icône écrits ou supprimés aussitôt) ; le choix est enregistré même si le
 * fichier n'a pas pu changer (erreur renvoyée).
 */
export function setMenuEntry(enabled: boolean): Promise<MenuEntryStatus> {
  return invoke<MenuEntryStatus>("set_menu_entry", { enabled });
}

/**
 * Vérifie l'Ollama qui sert l'app (celui de la machine ou l'intégré, démarré
 * au besoin).
 */
export function serverStatus(): Promise<ServerStatus> {
  return invoke<ServerStatus>("server_status");
}

/**
 * Ollama qui sert l'app : réglage `source`, celui de la machine ou
 * l'intégré (état, versions), dossier des modèles de l'intégré et modèles
 * qui s'y trouvent, journal.
 */
export function ollamaRuntime(): Promise<OllamaRuntime> {
  return invoke<OllamaRuntime>("ollama_runtime");
}

/**
 * Change le réglage `source` (choix automatique, ou toujours l'intégré) ;
 * l'intégré démarre au besoin, sans attendre qu'il réponde. Renvoie l'Ollama
 * qui sert désormais.
 */
export function setOllamaSource(source: OllamaSource): Promise<OllamaRuntime> {
  return invoke<OllamaRuntime>("set_ollama_source", { source });
}

/** Relance l'Ollama intégré après un échec (Retry). */
export function restartBuiltinOllama(): Promise<void> {
  return invoke<void>("restart_builtin_ollama");
}

/**
 * « Start Ollama » : démarre le service systemd d'Ollama arrêté (une invite
 * polkit pour un service du système) ; l'issue dit si systemd l'a lancé.
 */
export function startOllamaService(): Promise<ServiceStart> {
  return invoke<ServiceStart>("start_ollama_service");
}

/** Événement du moteur quand la cible change (ollama::target::TARGET_EVENT). */
export const OLLAMA_TARGET_EVENT = "ollama-target";

/** Écoute les changements de cible (source, adresse, intégré choisi seul). */
export function listenOllamaTarget(
  onChange: (target: OllamaTarget) => void,
): Promise<UnlistenFn> {
  return listen<OllamaTarget>(OLLAMA_TARGET_EVENT, (event) =>
    onChange(event.payload),
  );
}

/**
 * Change le dossier des modèles de l'Ollama intégré (choisi par
 * pick_models_folder et vérifié par check_models_folder), en y déplaçant les
 * modèles si `moveModels`, puis le redémarre ; la progression arrive sur
 * `onEvent`.
 */
export function changeBuiltinModelsDir(
  path: string,
  moveModels: boolean,
  onEvent: Channel<MoveEvent>,
): Promise<MoveOutcome> {
  return invoke<MoveOutcome>("change_builtin_models_dir", {
    path,
    moveModels,
    onEvent,
  });
}

/** Ouvre le dossier des modèles de l'Ollama intégré (créé au besoin). */
export function openBuiltinFolder(): Promise<void> {
  return invoke<void>("open_builtin_folder");
}

/** Ouvre le journal de l'Ollama intégré (son dossier s'il n'existe pas encore). */
export function openBuiltinLog(): Promise<void> {
  return invoke<void>("open_builtin_log");
}

/**
 * Où Ollama range ses modèles (service système, service de l'utilisateur,
 * lancement à la main, conteneur, autre machine...), pour l'adresse
 * effective du serveur.
 */
export function getModelsLocation(): Promise<ModelsLocation> {
  return invoke<ModelsLocation>("get_models_location");
}

/**
 * Sélecteur de dossier natif « Select Model Directory » ; null si
 * l'utilisateur annule. Seul le dossier remis ici peut ensuite être vérifié
 * et utilisé.
 */
export function pickModelsFolder(): Promise<string | null> {
  return invoke<string | null>("pick_models_folder");
}

/**
 * Vérifie le dossier choisi, sans droits ni invite : refus éventuel, modèles
 * à déplacer, place, commandes à copier.
 */
export function checkModelsFolder(path: string): Promise<FolderCheck> {
  return invoke<FolderCheck>("check_models_folder", { path });
}

/**
 * Change le dossier des modèles du service système d'Ollama (une invite
 * polkit), en y déplaçant les modèles si `moveModels` ; la progression arrive
 * sur `onEvent`. Rejette si une autre opération est en cours ou si le dossier
 * n'est pas celui du sélecteur.
 */
export function changeModelsLocation(
  path: string,
  moveModels: boolean,
  onEvent: Channel<MoveEvent>,
): Promise<MoveOutcome> {
  return invoke<MoveOutcome>("change_models_location", {
    path,
    moveModels,
    onEvent,
  });
}

/**
 * Dossier `.ollama` du service système d'Ollama (sa clé id_ed25519) que
 * `ProtectHome=` lui cache, pour l'adresse effective du serveur ; null : rien
 * à signaler.
 */
export function getOllamaHome(): Promise<HiddenHome | null> {
  return invoke<HiddenHome | null>("get_ollama_home");
}

/**
 * Rend au service système son dossier `.ollama` (une invite polkit, puis
 * redémarrage d'Ollama). Rejette si une autre opération ou des
 * téléchargements sont en cours.
 */
export function fixOllamaHome(): Promise<HomeFixOutcome> {
  return invoke<HomeFixOutcome>("fix_ollama_home");
}

/**
 * Compte ollama.com du serveur Ollama (/api/me) : relié (nom, e-mail, offre,
 * avatar en URL data:) ou non. Rejette si le serveur est injoignable ou
 * répond autre chose.
 */
export function accountStatus(): Promise<AccountStatus> {
  return invoke<AccountStatus>("account_status");
}

/**
 * Ouvre dans le navigateur la page de connexion d'ollama.com donnée par le
 * serveur ; sans effet si le compte est déjà relié. Rejette avec « Failed to
 * fetch connect URL » sans adresse valable.
 */
export function accountSignIn(): Promise<void> {
  return invoke<void>("account_sign_in");
}

/** Délie le compte du serveur Ollama (/api/signout). */
export function accountSignOut(): Promise<void> {
  return invoke<void>("account_sign_out");
}

/**
 * Le nuage d'Ollama est-il coupé sur le serveur (/api/status) ? null si le
 * serveur n'en dit rien (trop ancien, injoignable...).
 */
export function cloudStatus(): Promise<CloudStatus | null> {
  return invoke<CloudStatus | null>("cloud_status");
}

/** Ouvre un lien http(s) dans le navigateur par défaut ; rejette si l'ouverture échoue. */
export function openExternal(url: string): Promise<void> {
  return invoke<void>("open_external", { url });
}

/** Modèles installés utilisables en conversation, du plus récent au plus ancien. */
export function listModels(): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("list_models");
}

/**
 * Capacités et réglages du raisonnement d'un modèle (/api/show). Rejette
 * comme listModels (Ollama injoignable...), et si le modèle est absent.
 */
export function modelDetails(model: string): Promise<ModelDetails> {
  return invoke<ModelDetails>("model_details", { model });
}

/**
 * Modèles recommandés par le serveur Ollama
 * (/api/experimental/model-recommendations), sans les modèles cloud ni le
 * suffixe `:latest` ; liste vide si le serveur n'en propose pas.
 */
export function recommendedModels(): Promise<string[]> {
  return invoke<string[]>("recommended_models");
}

/**
 * Le modèle existe-t-il au registre (ollama.com) ? Rejette si le registre
 * est injoignable.
 */
export function modelExistsUpstream(model: string): Promise<boolean> {
  return invoke<boolean>("model_exists_upstream", { model });
}

/**
 * Parmi `models`, les modèles installés dont le registre (ollama.com,
 * Hugging Face) a une version plus récente, dans l'ordre reçu. Rejette comme
 * listModels (Ollama injoignable...) ; un registre injoignable ne signale
 * rien.
 */
export function outdatedModels(models: string[]): Promise<string[]> {
  return invoke<string[]>("outdated_models", { models });
}

/**
 * Modèles en mémoire (/api/ps), dans l'ordre d'Ollama. Rejette comme
 * listModels (Ollama injoignable...).
 */
export function runningModels(): Promise<RunningModel[]> {
  return invoke<RunningModel[]>("running_models");
}

/**
 * Charge un modèle avec le contexte et la durée des réglages ; résout une
 * fois le modèle chargé, rejette avec l'erreur d'Ollama (modèle absent,
 * mémoire insuffisante...).
 */
export function loadModel(model: string): Promise<void> {
  return invoke<void>("load_model", { model });
}

/** Décharge un modèle (sans effet s'il n'est pas chargé). */
export function unloadModel(model: string): Promise<void> {
  return invoke<void>("unload_model", { model });
}

/** Décharge tous les modèles en mémoire. */
export function unloadAllModels(): Promise<void> {
  return invoke<void>("unload_all_models");
}

/** Réglage du raisonnement mémorisé pour un modèle, ou null. */
export function getThinkPref(model: string): Promise<Think | null> {
  return invoke<Think | null>("get_think_pref", { model });
}

/** Mémorise le réglage du raisonnement choisi pour un modèle. */
export function setThinkPref(model: string, think: Think): Promise<void> {
  return invoke<void>("set_think_pref", { model, think });
}

/** Historique, de la conversation la plus récemment active à la plus ancienne. */
export function listChats(): Promise<ChatInfo[]> {
  return invoke<ChatInfo[]>("list_chats");
}

export function getChat(id: string): Promise<Chat> {
  return invoke<Chat>("get_chat", { id });
}

/**
 * Envoie un message ; les événements arrivent sur `onEvent`, et la promesse
 * se résout après l'événement final. Rejette si l'envoi est refusé d'emblée
 * (message vide, conversation occupée...).
 */
export function sendMessage(
  request: SendRequest,
  onEvent: Channel<ChatEvent>,
): Promise<void> {
  return invoke<void>("send_message", { request, onEvent });
}

/**
 * Prépare des fichiers choisis par leur chemin (sélecteur natif, dépôt), dans
 * l'ordre : chacun est validé, lu et son texte extrait une fois. Un dossier
 * donne ses fichiers de premier niveau, triés par nom. Un fichier refusé
 * porte le texte officiel (« File type not supported »...).
 */
export function stagePaths(paths: string[]): Promise<StageResult[]> {
  return invoke<StageResult[]>("stage_paths", { paths });
}

/**
 * Ouvre le sélecteur de fichiers natif (« Select Files », extensions
 * acceptées) ; chemins choisis, liste vide si l'utilisateur annule.
 */
export function pickFiles(): Promise<string[]> {
  return invoke<string[]>("pick_files");
}

/**
 * Prépare un fichier collé : son contenu part en octets bruts, son nom dans
 * l'en-tête `x-filename`. `size` (taille réelle) permet au backend de refuser
 * un fichier trop gros envoyé sans ses octets.
 */
export function stageBytes(
  filename: string,
  data: Uint8Array,
  size?: number,
): Promise<StageResult> {
  return invoke<StageResult>("stage_bytes", data, {
    headers: {
      "x-filename": encodeURIComponent(filename),
      ...(size !== undefined && { "x-size": String(size) }),
    },
  });
}

/**
 * Prépare l'image du presse-papiers (capture d'écran collée, que WebKitGTK
 * ne donne pas à la page) sous le nom « image.png » ; null s'il n'y en a pas.
 */
export function stageClipboardImage(): Promise<StageResult | null> {
  return invoke<StageResult | null>("stage_clipboard_image");
}

/** Retire des fichiers préparés ; sans effet pour un identifiant inconnu. */
export function unstage(ids: string[]): Promise<void> {
  return invoke<void>("unstage", { ids });
}

/** Arrête une réponse en cours. */
export function cancelMessage(streamId: string): Promise<void> {
  return invoke<void>("cancel_message", { streamId });
}

export function renameChat(id: string, title: string): Promise<void> {
  return invoke<void>("rename_chat", { id, title });
}

/**
 * Range une conversation dans les favoris ou l'en retire (sa date d'activité
 * ne change pas) ; rejette si elle n'existe plus.
 */
export function setChatFavorite(id: string, favorite: boolean): Promise<void> {
  return invoke<void>("set_chat_favorite", { id, favorite });
}

export function deleteChat(id: string): Promise<void> {
  return invoke<void>("delete_chat", { id });
}

// ModelHub (src-tauri/src/commands/hub.rs). Les problèmes de Hugging Face
// (hors ligne, panne, limite de requêtes...) arrivent dans `status.problem`
// des réponses ; une commande ne rejette que pour une vraie panne interne,
// un nom invalide ou une erreur d'Ollama.

/** Accueil : rubriques (6 dépôts au plus) et familles, filtrées. */
export function hubHome(filters: HubFilters): Promise<HubHome> {
  return invoke<HubHome>("hub_home", { filters });
}

/** Liste complète d'une rubrique ou d'une famille, filtrée. */
export function hubBrowse(
  scope: HubScope,
  filters: HubFilters,
): Promise<HubList> {
  return invoke<HubList>("hub_browse", { scope, filters });
}

/** Recherche libre sur Hugging Face ; page suivante par `search.cursor`. */
export function hubSearch(search: HubSearch): Promise<HubList> {
  return invoke<HubList>("hub_search", { search });
}

/** Fiche d'un dépôt (`id` = « auteur/dépôt »). */
export function hubRepo(id: string): Promise<HubRepoResult> {
  return invoke<HubRepoResult>("hub_repo", { id });
}

/** README d'un dépôt à une révision (`sha` de la fiche, sinon « main »). */
export function hubReadme(id: string, revision: string): Promise<HubReadme> {
  return invoke<HubReadme>("hub_readme", { id, revision });
}

/** Mémoire de la machine du serveur ; null pour un serveur distant. */
export function hubHardware(): Promise<HubHardware | null> {
  return invoke<HubHardware | null>("hub_hardware");
}

/**
 * Lance (ou met en file) un téléchargement ; sans effet s'il est déjà en
 * cours ou en file. Rejette pour un nom invalide.
 */
export function hubDownload(request: HubDownloadRequest): Promise<void> {
  return invoke<void>("hub_download", { request });
}

/** Arrête un téléchargement en cours ou en file. */
export function hubCancelDownload(model: string): Promise<void> {
  return invoke<void>("hub_cancel_download", { model });
}

/**
 * Met en pause un téléchargement en file ou en cours : Ollama garde ce qui
 * est reçu, la reprise repart de là.
 */
export function hubPauseDownload(model: string): Promise<void> {
  return invoke<void>("hub_pause_download", { model });
}

/** Reprend un téléchargement en pause (de nouveau en file, à sa place). */
export function hubResumeDownload(model: string): Promise<void> {
  return invoke<void>("hub_resume_download", { model });
}

/**
 * Fait passer un téléchargement en premier : il démarre tout de suite, le
 * dernier en cours retournant en file si les places sont prises.
 */
export function hubPrioritizeDownload(model: string): Promise<void> {
  return invoke<void>("hub_prioritize_download", { model });
}

/** Téléchargements en cours, en file et terminés, dans l'ordre de demande. */
export function hubDownloads(): Promise<HubDownload[]> {
  return invoke<HubDownload[]>("hub_downloads");
}

/** Retire de la liste un téléchargement terminé, échoué ou arrêté. */
export function hubDismissDownload(model: string): Promise<void> {
  return invoke<void>("hub_dismiss_download", { model });
}

/** Tous les modèles installés (embeddings compris), du plus récent au plus ancien. */
export function hubInstalled(): Promise<HubInstalled[]> {
  return invoke<HubInstalled[]>("hub_installed");
}

/** Supprime un modèle du serveur. */
export function hubDeleteModel(model: string): Promise<void> {
  return invoke<void>("hub_delete_model", { model });
}

/** Événement du moteur à chaque changement des téléchargements (hub::DOWNLOADS_EVENT). */
export const HUB_DOWNLOADS_EVENT = "hub-downloads";

/** Écoute la liste complète des téléchargements, émise à chaque changement. */
export function listenHubDownloads(
  onChange: (downloads: HubDownload[]) => void,
): Promise<UnlistenFn> {
  return listen<HubDownload[]>(HUB_DOWNLOADS_EVENT, (event) =>
    onChange(event.payload),
  );
}

// Dictée (étape 17, propre à ChatonLM).

/** Réglages de la dictée. */
export function getDictationSettings(): Promise<DictationSettings> {
  return invoke<DictationSettings>("get_dictation_settings");
}

/**
 * Enregistre puis applique (raccourci lié ou délié selon `enabled`) ; renvoie
 * les réglages à jour.
 */
export function updateDictationSettings(
  patch: DictationPatch,
): Promise<DictationSettings> {
  return invoke<DictationSettings>("update_dictation_settings", { patch });
}

/** Consignes par défaut du nettoyage et langues proposées. */
export function dictationInfo(): Promise<DictationInfo> {
  return invoke<DictationInfo>("dictation_info");
}

/** Phase, raccourci, zone de notification et dernier résultat. */
export function dictationStatus(): Promise<DictationStatus> {
  return invoke<DictationStatus>("dictation_status");
}

/** Commence ou arrête une dictée (comme le raccourci en mode bascule). */
export function dictationToggle(): Promise<void> {
  return invoke<void>("dictation_toggle");
}

/** Abandonne la dictée en cours sans rien copier. */
export function dictationCancel(): Promise<void> {
  return invoke<void>("dictation_cancel");
}

/**
 * Copie le texte du dernier résultat dans le presse-papiers (copie
 * automatique coupée) ; `lastResult.copied` passe à vrai. Erreur « Nothing to
 * copy » sans texte.
 */
export function dictationCopyLast(): Promise<void> {
  return invoke<void>("dictation_copy_last");
}

/** Nouvel essai de liaison du raccourci global. */
export function dictationBindShortcut(): Promise<void> {
  return invoke<void>("dictation_bind_shortcut");
}

/** Ouvre la boîte du bureau qui choisit la touche du raccourci. */
export function dictationConfigureShortcut(): Promise<void> {
  return invoke<void>("dictation_configure_shortcut");
}

/** Événement du moteur à chaque changement du statut (dictation::STATUS_EVENT). */
export const DICTATION_STATUS_EVENT = "dictation-status";

export function listenDictationStatus(
  onChange: (status: DictationStatus) => void,
): Promise<UnlistenFn> {
  return listen<DictationStatus>(DICTATION_STATUS_EVENT, (event) =>
    onChange(event.payload),
  );
}

/**
 * Corrige ou reformule `text` avec le modèle de la correction (réglage
 * `proofreadModel`, sinon celui du chat) ; les évènements arrivent sur
 * `onEvent` (Started, Delta, PartDone, puis Done ou Error) et la promesse se
 * résout après le dernier. `jobId`, choisi par la page, sert à l'arrêter.
 */
export function proofreadRun(
  jobId: string,
  text: string,
  mode: ProofreadMode,
  onEvent: Channel<ProofreadEvent>,
): Promise<void> {
  return invoke<void>("proofread_run", { jobId, text, mode, onEvent });
}

/** Arrête une correction ; sans effet si elle est déjà finie. */
export function proofreadCancel(jobId: string): Promise<void> {
  return invoke<void>("proofread_cancel", { jobId });
}

/**
 * Arrête toutes les corrections en cours : au chargement de la page, celles
 * d'une page rechargée (plus personne ne lit leurs évènements).
 */
export function proofreadCancelAll(): Promise<void> {
  return invoke<void>("proofread_cancel_all");
}

/** Dernière correction du raccourci global (action « Show »), ou null. */
export function proofreadLastSelection(): Promise<SelectionResult | null> {
  return invoke<SelectionResult | null>("proofread_last_selection");
}

/** Raccourcis globaux (dictée, correction de la sélection). */
export function shortcutsStatus(): Promise<ShortcutsStatus> {
  return invoke<ShortcutsStatus>("shortcuts_status");
}

/** Nouvel essai de liaison des raccourcis globaux demandés. */
export function shortcutsBind(): Promise<void> {
  return invoke<void>("shortcuts_bind");
}

/** Ouvre la boîte du bureau qui choisit les touches des raccourcis. */
export function shortcutsConfigure(): Promise<void> {
  return invoke<void>("shortcuts_configure");
}

/** Raccourci global que l'app fait choisir (Windows). */
export type GlobalShortcutId = "dictation" | "proofread";

/**
 * Combinaison saisie dans l'app pour un raccourci global (Windows, où
 * ChatonLM enregistre lui-même les touches : `chooser` vaut « app ») : le
 * moteur la contrôle, l'enregistre et relie. Rejette avec un message traduit
 * (touche refusée, modificateur manquant, même combinaison que l'autre
 * raccourci) ; une combinaison tenue par une autre application est gardée,
 * et le statut rendu le dit (`taken`).
 */
export function shortcutsSetTrigger(
  id: GlobalShortcutId,
  keys: ShortcutKeys,
): Promise<ShortcutsStatus> {
  return invoke<ShortcutsStatus>("shortcuts_set_trigger", { id, keys });
}

/** Oublie la combinaison choisie : retour à celle par défaut. */
export function shortcutsResetTrigger(
  id: GlobalShortcutId,
): Promise<ShortcutsStatus> {
  return invoke<ShortcutsStatus>("shortcuts_reset_trigger", { id });
}

/**
 * Saisie d'une combinaison ouverte ou fermée : ouverte, les raccourcis
 * globaux sont suspendus (la combinaison actuelle arrive à la page au lieu de
 * lancer la dictée).
 */
export function shortcutsRecording(active: boolean): Promise<void> {
  return invoke<void>("shortcuts_recording", { active });
}

/** Événement du moteur à chaque changement (desktop::SHORTCUTS_EVENT). */
export const SHORTCUTS_STATUS_EVENT = "shortcuts-status";

export function listenShortcutsStatus(
  onChange: (status: ShortcutsStatus) => void,
): Promise<UnlistenFn> {
  return listen<ShortcutsStatus>(SHORTCUTS_STATUS_EVENT, (event) =>
    onChange(event.payload),
  );
}

/** Cours (onglet Study), du plus récemment actif au plus ancien. */
export function studyList(): Promise<CourseSummary[]> {
  return invoke<CourseSummary[]>("study_list");
}

/**
 * Cours complet ; le moteur relance la génération de ce qui manque.
 * Erreur `notFound` s'il n'existe pas (supprimé).
 */
export function studyGet(courseId: string): Promise<Course> {
  return invoke<Course>("study_get", { courseId });
}

/**
 * Crée un cours et lance la préparation du plan ; renvoie son identifiant.
 * Les documents (`documentIds`, fichiers préparés par `pickFiles` /
 * `stagePaths`) sont repris par le cours.
 */
export function studyCreate(course: NewCourse): Promise<string> {
  return invoke<string>("study_create", { course });
}

/** Sens choisi d'un sujet ambigu (`null` : garder le sujet tel quel). */
export function studyClarify(
  courseId: string,
  choice: string | null,
): Promise<void> {
  return invoke<void>("study_clarify", { courseId, choice });
}

/** Nouveau plan (cours pas encore commencé). */
export function studyReplan(courseId: string): Promise<void> {
  return invoke<void>("study_replan", { courseId });
}

/** Commence le cours (plan prêt). */
export function studyStart(courseId: string): Promise<void> {
  return invoke<void>("study_start", { courseId });
}

/**
 * Corrige une réponse ; une question déjà répondue renvoie sa première
 * réponse, inchangée.
 */
export function studyAnswer(
  courseId: string,
  questionId: string,
  answer: AnswerValue,
): Promise<Answered> {
  return invoke<Answered>("study_answer", { courseId, questionId, answer });
}

/** Enregistre l'étape et le temps passé (sans `study-updated`). */
export function studyProgress(
  courseId: string,
  position: string,
  elapsedMs: number,
): Promise<void> {
  return invoke<void>("study_progress", { courseId, position, elapsedMs });
}

/** « Explain differently » : nouvelle explication et nouvelles questions. */
export function studyRemedy(courseId: string, level: number): Promise<void> {
  return invoke<void>("study_remedy", { courseId, level });
}

/** « Continue anyway » : palier marqué à revoir. */
export function studySkip(courseId: string, level: number): Promise<void> {
  return invoke<void>("study_skip", { courseId, level });
}

/** Refait un palier (réponses du palier effacées). */
export function studyRegenerate(
  courseId: string,
  level: number,
): Promise<void> {
  return invoke<void>("study_regenerate", { courseId, level });
}

/** Relance les parties en échec ; `withoutWeb` : sans recherche web. */
export function studyRetry(
  courseId: string,
  withoutWeb = false,
): Promise<void> {
  return invoke<void>("study_retry", { courseId, withoutWeb });
}

/**
 * « I don't understand » sur une section : explication en flux sur
 * `onEvent` (Delta, puis Done ou Error), enregistrée à la fin.
 */
export function studyExplain(
  courseId: string,
  level: number,
  section: number,
  streamId: string,
  onEvent: Channel<StudyStreamEvent>,
): Promise<void> {
  return invoke<void>("study_explain", {
    courseId,
    level,
    section,
    streamId,
    onEvent,
  });
}

/** « Ask a question » sur un palier : réponse en flux, enregistrée. */
export function studyAsk(
  courseId: string,
  level: number,
  question: string,
  streamId: string,
  onEvent: Channel<StudyStreamEvent>,
): Promise<void> {
  return invoke<void>("study_ask", {
    courseId,
    level,
    question,
    streamId,
    onEvent,
  });
}

/** Arrête une réponse en flux ; sans effet si elle est finie. */
export function studyStreamCancel(streamId: string): Promise<void> {
  return invoke<void>("study_stream_cancel", { streamId });
}

/** Fiche du cours en Markdown (à copier). */
export function studyNotes(courseId: string): Promise<string> {
  return invoke<string>("study_notes", { courseId });
}

/** Enregistre la fiche (boîte native) ; `false` si annulé. */
export function studySaveNotes(courseId: string): Promise<boolean> {
  return invoke<boolean>("study_save_notes", { courseId });
}

/** Supprime un cours. */
export function studyDelete(courseId: string): Promise<void> {
  return invoke<void>("study_delete", { courseId });
}

/**
 * Met en pause (`paused`) ou reprend la génération d'un cours : en pause,
 * sa partie en cours d'écriture est arrêtée et plus rien n'est écrit pour
 * lui (les autres cours continuent) ; enregistré par le moteur.
 */
export function studyPause(courseId: string, paused: boolean): Promise<void> {
  return invoke<void>("study_pause", { courseId, paused });
}

/** Événement du moteur à chaque changement d'un cours (study::UPDATED_EVENT). */
export const STUDY_UPDATED_EVENT = "study-updated";

export function listenStudyUpdated(
  onChange: (update: StudyUpdated) => void,
): Promise<UnlistenFn> {
  return listen<StudyUpdated>(STUDY_UPDATED_EVENT, (event) =>
    onChange(event.payload),
  );
}

/** Catalogue des modèles de transcription, avec leur présence sur le disque. */
export function speechModels(): Promise<SpeechModelInfo[]> {
  return invoke<SpeechModelInfo[]>("speech_models");
}

/** Téléchargements des modèles de transcription. */
export function speechDownloads(): Promise<SpeechDownload[]> {
  return invoke<SpeechDownload[]>("speech_downloads");
}

/** Met un modèle de transcription en file (sans effet s'il y est déjà). */
export function speechDownload(id: string): Promise<void> {
  return invoke<void>("speech_download", { id });
}

export function speechCancelDownload(id: string): Promise<void> {
  return invoke<void>("speech_cancel_download", { id });
}

/** Retire de la liste un téléchargement fini, échoué ou annulé. */
export function speechDismissDownload(id: string): Promise<void> {
  return invoke<void>("speech_dismiss_download", { id });
}

/** Supprime les fichiers d'un modèle de transcription. */
export function speechDeleteModel(id: string): Promise<void> {
  return invoke<void>("speech_delete_model", { id });
}

/** Événement du moteur à chaque changement des téléchargements (dictation::downloads::DOWNLOADS_EVENT). */
export const SPEECH_DOWNLOADS_EVENT = "speech-downloads";

export function listenSpeechDownloads(
  onChange: (downloads: SpeechDownload[]) => void,
): Promise<UnlistenFn> {
  return listen<SpeechDownload[]>(SPEECH_DOWNLOADS_EVENT, (event) =>
    onChange(event.payload),
  );
}

// Lecture à voix haute (propre à ChatonLM, src-tauri/src/tts).

/**
 * Lit la réponse `messageId` de `chatId` (une seule lecture dans l'app : la
 * précédente s'arrête). Voix absente : `needsVoice`, sauf avec `download`
 * (accord donné) : la voix est téléchargée, puis la lecture démarre.
 */
export function readAloud(
  chatId: string,
  messageId: number,
  download: boolean,
): Promise<ReadAloudOutcome> {
  return invoke<ReadAloudOutcome>("read_aloud", {
    chatId,
    messageId,
    download,
  });
}

/**
 * Lit le passage `passage` du cours `courseId` (onglet Study ; clés dans
 * src-tauri/src/study/passage.rs), comme `readAloud`.
 */
export function readAloudStudy(
  courseId: string,
  passage: string,
  download: boolean,
): Promise<ReadAloudOutcome> {
  return invoke<ReadAloudOutcome>("read_aloud_study", {
    courseId,
    passage,
    download,
  });
}

/** Arrête la lecture en cours (sans effet au repos). */
export function readAloudStop(): Promise<void> {
  return invoke<void>("read_aloud_stop");
}

/** État de la lecture (lu au montage, puis tenu à jour par l'événement). */
export function readAloudState(): Promise<ReadAloudState> {
  return invoke<ReadAloudState>("read_aloud_state");
}

/** Événement du moteur à chaque changement de la lecture (tts::READ_ALOUD_EVENT). */
export const READ_ALOUD_EVENT = "read-aloud";

export function listenReadAloud(
  onChange: (state: ReadAloudState) => void,
): Promise<UnlistenFn> {
  return listen<ReadAloudState>(READ_ALOUD_EVENT, (event) =>
    onChange(event.payload),
  );
}

/** Catalogue des voix, avec leur présence sur le disque. */
export function voiceModels(): Promise<VoiceModelInfo[]> {
  return invoke<VoiceModelInfo[]>("voice_models");
}

/** Téléchargements des voix. */
export function voiceDownloads(): Promise<SpeechDownload[]> {
  return invoke<SpeechDownload[]>("voice_downloads");
}

/** Met une voix en file (sans effet si elle y est déjà). */
export function voiceDownload(id: string): Promise<void> {
  return invoke<void>("voice_download", { id });
}

export function voiceCancelDownload(id: string): Promise<void> {
  return invoke<void>("voice_cancel_download", { id });
}

/** Retire de la liste un téléchargement fini, échoué ou annulé. */
export function voiceDismissDownload(id: string): Promise<void> {
  return invoke<void>("voice_dismiss_download", { id });
}

/** Supprime les fichiers d'une voix (refusé pendant son téléchargement). */
export function voiceDeleteModel(id: string): Promise<void> {
  return invoke<void>("voice_delete_model", { id });
}

/** Événement du moteur à chaque changement des téléchargements des voix (tts::VOICE_DOWNLOADS_EVENT). */
export const VOICE_DOWNLOADS_EVENT = "voice-downloads";

export function listenVoiceDownloads(
  onChange: (downloads: SpeechDownload[]) => void,
): Promise<UnlistenFn> {
  return listen<SpeechDownload[]>(VOICE_DOWNLOADS_EVENT, (event) =>
    onChange(event.payload),
  );
}

// Mode conversation vocale (propre à ChatonLM, src-tauri/src/voice_mode) : une
// seule séance dans l'app ; la page qui l'a lancée envoie les phrases reçues.

/**
 * Ce qui manque avant de démarrer : modèle de transcription et voix de la
 * langue attendue (tailles des téléchargements), ou ce qui rend la machine
 * incapable (`unavailable`, texte traduit).
 */
export function voiceModeRequirements(): Promise<VoiceModeRequirements> {
  return invoke<VoiceModeRequirements>("voice_mode_requirements");
}

/**
 * Démarre la séance (sans effet si elle est déjà active) ; rejette avec un
 * texte traduit si un prérequis manque, si une dictée est en cours ou si le
 * micro ne s'ouvre pas.
 */
export function voiceModeStart(): Promise<void> {
  return invoke<void>("voice_mode_start");
}

/** Termine la séance (la voix s'arrête ; la réponse écrite continue). */
export function voiceModeStop(): Promise<void> {
  return invoke<void>("voice_mode_stop");
}

/** Ferme (`true`) ou rouvre le micro ; une réponse en cours reste lue. */
export function voiceModeMute(muted: boolean): Promise<void> {
  return invoke<void>("voice_mode_mute", { muted });
}

/** Coupe la voix et la réponse en cours (Responding, Speaking), puis écoute. */
export function voiceModeInterrupt(): Promise<void> {
  return invoke<void>("voice_mode_interrupt");
}

/** « J'ai fini » : la phrase en cours (Hearing) part tout de suite. */
export function voiceModeEndTurn(): Promise<void> {
  return invoke<void>("voice_mode_end_turn");
}

/**
 * Push-to-talk : appui (`true`, la réponse en cours est d'abord coupée) puis
 * relâche (`false`, fin du tour).
 */
export function voiceModeHold(pressed: boolean): Promise<void> {
  return invoke<void>("voice_mode_hold", { pressed });
}

/** Reprend l'écoute après la pause (2 min sans parole). */
export function voiceModeResume(): Promise<void> {
  return invoke<void>("voice_mode_resume");
}

/**
 * L'envoi de la phrase du tour `turn` a été refusé d'emblée par la page :
 * avis `failed` avec `message` (déjà traduit), puis nouvelle écoute.
 */
export function voiceModeReplyFailed(
  turn: number,
  message: string,
): Promise<void> {
  return invoke<void>("voice_mode_reply_failed", { turn, message });
}

/** État de la séance (lu au montage, puis tenu à jour par l'événement). */
export function voiceModeState(): Promise<VoiceModeState> {
  return invoke<VoiceModeState>("voice_mode_state");
}

/** Consigne vocale par défaut. */
export function voiceModeInfo(): Promise<VoiceModeInfo> {
  return invoke<VoiceModeInfo>("voice_mode_info");
}

export function getVoiceModeSettings(): Promise<VoiceModeSettings> {
  return invoke<VoiceModeSettings>("get_voice_mode_settings");
}

/** Enregistre et applique aussitôt à une séance en cours. */
export function updateVoiceModeSettings(
  patch: VoiceModeSettingsPatch,
): Promise<VoiceModeSettings> {
  return invoke<VoiceModeSettings>("update_voice_mode_settings", { patch });
}

/** Événement du moteur à chaque changement de la séance. */
export const VOICE_MODE_EVENT = "voice-mode";
/** Niveaux du micro et de la voix, ~15 fois par seconde pendant une séance. */
export const VOICE_MODE_LEVEL_EVENT = "voice-mode-level";
/** Phrase transcrite, à envoyer par la page de la séance. */
export const VOICE_MODE_UTTERANCE_EVENT = "voice-mode-utterance";

export function listenVoiceMode(
  onChange: (state: VoiceModeState) => void,
): Promise<UnlistenFn> {
  return listen<VoiceModeState>(VOICE_MODE_EVENT, (event) =>
    onChange(event.payload),
  );
}

export function listenVoiceModeLevel(
  onLevel: (level: VoiceModeLevel) => void,
): Promise<UnlistenFn> {
  return listen<VoiceModeLevel>(VOICE_MODE_LEVEL_EVENT, (event) =>
    onLevel(event.payload),
  );
}

export function listenVoiceModeUtterance(
  onUtterance: (utterance: VoiceModeUtterance) => void,
): Promise<UnlistenFn> {
  return listen<VoiceModeUtterance>(VOICE_MODE_UTTERANCE_EVENT, (event) =>
    onUtterance(event.payload),
  );
}

// Mises à jour de ChatonLM (étape 18c, src-tauri/src/updater).

/** État des mises à jour et réglage Auto-download. */
export function updateStatus(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("update_status");
}

/**
 * Vérifie tout de suite (« Check for updates », Retry) ; pendant une autre
 * vérification, attend sa fin. Renvoie l'état d'après ; la suite
 * (téléchargement en fond) arrive par l'évènement « update-status ».
 */
export function checkForUpdates(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("check_for_updates");
}

/**
 * Télécharge en fond la mise à jour annoncée (Auto-download coupé) ;
 * rejette s'il n'y en a pas.
 */
export function downloadUpdate(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("download_update");
}

/**
 * Arrête les services, installe la mise à jour prête (boîte de mot de passe
 * du système pour un .deb) et relance ChatonLM ; rejette, sans rien arrêter,
 * si elle n'est pas prête ou si le dossier de l'AppImage n'est pas
 * inscriptible.
 */
export function restartToUpdate(): Promise<void> {
  return invoke<void>("restart_to_update");
}

/** Réglage « Auto-download updates » ; renvoie l'état d'après. */
export function setAutoDownload(enabled: boolean): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("set_auto_download", { enabled });
}

/** Versions installées : ChatonLM, et l'Ollama intégré s'il est livré. */
export function appVersions(): Promise<AppVersions> {
  return invoke<AppVersions>("app_versions");
}

/** Événement du moteur à chaque changement de l'état des mises à jour. */
export const UPDATE_STATUS_EVENT = "update-status";

export function listenUpdateStatus(
  onChange: (status: UpdateStatus) => void,
): Promise<UnlistenFn> {
  return listen<UpdateStatus>(UPDATE_STATUS_EVENT, (event) =>
    onChange(event.payload),
  );
}

/** Quitte l'app (Ctrl+Q, menu de la zone de notification). */
export function quitApp(): Promise<void> {
  return invoke<void>("quit_app");
}

/**
 * Événement du moteur qui demande d'afficher une page (menu de la zone de
 * notification : « Dictation settings… ») ; charge utile : chemin de l'app
 * avec ses paramètres, « /settings?section=dictation ».
 */
export const NAVIGATE_EVENT = "navigate";

export function listenNavigate(
  onNavigate: (path: string) => void,
): Promise<UnlistenFn> {
  return listen<string>(NAVIGATE_EVENT, (event) => onNavigate(event.payload));
}

/** Erreur renvoyée par une commande du backend. */
export function isAppError(error: unknown): error is AppError {
  return (
    typeof error === "object" &&
    error !== null &&
    "kind" in error &&
    "message" in error
  );
}

/** Message lisible d'une erreur quelconque. */
export function errorMessage(error: unknown): string {
  if (isAppError(error)) return error.message;
  if (error instanceof Error) return error.message;
  return String(error);
}
