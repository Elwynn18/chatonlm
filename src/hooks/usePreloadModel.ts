// Préchargement du modèle choisi (réglage « Preload the chosen model »,
// propre à ChatonLM) : le modèle que le prochain message utiliserait (celui
// du sélecteur : modèle choisi, ou celui de la conversation ouverte, que le
// sélecteur reprend dans les réglages) est chargé en mémoire dès qu'il est
// choisi et au démarrage, pour que le premier message n'attende pas.
// Seulement Ollama joignable et le modèle installé (pas dans le nuage),
// une fois le choix stable depuis une seconde (crans du contexte parcourus
// au clavier, modèles essayés dans le sélecteur : un seul chargement) ; une
// fois par adresse, modèle et contexte pendant la session (pas la durée
// avant déchargement : la changer ne recharge pas un modèle déchargé
// exprès) ; un échec est ignoré (l'envoi chargera le modèle et dira
// l'erreur).

import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type { Settings } from "@/bindings/Settings";
import { useModels } from "@/hooks/useModels";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { useServerHealth } from "@/hooks/useServerHealth";
import { isCloudModel } from "@/lib/dictation";
import { loadModel } from "@/lib/ipc";
import { selectedEntry } from "@/lib/models";

/** Préchargements demandés pendant la session : `adresse|modèle|contexte`. */
const preloaded = new Set<string>();

/** Choix stable depuis ce délai avant de charger. */
export const PRELOAD_DELAY_MS = 1000;

/**
 * Oublie les préchargements de la session : le modèle courant est de nouveau
 * préchargé (option activée de nouveau ; tests).
 */
export function forgetPreloads() {
  preloaded.clear();
}

/** Précharge le modèle choisi ; à n'utiliser que l'option activée. */
export function usePreloadModel(settings: Settings) {
  const queryClient = useQueryClient();
  const models = useModels();
  const { healthy } = useServerHealth();
  const entry = models.isSuccess
    ? selectedEntry(models.data, settings.selectedModel)
    : null;
  // Installé (orthographe de /api/tags) ; sinon rien à charger.
  const model =
    entry?.info && healthy && !isCloudModel(entry.model) ? entry.model : null;
  const key =
    model === null
      ? null
      : [settings.effectiveServerUrl, model, settings.contextLength].join("|");

  useEffect(() => {
    if (key === null || model === null || preloaded.has(key)) return;
    // Clé changée avant la fin du délai : minuteur annulé, rien de marqué.
    const timer = window.setTimeout(() => {
      if (preloaded.has(key)) return;
      preloaded.add(key);
      loadModel(model).then(
        // Onglet Models affiché : le modèle y apparaît aussitôt.
        () => queryClient.invalidateQueries({ queryKey: runningModelsKey }),
        () => {},
      );
    }, PRELOAD_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [key, model, queryClient]);
}
