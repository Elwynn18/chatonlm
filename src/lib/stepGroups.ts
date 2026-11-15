// Étapes d'une réponse (raisonnements, appels d'outils et leurs résultats),
// regroupées sous un seul bloc repliable quand il y en a plusieurs.
import { useSyncExternalStore } from "react";
import type { Message } from "@/bindings/Message";
import type { ToolCall } from "@/bindings/ToolCall";
import { IMAGE_TOOL } from "@/lib/generatedImages";

/**
 * Un appel est une étape, sauf la création d'une image : l'image est le
 * résultat attendu, jamais repliée avec les étapes.
 */
export function isStepCall(call: ToolCall): boolean {
  return call.name !== IMAGE_TOOL;
}

/** Nombre d'étapes d'un message : son raisonnement et ses appels. */
export function stepsOf(message: Message): number {
  if (message.role !== "assistant") return 0;
  const thinking = message.thinking.trim() !== "" ? 1 : 0;
  return thinking + message.toolCalls.filter(isStepCall).length;
}

/** Tour de réponse : suite de messages de l'assistant après un envoi. */
export type StepTurn = {
  /** Identifiant du premier message du tour, qui porte l'en-tête. */
  key: number;
  /** Identifiants des messages du tour, dans l'ordre. */
  ids: number[];
  /** Étapes de tout le tour. */
  steps: number;
  /**
   * Le texte final a commencé : dernier message sans appel, avec du texte
   * (un texte suivi d'appels n'est su intermédiaire qu'à leur arrivée).
   */
  finalStarted: boolean;
};

/** Tour de chaque message de l'assistant, par identifiant. */
export function stepTurns(messages: readonly Message[]): Map<number, StepTurn> {
  const byId = new Map<number, StepTurn>();
  let turn: StepTurn | null = null;
  for (const message of messages) {
    if (message.role !== "assistant") {
      turn = null;
      continue;
    }
    if (turn === null) {
      turn = { key: message.id, ids: [], steps: 0, finalStarted: false };
    }
    turn.ids.push(message.id);
    turn.steps += stepsOf(message);
    turn.finalStarted =
      message.toolCalls.length === 0 && message.content.trim() !== "";
    byId.set(message.id, turn);
  }
  return byId;
}

// Choix de l'utilisateur (déplié ou replié) par tour, gardé tant que la page
// vit ; sans choix, l'état suit la réponse (déplié pendant le travail).
let choices: ReadonlyMap<number, boolean> = new Map();
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

const snapshot = () => choices;

/** Note le choix de l'utilisateur pour le tour `key`. */
export function setStepsExpanded(key: number, expanded: boolean) {
  choices = new Map(choices).set(key, expanded);
  for (const listener of listeners) listener();
}

/** Choix de l'utilisateur, par tour. */
export function useStepChoices(): ReadonlyMap<number, boolean> {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}

/**
 * Oublie les choix (essais : src/test/setup.ts, après le démontage des
 * rendus, donc sans prévenir personne).
 */
export function resetStepChoicesForTests() {
  choices = new Map();
}
