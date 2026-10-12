// Ollama qui sert l'app (celui de la machine ou l'intégré) : lecture pour
// Settings › General et l'écran d'accueil, passage de l'un à l'autre, et
// relecture de tout ce qui dépend de la cible après un changement (le
// moteur le signale aussi : événement ollama-target).

import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { OllamaSource } from "@/bindings/OllamaSource";
import { hubKey } from "@/hooks/useHub";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { serverStatusKey } from "@/hooks/useServerStatus";
import {
  serverDependentKeys,
  serverIdentityKeys,
  settingsKey,
} from "@/hooks/useSettings";
import { listenOllamaTarget, ollamaRuntime, setOllamaSource } from "@/lib/ipc";

export const ollamaRuntimeKey = ["ollamaRuntime"] as const;

/** Dossier des modèles (Settings › Model location), propre à chaque cible. */
export const modelsLocationKey = ["modelsLocation"] as const;

/** Relecture tant que l'intégré qui sert démarre. */
export const RUNTIME_POLL_MS = 2000;

const starting = (runtime: OllamaRuntime | undefined) =>
  runtime?.kind === "builtin" &&
  (runtime.builtinState === "starting" || runtime.builtinState === "stopped");

export function useOllamaRuntime({ enabled = true } = {}) {
  return useQuery({
    queryKey: ollamaRuntimeKey,
    queryFn: ollamaRuntime,
    enabled,
    refetchInterval: (query) =>
      starting(query.state.data) ? RUNTIME_POLL_MS : false,
  });
}

/**
 * La cible a changé (source, dossier de l'intégré, adresse) : réglages
 * (adresse effective), état du serveur et Ollama qui sert relus, attendus ;
 * modèles, santé, dossier, modèles en mémoire et ModelHub relus sans
 * attendre ; compte et nuage de l'ancienne cible oubliés.
 */
export async function refreshTarget(queryClient: QueryClient) {
  for (const queryKey of [
    modelsLocationKey,
    runningModelsKey,
    hubKey,
    ...serverDependentKeys,
  ]) {
    void queryClient.invalidateQueries({ queryKey });
  }
  for (const queryKey of serverIdentityKeys) {
    void queryClient.resetQueries({ queryKey });
  }
  await Promise.all(
    [settingsKey, serverStatusKey, ollamaRuntimeKey].map((queryKey) =>
      queryClient.invalidateQueries({ queryKey }),
    ),
  );
}

/** Passe à l'intégré (`builtin`) ou revient au choix automatique (`auto`). */
export async function switchOllamaSource(
  queryClient: QueryClient,
  source: OllamaSource,
) {
  const runtime = await setOllamaSource(source);
  queryClient.setQueryData(ollamaRuntimeKey, runtime);
  await refreshTarget(queryClient);
}

/**
 * Relit ce qui dépend de la cible quand le moteur la change (intégré choisi
 * seul faute d'autre Ollama, source changée...). À monter une fois (ServerGate).
 */
export function useOllamaTargetEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    listenOllamaTarget(() => void refreshTarget(queryClient)).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for the Ollama target:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}
