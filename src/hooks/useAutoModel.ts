// Modèle que prend « Auto » en ce moment (pages Proofread et Study, rien de
// choisi : le meilleur modèle en mémoire, sinon celui du chat ; propre à
// ChatonLM). Relu toutes les 5 s tant qu'il sert : un modèle se charge ou se
// décharge sans que la page le sache.

import { useQuery } from "@tanstack/react-query";
import { autoModel } from "@/lib/ipc";

/** Intervalle des relectures. */
export const AUTO_MODEL_INTERVAL_MS = 5000;

/**
 * Modèle d'« Auto » ; `undefined` tant qu'il n'est pas lu (ou s'il est
 * illisible), `null` : aucun. `enabled` faux : rien n'est demandé.
 */
export function useAutoModel(
  chatModel: string,
  enabled: boolean,
): string | null | undefined {
  const query = useQuery({
    // Modèle du chat dans la clé : relu dès qu'il change.
    queryKey: ["autoModel", chatModel],
    queryFn: autoModel,
    enabled,
    refetchInterval: AUTO_MODEL_INTERVAL_MS,
    retry: false,
  });
  return query.data;
}
