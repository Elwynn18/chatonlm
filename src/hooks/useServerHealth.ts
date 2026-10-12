import { useQuery } from "@tanstack/react-query";
import { errorMessage, restartBuiltinOllama, serverStatus } from "@/lib/ipc";

export const healthKey = ["health"] as const;

/** Intervalle des vérifications pendant la session. */
export const HEALTH_INTERVAL_MS = 3000;

/**
 * Ollama répond-il encore ? Vérifié toutes les 3 s pendant la session (l'écran
 * d'accueil a sa propre vérification). Inconnu au départ : considéré comme
 * joignable, puisque l'app ne s'affiche qu'une fois Ollama joint. Seuls
 * `data` et `refetch` sont lus : react-query ne réaffiche que pour ce qui est
 * lu, pas à chaque vérification.
 */
export function useServerHealth() {
  const { data, refetch } = useQuery({
    queryKey: healthKey,
    queryFn: serverStatus,
    refetchInterval: HEALTH_INTERVAL_MS,
    refetchOnWindowFocus: true,
  });
  // L'Ollama intégré a échoué (plantages répétés) : le moteur ne le relance
  // que sur demande, comme Retry de l'écran d'accueil.
  const failed = data?.state === "builtinFailed";
  return {
    healthy: data === undefined || data.state === "running",
    // L'Ollama intégré (re)démarre : changement de source ou de dossier des
    // modèles, relance après un plantage. Pas « Ollama isn't responding ».
    starting: data?.state === "builtinStarting",
    failed,
    /**
     * Vérifie de nouveau ; l'intégré en échec est d'abord relancé. Rejette
     * si la relance a échoué (après la vérification).
     */
    retry: async () => {
      let failure: unknown = null;
      if (failed) {
        try {
          await restartBuiltinOllama();
        } catch (cause) {
          failure = cause;
        }
      }
      await refetch({ cancelRefetch: false });
      if (failure !== null) {
        throw new Error(errorMessage(failure), { cause: failure });
      }
    },
  };
}
