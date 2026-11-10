// Modèles du moteur pour l'onglet Models des réglages (propre à ChatonLM) :
// relus toutes les 2 s tant que l'onglet est affiché (échéances), mis à jour
// à chaque changement annoncé par le moteur (événement `engine-models`,
// useEngineModelEvents) et après chaque chargement ou déchargement
// (invalidation de runningModelsKey).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { listenEngineModels, runningModels } from "@/lib/ipc";

export const runningModelsKey = ["runningModels"] as const;

/** Intervalle des relectures pendant que l'onglet est affiché. */
export const RUNNING_INTERVAL_MS = 2000;

export function useRunningModels() {
  const query = useQuery({
    queryKey: runningModelsKey,
    queryFn: runningModels,
    refetchInterval: RUNNING_INTERVAL_MS,
    // Onglet quitté : rien de gardé, la liste d'un autre moment n'est jamais
    // remontrée en revenant.
    gcTime: 0,
    retry: false,
  });
  // Dernier échec, gardé jusqu'à la prochaine lecture réussie : sans liste
  // lue, React Query efface l'erreur au départ de chaque relecture, et
  // l'erreur (avec Try again) disparaîtrait toutes les 2 s.
  const [failure, setFailure] = useState<unknown>(null);
  if (query.isError && query.error !== failure) setFailure(query.error);
  if (query.isSuccess && failure !== null) setFailure(null);
  return {
    data: query.data,
    /** Heure de la dernière lecture réussie (échéances relatives). */
    dataUpdatedAt: query.dataUpdatedAt,
    /** Échec de la dernière lecture terminée, ou null. */
    failure: query.isError ? query.error : failure,
    refetch: query.refetch,
  };
}

/**
 * Liste du moteur suivie de toute page (route racine) : chaque changement
 * (chargement commencé ou fini, déchargement, repli sur le processeur)
 * remplace aussitôt les listes lues (onglet Models, escargot), sans
 * attendre la relecture suivante.
 */
export function useEngineModelEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenEngineModels((models) =>
      queryClient.setQueriesData({ queryKey: runningModelsKey }, models),
    ).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error: unknown) =>
        console.error("Failed to listen for the engine models:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [queryClient]);
}
