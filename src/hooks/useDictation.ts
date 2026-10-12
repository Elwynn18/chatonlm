// Dictée (étape 17, propre à ChatonLM) : réglages, valeurs fixes, statut,
// modèles de transcription et leurs téléchargements, dans le cache de React
// Query. Le statut (événement « dictation-status ») et les téléchargements
// (« speech-downloads ») sont émis par le moteur à chaque changement ; un
// seul écouteur, monté avec l'app (useDictationEvents, route racine), les
// verse dans le cache : la phase et la progression sont vues de toute page.

import {
  type QueryClient,
  type QueryKey,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { DictationSettings } from "@/bindings/DictationSettings";
import type { DictationStatus } from "@/bindings/DictationStatus";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import {
  dictationBindShortcut,
  dictationCancel,
  dictationConfigureShortcut,
  dictationCopyLast,
  dictationInfo,
  dictationStatus,
  dictationToggle,
  getDictationSettings,
  listenDictationStatus,
  listenSpeechDownloads,
  speechCancelDownload,
  speechDeleteModel,
  speechDismissDownload,
  speechDownload,
  speechDownloads,
  speechModels,
  updateDictationSettings,
} from "@/lib/ipc";

export const dictationSettingsKey = ["dictation", "settings"] as const;
export const dictationInfoKey = ["dictation", "info"] as const;
export const dictationStatusKey = ["dictation", "status"] as const;
export const speechModelsKey = ["speech", "models"] as const;
export const speechDownloadsKey = ["speech", "downloads"] as const;

/** Mises à jour des réglages : en série, dans l'ordre où elles sont lancées. */
const settingsMutationKey = ["dictation", "update"] as const;
export const dictationMutationScope = { id: "dictation-update" } as const;

const NO_DOWNLOADS: SpeechDownload[] = [];

/**
 * Événements reçus, par client et par clé : une lecture pendant laquelle un
 * événement arrive renvoie un état plus ancien que lui (réponse et
 * événements ne sont pas ordonnés entre eux) ; elle garde alors celui de
 * l'événement (comme useDownloads).
 */
const received = new WeakMap<QueryClient, Map<string, number>>();

function eventCount(queryClient: QueryClient, key: QueryKey): number {
  return received.get(queryClient)?.get(JSON.stringify(key)) ?? 0;
}

function countEvent(queryClient: QueryClient, key: QueryKey) {
  let counts = received.get(queryClient);
  if (!counts) {
    counts = new Map();
    received.set(queryClient, counts);
  }
  const name = JSON.stringify(key);
  counts.set(name, (counts.get(name) ?? 0) + 1);
}

/** Lecture qui cède la place à un événement arrivé pendant elle. */
async function readUnlessEvent<T>(
  queryClient: QueryClient,
  key: QueryKey,
  read: () => Promise<T>,
): Promise<T> {
  const before = eventCount(queryClient, key);
  const value = await read();
  if (eventCount(queryClient, key) === before) return value;
  return queryClient.getQueryData<T>(key) ?? value;
}

/** Applique un statut reçu du moteur. */
export function applyDictationStatus(
  queryClient: QueryClient,
  status: DictationStatus,
) {
  countEvent(queryClient, dictationStatusKey);
  queryClient.setQueryData(dictationStatusKey, status);
}

/** Un téléchargement a-t-il fini depuis `previous` ? */
function finishedSince(
  previous: readonly SpeechDownload[],
  downloads: readonly SpeechDownload[],
): boolean {
  return downloads.some(
    (download) =>
      download.state === "done" &&
      !previous.some(
        (before) => before.id === download.id && before.state === "done",
      ),
  );
}

/**
 * Relit les modèles de transcription si un téléchargement a fini entre la
 * liste en cache et `downloads`.
 */
function refreshModelsIfFinished(
  queryClient: QueryClient,
  previous: readonly SpeechDownload[] | undefined,
  downloads: readonly SpeechDownload[],
) {
  if (finishedSince(previous ?? NO_DOWNLOADS, downloads)) {
    void queryClient.invalidateQueries({ queryKey: speechModelsKey });
  }
}

/**
 * Applique une liste de téléchargements reçue du moteur ; relit les modèles
 * de transcription si l'un d'eux vient de finir.
 */
export function applySpeechDownloads(
  queryClient: QueryClient,
  downloads: SpeechDownload[],
) {
  countEvent(queryClient, speechDownloadsKey);
  const previous =
    queryClient.getQueryData<SpeechDownload[]>(speechDownloadsKey);
  queryClient.setQueryData(speechDownloadsKey, downloads);
  refreshModelsIfFinished(queryClient, previous, downloads);
}

/**
 * Écoute « dictation-status » et « speech-downloads » tant que le composant
 * est monté : une seule fois, au niveau de l'app (route racine).
 */
export function useDictationEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    const keep = (stop: () => void) => {
      if (active) stops.push(stop);
      else stop();
    };
    listenDictationStatus((status) =>
      applyDictationStatus(queryClient, status),
    ).then(keep, (error: unknown) =>
      console.error("Failed to listen for the dictation status:", error),
    );
    listenSpeechDownloads((downloads) =>
      applySpeechDownloads(queryClient, downloads),
    ).then(keep, (error: unknown) =>
      console.error("Failed to listen for speech model downloads:", error),
    );
    return () => {
      active = false;
      for (const stop of stops) stop();
    };
  }, [queryClient]);
}

