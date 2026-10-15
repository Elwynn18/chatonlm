// Jauge de la VRAM de l'onglet Models des réglages (propre à ChatonLM) :
// remplissage de la mémoire graphique d'après la détection de la machine
// (nvidia-smi, sysfs, DXGI) et les modèles en mémoire (/api/ps). Fonctions
// pures, testées à part.

import type { MachineMemory } from "@/bindings/MachineMemory";
import type { RunningModel } from "@/bindings/RunningModel";

/** Teintes des modèles, dans l'ordre de /api/ps (puis de nouveau). */
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
  name: string;
  /** Rang dans /api/ps (teinte). */
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
      /** Modèles en VRAM (part nulle exclue), dans l'ordre de /api/ps. */
      models: VramModel[];
      /**
       * Occupé hors des modèles (bureau, autres programmes, Ollama lui-même) ;
       * 0 si la mémoire libre d'une carte est inconnue (AMD sous Windows).
       */
      other: number;
      /** Occupé en tout (modèles et autres usages), au plus `total`. */
      used: number;
      /** Libre. */
      free: number;
      /** Part des modèles hors de la VRAM (en mémoire vive). */
      spilled: number;
    };

/**
 * Remplissage de la VRAM. Les deux lectures ne sont pas simultanées : les
 * autres usages, déduits de la mémoire libre, ne sont jamais négatifs, et la
 * jauge ne déborde jamais (modèles tronqués au total).
 */
export function vramView(
  memory: MachineMemory | null | undefined,
  running: readonly RunningModel[],
): VramView {
  if (!memory) return { kind: "unknown" };
  // Aucune carte (ou des cartes sans mémoire dite) : tout sur le processeur,
  // sauf si Ollama a pourtant mis des modèles en VRAM (carte non reconnue).
  const total = memory.gpus.reduce((sum, gpu) => sum + gpu.vramTotal, 0);
  if (total <= 0) {
    return running.some((model) => model.sizeVram > 0)
      ? { kind: "unknown" }
      : { kind: "noGpu" };
  }
  const models = running
    .map((model, index) => ({
      name: model.name,
      index,
      bytes: Math.max(0, model.sizeVram),
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
  const spilled = running.reduce(
    (sum, model) =>
      model.size > model.sizeVram ? sum + (model.size - model.sizeVram) : sum,
    0,
  );
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

/** Largeur d'une part sur la jauge, en pourcentage du total (au plus 100). */
export function share(bytes: number, total: number): number {
  if (total <= 0 || bytes <= 0) return 0;
  return Math.min(100, (bytes / total) * 100);
}
