// Textes de l'onglet Models des réglages (propre à ChatonLM) : modèles du
// moteur (en chargement ou chargés : mémoire, part sur la carte graphique
// d'après les couches, contexte, échéance du déchargement), durée avant
// déchargement. Fonctions pures, testées à part ; celles qui écrivent un
// texte le font dans la langue de `t` (src/i18n).

import type { EngineRunningModel } from "@/bindings/EngineRunningModel";
import type { TFunction } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import { sameModel } from "@/lib/modelNames";

/**
 * Crans de « Keep models loaded », dans l'ordre du curseur : `KEEP_ALIVES`
 * du moteur (secondes ; 0 = défaut du moteur, 5 minutes ; -1 = toujours).
 * Libellés : `keepAliveLabel`.
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

/** Modèle du moteur qui porte cet identifiant, ou null. */
export function runningOf(
  running: readonly EngineRunningModel[],
  model: string,
): EngineRunningModel | null {
  return running.find((each) => sameModel(each.id, model)) ?? null;
}

/**
 * Part (en %) du modèle sur la carte graphique, d'après ses couches
 * (`offloaded N/M layers` du moteur) : 0 sur le processeur seulement, 100
 * tout sur la carte ; null tant qu'elle n'est pas connue (chargement, pas
 * de carte, journal illisible).
 */
function gpuPercent(model: EngineRunningModel): number | null {
  if (model.cpuOnly) return 0;
  const { gpuLayers, totalLayers } = model;
  if (gpuLayers === null || totalLayers === null || totalLayers <= 0) {
    return null;
  }
  if (gpuLayers <= 0) return 0;
  if (gpuLayers >= totalLayers) return 100;
  // Jamais 0 ni 100 : une part reste de chaque côté.
  return Math.min(99, Math.max(1, Math.round((gpuLayers / totalLayers) * 100)));
}

/**
 * Processeur : « 100% GPU », « 100% CPU », « 25%/75% CPU/GPU » (couches sur
 * la carte) ; null si la part est inconnue.
 */
export function processorText(
  t: TFunction,
  model: EngineRunningModel,
): string | null {
  const percent = (value: number) => formatPercent(t.lang, value);
  const gpu = gpuPercent(model);
  if (gpu === null) return null;
  if (gpu === 0) {
    return t("settings.models.loaded.processor.cpu", { percent: percent(100) });
  }
  if (gpu === 100) {
    return t("settings.models.loaded.processor.gpu", { percent: percent(100) });
  }
  return t("settings.models.loaded.processor.split", {
    cpu: percent(100 - gpu),
    gpu: percent(gpu),
  });
}

/** Lenteur du modèle choisi, montrée par l'escargot (SlowModelSnail.tsx). */
export type SlowRun =
  /** Une part (1 à 99 %) de ses couches tourne sur le processeur. */
  | { kind: "split"; cpu: number }
  /** La carte a échoué : tout sur le processeur jusqu'au prochain lancement. */
  | { kind: "fallback" };

/**
 * Lenteur du modèle `model` : réparti entre la carte et le processeur faute
 * de mémoire graphique, ou passé sur le processeur après l'échec de la
 * carte ; null s'il n'est pas chargé, tout sur la carte, sur le processeur
 * par choix ou faute de carte (normal), ou en cours de chargement.
 */
export function slowRun(
  running: readonly EngineRunningModel[],
  model: string,
): SlowRun | null {
  const loaded = runningOf(running, model);
  if (!loaded || loaded.state !== "ready") return null;
  if (loaded.cpuFallback) return { kind: "fallback" };
  if (loaded.cpuOnly) return null;
  const gpu = gpuPercent(loaded);
  if (gpu === null || gpu === 0 || gpu === 100) return null;
  return { kind: "split", cpu: 100 - gpu };
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
 * arrondies), « Unloads soon » (passée ou à moins d'une seconde, une
 * génération en cours la garde), null si inconnue.
 */
export function expiryText(
  t: TFunction,
  model: Pick<EngineRunningModel, "expiresAt" | "kept">,
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
 * Ligne de détail d'un modèle du moteur :
 * « 1.0 GB · 100% GPU · 4k context · Unloads in 4 minutes », sans ce qui
 * est inconnu ; « Loading… » pendant son chargement.
 */
export function runningDetails(
  t: TFunction,
  model: EngineRunningModel,
  now: number,
): string {
  if (model.state === "loading") {
    return t("settings.models.loaded.loadingState");
  }
  const context = contextText(model.contextLength);
  return [
    formatBytes(t, model.sizeBytes),
    processorText(t, model),
    context && t("settings.models.loaded.context", { context }),
    expiryText(t, model, now),
  ]
    .filter(Boolean)
    .join(" · ");
}

/** Résumé : « 2 models loaded · 2.0 GB in memory » (« 1 model loaded »). */
export function loadedSummary(
  t: TFunction,
  models: readonly EngineRunningModel[],
): string {
  const total = models.reduce((sum, model) => sum + model.sizeBytes, 0);
  return t("settings.models.loaded.summary", {
    count: models.length,
    size: formatBytes(t, total),
  });
}
