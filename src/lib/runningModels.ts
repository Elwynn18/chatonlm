// Textes de l'onglet Models des réglages (propre à ChatonLM) : modèles en
// mémoire d'après /api/ps (processeur arrondi comme `ollama ps`, contexte,
// échéance du déchargement), durée avant déchargement. Fonctions pures,
// testées à part ; celles qui écrivent un texte le font dans la langue de
// `t` (src/i18n).

import type { RunningModel } from "@/bindings/RunningModel";
import type { TFunction } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import { sameModel } from "@/lib/modelNames";

/**
 * Crans de « Keep models loaded », dans l'ordre du curseur : `KEEP_ALIVES`
 * du moteur (secondes ; 0 = défaut du serveur, -1 = toujours). Libellés :
 * `keepAliveLabel`.
 */
export const KEEP_ALIVE_STEPS: readonly { seconds: number }[] = [
  { seconds: 0 },
  { seconds: 60 },
  { seconds: 300 },
  { seconds: 900 },
  { seconds: 1800 },
  { seconds: 3600 },
  { seconds: 14400 },
  { seconds: -1 },
];

/**
 * Libellé d'un cran : « Default » (0), « Always » (-1), « 5 min » sous une
 * heure, sinon « 1 hour », « 4 hours ».
 */
export function keepAliveLabel(t: TFunction, seconds: number): string {
  if (seconds === 0) return t("settings.models.keepAlive.default");
  if (seconds < 0) return t("settings.models.keepAlive.always");
  if (seconds < 3600) {
    return t("settings.models.keepAlive.minutes", {
      count: Math.round(seconds / 60),
    });
  }
  return t("settings.models.keepAlive.hours", {
    count: Math.round(seconds / 3600),
  });
}

/**
 * Nom complet d'un modèle : « :latest » ajouté à un nom sans étiquette
 * (étiquette = après le dernier « : » du dernier segment « / »), comme
 * Ollama le nomme dans /api/ps.
 */
function withTag(name: string): string {
  const trimmed = name.trim();
  const last = trimmed.slice(trimmed.lastIndexOf("/") + 1);
  return last.includes(":") ? trimmed : `${trimmed}:latest`;
}

/**
 * Deux noms désignent-ils le même modèle, l'un dit par /api/ps et l'autre
 * par /api/tags ou les réglages (casse ignorée, « :latest » implicite) ?
 */
export function sameRunningModel(a: string, b: string): boolean {
  return sameModel(withTag(a), withTag(b));
}

/**
 * Processeur, arrondi comme `ollama ps` : « 100% GPU », « 100% CPU »,
 * « 25%/75% CPU/GPU » ; null pour des tailles incohérentes (VRAM au-delà
 * de la taille, taille nulle), qu'`ollama ps` dit « Unknown ».
 */
export function processorText(
  t: TFunction,
  size: number,
  sizeVram: number,
): string | null {
  const percent = (value: number) => formatPercent(t.lang, value);
  if (sizeVram === 0) {
    return t("settings.models.loaded.processor.cpu", { percent: percent(100) });
  }
  if (sizeVram === size) {
    return t("settings.models.loaded.processor.gpu", { percent: percent(100) });
  }
  if (sizeVram > size || size <= 0) return null;
  const cpu = Math.round(((size - sizeVram) / size) * 100);
  return t("settings.models.loaded.processor.split", {
    cpu: percent(cpu),
    gpu: percent(100 - cpu),
  });
}

/** Contexte : « 32k » pour un multiple de 1024, sinon le nombre ; null si inconnu. */
export function contextText(contextLength: number | null): string | null {
  if (contextLength === null || contextLength <= 0) return null;
  return contextLength % 1024 === 0
    ? `${contextLength / 1024}k`
    : String(contextLength);
}

/**
 * Échéance du déchargement à `now` (ms) : « Stays loaded » (gardé),
 * « Unloads in 4 minutes » (secondes et minutes entières écoulées, heures
 * arrondies, comme `ollama ps`), « Unloads soon » (passée ou à moins d'une
 * seconde, une génération en cours la garde), null si inconnue.
 */
export function expiryText(
  t: TFunction,
  model: Pick<RunningModel, "expiresAt" | "kept">,
  now: number,
): string | null {
  if (model.kept) return t("settings.models.loaded.expiry.kept");
  if (model.expiresAt === null) return null;
  const seconds = Math.floor((model.expiresAt - now) / 1000);
  if (seconds < 1) return t("settings.models.loaded.expiry.soon");
  if (seconds < 60) {
    return t("settings.models.loaded.expiry.seconds", { count: seconds });
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return t("settings.models.loaded.expiry.minutes", { count: minutes });
  }
  return t("settings.models.loaded.expiry.hours", {
    count: Math.round(seconds / 3600),
  });
}

/**
 * Ligne de détail d'un modèle chargé :
 * « 1.0 GB · 100% GPU · 4k context · Unloads in 4 minutes », sans ce qui
 * est inconnu.
 */
export function runningDetails(
  t: TFunction,
  model: RunningModel,
  now: number,
): string {
  const context = contextText(model.contextLength);
  return [
    formatBytes(t, model.size),
    processorText(t, model.size, model.sizeVram),
    context && t("settings.models.loaded.context", { context }),
    expiryText(t, model, now),
  ]
    .filter(Boolean)
    .join(" · ");
}

/** Résumé : « 2 models loaded · 2.0 GB in memory » (« 1 model loaded »). */
export function loadedSummary(
  t: TFunction,
  models: readonly RunningModel[],
): string {
  const total = models.reduce((sum, model) => sum + model.size, 0);
  return t("settings.models.loaded.summary", {
    count: models.length,
    size: formatBytes(t, total),
  });
}
