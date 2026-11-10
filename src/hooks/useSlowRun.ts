// Lenteur du modèle choisi dans une conversation (escargot de la zone de
// saisie, propre à ChatonLM) : modèles du moteur relus toutes les 5 s tant
// que la zone est affichée, mis à jour à chaque changement annoncé par le
// moteur (événement `engine-models`, useEngineModelEvents) et après chaque
// chargement ou déchargement (invalidation de runningModelsKey, qui couvre
// cette clé).

import { useQuery } from "@tanstack/react-query";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { runningModels } from "@/lib/ipc";
import { type SlowRun, slowRun } from "@/lib/runningModels";

export const SLOW_RUN_INTERVAL_MS = 5000;

/** Lenteur de `model` ; null : rien à signaler. */
export function useSlowRun(model: string | null): SlowRun | null {
  const query = useQuery({
    queryKey: [...runningModelsKey, "chat"],
    queryFn: runningModels,
    refetchInterval: SLOW_RUN_INTERVAL_MS,
    enabled: model !== null,
    retry: false,
  });
  if (model === null || !query.data) return null;
  return slowRun(query.data, model);
}
