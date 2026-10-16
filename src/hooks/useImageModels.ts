// Modèles d'images du ModelHub (propre à ChatonLM ; contrat
// .tools/research/83-image-models-contract.md) dans le cache de React Query :
// modèles (installés, en cours, recommandés), machine, recherche et fiches
// de Hugging Face, téléchargements. Les téléchargements sont émis par le
// moteur à chaque changement (« image-downloads ») ; un seul écouteur, monté
// avec l'app (useImageModelEvents, route racine), les verse dans le cache.
// Un téléchargement qui commence, finit, échoue ou disparaît change les
// modèles (ligne en base, pièces présentes, place libre) : modèles, fiches et
// machine sont alors relus.

import {
  keepPreviousData,
  type QueryClient,
  type QueryKey,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { useSettings } from "@/hooks/useSettings";
import {
  imageCancelDownload,
  imageDeleteModel,
  imageDismissDownload,
  imageDownload,
  imageDownloads,
  imageMachine,
  imageModels,
  imageRepo,
  imageSearch,
  imageUseModel,
  listenImageDownloads,
} from "@/lib/ipc";

/** Préfixe de toutes les requêtes des modèles d'images. */
export const imagesKey = ["images"] as const;
export const imageModelsKey = ["images", "models"] as const;
export const imageMachineKey = ["images", "machine"] as const;
export const imageDownloadsKey = ["images", "downloads"] as const;
/** Préfixe des fiches (toutes). */
export const imageReposKey = ["images", "repo"] as const;

/** Recherches et fiches fraîches une minute (le moteur a son propre cache). */
const LIST_STALE_MS = 60 * 1000;

const NO_DOWNLOADS: SpeechDownload[] = [];

/**
 * Événements reçus, par client : une lecture pendant laquelle un événement
 * arrive renvoie une liste plus ancienne que lui ; elle garde alors celle de
 * l'événement (comme useReadAloud).
 */
const received = new WeakMap<QueryClient, number>();

function eventCount(queryClient: QueryClient): number {
  return received.get(queryClient) ?? 0;
}

function countEvent(queryClient: QueryClient) {
  received.set(queryClient, eventCount(queryClient) + 1);
}

/** États des téléchargements, sans leur progression. */
function states(downloads: readonly SpeechDownload[]): string {
  return downloads
    .map((download) => `${download.id}\u0000${download.state}`)
    .sort()
    .join("\u0001");
}

/** Modèles, fiches et machine relus (fin, échec, nouveau téléchargement...). */
function refreshModels(queryClient: QueryClient) {
  const keys: QueryKey[] = [imageModelsKey, imageReposKey, imageMachineKey];
  for (const queryKey of keys) {
    void queryClient.invalidateQueries({ queryKey });
  }
}

/**
 * Relit les modèles si un téléchargement a changé d'état depuis `previous` ;
 * un modèle qui vient de finir est marqué installé aussitôt (sa carte ne
 * repasse pas un instant parmi les recommandations avant la relecture).
 */
function refreshIfChanged(
  queryClient: QueryClient,
  previous: readonly SpeechDownload[] | undefined,
  downloads: readonly SpeechDownload[],
) {
  if (previous === undefined || states(previous) === states(downloads)) return;
  const finished = new Set(
    downloads
      .filter(
        (download) =>
          download.state === "done" &&
          !previous.some(
            (before) => before.id === download.id && before.state === "done",
          ),
      )
      .map((download) => download.id),
  );
  if (finished.size > 0) {
    queryClient.setQueryData<ImageModelInfo[]>(imageModelsKey, (models) =>
      models?.map((model) =>
        finished.has(model.id)
          ? { ...model, installed: true, missingBytes: 0 }
          : model,
      ),
    );
  }
  refreshModels(queryClient);
}

/** Applique une liste de téléchargements reçue du moteur. */
export function applyImageDownloads(
  queryClient: QueryClient,
  downloads: SpeechDownload[],
) {
  countEvent(queryClient);
  const previous =
    queryClient.getQueryData<SpeechDownload[]>(imageDownloadsKey);
  queryClient.setQueryData(imageDownloadsKey, downloads);
  refreshIfChanged(queryClient, previous, downloads);
}

/**
 * Écoute « image-downloads » tant que le composant est monté : une seule
 * fois, au niveau de l'app (route racine).
 */
export function useImageModelEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenImageDownloads((downloads) =>
      applyImageDownloads(queryClient, downloads),
    ).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error: unknown) =>
        console.error("Failed to listen for image model downloads:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [queryClient]);
}

/** Installés et en cours, puis recommandations non installées. */
export function useImageModels() {
  return useQuery({ queryKey: imageModelsKey, queryFn: imageModels });
}

/**
 * Modèle d'images du mode Image de la zone de saisie (lot 4) : `chosen`
 * (choisi et installé), `none`, `loading` tant que les modèles ne sont pas
 * lus (l'envoi attend), `unknown` si leur lecture échoue (le moteur décide
 * alors, et refuse avec son texte).
 */
export type ImageModelState = "chosen" | "none" | "loading" | "unknown";

