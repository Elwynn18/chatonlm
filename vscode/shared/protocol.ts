// Messages entre l'hôte de l'extension (Node) et le panneau (webview React).
// Contrat : .tools/research/87-lot1b-contract.md.

import type { Chat } from "../../src/bindings/Chat";
import type { ChatEvent } from "../../src/bindings/ChatEvent";

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
};

export type ReceiptItem = {
  kind: "selection" | "file" | "diagnostic" | "diff";
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
  | { type: "theme"; theme: Theme };

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
  | { type: "panelError"; message: string };
