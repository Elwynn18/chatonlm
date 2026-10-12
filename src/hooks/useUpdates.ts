// Mises à jour de ChatonLM (carte « Updates » de Settings › General, étape
// 18c) : état lu par update_status, puis tenu à jour par l'évènement
// « update-status » du moteur, dans le cache de React Query, tant que la
// carte est montée (seul endroit de la page qui le montre : la zone de
// notification est tenue par le moteur). Une lecture pendant laquelle un
// évènement arrive garde celui de l'évènement (plus récent). Versions
// installées lues une fois.

import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { UpdateStatus } from "@/bindings/UpdateStatus";
import { appVersions, listenUpdateStatus, updateStatus } from "@/lib/ipc";

export const updateStatusKey = ["updates", "status"] as const;
export const appVersionsKey = ["updates", "versions"] as const;

/** Évènements reçus, par client (voir useDownloads.ts). */
const received = new WeakMap<QueryClient, number>();

/** Applique un état reçu du moteur. */
export function applyUpdateStatus(
  queryClient: QueryClient,
  status: UpdateStatus,
) {
  received.set(queryClient, (received.get(queryClient) ?? 0) + 1);
  queryClient.setQueryData(updateStatusKey, status);
}

/** État des mises à jour, tenu à jour par les évènements du moteur. */
export function useUpdateStatus() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenUpdateStatus((status) => applyUpdateStatus(queryClient, status)).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for the update status:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
  return useQuery({
    queryKey: updateStatusKey,
    queryFn: async () => {
      const before = received.get(queryClient) ?? 0;
      const status = await updateStatus();
      if ((received.get(queryClient) ?? 0) === before) return status;
      return queryClient.getQueryData<UpdateStatus>(updateStatusKey) ?? status;
    },
  });
}

/** Versions installées (elles ne changent qu'avec une relance). */
export function useAppVersions() {
  return useQuery({
    queryKey: appVersionsKey,
    queryFn: appVersions,
    staleTime: Infinity,
  });
}
