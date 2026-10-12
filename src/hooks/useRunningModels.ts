// Modèles en mémoire (/api/ps) pour l'onglet Models des réglages (propre à
// ChatonLM) : relus toutes les 2 s tant que l'onglet est affiché, et après
// chaque chargement ou déchargement (invalidation de runningModelsKey).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { runningModels } from "@/lib/ipc";

export const runningModelsKey = ["runningModels"] as const;

/** Intervalle des relectures pendant que l'onglet est affiché. */
export const RUNNING_INTERVAL_MS = 2000;

export function useRunningModels() {
  const query = useQuery({
    queryKey: runningModelsKey,
    queryFn: runningModels,
    refetchInterval: RUNNING_INTERVAL_MS,
    // Onglet quitté : rien de gardé, la liste d'un autre moment (ou d'un
    // autre serveur) n'est jamais remontrée en revenant.
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
