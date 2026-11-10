import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { setLanguage } from "@/i18n";
import { getSettings, updateSettings } from "@/lib/ipc";

export const settingsKey = ["settings"] as const;

/**
 * Portée commune des mises à jour : elles partent en série, dans l'ordre où
 * elles ont été lancées (comme `settingsMutationScope` de l'officiel).
 */
export const settingsMutationScope = { id: "settings-update" } as const;

/**
 * Requêtes relues quand le réglage des modèles distants change (ils entrent
 * dans la liste du sélecteur ou en sortent). Littéraux plutôt que les
 * constantes de useModels.ts et useThink.ts, qui importent ce fichier
 * (import circulaire) : `["models"]` (modelsKey), `["modelDetails"]`
 * (modelDetailsRoot).
 */
export const remoteDependentKeys = [["models"], ["modelDetails"]] as const;

/**
 * Réglages de l'app. Ils ne changent que par `setSettings`, qui met le cache à
 * jour avec la réponse du backend (valeurs normalisées).
 *
 * `refresh` : relit les réglages au montage de cet observateur (page
 * Settings), même s'ils sont déjà en cache. `error` : erreur de la dernière
 * lecture, même si des réglages lus avant restent en cache (`settings`).
 */
export function useSettings({ refresh = false }: { refresh?: boolean } = {}) {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: settingsKey,
    queryFn: readSettings,
    staleTime: Infinity,
    refetchOnMount: refresh ? "always" : true,
  });
  const mutation = useMutation({
    mutationFn: (patch: SettingsPatch) => updateSettings(patch),
    scope: settingsMutationScope,
    // Une relecture en cours rendrait les réglages d'avant cette mise à jour.
    onMutate: () => queryClient.cancelQueries({ queryKey: settingsKey }),
    onSuccess: (settings) => applySettings(queryClient, settings),
  });
  return {
    settings: query.data,
    error: query.error,
    setSettings: mutation.mutateAsync,
  };
}

/** Lit les réglages ; la langue affichée suit celle qu'ils donnent. */
async function readSettings(): Promise<Settings> {
  const settings = await getSettings();
  setLanguage(settings.effectiveLanguage);
  return settings;
}

/**
 * Remplace le cache par la réponse (la langue affichée suit la sienne) ; si
 * le réglage des modèles distants a changé (ou n'était pas connu), relit la
 * liste des modèles, sans attendre (la mise à jour suivante n'attend pas
 * ces relectures).
 */
function applySettings(queryClient: QueryClient, settings: Settings) {
  const previous = queryClient.getQueryData<Settings>(settingsKey);
  queryClient.setQueryData(settingsKey, settings);
  setLanguage(settings.effectiveLanguage);
  if (previous?.remoteModels === settings.remoteModels) return;
  for (const queryKey of remoteDependentKeys) {
    void queryClient.invalidateQueries({ queryKey });
  }
}
