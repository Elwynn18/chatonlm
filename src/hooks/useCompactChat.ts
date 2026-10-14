// Compactage manuel d'une conversation (« Compact now », propre à ChatonLM) :
// le modèle choisi dans la zone de saisie résume tout ce qui suit le dernier
// compactage (commande compact_chat). Un seul à la fois par conversation,
// partagé entre le panneau de la jauge et l'action sous l'erreur
// `contextFull` (magasin hors de React). Réussi : la conversation est relue
// (séparateur, jauge), l'erreur `contextFull` affichée disparaît.

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useState, useSyncExternalStore } from "react";
import { chatKey } from "@/hooks/useChats";
import { useModels } from "@/hooks/useModels";
import { useSettings } from "@/hooks/useSettings";
import { compactChat, errorMessage } from "@/lib/ipc";
import { selectedEntry } from "@/lib/models";
import { clearError, errorFor } from "@/lib/streams";

/** Conversations en cours de compactage manuel. */
let running = new Set<string>();
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function setRunning(chatId: string, on: boolean) {
  if (running.has(chatId) === on) return;
  running = new Set(running);
  if (on) running.add(chatId);
  else running.delete(chatId);
  for (const listener of listeners) listener();
}

/**
 * Identifiant sous lequel le moteur tient le compactage manuel : Stop
 * l'arrête par `cancel_message`, comme une réponse.
 */
export function compactStreamId(chatId: string): string {
  return `compact:${chatId}`;
}

/** Compactage manuel en cours dans cette conversation. */
export function useManualCompacting(chatId: string): boolean {
  return useSyncExternalStore(subscribe, () => running.has(chatId));
}

/** Pour les tests : aucun compactage en cours. */
export function resetCompactions() {
  running = new Set();
}

/**
 * Modèle choisi dans la zone de saisie (nom installé), sans les effets de
 * useSelectedModel (déjà rendus par la zone de saisie) ; null : aucun.
 */
function useComposerModel(): string | null {
  const { settings } = useSettings();
  const { data: models } = useModels();
  return (
    selectedEntry(models ?? [], settings?.selectedModel ?? "")?.model ?? null
  );
}

export function useCompactChat(chatId: string) {
  const queryClient = useQueryClient();
  const model = useComposerModel();
  const compacting = useSyncExternalStore(subscribe, () => running.has(chatId));
  // Échec du dernier essai fait ici (message du moteur), effacé au suivant.
  const [error, setError] = useState<string | null>(null);

  const compact = useCallback(async () => {
    if (model === null || running.has(chatId)) return;
    setError(null);
    setRunning(chatId, true);
    try {
      await compactChat(chatId, model);
      if (errorFor(chatId)?.kind === "contextFull") clearError(chatId);
      await queryClient.invalidateQueries({ queryKey: chatKey(chatId) });
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setRunning(chatId, false);
    }
  }, [chatId, model, queryClient]);

  return { compact, compacting, error, hasModel: model !== null };
}
