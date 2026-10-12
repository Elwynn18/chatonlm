import { useQuery } from "@tanstack/react-query";
import type { ServerStatus } from "@/bindings/ServerStatus";
import { serverStatus } from "@/lib/ipc";

/** Délai entre deux vérifications tant qu'Ollama ne répond pas. */
export const RECHECK_INTERVAL_MS = 3000;

export const serverStatusKey = ["serverStatus"] as const;

const isRunning = (query: { state: { data?: ServerStatus } }) =>
  query.state.data?.state === "running";

/**
 * État du serveur Ollama (celui de la machine ou l'intégré, pendant qu'il
 * démarre), pour l'écran d'accueil. Tant qu'il ne répond pas, il
 * est revérifié toutes les 3 s et quand la fenêtre redevient active ; une fois
 * en marche, plus jamais (une perte de connexion en cours de session se
 * signale dans la conversation).
 */
export function useServerStatus() {
  return useQuery({
    queryKey: serverStatusKey,
    queryFn: serverStatus,
    refetchInterval: (query) =>
      isRunning(query) ? false : RECHECK_INTERVAL_MS,
    refetchOnWindowFocus: (query) => !isRunning(query),
    refetchOnReconnect: (query) => !isRunning(query),
    staleTime: (query) => (isRunning(query) ? Infinity : 0),
  });
}
