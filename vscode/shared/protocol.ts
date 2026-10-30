// Messages entre l'hôte de l'extension (Node) et le panneau (webview React).
// Contrats : .tools/research/87-lot1b-contract.md, 89-lot2-voice-contract.md
// (X : visite guidée parlée, chaton en caoutchouc), 91-lot3-gpu-completion-contract.md
// (X : autocomplétion locale, carte « Graphics card »).

import type { Chat } from "../../src/bindings/Chat";
import type { ChatEvent } from "../../src/bindings/ChatEvent";
import type { VoiceModeState } from "../../src/bindings/VoiceModeState";

/** Liaison de l'extension avec l'app. */
export type Connection =
  | "connecting"
  | "connected"
  | "notPaired"
  | "appClosed"
  | "bridgeOff"
  | "incompatible"
  | "error";

/** Avis montré au-dessus de la zone de saisie, jusqu'au prochain envoi. */
export type Notice =
  /** Le modèle choisi tourne hors de cette machine : la conversation
   * commencée sur la machine n'est pas continuée avec lui (son historique
   * partirait avec), une nouvelle est ouverte. */
  "offMachineNewChat";

export type PanelState = {
  connection: Connection;
  /** Texte d'erreur déjà traduit (état `error`). */
  error?: string;
  models: string[];
  /** Ceux de `models` qui tournent dans le cloud (marqués dans le sélecteur). */
  cloudModels: string[];
  model: string | null;
  /** La prochaine conversation sera éphémère (chaton-fantôme). */
  ephemeral: boolean;
  chatId: string | null;
  /** Une réponse s'écrit. */
  busy: boolean;
  /** Augmente à chaque nouvelle conversation : le panneau oublie alors tous
   * les envois montrés (ceux d'avant ne lui sont plus relayés). */
  generation: number;
  notice: Notice | null;
  /** Passage joint au prochain message tapé (« Ask about this part »). */
  attachment?: { path: string; lines: [number, number] } | null;
};

export type ReceiptItem = {
  kind: "selection" | "file" | "diagnostic" | "diff" | "terminal";
  label: string;
  /** Chemin relatif au dossier de travail. */
  path?: string;
  /** Première et dernière lignes, à partir de 1. */
  lines?: [number, number];
  chars: number;
};

/** Ce qui part au modèle avec un envoi. */
export type Receipt = {
  items: ReceiptItem[];
  chars: number;
  /** Estimation : caractères / 4. */
  tokens: number;
  contextLength: number | null;
  /**
   * Où part le texte : `local` = modèle sur cette machine (rien n'est
   * retiré) ; `cloud` = modèle cloud ; `remote` = Ollama sur une autre
   * machine ; `unknown` = l'app n'a pas pu le dire. Hors `local`, les
   * secrets sont retirés.
   */
  destination: "local" | "cloud" | "remote" | "unknown";
  /** Messages précédents de la conversation, envoyés avec (le moteur
   * renvoie tout l'historique au modèle), tels quels. */
  history?: { messages: number; chars: number };
  /** Retiré avant l'envoi (hors de cette machine seulement). */
  blocked: { label: string; reason: "secretFile" | "secretValue" }[];
};

export type Locale = "en" | "es" | "fr";
export type Theme = "light" | "dark";

export type QuickAction = "explain" | "fix" | "tests" | "commitMessage";

/** Ce que parcourt une visite guidée. */
export type TourKind = "file" | "selection" | "diff";

/** Segment d'une visite : lignes du fichier (à partir de 1) et texte lu. */
export type TourSegment = { lines: [number, number]; text: string };

/** Visite guidée parlée, telle que le panneau la montre. */
export type TourView = {
  /** Augmente à chaque nouvelle visite. */
  id: number;
  kind: TourKind;
  /** Chemin relatif au dossier de travail. */
  path: string;
  /** `planning` : le modèle prépare les segments ; `playing` : lecture ;
   * `paused` : arrêtée sur un segment (reprise possible) ; `ended` : finie
   * ou arrêtée (segments encore lisibles). */
  status: "planning" | "playing" | "paused" | "ended";
  segments: TourSegment[];
  /** Segment en cours (ou dernier lu), -1 avant le premier. */
  current: number;
  /** Ce qui est parti au modèle pour préparer la visite. */
  receipt: Receipt | null;
};

/** Phase d'une séance vocale (type ts-rs de la page de l'app). */
export type VoicePhase = VoiceModeState["phase"];

