// Jauge de la VRAM de l'onglet Models des réglages (propre à ChatonLM) :
// remplissage de la mémoire graphique d'après la détection de la machine
// (nvidia-smi, sysfs, DXGI), les modèles du moteur (mémoire réservée sur
// la carte, lue dans son journal) et la vue des ressources (Whisper et
// moteur d'images sur la carte). Fonctions pures, testées à part.

import type { MachineMemory } from "@/bindings/MachineMemory";
import type { EngineRunningModel } from "@/bindings/EngineRunningModel";
import type { ResourcesView } from "@/bindings/ResourcesView";

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

/** Autre partie de l'app sur la carte : moteur d'images, Whisper. */
export interface VramExtra {
  kind: "images" | "speech";
  /** Octets en VRAM. */
  bytes: number;
}

/**
 * Teintes des autres parties de l'app, dans l'échelle du thème : distinctes
 * du roux des modèles, des autres usages (neutral-500, 400 en sombre) qui les
 * suivent et du libre (neutral-200, 700 en sombre). Ordre de la jauge :
 * modèles, images, Whisper, autres usages ; deux parts voisines restent
 * contrastées dans les deux thèmes.
 */
export const EXTRA_SHADES: Record<VramExtra["kind"], string> = {
  images: "bg-neutral-400 dark:bg-neutral-600",
  speech: "bg-neutral-800 dark:bg-neutral-100",
};

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
       * Moteur d'images puis Whisper, quand ils sont sur la carte et que leur
       * VRAM est connue.
       */
      extras: VramExtra[];
      /**
       * Occupé hors de l'app (bureau, autres programmes) ; 0 si la mémoire
       * libre d'une carte est inconnue (AMD sous Windows).
       */
      other: number;
      /** Occupé en tout (app et autres usages), au plus `total`. */
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
 * Parts de VRAM des autres parties de l'app : moteur d'images (VRAM dite par
 * le pilote), puis Whisper sur la carte ; rien pour une part inconnue ou
 * nulle, ni pour Whisper sur le processeur.
 */
export function vramExtras(
  resources: ResourcesView | null | undefined,
): VramExtra[] {
  const extras: VramExtra[] = [];
  const images = resources?.images?.vramBytes ?? 0;
  if (images > 0) extras.push({ kind: "images", bytes: images });
  const speech = resources?.speech;
  if (speech?.device === "card" && (speech.vramBytes ?? 0) > 0) {
    extras.push({ kind: "speech", bytes: speech.vramBytes ?? 0 });
  }
  return extras;
}

/**
 * Remplissage de la VRAM. Les lectures ne sont pas simultanées : les autres
 * usages, déduits de la mémoire libre, ne sont jamais négatifs, et la jauge
 * ne déborde jamais (parts de l'app tronquées au total).
 */
export function vramView(
  memory: MachineMemory | null | undefined,
  running: readonly EngineRunningModel[],
  resources?: ResourcesView | null,
): VramView {
  if (!memory) return { kind: "unknown" };
  const extras = vramExtras(resources);
  // Aucune carte (ou des cartes sans mémoire dite) : tout sur le processeur,
  // sauf si le moteur a pourtant mis des modèles (ou Whisper, ou les images)
  // en VRAM (carte non reconnue).
  const total = memory.gpus.reduce((sum, gpu) => sum + gpu.vramTotal, 0);
  if (total <= 0) {
    return running.some((model) => model.vramBytes > 0) || extras.length > 0
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
  // L'app sur la carte : modèles de texte, moteur d'images, Whisper ; le
  // reste de l'occupé est aux autres programmes.
  const inApp = extras.reduce((sum, extra) => sum + extra.bytes, inModels);
  const frees = memory.gpus.map((gpu) => gpu.vramFree);
  const reportedUsed = frees.every((free) => free !== null)
    ? total - frees.reduce((sum, free) => sum + free, 0)
    : null;
  const other =
    reportedUsed === null
      ? 0
      : Math.max(0, Math.min(total - inApp, reportedUsed - inApp));
  const used = Math.min(total, inApp + other);
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
    extras,
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
