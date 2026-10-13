// Easy Link (onglet des réglages, propre à ChatonLM) : bots Telegram et
// Discord, comptes liés et code d'appairage, dans le cache de React Query. Le
// moteur émet « easy-link » à chaque changement (liaison établie ou perdue,
// compte lié par un bot, code consommé ou expiré) ; un seul écouteur, monté
// avec l'app (useEasyLinkEvents, route racine), le verse dans le cache : en
// revenant sur l'onglet, un code consommé entre-temps n'y est déjà plus. Une
// lecture pendant laquelle un évènement arrive garde celui de l'évènement
// (plus récent). Chaque commande renvoie l'état d'après ; sa réponse et les
// évènements ne sont pas ordonnés entre eux : si un évènement est arrivé
// pendant la commande, l'état est relu plutôt que de poser une réponse
// peut-être plus ancienne.

import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { EasyLinkState } from "@/bindings/EasyLinkState";
import { easyLinkState, listenEasyLink } from "@/lib/ipc";

export const easyLinkKey = ["easyLink"] as const;

/** Évènements reçus, par client (voir useDownloads.ts). */
const received = new WeakMap<QueryClient, number>();

/** Applique un état reçu du moteur. */
export function applyEasyLinkState(
  queryClient: QueryClient,
  state: EasyLinkState,
) {
  received.set(queryClient, (received.get(queryClient) ?? 0) + 1);
  queryClient.setQueryData(easyLinkKey, state);
}

/** Écoute « easy-link » tant que l'app est montée (route racine). */
export function useEasyLinkEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenEasyLink((state) => applyEasyLinkState(queryClient, state)).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for the Easy Link state:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}

/** État d'Easy Link, tenu à jour par les évènements du moteur. */
export function useEasyLink() {
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: easyLinkKey,
    queryFn: async () => {
      const before = received.get(queryClient) ?? 0;
      const state = await easyLinkState();
      if ((received.get(queryClient) ?? 0) === before) return state;
      return queryClient.getQueryData<EasyLinkState>(easyLinkKey) ?? state;
    },
  });
}

/**
 * Commande d'Easy Link (connexion, appairage, déliaison...) : l'état qu'elle
 * renvoie va dans le cache. `error` : son refus, à montrer ; `reset`
 * l'efface.
 */
export function useEasyLinkCommand<A = void>(
  command: (argument: A) => Promise<EasyLinkState>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (argument: A) => {
      const before = received.get(queryClient) ?? 0;
      const state = await command(argument);
      if ((received.get(queryClient) ?? 0) === before) {
        queryClient.setQueryData(easyLinkKey, state);
      } else {
        void queryClient.invalidateQueries({ queryKey: easyLinkKey });
      }
      return state;
    },
  });
}
