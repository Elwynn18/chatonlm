// Lecture à voix haute (propre à ChatonLM) : état de la lecture, voix et
// leurs téléchargements, dans le cache de React Query. L'état (événement
// « read-aloud », une seule lecture dans l'app) et les téléchargements des
// voix (« voice-downloads ») sont émis par le moteur à chaque changement ;
// un seul écouteur, monté avec l'app (useReadAloudEvents, route racine), les
// verse dans le cache : le bouton de chaque réponse, les réglages et le
// ModelHub les voient. Il arrête aussi la lecture quand l'utilisateur ouvre
// une autre conversation.

import {
  type QueryClient,
  type QueryKey,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { useEffect, useRef } from "react";
import type { ReadAloudState } from "@/bindings/ReadAloudState";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import {
  listenReadAloud,
  listenVoiceDownloads,
  readAloud,
  readAloudState,
  readAloudStop,
  voiceCancelDownload,
  voiceDeleteModel,
  voiceDismissDownload,
  voiceDownload,
  voiceDownloads,
  voiceModels,
} from "@/lib/ipc";
import { chatOfPath, IDLE, leavesChat } from "@/lib/readAloud";

export const readAloudKey = ["readAloud", "state"] as const;
export const voiceModelsKey = ["voice", "models"] as const;
export const voiceDownloadsKey = ["voice", "downloads"] as const;

const NO_DOWNLOADS: SpeechDownload[] = [];

/**
 * Événements reçus, par client et par clé : une lecture pendant laquelle un
 * événement arrive renvoie un état plus ancien que lui ; elle garde alors
 * celui de l'événement (comme useDictation).
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

/** Applique un état de la lecture reçu du moteur. */
export function applyReadAloudState(
  queryClient: QueryClient,
  state: ReadAloudState,
) {
  countEvent(queryClient, readAloudKey);
  queryClient.setQueryData(readAloudKey, state);
}

/** Une voix a-t-elle fini de se télécharger depuis `previous` ? */
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

function refreshVoicesIfFinished(
  queryClient: QueryClient,
  previous: readonly SpeechDownload[] | undefined,
  downloads: readonly SpeechDownload[],
) {
  if (finishedSince(previous ?? NO_DOWNLOADS, downloads)) {
    void queryClient.invalidateQueries({ queryKey: voiceModelsKey });
  }
}

/**
 * Applique une liste de téléchargements des voix reçue du moteur ; relit les
 * voix si l'une d'elles vient de finir.
 */
export function applyVoiceDownloads(
  queryClient: QueryClient,
  downloads: SpeechDownload[],
) {
  countEvent(queryClient, voiceDownloadsKey);
  const previous =
    queryClient.getQueryData<SpeechDownload[]>(voiceDownloadsKey);
  queryClient.setQueryData(voiceDownloadsKey, downloads);
  refreshVoicesIfFinished(queryClient, previous, downloads);
}

/**
 * Écoute « read-aloud » et « voice-downloads » tant que le composant est
 * monté : une seule fois, au niveau de l'app (route racine). Arrête la
 * lecture quand une autre conversation s'ouvre (pas en passant par les
 * réglages ou le ModelHub pour revenir à la même, ni quand une nouvelle
 * conversation reçoit son identifiant), quel que soit l'état connu : la
 * lecture automatique d'une réponse reste au repos tant que sa langue n'est
 * pas connue, et l'état en cache peut avoir été oublié (l'arrêt est sans
 * effet au repos pour le moteur).
 */
export function useReadAloudEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    const keep = (stop: () => void) => {
      if (active) stops.push(stop);
      else stop();
    };
    listenReadAloud((state) => applyReadAloudState(queryClient, state)).then(
      keep,
      (error: unknown) =>
        console.error("Failed to listen for the reading state:", error),
    );
    listenVoiceDownloads((downloads) =>
      applyVoiceDownloads(queryClient, downloads),
    ).then(keep, (error: unknown) =>
      console.error("Failed to listen for voice downloads:", error),
    );
    return () => {
      active = false;
      for (const stop of stops) stop();
    };
  }, [queryClient]);

  const chat = useRouterState({
    select: (state) => chatOfPath(state.location.pathname),
  });
  // Dernière conversation affichée (les autres pages ne la changent pas).
  const lastChat = useRef<string | null>(null);
  useEffect(() => {
    if (chat === null) return;
    const previous = lastChat.current;
    lastChat.current = chat;
    if (!leavesChat(previous, chat)) return;
    readAloudStop().catch((error: unknown) =>
      console.error("Failed to stop reading aloud:", error),
    );
  }, [chat]);
}

