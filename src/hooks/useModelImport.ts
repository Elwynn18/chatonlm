// Reprise des modèles d'Ollama (propre à ChatonLM, carte « Models from
// Ollama » de Settings › Models) : état de la reprise, tenu à jour par
// l'événement `model-import`, suivi de toute page (route racine). Une
// reprise finie relit les dossiers, le ménage, les modèles installés, le
// ModelHub et les réglages : le moteur a réécrit le modèle choisi (et ceux
// de Proofread, de Study) sous son nouvel identifiant, y compris pour la
// reprise automatique du premier lancement, page ouverte ailleurs.

import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { ImportState } from "@/bindings/ImportState";
import { hubKey } from "@/hooks/useHub";
import { modelsKey } from "@/hooks/useModels";
import { settingsKey } from "@/hooks/useSettings";
import { listenModelImport, modelImportState } from "@/lib/ipc";
import { modelImportKey } from "@/lib/modelImport";

export const modelImportStateKey = [...modelImportKey, "state"] as const;
export const modelImportScansKey = [...modelImportKey, "scan"] as const;
export const modelImportLeftoversKey = [
  ...modelImportKey,
  "leftovers",
] as const;

/** Reprise finie : ce qu'elle a pu changer est relu. */
function refreshAfterImport(queryClient: QueryClient) {
  for (const queryKey of [
    modelImportScansKey,
    modelImportLeftoversKey,
    modelsKey,
    hubKey,
    settingsKey,
  ]) {
    void queryClient.invalidateQueries({ queryKey });
  }
}

/** Événements `model-import` reçus, par client (voir useImportState). */
const received = new WeakMap<QueryClient, number>();

/**
 * État de la reprise : lu une fois, puis tenu à jour par l'événement
 * (useModelImportEvents, route racine).
 */
export function useImportState(): ImportState | undefined {
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: modelImportStateKey,
    queryFn: async () => {
      // Réponse et événements ne sont pas ordonnés entre eux : un événement
      // arrivé pendant la lecture est plus récent qu'elle, il est gardé.
      const before = received.get(queryClient) ?? 0;
      const state = await modelImportState();
      if ((received.get(queryClient) ?? 0) === before) return state;
      return (
        queryClient.getQueryData<ImportState>(modelImportStateKey) ?? state
      );
    },
  });
  return data;
}

/**
 * Événement `model-import` suivi de toute page (état complet à chaque
 * changement) : l'état en cache est remplacé ; une reprise finie (plus rien
 * en cours, ou nouveau bilan) relit ce qu'elle a changé.
 */
export function useModelImportEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenModelImport((state) => {
      received.set(queryClient, (received.get(queryClient) ?? 0) + 1);
      const previous =
        queryClient.getQueryData<ImportState>(modelImportStateKey);
      queryClient.setQueryData(modelImportStateKey, state);
      const finished =
        (previous?.running && !state.running) ||
        (state.last !== null &&
          JSON.stringify(state.last) !== JSON.stringify(previous?.last));
      if (finished) refreshAfterImport(queryClient);
    }).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for the model import:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}
