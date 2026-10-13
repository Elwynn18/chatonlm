/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/hooks/useChats.ts (useStaleModels,
 * useDismissedStaleModels, useShouldShowStaleDisplay, useDismissStaleModel)
 * and app/ui/app/src/components/ModelPicker.tsx (checkModelStaleness),
 * commit 42e911bc3d05.
 * Modified for ChatonLM: a Tauri command instead of an HTTP call, which
 * compares the installed models with their registry in the Rust backend
 * (src-tauri/src/ollama/outdated.rs: Hugging Face models included); several
 * models in one request (the Installed tab of the ModelHub); the answer is
 * kept by React Query for 5 minutes per installed version (the official
 * cache: 5 minutes per model name), so a model downloaded again outside of
 * ChatonLM is checked again at once; a model whose download has just ended is
 * no longer stale without waiting for the next check.
 */

import {
  keepPreviousData,
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback } from "react";
import { useSettings } from "@/hooks/useSettings";
import { outdatedModels } from "@/lib/ipc";
import { withTag } from "@/lib/modelNames";

/** Préfixe des vérifications (une par ensemble de versions installées). */
export const outdatedKey = ["outdatedModels"] as const;
/** Modèles dont la bannière a été fermée, jusqu'à la fin de la session. */
export const dismissedOutdatedKey = ["dismissedOutdatedModels"] as const;

/** Une réponse du registre vaut 5 minutes, comme dans l'officiel. */
const CHECK_STALE_MS = 5 * 60 * 1000;

const NONE: readonly string[] = [];

/**
 * Version installée d'un modèle : son nom, et ce qui change quand elle est
 * remplacée (empreinte du sélecteur, date de la liste des installés).
 */
export interface InstalledVersion {
  model: string;
  version: string;
}

/** Même modèle, à la casse et à `:latest` près. */
function same(a: string, b: string): boolean {
  return withTag(a).toLowerCase() === withTag(b).toLowerCase();
}

/** `model` est-il dans cette liste de noms (à la casse et à `:latest` près) ? */
export function listsModel(names: readonly string[], model: string): boolean {
  return names.some((name) => same(name, model));
}

/**
 * Parmi ces modèles installés, ceux dont le registre a une version plus
 * récente ; aucun tant que rien n'est su, ou si le registre ou Ollama ne
 * répondent pas. Rien n'est demandé pour une liste vide.
 */
export function useOutdatedModels(
  installed: readonly InstalledVersion[],
): readonly string[] {
  const { settings } = useSettings();
  const server = settings?.effectiveServerUrl ?? null;
  const models = installed.map((entry) => entry.model);
  // Clé : le serveur et les versions installées, dans un ordre stable.
  const versions = installed
    .map((entry) => `${entry.model}@${entry.version}`)
    .sort()
    .join("\n");
  const { data } = useQuery({
    queryKey: [...outdatedKey, server, versions],
    queryFn: () => outdatedModels(models),
    enabled: server !== null && models.length > 0,
    staleTime: CHECK_STALE_MS,
    retry: false,
    // Une liste qui change (modèle mis à jour, autre modèle choisi) garde la
    // réponse précédente le temps de la nouvelle : les pastilles des autres
    // modèles ne clignotent pas.
    placeholderData: keepPreviousData,
  });
  return data ?? NONE;
}

/**
 * `model` vient d'être téléchargé : il n'est plus périmé, dans toutes les
 * vérifications gardées (la suivante le redira s'il l'est encore).
 */
export function forgetOutdated(queryClient: QueryClient, model: string) {
  queryClient.setQueriesData<readonly string[]>(
    { queryKey: outdatedKey },
    (names) =>
      names && listsModel(names, model)
        ? names.filter((name) => !same(name, model))
        : names,
  );
}

/**
 * Bannières fermées pendant la session : `isDismissed(model)` et
 * `dismiss(model)`.
 */
export function useDismissedOutdated() {
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: dismissedOutdatedKey,
    queryFn: () => NONE,
    initialData: NONE,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  const dismiss = useCallback(
    (model: string) => {
      queryClient.setQueryData<readonly string[]>(
        dismissedOutdatedKey,
        (names = NONE) =>
          listsModel(names, model) ? names : [...names, model],
      );
    },
    [queryClient],
  );
  return { dismissed: data, dismiss };
}
