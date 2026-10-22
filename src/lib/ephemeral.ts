// Conversations éphémères (propres à ChatonLM) : absentes de l'historique,
// effacées par le moteur dès qu'on ouvre une autre conversation (`forget_chat`),
// à la fin de l'app et au démarrage suivant. Tenu hors des composants, comme
// la barre latérale (src/lib/sidebar.ts) : le bouton du chaton-fantôme est
// dans la mise en page, la zone de saisie dans la vue de la conversation.

import { useSyncExternalStore } from "react";

/** Bouton activé sur la page de nouvelle conversation. */
let newChatEphemeral = false;
/**
 * Conversation éphémère ouverte en dernier (créée par la page, ou relue
 * éphémère), à effacer quand on en ouvre une autre.
 */
let openEphemeral: string | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function notify() {
  for (const listener of listeners) listener();
}

/**
 * Active ou coupe le mode éphémère de la prochaine nouvelle conversation ;
 * gardé en changeant de page, coupé à la création de la conversation.
 */
export function setNewChatEphemeral(next: boolean) {
  if (next === newChatEphemeral) return;
  newChatEphemeral = next;
  notify();
}

/** Mode éphémère de la prochaine nouvelle conversation. */
export function newChatIsEphemeral(): boolean {
  return newChatEphemeral;
}

/** Mode éphémère de la page de nouvelle conversation ; suivi au rendu. */
export function useNewChatEphemeral(): boolean {
  return useSyncExternalStore(subscribe, () => newChatEphemeral);
}

/**
 * Conversation éphémère ouverte (créée par la page, ou relue éphémère après
 * un rechargement) ; la précédente, s'il y en avait une autre, est rendue
 * pour être effacée.
 */
export function markEphemeral(chatId: string): string | null {
  const previous = openEphemeral !== chatId ? openEphemeral : null;
  openEphemeral = chatId;
  return previous;
}

/** Conversation gardée (« Keep ») : plus rien à effacer pour elle. */
export function unmarkEphemeral(chatId: string) {
  if (openEphemeral === chatId) openEphemeral = null;
}

/**
 * Conversation ouverte (`chatId`, ou "new") : la conversation éphémère
 * ouverte avant elle, si c'en est une autre, est rendue pour être effacée.
 * Quitter la conversation pour une autre page (réglages, ModelHub…) ne
 * l'efface pas : on peut y revenir.
 */
export function openedChat(chatId: string): string | null {
  if (openEphemeral === null || openEphemeral === chatId) return null;
  const left = openEphemeral;
  openEphemeral = null;
  return left;
}

/** Remise à zéro (essais). */
export function resetEphemeral() {
  newChatEphemeral = false;
  openEphemeral = null;
  notify();
}