/**
 * Réglages de la dictée. `update` montre aussitôt le changement (mise à jour
 * optimiste), puis la réponse du moteur (valeurs nettoyées) quand plus
 * aucune autre mise à jour n'attend ; un échec relit les réglages et rejette.
 * Les mises à jour partent en série. `refresh` : relus au montage (page
 * Settings), même s'ils sont en cache.
 */
export function useDictationSettings({
  refresh = false,
}: { refresh?: boolean } = {}) {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: dictationSettingsKey,
    queryFn: getDictationSettings,
    staleTime: Infinity,
    refetchOnMount: refresh ? "always" : true,
  });
  const mutation = useMutation({
    mutationKey: settingsMutationKey,
    mutationFn: (patch: DictationPatch) => updateDictationSettings(patch),
    scope: dictationMutationScope,
    onMutate: async (patch) => {
      // Une relecture en cours rendrait les réglages d'avant ce changement.
      await queryClient.cancelQueries({ queryKey: dictationSettingsKey });
      queryClient.setQueryData<DictationSettings>(
        dictationSettingsKey,
        (old) => (old ? { ...old, ...patch } : old),
      );
    },
    onSuccess: (settings) => {
      // Une autre mise à jour attend encore : la réponse ne la contient pas
      // (le changement montré reculerait un instant) ; la dernière écrit.
      if (queryClient.isMutating({ mutationKey: settingsMutationKey }) <= 1) {
        queryClient.setQueryData(dictationSettingsKey, settings);
      }
      // Raccourci lié ou délié par le moteur : statut relu (l'événement
      // l'apporte aussi).
      void queryClient.invalidateQueries({ queryKey: dictationStatusKey });
    },
    onError: () =>
      queryClient.invalidateQueries({ queryKey: dictationSettingsKey }),
  });
  return {
    settings: query.data,
    error: query.error,
    /** Nouvelle lecture (après un échec). */
    reload: query.refetch,
    update: mutation.mutateAsync,
  };
}

/** Consignes par défaut du nettoyage et langues : fixes, lues une fois. */
export function useDictationInfo() {
  return useQuery({
    queryKey: dictationInfoKey,
    queryFn: dictationInfo,
    staleTime: Infinity,
  });
}

/** Phase, raccourci et dernier résultat, tenus à jour par les événements. */
export function useDictationStatus() {
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: dictationStatusKey,
    queryFn: () =>
      readUnlessEvent(queryClient, dictationStatusKey, dictationStatus),
  });
}

/**
 * Essai depuis les réglages, abandon, raccourci : le statut est relu après
 * chaque commande (le moteur émet aussi l'événement).
 */
export function useDictationActions() {
  const queryClient = useQueryClient();
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: dictationStatusKey });
  const toggle = useMutation({
    mutationFn: dictationToggle,
    onSettled: refresh,
  });
  const cancel = useMutation({
    mutationFn: dictationCancel,
    onSettled: refresh,
  });
  const bindShortcut = useMutation({
    mutationFn: dictationBindShortcut,
    onSettled: refresh,
  });
  const configureShortcut = useMutation({
    mutationFn: dictationConfigureShortcut,
    onSettled: refresh,
  });
  return { toggle, cancel, bindShortcut, configureShortcut };
}

/**
 * Copie le texte de la dernière dictée (copie automatique coupée, ou
 * presse-papiers écrasé depuis) ; le statut est relu ensuite (`copied`).
 */
export function useCopyLastDictation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => dictationCopyLast(),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: dictationStatusKey }),
  });
}

/** Catalogue des modèles de transcription, avec leur présence sur le disque. */
export function useSpeechModels() {
  return useQuery({ queryKey: speechModelsKey, queryFn: speechModels });
}

/**
 * Téléchargements des modèles de transcription (vides tant qu'ils ne sont
 * pas lus : `loaded` ; une lecture en échec compte comme lue, liste vide).
 */
export function useSpeechDownloads(): {
  downloads: SpeechDownload[];
  loaded: boolean;
} {
  const queryClient = useQueryClient();
  const { data, isSuccess, isError } = useQuery({
    queryKey: speechDownloadsKey,
    queryFn: async () => {
      const previous =
        queryClient.getQueryData<SpeechDownload[]>(speechDownloadsKey);
      const downloads = await readUnlessEvent(
        queryClient,
        speechDownloadsKey,
        speechDownloads,
      );
      // Fin vue par cette lecture (événement manqué, page rouverte) : les
      // modèles aussi sont relus.
      refreshModelsIfFinished(queryClient, previous, downloads);
      return downloads;
    },
  });
  return { downloads: data ?? NO_DOWNLOADS, loaded: isSuccess || isError };
}

/**
 * Télécharger, arrêter, retirer, supprimer : la liste est relue après chaque
 * commande ; la suppression relit les modèles.
 */
export function useSpeechActions() {
  const queryClient = useQueryClient();
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: speechDownloadsKey });
  const download = useMutation({
    mutationFn: speechDownload,
    onSettled: refresh,
  });
  const cancel = useMutation({
    mutationFn: speechCancelDownload,
    onSettled: refresh,
  });
  const dismiss = useMutation({
    mutationFn: speechDismissDownload,
    onSettled: refresh,
  });
  const remove = useMutation({
    mutationFn: speechDeleteModel,
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: speechModelsKey }),
  });
  return { download, cancel, dismiss, remove };
}
