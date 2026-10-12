// Téléchargements du ModelHub (propre à ChatonLM) : liste lue une fois par
// hub_downloads, puis tenue à jour par l'événement « hub-downloads » du
// moteur (liste complète à chaque changement), dans le cache de React Query.
// Un seul écouteur, monté avec l'app (useDownloadEvents, route racine) : la
// fin d'un téléchargement est vue de toute page, barre latérale fermée
// comprise, et le sélecteur de modèles et la liste des installés sont relus.
// useDownloads ne fait que lire le cache.

import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { HubDownload } from "@/bindings/HubDownload";
import { hubInstalledKey } from "@/hooks/useHub";
import { modelsKey } from "@/hooks/useModels";
import {
  hubCancelDownload,
  hubDismissDownload,
  hubDownload,
  hubDownloads,
  listenHubDownloads,
} from "@/lib/ipc";
import { sameModel } from "@/lib/modelNames";

export const hubDownloadsKey = ["hub", "downloads"] as const;

const NONE: HubDownload[] = [];

/**
 * Événements reçus, par client : une lecture de hub_downloads pendant
 * laquelle un événement arrive renvoie une liste plus ancienne que lui
 * (réponse et événements ne sont pas ordonnés entre eux) ; elle garde alors
 * celle de l'événement.
 */
const received = new WeakMap<QueryClient, number>();

/**
 * Applique une liste reçue du moteur ; relit les modèles installés si un
 * téléchargement vient de finir.
 */
export function applyDownloads(
  queryClient: QueryClient,
  downloads: HubDownload[],
) {
  received.set(queryClient, (received.get(queryClient) ?? 0) + 1);
  const previous =
    queryClient.getQueryData<HubDownload[]>(hubDownloadsKey) ?? NONE;
  queryClient.setQueryData(hubDownloadsKey, downloads);
  const finished = downloads.some(
    (download) =>
      download.state === "done" &&
      !previous.some(
        (before) =>
          before.state === "done" &&
          sameModel(before.model, download.model) &&
          before.finishedAt === download.finishedAt,
      ),
  );
  if (finished) {
    void queryClient.invalidateQueries({ queryKey: modelsKey });
    void queryClient.invalidateQueries({ queryKey: hubInstalledKey });
  }
}

/**
 * Téléchargements, dans l'ordre de demande (vide tant que la liste n'est pas
 * lue : `loaded`). Lecture seule : les événements du moteur arrivent par
 * useDownloadEvents.
 */
export function useDownloads(): { downloads: HubDownload[]; loaded: boolean } {
  const queryClient = useQueryClient();
  const { data, isSuccess } = useQuery({
    queryKey: hubDownloadsKey,
    queryFn: async () => {
      const before = received.get(queryClient) ?? 0;
      const downloads = await hubDownloads();
      if ((received.get(queryClient) ?? 0) === before) return downloads;
      return (
        queryClient.getQueryData<HubDownload[]>(hubDownloadsKey) ?? downloads
      );
    },
  });
  return { downloads: data ?? NONE, loaded: isSuccess };
}

/**
 * Écoute l'événement « hub-downloads » tant que le composant est monté : une
 * seule fois, au niveau de l'app (route racine).
 */
export function useDownloadEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenHubDownloads((downloads) =>
      applyDownloads(queryClient, downloads),
    ).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for downloads:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}

/** Téléchargement d'un modèle (comparaison sans la casse), ou null. */
export function findDownload(
  downloads: readonly HubDownload[],
  model: string | null,
): HubDownload | null {
  if (!model) return null;
  return downloads.find((download) => sameModel(download.model, model)) ?? null;
}

/**
 * Lancer, arrêter, retirer : la liste est relue après chaque commande (le
 * moteur émet aussi l'événement).
 */
export function useDownloadActions() {
  const queryClient = useQueryClient();
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: hubDownloadsKey });
  const start = useMutation({ mutationFn: hubDownload, onSettled: refresh });
  const cancel = useMutation({
    mutationFn: hubCancelDownload,
    onSettled: refresh,
  });
  const dismiss = useMutation({
    mutationFn: hubDismissDownload,
    onSettled: refresh,
  });
  return { start, cancel, dismiss };
}