/** État de la lecture : lu une fois, puis tenu à jour par l'événement. */
export function useReadAloud(): ReadAloudState {
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: readAloudKey,
    queryFn: async (): Promise<ReadAloudState> => {
      const before = eventCount(queryClient, readAloudKey);
      const state = await readAloudState();
      const cached = queryClient.getQueryData<ReadAloudState>(readAloudKey);
      // Un événement arrivé pendant la lecture est plus récent qu'elle.
      if (eventCount(queryClient, readAloudKey) !== before && cached) {
        return cached;
      }
      return state;
    },
    staleTime: Infinity,
  });
  return data ?? IDLE;
}

/** Lire une réponse (accord de téléchargement compris), arrêter la lecture. */
export function useReadAloudActions() {
  const read = useMutation({
    mutationFn: ({
      chatId,
      messageId,
      download,
    }: {
      chatId: string;
      messageId: number;
      download: boolean;
    }) => readAloud(chatId, messageId, download),
  });
  const stop = useMutation({ mutationFn: readAloudStop });
  return { read, stop };
}

/** Catalogue des voix, avec leur présence sur le disque. */
export function useVoiceModels() {
  return useQuery({ queryKey: voiceModelsKey, queryFn: voiceModels });
}

/**
 * Téléchargements des voix (vides tant qu'ils ne sont pas lus : `loaded` ;
 * une lecture en échec compte comme lue, liste vide).
 */
export function useVoiceDownloads(): {
  downloads: SpeechDownload[];
  loaded: boolean;
} {
  const queryClient = useQueryClient();
  const { data, isSuccess, isError } = useQuery({
    queryKey: voiceDownloadsKey,
    queryFn: async () => {
      const previous =
        queryClient.getQueryData<SpeechDownload[]>(voiceDownloadsKey);
      const before = eventCount(queryClient, voiceDownloadsKey);
      let downloads = await voiceDownloads();
      if (eventCount(queryClient, voiceDownloadsKey) !== before) {
        downloads =
          queryClient.getQueryData<SpeechDownload[]>(voiceDownloadsKey) ??
          downloads;
      }
      // Fin vue par cette lecture (événement manqué, page rouverte) : les
      // voix aussi sont relues.
      refreshVoicesIfFinished(queryClient, previous, downloads);
      return downloads;
    },
  });
  return { downloads: data ?? NO_DOWNLOADS, loaded: isSuccess || isError };
}

/**
 * Télécharger, arrêter, retirer, supprimer une voix : la liste est relue
 * après chaque commande ; la suppression relit les voix.
 */
export function useVoiceActions() {
  const queryClient = useQueryClient();
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: voiceDownloadsKey });
  const download = useMutation({
    mutationFn: voiceDownload,
    onSettled: refresh,
  });
  const cancel = useMutation({
    mutationFn: voiceCancelDownload,
    onSettled: refresh,
  });
  const dismiss = useMutation({
    mutationFn: voiceDismissDownload,
    onSettled: refresh,
  });
  const remove = useMutation({
    mutationFn: voiceDeleteModel,
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: voiceModelsKey }),
  });
  return { download, cancel, dismiss, remove };
}
