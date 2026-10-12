// Bouton Think du modèle choisi : détails du modèle (/api/show) et réglage
// mémorisé pour lui, combinés par src/lib/think.ts. Code propre à ChatonLM
// (l'officiel garde un réglage global et ne lit pas thinking.values).

import {
  hashKey,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback, useEffect, useMemo } from "react";
import type { Think } from "@/bindings/Think";
import { modelsKey } from "@/hooks/useModels";
import { useT } from "@/i18n";
import {
  getThinkPref,
  isAppError,
  modelDetails,
  setThinkPref,
} from "@/lib/ipc";
import { NO_THINK_CONTROL, thinkControl } from "@/lib/think";

export const modelDetailsRoot = ["modelDetails"] as const;
export const modelDetailsKey = (model: string) =>
  [...modelDetailsRoot, model] as const;
export const thinkPrefKey = (model: string) => ["thinkPref", model] as const;

/**
 * Contrôle du raisonnement pour `model` (null : aucun modèle choisi) et son
 * changement, mémorisé pour ce modèle. Tant que les détails ou le réglage ne
 * sont pas connus, si les détails n'ont jamais pu être lus (modèle absent,
 * Ollama injoignable) ou si le modèle a disparu, aucun bouton et `think` non
 * envoyé. Une relecture en échec passager garde les derniers détails lus.
 */
export function useThink(model: string | null) {
  const queryClient = useQueryClient();
  const name = model ?? "";
  const enabled = name !== "";

  // Relus quand la liste des modèles l'est (voir plus bas) : un modèle
  // retéléchargé ou un autre serveur peut changer ses capacités.
  const details = useQuery({
    queryKey: modelDetailsKey(name),
    queryFn: () => modelDetails(name),
    enabled,
    staleTime: Infinity,
  });
  // Ne change que par setThink, qui met le cache à jour.
  const pref = useQuery({
    queryKey: thinkPrefKey(name),
    queryFn: () => getThinkPref(name),
    enabled,
    staleTime: Infinity,
  });

  // Une relecture de la liste des modèles (toutes les 30 s, au retour sur
  // la fenêtre...) qui la change (modèle retéléchargé : autre empreinte,
  // supprimé, autre serveur) fait relire les détails ; une liste identique
  // garde sa référence (partage structurel de react-query) et ne relit
  // rien. Écoute du cache plutôt qu'un useModels de plus : pas de minuterie
  // en double.
  useEffect(() => {
    const models = hashKey(modelsKey);
    let seen = queryClient.getQueryData(modelsKey);
    return queryClient.getQueryCache().subscribe((event) => {
      if (
        event.type === "updated" &&
        event.action.type === "success" &&
        event.query.queryHash === models &&
        event.query.state.data !== seen
      ) {
        seen = event.query.state.data;
        void queryClient.invalidateQueries(
          { queryKey: modelDetailsRoot },
          { cancelRefetch: false },
        );
      }
    });
  }, [queryClient]);

  const { mutate } = useMutation({
    mutationFn: (variables: { model: string; think: Think }) =>
      setThinkPref(variables.model, variables.think),
    // Écritures dans l'ordre des clics : la dernière choisie reste en base.
    scope: { id: "thinkPref" },
    // Le choix reste valable pour la session ; seul son souvenir est perdu.
    onError: (error) => console.error("Failed to save the think mode:", error),
  });

  // Une relecture en échec garde ses données (status « error ») : un délai
  // dépassé ne doit ni cacher le bouton ni oublier un Off mémorisé. Seul un
  // modèle supprimé (notFound) les rend caduques.
  const gone = isAppError(details.error) && details.error.kind === "notFound";
  const known = gone ? undefined : details.data;
  // Libellés du menu dans la langue affichée.
  const t = useT();
  const control = useMemo(() => {
    if (!enabled || known === undefined || pref.isPending) {
      return NO_THINK_CONTROL;
    }
    // Réglage illisible : défaut du modèle.
    return thinkControl(t, known, pref.isSuccess ? pref.data : null);
  }, [t, enabled, known, pref.isPending, pref.isSuccess, pref.data]);

  const setThink = useCallback(
    (think: Think) => {
      if (!enabled) return;
      queryClient.setQueryData(thinkPrefKey(name), think);
      mutate({ model: name, think });
    },
    [enabled, name, queryClient, mutate],
  );

  return { control, setThink };
}