/** `enabled` : modèles lus seulement quand il le faut (mode Image actif). */
export function useImageModelState(enabled: boolean): ImageModelState {
  const { data, isError } = useQuery({
    queryKey: imageModelsKey,
    queryFn: imageModels,
    enabled,
  });
  if (data) {
    return data.some((model) => model.selected && model.installed)
      ? "chosen"
      : "none";
  }
  return isError ? "unknown" : "loading";
}

/** Ce que la machine permet (relue avec les modèles : place libre). */
export function useImageMachine() {
  return useQuery({ queryKey: imageMachineKey, queryFn: imageMachine });
}

/**
 * Réglage « Show uncensored models » (null tant que les réglages ne sont
 * pas lus) : la recherche et les fiches en dépendent.
 */
export function useShowUncensoredImages(): boolean | null {
  return useSettings().settings?.showUncensored ?? null;
}

/** Recherche (texte vide : dépôts les plus téléchargés). */
export function useImageSearch(query: string) {
  const uncensored = useShowUncensoredImages();
  return useQuery({
    queryKey: ["images", "search", uncensored, query],
    queryFn: () => imageSearch(query),
    enabled: uncensored !== null,
    staleTime: LIST_STALE_MS,
    placeholderData: keepPreviousData,
  });
}

/** Fiche d'un dépôt. */
export function useImageRepo(id: string) {
  const uncensored = useShowUncensoredImages();
  return useQuery({
    queryKey: [...imageReposKey, uncensored, id],
    queryFn: () => imageRepo(id),
    enabled: uncensored !== null,
    staleTime: LIST_STALE_MS,
  });
}

/**
 * Téléchargements des modèles d'images (vides tant qu'ils ne sont pas lus :
 * `loaded` ; une lecture en échec compte comme lue, liste vide).
 */
export function useImageDownloads(): {
  downloads: SpeechDownload[];
  loaded: boolean;
} {
  const queryClient = useQueryClient();
  const { data, isSuccess, isError } = useQuery({
    queryKey: imageDownloadsKey,
    queryFn: async () => {
      const previous =
        queryClient.getQueryData<SpeechDownload[]>(imageDownloadsKey);
      const before = eventCount(queryClient);
      let downloads = await imageDownloads();
      if (eventCount(queryClient) !== before) {
        downloads =
          queryClient.getQueryData<SpeechDownload[]>(imageDownloadsKey) ??
          downloads;
      }
      // Changement vu par cette lecture (événement manqué, page rouverte) :
      // les modèles aussi sont relus.
      refreshIfChanged(queryClient, previous, downloads);
      return downloads;
    },
  });
  return { downloads: data ?? NO_DOWNLOADS, loaded: isSuccess || isError };
}

/** `auteur/nom` et chemin du fichier d'un identifiant `<dépôt>/<fichier>`. */
export function splitImageModelId(id: string): { repo: string; file: string } {
  const parts = id.split("/");
  return { repo: parts.slice(0, 2).join("/"), file: parts.slice(2).join("/") };
}

/**
 * Révision d'un modèle connu (modèles en cache, sinon relus) ; à défaut,
 * celle de la fiche de son dépôt.
 */
async function revisionOf(
  queryClient: QueryClient,
  id: string,
): Promise<{ repo: string; file: string; revision: string }> {
  const models = await queryClient.fetchQuery({
    queryKey: imageModelsKey,
    queryFn: imageModels,
  });
  const model = models.find((entry: ImageModelInfo) => entry.id === id);
  if (model) {
    return { repo: model.repo, file: model.file, revision: model.revision };
  }
  const { repo, file } = splitImageModelId(id);
  const detail = await imageRepo(repo);
  return { repo, file, revision: detail.revision };
}

/**
 * Télécharger (fichier choisi, ou Try again d'un identifiant), arrêter,
 * retirer, supprimer, choisir un modèle : listes relues après chaque
 * commande.
 */
export function useImageActions() {
  const queryClient = useQueryClient();
  const refresh = () => {
    refreshModels(queryClient);
    return queryClient.invalidateQueries({ queryKey: imageDownloadsKey });
  };
  const download = useMutation({
    mutationFn: ({
      repo,
      file,
      revision,
    }: {
      repo: string;
      file: string;
      revision: string;
    }) => imageDownload(repo, file, revision),
    onSettled: refresh,
  });
  // Nouvel essai par l'identifiant (onglet Downloads, carte en échec).
  const retry = useMutation({
    mutationFn: async (id: string) => {
      const { repo, file, revision } = await revisionOf(queryClient, id);
      await imageDownload(repo, file, revision);
    },
    onSettled: refresh,
  });
  const cancel = useMutation({
    mutationFn: imageCancelDownload,
    onSettled: refresh,
  });
  const dismiss = useMutation({
    mutationFn: imageDismissDownload,
    onSettled: refresh,
  });
  const remove = useMutation({
    mutationFn: imageDeleteModel,
    onSettled: refresh,
  });
  const use = useMutation({
    mutationFn: imageUseModel,
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: imageModelsKey }),
  });
  return { download, retry, cancel, dismiss, remove, use };
}
