/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/hooks/useSelectedModel.ts,
 * app/ui/app/src/hooks/useModels.ts and app/ui/app/src/api.ts,
 * commit 42e911bc3d05 (selected model, model of a chat, order of the list,
 * search filter, name looked up upstream).
 * Modified for ChatonLM: pure functions over the models of ChatonLM's own
 * engine (Hugging Face ids, models taken from Ollama, and remote models only
 * once the "remote models" setting lists them); default model = the most
 * recently installed local one, never a remote model nor a recommended model
 * that is not installed; the selected model is found in the full list, so a
 * search never marks it as not installed; one substring filter for all the
 * entries (id or shown name; the official one first keeps the installed
 * models whose name starts with the search, so an installed recommended
 * model could show as not installed); a typed Hugging Face id
 * (`author/repo[:QUANT]`) is looked up on Hugging Face (the ModelHub's
 * repository card) instead of Ollama's registry, only when no entry at all
 * has exactly that id; ids compared like the engine does (the quantization
 * without case).
 */

import type { ChatModel } from "@/bindings/ChatModel";
import type { HubRepo } from "@/bindings/HubRepo";
import type { Message } from "@/bindings/Message";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import { OLLAMA_PREFIX, parseModelName, sameModel } from "@/lib/modelNames";

/** Ligne du sélecteur ; `info` vaut null pour un modèle non installé. */
export interface ModelEntry {
  model: string;
  info: ChatModel | null;
}

/** Modèle de la liste qui porte cet identifiant, ou null. */
function installed(
  models: readonly ChatModel[],
  model: string,
): ChatModel | null {
  return models.find((m) => sameModel(m.id, model)) ?? null;
}

/**
 * Modèle choisi : celui des réglages, installé ou non (il sera téléchargé au
 * prochain envoi) ; null si aucun n'est choisi. Installé sous une autre
 * casse de quantification, il prend l'identifiant de la base.
 */
export function selectedEntry(
  models: readonly ChatModel[],
  selected: string,
): ModelEntry | null {
  if (!selected) return null;
  const info = installed(models, selected);
  return { model: info?.id ?? selected, info };
}

/**
 * Modèle par défaut quand aucun n'est choisi : le plus récemment installé
 * des modèles locaux (la liste les donne du plus récent au plus ancien),
 * jamais un modèle distant (rien ne part chez un fournisseur sans un choix).
 */
export function defaultModel(models: readonly ChatModel[]): string | null {
  return models.find((model) => model.origin !== "remote")?.id ?? null;
}

/**
 * Que faire du dernier modèle d'une conversation à son ouverture :
 * - `adopt` : il devient le modèle choisi (`model` : identifiant de la base
 *   s'il est installé) ;
 * - `legacy` : ancien nom d'Ollama (`qwen3:0.6b`, `hf.co/…`) que la page ne
 *   retrouve pas parmi les installés : le moteur le traduit à
 *   l'enregistrement s'il a été repris (table des noms repris), sinon le
 *   modèle choisi reste ;
 * - null : le modèle choisi reste (distant pas permis, modèle repris
 *   d'Ollama puis supprimé, qui ne se télécharge pas).
 */
export type RestoredModel =
  { kind: "adopt"; model: string } | { kind: "legacy"; model: string };

/**
 * Modèle d'une conversation à reprendre (voir RestoredModel). Une conversation
 * d'avant la 0.10 garde l'ancien nom d'Ollama de ses messages (l'historique
 * n'est jamais réécrit) : il est d'abord cherché parmi les installés sous
 * `@ollama/<nom>` (étiquette `latest` si absente), ou sous l'identifiant de
 * Hugging Face d'un ancien `hf.co/<auteur>/<dépôt>:<étiquette>`.
 */
export function restoredModel(
  models: readonly ChatModel[],
  name: string,
  remoteAllowed: boolean,
): RestoredModel | null {
  const model = name.trim();
  if (!model) return null;
  const parsed = parseModelName(model);
  if (parsed.kind === "remote") {
    return remoteAllowed ? { kind: "adopt", model } : null;
  }
  const found = installed(models, model);
  if (found) return { kind: "adopt", model: found.id };
  if (parsed.kind === "ollama") return null;
  const legacyHf = parsed.kind === "hf" && isLegacyHf(model);
  // Identifiant de Hugging Face : téléchargé au prochain envoi.
  if (parsed.kind === "hf" && !legacyHf) return { kind: "adopt", model };
  const renamed = legacyHf
    ? parsed.tag
      ? installed(models, `${parsed.repo}:${parsed.tag}`)
      : null
    : installed(models, `${OLLAMA_PREFIX}${withTag(model)}`);
  return renamed
    ? { kind: "adopt", model: renamed.id }
    : { kind: "legacy", model };
}

