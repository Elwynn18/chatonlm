// Requêtes du ModelHub (propre à ChatonLM). Le moteur garde les réponses de
// Hugging Face en cache disque : relire une liste ne coûte qu'un appel local.
// Les estimations de mémoire (pastilles) dépendent du serveur (local ou non)
// et de la longueur de contexte réglée : elles font partie des clés, pour que
// tout soit relu quand l'une change.

import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useCallback } from "react";
import type { HubFilters } from "@/bindings/HubFilters";
import type { HubScope } from "@/bindings/HubScope";
import { modelsKey } from "@/hooks/useModels";
import { useSettings } from "@/hooks/useSettings";
import {
  hubBrowse,
  hubDeleteModel,
  hubHardware,
  hubHome,
  hubInstalled,
  hubReadme,
  hubRepo,
  hubSearch,
} from "@/lib/ipc";

/** Préfixe de toutes les requêtes du ModelHub. */
export const hubKey = ["hub"] as const;
/** Préfixe des modèles installés (relus à la fin d'un téléchargement). */
export const hubInstalledKey = ["hub", "installed"] as const;

/** Les listes restent fraîches une minute (le moteur a son propre cache). */
const LIST_STALE_MS = 60 * 1000;

/**
 * Ce dont dépendent les estimations : adresse effective du serveur et
 * longueur de contexte réglée ; null tant que les réglages ne sont pas lus.
 */
function useMachine(): { server: string; context: number } | null {
  const { settings } = useSettings();
  if (!settings) return null;
  return {
    server: settings.effectiveServerUrl,
    context: settings.contextLength,
  };
}

/** Mémoire de la machine du serveur ; null : serveur distant. */
export function useHubHardware() {
  const machine = useMachine();
  return useQuery({
    queryKey: ["hub", "hardware", machine],
    queryFn: hubHardware,
    enabled: machine !== null,
    staleTime: LIST_STALE_MS,
  });
}

/** Accueil filtré ; les données précédentes restent affichées pendant un changement de filtre. */
export function useHubHome(filters: HubFilters, enabled = true) {
  const machine = useMachine();
  return useQuery({
    queryKey: ["hub", "home", machine, filters],
    queryFn: () => hubHome(filters),
    enabled: enabled && machine !== null,
    staleTime: LIST_STALE_MS,
    placeholderData: keepPreviousData,
  });
}

/** Liste complète d'une rubrique ou d'une famille. */
export function useHubBrowse(scope: HubScope | null, filters: HubFilters) {
  const machine = useMachine();
  return useQuery({
    queryKey: ["hub", "browse", machine, scope, filters],
    queryFn: () => hubBrowse(scope!, filters),
    enabled: scope !== null && machine !== null,
    staleTime: LIST_STALE_MS,
    placeholderData: keepPreviousData,
  });
}

/** Recherche sur Hugging Face, page par page (curseur opaque du moteur). */
export function useHubSearch(text: string, filters: HubFilters) {
  const machine = useMachine();
  return useInfiniteQuery({
    queryKey: ["hub", "search", machine, text, filters],
    queryFn: ({ pageParam }) => hubSearch({ text, filters, cursor: pageParam }),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    enabled: text !== "" && machine !== null,
    staleTime: LIST_STALE_MS,
    placeholderData: keepPreviousData,
  });
}

/** Fiche d'un dépôt. */
export function useHubRepo(id: string) {
  const machine = useMachine();
  return useQuery({
    queryKey: ["hub", "repo", machine, id],
    queryFn: () => hubRepo(id),
    enabled: machine !== null,
    staleTime: LIST_STALE_MS,
  });
}

/** Préfixe des README d'un dépôt (toutes révisions). */
export function hubReadmeKey(id: string) {
  return ["hub", "readme", id] as const;
}

/**
 * README d'un dépôt à une révision ; attendu tant que `revision` est null.
 * Lu, il ne change plus à cette révision ; en échec (hors ligne, limite de
 * requêtes), il n'est pas gardé : relu à la prochaine ouverture de la fiche.
 */
export function useHubReadme(id: string, revision: string | null) {
  return useQuery({
    queryKey: [...hubReadmeKey(id), revision],
    queryFn: () => hubReadme(id, revision!),
    enabled: revision !== null,
    staleTime: (query) => (query.state.data?.problem ? 0 : Infinity),
  });
}

/** Modèles installés sur le serveur (onglet Installed, badges). */
export function useHubInstalled() {
  const machine = useMachine();
  return useQuery({
    queryKey: [...hubInstalledKey, machine?.server ?? null],
    queryFn: hubInstalled,
    enabled: machine !== null,
  });
}

/** Suppression d'un modèle ; installés et sélecteur relus ensuite. */
export function useDeleteModel() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: hubDeleteModel,
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: hubInstalledKey }),
        queryClient.invalidateQueries({ queryKey: modelsKey }),
      ]),
  });
}

/**
 * « Use in chat » : le modèle devient le modèle choisi, puis une nouvelle
 * conversation s'ouvre. Rejette si le réglage n'a pas pu être enregistré.
 */
export function useUseInChat() {
  const { setSettings } = useSettings();
  const navigate = useNavigate();
  return useCallback(
    async (model: string) => {
      await setSettings({ selectedModel: model });
      await navigate({ to: "/c/$chatId", params: { chatId: "new" } });
    },
    [setSettings, navigate],
  );
}