/** Séance vocale tenue par VS Code. */
export type VoiceView = {
  /** `duck` : chaton en caoutchouc ; `tour` : question sur un passage. */
  profile: "duck" | "tour";
  phase: VoicePhase;
  muted: boolean;
  /** Texte d'un avis de l'app (déjà traduit), ou null. */
  notice: string | null;
  /** Contexte partagé avec le modèle (passage, dernière commande échouée),
   * après filtrage hors machine. */
  shared: ReceiptItem[];
  /** Retiré du contexte avant l'envoi (hors de cette machine). */
  blocked: Receipt["blocked"];
  destination: Receipt["destination"];
  /** Seulement en `duck` : l'intégration du shell n'a donné aucune
   * commande échouée. */
  noTerminalError: boolean;
};

export type TourCommand = "previous" | "next" | "pause" | "resume" | "stop";

/** Modèle tenu par le chef de la carte graphique de l'app (`gpu.status`). */
export type GpuSlot = {
  model: string;
  /** En mémoire (constat de `/api/ps`). */
  loaded: boolean;
  sizeBytes: number;
  /** Part dans la mémoire de la carte graphique. */
  vramBytes: number;
  /** Estimation avant chargement, ou null. */
  estimatedBytes: number | null;
  /** Une partie tourne sur le processeur. */
  partialGpu: boolean;
};

/** Plan du chef : conversation et autocomplétion ensemble, l'une après
 * l'autre, ou l'autocomplétion ne tient pas (`unknown` : pas de mesure). */
export type GpuPlan = {
  mode: "together" | "exclusive" | "doesNotFit" | "unknown";
  chat: GpuSlot | null;
  completion: GpuSlot | null;
  /** VRAM d'une carte reconnue (`gpu`), sinon mémoire vive ; `free` null :
   * inconnue (AMD sous Windows) ; null : serveur d'une autre machine. */
  memory: { total: number; free: number | null; gpu: boolean } | null;
  /** Une réponse de conversation s'écrit (page, bot, pont, voix). */
  chatBusy: boolean;
  /** Pourquoi l'autocomplétion ne peut pas servir ; absent : prête. */
  reason?: CompletionPause;
};

/** Raison d'une autocomplétion en pause (plan, ou refus -32007 `paused`). */
export type CompletionPause =
  | "exclusive"
  | "chatBusy"
  | "doesNotFit"
  | "notInstalled"
  | "notLocal"
  | "notLoaded";

/** Modèle d'autocomplétion proposé (`completion.suggest`). */
export type CompletionOption = {
  model: string;
  installed: boolean;
  license: string;
  /** Faux : licence non commerciale (Qwen2.5-Coder 3B). */
  commercialUse: boolean;
  /** Tient dans la mémoire de cette machine (estimation) ; null : machine
   * inconnue. */
  fits: boolean | null;
  /** Taille estimée en mémoire, ou null. */
  estimatedBytes: number | null;
};

/** Verdict de `completion.measure`. */
export type CompletionMeasure = {
  rating: "smooth" | "usable" | "tooSlow";
  firstTokenMs: number;
  tokensPerSecond: number;
  partialGpu: boolean;
};

/** Autocomplétion, telle que la barre d'état et le panneau la montrent. */
export type CompletionView = {
  enabled: boolean;
  /** `off` : coupée ; `disconnected` : app non reliée ; `notHere` : rien
   * n'est proposé dans le fichier actif (fichier sensible, sortie, réglages…) ;
   * `remoteWindow` : fenêtre distante (WSL, SSH, conteneur), fichiers hors de
   * cette machine ; `otherProvider` : une autre complétion en ligne (Copilot)
   * suggère sûrement dans le fichier actif ; `notLocal` : serveur ou modèle
   * hors de cette machine ; `missing` : modèle pas installé ; `checking` :
   * modèle pas encore connu ; `unusable` : l'app ne peut pas compléter avec
   * ce modèle (`error`) ; `slow` : prête, mais les dernières propositions
   * sont arrivées trop tard (délai de garde du moteur, plusieurs fois). */
  status:
    | "off"
    | "disconnected"
    | "notHere"
    | "remoteWindow"
    | "otherProvider"
    | "notLocal"
    | "missing"
    | "checking"
    | "paused"
    | "unusable"
    | "slow"
    | "ready";
  /** Raison de `paused`. */
  reason: CompletionPause | null;
  /** Texte de l'app (déjà traduit) pour `unusable`. */
  error: string | null;
  /** Modèle employé (réglage, ou celui recommandé pour `auto`). */
  model: string | null;
  /** Réglage `chatonlm.completion.model` (`auto` = recommandé). */
  setting: string;
  /** Nom de l'autre complétion en ligne (`otherProvider`). */
  otherProvider: string | null;
  /** Autre complétion en ligne installée, peut-être active ici (on ne peut
   * pas le savoir) : ChatonLM propose quand même et le dit. */
  alsoSuggests: string | null;
  plan: GpuPlan | null;
  recommended: string | null;
  options: CompletionOption[];
  preparing: boolean;
  /** Mesure du modèle employé. */
  measure:
    | { state: "running" }
    | { state: "done"; result: CompletionMeasure }
    | { state: "failed"; message: string }
    | null;
};

