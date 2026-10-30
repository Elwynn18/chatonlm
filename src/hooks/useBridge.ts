// Pont de l'extension ChatonLM pour VS Code (onglet « VS Code » des réglages,
// propre à ChatonLM) : état du pont, éditeurs appairés et code d'appairage,
// dans le cache de React Query. Le moteur émet « bridge » à chaque changement
// (éditeur appairé, connecté ou déconnecté, code consommé ou expiré, pont
// ouvert ou en échec) ; un seul écouteur, monté avec l'app (useBridgeEvents,
// route racine), le verse dans le cache : en revenant sur l'onglet, un code
// consommé entre-temps n'y est déjà plus. Mêmes règles d'ordre que
// useEasyLink.ts : une lecture pendant laquelle un évènement arrive garde
// celui de l'évènement (plus récent) ; une commande dont la réponse arrive
// après un évènement fait relire l'état plutôt que de poser une réponse
// peut-être plus ancienne.

import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { BridgeState } from "@/bindings/BridgeState";
import { bridgeState, listenBridge } from "@/lib/ipc";

export const bridgeKey = ["bridge"] as const;

/** Évènements reçus, par client (voir useEasyLink.ts). */
const received = new WeakMap<QueryClient, number>();

/** Applique un état reçu du moteur. */
export function applyBridgeState(queryClient: QueryClient, state: BridgeState) {
  received.set(queryClient, (received.get(queryClient) ?? 0) + 1);
  queryClient.setQueryData(bridgeKey, state);
}

/** Écoute « bridge » tant que l'app est montée (route racine). */
export function useBridgeEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenBridge((state) => applyBridgeState(queryClient, state)).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for the VS Code bridge state:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}

/** État du pont, tenu à jour par les évènements du moteur. */
export function useBridge() {
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: bridgeKey,
    queryFn: async () => {
      const before = received.get(queryClient) ?? 0;
      const state = await bridgeState();
      if ((received.get(queryClient) ?? 0) === before) return state;
      return queryClient.getQueryData<BridgeState>(bridgeKey) ?? state;
    },
  });
}

/**
 * Commande du pont (ouverture, appairage, retrait...) : l'état qu'elle
 * renvoie va dans le cache. `error` : son refus, à montrer ; `reset`
 * l'efface.
 */
export function useBridgeCommand<A = void>(
  command: (argument: A) => Promise<BridgeState>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (argument: A) => {
      const before = received.get(queryClient) ?? 0;
      const state = await command(argument);
      if ((received.get(queryClient) ?? 0) === before) {
        queryClient.setQueryData(bridgeKey, state);
      } else {
        void queryClient.invalidateQueries({ queryKey: bridgeKey });
      }
      return state;
    },
  });
}
