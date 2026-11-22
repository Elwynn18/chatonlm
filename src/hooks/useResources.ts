// Vue des ressources de l'onglet Models des réglages (propre à ChatonLM) :
// où calculent le modèle de transcription (Whisper) et le moteur d'images,
// et la VRAM qu'ils prennent ; relue au rythme des modèles du moteur tant
// que l'onglet est affiché.

import { useQuery } from "@tanstack/react-query";
import { RUNNING_INTERVAL_MS } from "@/hooks/useRunningModels";
import { resourcesView } from "@/lib/ipc";

export const resourcesKey = ["resources"] as const;

export function useResources() {
  return useQuery({
    queryKey: resourcesKey,
    queryFn: resourcesView,
    refetchInterval: RUNNING_INTERVAL_MS,
    // Onglet quitté : rien de gardé (autre moment).
    gcTime: 0,
    retry: false,
  });
}
