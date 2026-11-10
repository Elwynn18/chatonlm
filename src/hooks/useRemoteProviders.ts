// Fournisseurs des modèles distants enregistrés (propre à ChatonLM) : réglages
// (settings/RemoteModels.tsx) et nom montré d'un fournisseur dans le
// sélecteur de modèles (le nom réglé, pas son identifiant court).

import { useQuery } from "@tanstack/react-query";
import { remoteProviders } from "@/lib/ipc";
import { remoteProvidersKey } from "@/lib/remoteModels";

/** Fournisseurs enregistrés ; `enabled` faux : rien n'est demandé. */
export function useRemoteProviders(enabled = true) {
  return useQuery({
    queryKey: remoteProvidersKey,
    queryFn: remoteProviders,
    enabled,
  });
}