/** « Prepare » peut réussir : modèle pas chargé, ou l'un après l'autre avec
 * la conversation absente de la mémoire. Pas pour une pause que la
 * préparation ne lèverait pas (ne tient pas, pas installé, réponse en cours,
 * serveur distant). */
export function canPrepare(
  view: Pick<CompletionView, "enabled" | "status" | "reason" | "plan">,
): boolean {
  if (!view.enabled || view.status !== "paused") return false;
  if (view.reason === "notLoaded") return true;
  return view.reason === "exclusive" && view.plan?.chat?.loaded !== true;
}

export type CompletionCommand =
  | "enable"
  | "disable"
  | "prepare"
  | "measure"
  /** Carte ouverte : plan et propositions relus. */
  | "refresh"
  /** « Always suggest with ChatonLM » : les autres complétions en ligne ne
   * sont plus cherchées (réglage `chatonlm.completion.ignoreOthers`). */
  | "ignoreOthers";

/** Hôte → panneau. */
export type HostMessage =
  | { type: "init"; locale: Locale; theme: Theme; state: PanelState }
  | { type: "state"; state: PanelState }
  | { type: "chat"; chat: Chat }
  | { type: "event"; requestId: number; event: ChatEvent }
  | { type: "receipt"; requestId: number; receipt: Receipt }
  /** Reçus déjà donnés, par numéro du message de l'utilisateur : renvoyés à
   * l'ouverture du panneau (un envoi fait panneau fermé, ou avant un
   * rechargement, garde son reçu). */
  | { type: "receipts"; receipts: Record<number, Receipt> }
  | { type: "theme"; theme: Theme }
  /** Visite guidée en cours (ou dernière), null : aucune. */
  | { type: "tour"; tour: TourView | null }
  /** Séance vocale tenue par VS Code, null : aucune. */
  | { type: "voice"; voice: VoiceView | null }
  /** Niveau du micro, 0 à 1 (à part : jusqu'à ~15 par seconde). */
  | { type: "voiceLevel"; level: number }
  /** Autocomplétion et carte graphique (null : avant la première lecture). */
  | { type: "completion"; completion: CompletionView | null }
  /** Montre la carte « Graphics card » (demande ponctuelle de l'hôte, gardée
   * jusqu'au `ready` d'un panneau en cours d'ouverture). */
  | { type: "revealGpu" };

/** Panneau → hôte. */
export type PanelMessage =
  | { type: "ready" }
  | { type: "send"; text: string }
  | { type: "stop" }
  | { type: "newChat"; ephemeral: boolean }
  | { type: "selectModel"; model: string }
  | { type: "pair" }
  | { type: "launchApp" }
  | { type: "openApp" }
  | { type: "openLink"; url: string }
  | { type: "copy"; text: string }
  | { type: "action"; action: QuickAction }
  /** Garde la conversation éphémère ouverte (comme « Keep » de l'app). */
  | { type: "keep" }
  /** Relit les modèles de l'app (écran « aucun modèle »). */
  | { type: "refresh" }
  /** Erreur du panneau (rendu, script), écrite dans la sortie « ChatonLM ». */
  | { type: "panelError"; message: string }
  /** Commande de la visite guidée. */
  | { type: "tour"; command: TourCommand }
  /** Lecture reprise au segment `index`. */
  | { type: "tourGoto"; index: number }
  /** « Ask about this part » : tapée (passage joint au prochain message),
   * ou à la voix (séance `tour`). */
  | { type: "tourAsk"; voice: boolean }
  /** Retire le passage joint au prochain message. */
  | { type: "clearAttachment" }
  /** Chaton en caoutchouc (« Talk it through »), couper, arrêter, reprendre
   * une séance en pause. */
  | {
      type: "voice";
      command: "start" | "stop" | "mute" | "unmute" | "resume";
    }
  /** Autocomplétion : activer, couper, préparer, mesurer, relire. */
  | { type: "completion"; command: CompletionCommand }
  /** Modèle d'autocomplétion choisi dans la carte. */
  | { type: "completionModel"; model: string }
  /** « Install in ChatonLM » : l'app montre sa proposition. */
  | { type: "offerPull"; model: string };
