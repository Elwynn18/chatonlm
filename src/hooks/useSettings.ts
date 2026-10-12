import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { accountKey } from "@/hooks/useAccount";
import { cloudStatusKey } from "@/hooks/useCloudStatus";
import { ollamaHomeKey } from "@/hooks/useOllamaHome";
import { setLanguage } from "@/i18n";
import { healthKey } from "@/hooks/useServerHealth";
import { getSettings, updateSettings } from "@/lib/ipc";

export const settingsKey = ["settings"] as const;

/**
 * Portée commune des mises à jour : elles partent en série, dans l'ordre où
 * elles ont été lancées (comme `settingsMutationScope` de l'officiel).
 */
export const settingsMutationScope = { id: "settings-update" } as const;

/**
 * Requêtes qui dépendent du serveur Ollama, relues quand son adresse
 * effective change. Littéraux plutôt que les constantes de useModels.ts et
 * useThink.ts, qui importent ce fichier (import circulaire) : `["models"]`
 * (modelsKey), `["recommendedModels"]` (recommendedKey),
 * `["modelExistsUpstream"]` (préfixe d'upstreamKey), `["modelDetails"]`
 * (modelDetailsRoot). `["serverStatus"]` reste à ServerGate. Aussi le dossier
 * .ollama caché du service (ollamaHomeKey), relu après un changement de Model
 * location (son drop-in peut le rendre visible).
 */
export const serverDependentKeys = [
  ["models"],
  ["recommendedModels"],
  ["modelExistsUpstream"],
  ["modelDetails"],
  healthKey,
  ollamaHomeKey,
] as const;

/**
 * Requêtes propres à un serveur, remises à zéro quand son adresse effective
 * change : compte ollama.com et état du nuage. Une relecture en échec sur le
 * nouveau serveur ne laisse pas en place ceux de l'ancien (nom et Sign out
 * d'un autre compte, bandeau à tort).
 */
export const serverIdentityKeys = [accountKey, cloudStatusKey] as const;

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
 * Remplace le cache par la réponse (la langue affichée suit la sienne) ; si l'adresse effective du serveur a
 * changé (ou n'était pas connue), relit ce qui venait de l'ancien serveur,
 * sans attendre (la mise à jour suivante n'attend pas ces relectures), et
 * oublie le compte et l'état du nuage de l'ancien.
 */
function applySettings(queryClient: QueryClient, settings: Settings) {
  const previous = queryClient.getQueryData<Settings>(settingsKey);
  queryClient.setQueryData(settingsKey, settings);
  setLanguage(settings.effectiveLanguage);
  if (previous?.effectiveServerUrl === settings.effectiveServerUrl) return;
  for (const queryKey of serverDependentKeys) {
    void queryClient.invalidateQueries({ queryKey });
  }
  for (const queryKey of serverIdentityKeys) {
    void queryClient.resetQueries({ queryKey });
  }
}
