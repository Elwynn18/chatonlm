// Appels d'outils (recherche web) enregistrés avec un message de l'assistant.
import type { ToolCall } from "@/bindings/ToolCall";

/** L'appel est terminé : résultat ou erreur reçus. */
export function isFinished(call: ToolCall): boolean {
  return call.result !== null || call.error !== null;
}
