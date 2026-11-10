// Jauge de la VRAM de l'onglet Models des réglages (propre à ChatonLM) :
// remplissage de la mémoire graphique d'après la détection de la machine
// (nvidia-smi, sysfs, DXGI) et les modèles du moteur (mémoire réservée sur
// la carte, lue dans son journal). Fonctions pures, testées à part.

import type { MachineMemory } from "@/bindings/MachineMemory";
import type { EngineRunningModel } from "@/bindings/EngineRunningModel";

/** Teintes des modèles, dans l'ordre du moteur (puis de nouveau). */
export const MODEL_SHADES = [
  "bg-accent",
  "bg-accent/60",
  "bg-accent/30",
] as const;

/** Teinte du modèle de rang `index`, sur la jauge et sur sa ligne. */
export function modelShade(index: number): string {
  return MODEL_SHADES[index % MODEL_SHADES.length];
}

export interface VramModel {
  /** Identifiant du modèle. */
  name: string;
  /** Rang dans la liste du moteur (teinte). */
  index: number;
  /** Octets en VRAM. */
  bytes: number;
}

export type VramView =
  /**
   * Serveur distant, mémoire illisible, ou carte non reconnue alors que des
   * modèles sont en VRAM (puce graphique servie par Vulkan).
   */
  | { kind: "unknown" }
  /** Aucune carte graphique reconnue : tout tourne sur le processeur. */
  | { kind: "noGpu" }
  | {
      kind: "gauge";
      /** Noms des cartes graphiques. */
      gpus: string[];
      /** VRAM totale, en octets. */
      total: number;
      /** Modèles en VRAM (part nulle exclue), dans l'ordre du moteur. */
      models: VramModel[];
      /**
       * Occupé hors des modèles (bureau, autres programmes) ;
       * 0 si la mémoire libre d'une carte est inconnue (AMD sous Windows).
       */
      other: number;
      /** Occupé en tout (modèles et autres usages), au plus `total`. */
      used: number;
      /** Libre. */
      free: number;
      /**
       * Part des modèles qui ne tient pas en VRAM : couches restées sur le
       * processeur, ou réservé au-delà du total.
       */
      spilled: number;
    };

/**
 * Remplissage de la VRAM. Les deux lectures ne sont pas simultanées : les
 * autres usages, déduits de la mémoire libre, ne sont jamais négatifs, et la
 * jauge ne déborde jamais (modèles tronqués au total).
 */
export function vramView(
  memory: MachineMemory | null | undefined,
  running: readonly EngineRunningModel[],
): VramView {
  if (!memory) return { kind: "unknown" };
  // Aucune carte (ou des cartes sans mémoire dite) : tout sur le processeur,
  // sauf si le moteur a pourtant mis des modèles en VRAM (carte non
  // reconnue).
  const total = memory.gpus.reduce((sum, gpu) => sum + gpu.vramTotal, 0);
  if (total <= 0) {
    return running.some((model) => model.vramBytes > 0)
      ? { kind: "unknown" }
      : { kind: "noGpu" };
  }
  const models = running
    .map((model, index) => ({
      name: model.id,
      index,
      bytes: Math.max(0, model.vramBytes),
    }))
    .filter((model) => model.bytes > 0);
  const inModels = models.reduce((sum, model) => sum + model.bytes, 0);
  const frees = memory.gpus.map((gpu) => gpu.vramFree);
  const reportedUsed = frees.every((free) => free !== null)
    ? total - frees.reduce((sum, free) => sum + free, 0)
    : null;
  const other =
    reportedUsed === null
      ? 0
      : Math.max(0, Math.min(total - inModels, reportedUsed - inModels));
  const used = Math.min(total, inModels + other);
  // Débordement : la part des modèles prêts dont des couches sont restées
  // sur le processeur (la mémoire vive que llama.cpp prend toujours, table
  // des jetons et tampons, n'en est pas : `sizeBytes` la compte aussi pour
  // un modèle tout sur la carte), plus ce que les modèles réservent au-delà
  // de la VRAM (pilote qui déborde en mémoire partagée). Rien pendant un
  // chargement (estimation, aucune part encore sur la carte) ni pour un
  // modèle sur le processeur seul (choisi ou repli, dit par l'escargot).
  const spilled =
    running.reduce(
      (sum, model) =>
        splitsLayers(model)
          ? sum + Math.max(0, model.sizeBytes - model.vramBytes)
          : sum,
      0,
    ) + Math.max(0, inModels - total);
  return {
    kind: "gauge",
    gpus: memory.gpus.map((gpu) => gpu.name),
    total,
    models,
    other,
    used,
    free: total - used,
    spilled,
  };
}

/** Modèle prêt dont une part des couches est sur le processeur. */
function splitsLayers(model: EngineRunningModel): boolean {
  return (
    model.state === "ready" &&
    !model.cpuOnly &&
    model.gpuLayers !== null &&
    model.totalLayers !== null &&
    model.gpuLayers < model.totalLayers
  );
}

/** Largeur d'une part sur la jauge, en pourcentage du total (au plus 100). */
export function share(bytes: number, total: number): number {
  if (total <= 0 || bytes <= 0) return 0;
  return Math.min(100, (bytes / total) * 100);
}
