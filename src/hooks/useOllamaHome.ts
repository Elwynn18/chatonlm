// Dossier .ollama du service système d'Ollama (sa clé id_ed25519) que
// ProtectHome= lui cache : alerte et « Fix » de Settings > General, liens
// « Fix in Settings › General » sous les erreurs qu'il provoque (compte
// Ollama, téléchargements d'ollama.com).

import { useQuery } from "@tanstack/react-query";
import { getOllamaHome } from "@/lib/ipc";

/** Relue avec les requêtes du serveur (serverDependentKeys de useSettings). */
export const ollamaHomeKey = ["ollamaHome"] as const;

/**
 * Erreurs d'Ollama que ce dossier caché explique : clé illisible (« open
 * /home/…/.ollama/id_ed25519: permission denied », téléchargements du
 * registre), compte injoignable (503 « account unavailable » de /api/me).
 */
const HOME_ERRORS = /id_ed25519: permission denied|account unavailable/i;

export function isHomeError(message: string | null | undefined): boolean {
  return !!message && HOME_ERRORS.test(message);
}

/**
 * Dossier caché du service auquel l'app parle, ou null. `enabled` : lu
 * seulement quand il sert (une erreur qu'il explique).
 */
export function useOllamaHome({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: ollamaHomeKey,
    queryFn: getOllamaHome,
    enabled,
  });
}
