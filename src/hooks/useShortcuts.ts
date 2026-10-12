// Raccourcis globaux (onglet « Keyboard shortcuts » des réglages, propre à
// ChatonLM) : statut de la dictée et de la correction de la sélection, dans
// le cache de React Query. Le moteur émet « shortcuts-status » à chaque
// changement ; un seul écouteur, monté avec l'app (useShortcutsEvents, route
// racine), le verse dans le cache. Une lecture pendant laquelle un
// évènement arrive garde celui de l'évènement (plus récent).

import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { ShortcutsStatus } from "@/bindings/ShortcutsStatus";
import {
  listenShortcutsStatus,
  shortcutsBind,
  shortcutsConfigure,
  shortcutsStatus,
} from "@/lib/ipc";

export const shortcutsStatusKey = ["shortcuts", "status"] as const;

/** Évènements reçus, par client (voir useDictation.ts). */
const received = new WeakMap<QueryClient, number>();

/** Applique un statut reçu du moteur. */
export function applyShortcutsStatus(
  queryClient: QueryClient,
  status: ShortcutsStatus,
) {
  received.set(queryClient, (received.get(queryClient) ?? 0) + 1);
  queryClient.setQueryData(shortcutsStatusKey, status);
}

/** Écoute « shortcuts-status » tant que l'app est montée (route racine). */
export function useShortcutsEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenShortcutsStatus((status) =>
      applyShortcutsStatus(queryClient, status),
    ).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for the shortcuts status:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}

/** Statut des raccourcis globaux, tenu à jour par les évènements. */
export function useShortcutsStatus() {
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: shortcutsStatusKey,
    queryFn: async () => {
      const before = received.get(queryClient) ?? 0;
      const status = await shortcutsStatus();
      if ((received.get(queryClient) ?? 0) === before) return status;
      return (
        queryClient.getQueryData<ShortcutsStatus>(shortcutsStatusKey) ?? status
      );
    },
  });
}

/**
 * Nouvel essai de liaison, boîte du bureau : le statut est relu après chaque
 * commande (le moteur émet aussi l'évènement).
 */
export function useShortcutsActions() {
  const queryClient = useQueryClient();
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: shortcutsStatusKey });
  const bind = useMutation({ mutationFn: shortcutsBind, onSettled: refresh });
  const configure = useMutation({
    mutationFn: shortcutsConfigure,
    onSettled: refresh,
  });
  return { bind, configure, refresh };
}
