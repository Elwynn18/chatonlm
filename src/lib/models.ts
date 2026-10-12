/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/hooks/useSelectedModel.ts,
 * app/ui/app/src/hooks/useModels.ts and app/ui/app/src/api.ts,
 * commit 42e911bc3d05 (selected model, model of a chat, order of the list,
 * search filter, name looked up in the registry).
 * Modified for ChatonLM: pure functions; no cloud models; default model = the
 * most recently installed one, never a recommended model that is not
 * installed; the selected model is found in the full list, so a search never
 * marks it as not installed; one substring filter for all the entries (the
 * official one first keeps the installed models whose name starts with the
 * search, so an installed recommended model could show as not installed);
 * the registry is asked about the trimmed typed name, only when no entry at
 * all (recommended and selected models included) has exactly that name;
 * names compared without case, as Ollama resolves them (a selected
 * "...smollm2...:q4_k_m" is the installed "...SmolLM2...:Q4_K_M", shown once,
 * with the installed spelling).
 */

import type { Message } from "@/bindings/Message";
import type { ModelInfo } from "@/bindings/ModelInfo";
import { sameModel } from "@/lib/modelNames";

/** Ligne du sélecteur ; `info` vaut null pour un modèle non installé. */
export interface ModelEntry {
  model: string;
  info: ModelInfo | null;
}

/** Modèle installé de ce nom, sans tenir compte de la casse, ou null. */
function installed(models: ModelInfo[], model: string): ModelInfo | null {
  return models.find((m) => sameModel(m.model, model)) ?? null;
}

/**
 * Modèle choisi : celui des réglages, installé ou non (il sera téléchargé au
 * prochain envoi) ; null si aucun n'est choisi. Installé sous une autre
 * casse (Ollama ne la distingue pas), il prend le nom installé.
 */
export function selectedEntry(
  models: ModelInfo[],
  selected: string,
): ModelEntry | null {
  if (!selected) return null;
  const info = installed(models, selected);
  return { model: info?.model ?? selected, info };
}

/** Modèle par défaut quand aucun n'est choisi : le plus récemment installé. */
export function defaultModel(models: ModelInfo[]): string | null {
  return models[0]?.model ?? null;
}

/** Dernier modèle utilisé dans une conversation. */
export function chatModel(messages: Message[]): string | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const model = messages[i].model;
    if (model) return model;
  }
  return null;
}

/**
 * Lignes du sélecteur, filtrées par la recherche (sous-chaîne, sans casse) :
 * les modèles recommandés, dans l'ordre du serveur (copie installée s'il y en
 * a une, sinon non installé), puis les autres modèles installés dans l'ordre
 * de /api/tags, puis le modèle choisi s'il n'est pas installé, enfin le nom
 * tapé trouvé au registre (`upstream`). Chaque nom n'apparaît qu'une fois,
 * sans tenir compte de la casse (orthographe installée de préférence).
 */
export function pickerEntries(
  models: ModelInfo[],
  selected: ModelEntry | null,
  search: string,
  recommended: readonly string[] = [],
  upstream: string | null = null,
): ModelEntry[] {
  const query = search.trim().toLowerCase();
  const entries: ModelEntry[] = [];
  const seen = new Set<string>();
  const add = (entry: ModelEntry) => {
    const key = entry.model.trim().toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    if (key.includes(query)) entries.push(entry);
  };
  for (const model of recommended) {
    const info = installed(models, model);
    add({ model: info?.model ?? model, info });
  }
  for (const info of models) add({ model: info.model, info });
  if (selected && selected.info === null) add(selected);
  if (upstream) add({ model: upstream, info: null });
  return entries;
}

/**
 * Nom à chercher au registre : la recherche rognée, si aucune ligne du
 * sélecteur ne porte exactement ce nom (sans casse) ; sinon null.
 */
export function upstreamCandidate(
  entries: readonly ModelEntry[],
  search: string,
): string | null {
  const name = search.trim();
  if (!name) return null;
  const lower = name.toLowerCase();
  return entries.some((entry) => entry.model.toLowerCase() === lower)
    ? null
    : name;
}
