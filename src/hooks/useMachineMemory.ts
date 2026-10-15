// Mémoire de la machine du serveur pour la jauge de VRAM de l'onglet Models
// des réglages (propre à ChatonLM) : relue au rythme des modèles en mémoire
// tant que l'onglet est affiché, et après chaque chargement ou déchargement
// (invalidation de machineMemoryKey).

import { useQuery } from "@tanstack/react-query";
import { RUNNING_INTERVAL_MS } from "@/hooks/useRunningModels";
import { machineMemory } from "@/lib/ipc";

export const machineMemoryKey = ["machineMemory"] as const;

export function useMachineMemory() {
  return useQuery({
    queryKey: machineMemoryKey,
    queryFn: machineMemory,
    refetchInterval: RUNNING_INTERVAL_MS,
    // Onglet quitté : rien de gardé (autre serveur, autre moment).
    gcTime: 0,
    retry: false,
  });
}
