// Préchargement du modèle choisi, monté une fois pour toute l'app (route
// racine) : rien n'est lu tant que l'option est coupée (ni liste des
// modèles ni état d'Ollama en plus de ce que la page affichée lit déjà).

import type { Settings } from "@/bindings/Settings";
import { usePreloadModel } from "@/hooks/usePreloadModel";
import { useSettings } from "@/hooks/useSettings";

export function ModelPreloader() {
  const { settings } = useSettings();
  if (!settings?.preloadModel) return null;
  return <Preload settings={settings} />;
}

function Preload({ settings }: { settings: Settings }) {
  usePreloadModel(settings);
  return null;
}
