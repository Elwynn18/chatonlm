// Part sur le processeur du modèle choisi dans une conversation (escargot de
// la zone de saisie, propre à ChatonLM) : modèles en mémoire (/api/ps) relus
// toutes les 5 s tant que la zone est affichée, et après chaque chargement ou
// déchargement (invalidation de runningModelsKey, qui couvre cette clé).

import { useQuery } from "@tanstack/react-query";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { runningModels } from "@/lib/ipc";
import { cpuShare } from "@/lib/runningModels";

export const CPU_SHARE_INTERVAL_MS = 5000;

/** Part (%) de `model` sur le processeur ; null : rien à signaler. */
export function useCpuShare(model: string | null): number | null {
  const query = useQuery({
    queryKey: [...runningModelsKey, "chat"],
    queryFn: runningModels,
    refetchInterval: CPU_SHARE_INTERVAL_MS,
    enabled: model !== null,
    retry: false,
  });
  if (model === null || !query.data) return null;
  return cpuShare(query.data, model);
}