/** Ancien nom d'Ollama avec son étiquette (`latest` si absente). */
function withTag(model: string): string {
  return parseModelName(model).tag
    ? model
    : `${model.replace(/:$/, "")}:latest`;
}

/** Ancien nom de Hugging Face d'Ollama (`hf.co/<auteur>/<dépôt>…`). */
function isLegacyHf(model: string): boolean {
  const host = model.split("/")[0].toLowerCase();
  return host === "hf.co" || host === "huggingface.co";
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
 * Lignes du sélecteur, filtrées par la recherche (sous-chaîne, sans casse,
 * dans l'identifiant ou le nom montré) : les modèles recommandés, dans
 * l'ordre reçu (copie installée s'il y en a une, sinon non installé), puis
 * les autres modèles de la liste dans son ordre, puis le modèle choisi s'il
 * n'est pas installé, enfin l'identifiant tapé trouvé sur Hugging Face
 * (`upstream`, montré même si la recherche ne le contient pas mot pour mot :
 * adresse, autre casse). Chaque modèle n'apparaît qu'une fois (identifiant de la
 * base de préférence).
 */
export function pickerEntries(
  models: readonly ChatModel[],
  selected: ModelEntry | null,
  search: string,
  recommended: readonly string[] = [],
  upstream: string | null = null,
): ModelEntry[] {
  const query = search.trim().toLowerCase();
  const entries: ModelEntry[] = [];
  const seen: ModelEntry[] = [];
  /** `found` : réponse à la recherche elle-même, gardée sans filtre. */
  const once = (entry: ModelEntry, found = false) => {
    if (seen.some((known) => sameModel(known.model, entry.model))) return;
    seen.push(entry);
    const shown = entry.info?.displayName.toLowerCase() ?? "";
    if (
      found ||
      entry.model.trim().toLowerCase().includes(query) ||
      shown.includes(query)
    ) {
      entries.push(entry);
    }
  };
  for (const model of recommended) {
    const info = installed(models, model);
    once({ model: info?.id ?? model, info });
  }
  for (const info of models) once({ model: info.id, info });
  if (selected && selected.info === null) once(selected);
  // Trouvé d'après la recherche (adresse, autre casse) : toujours montré.
  if (upstream) once({ model: upstream, info: null }, true);
  return entries;
}

/** Identifiant de Hugging Face tapé : dépôt et quantification demandée. */
export interface TypedRepo {
  /** « auteur/dépôt ». */
  repo: string;
  /** « Q4_K_M » ; null : la version conseillée pour la machine. */
  quant: string | null;
}

const TYPED_REPO =
  /^(?:(?:https?:\/\/)?(?:hf\.co|huggingface\.co)\/)?([A-Za-z0-9][\w.-]*)\/([A-Za-z0-9][\w.-]*?)(?::([\w.-]+))?\/?$/i;

/**
 * Dépôt de Hugging Face à chercher pour la recherche tapée
 * (`auteur/dépôt[:QUANT]`, adresse `hf.co/…` ou `huggingface.co/…`
 * comprise), si aucune ligne du sélecteur ne porte exactement cet
 * identifiant ; sinon null.
 */
export function typedRepo(
  entries: readonly ModelEntry[],
  search: string,
): TypedRepo | null {
  const name = search.trim();
  const match = TYPED_REPO.exec(name);
  if (!match) return null;
  const repo = `${match[1]}/${match[2]}`;
  const quant = match[3] ?? null;
  if (
    quant !== null &&
    entries.some((e) => sameModel(e.model, `${repo}:${quant}`))
  ) {
    return null;
  }
  return { repo, quant };
}

/**
 * Identifiant à proposer d'après la fiche du dépôt : la version demandée
 * (étiquette sans casse), sinon la version conseillée pour la machine ;
 * null si elle n'existe pas ou ne se télécharge pas (dépôt restreint).
 */
export function upstreamModel(
  repo: HubRepo | null,
  quant: string | null,
): string | null {
  if (!repo) return null;
  const version =
    quant === null
      ? repo.quants.find((q) => q.recommended)
      : repo.quants.find((q) => q.label.toUpperCase() === quant.toUpperCase());
  return version?.model ?? null;
}

/** Identifiants des modèles recommandés, dans l'ordre reçu. */
export function recommendedNames(
  recommended: readonly RecommendedModel[],
): string[] {
  return recommended.map((entry) => entry.model);
}
