// Zoom de la page, comme l'officiel : ±0,1 par appui, borné à 90-120 %,
// remis à 100 % au lancement (jamais enregistré).

import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export const ZOOM_MIN = 0.9;
export const ZOOM_MAX = 1.2;
const ZOOM_STEP = 0.1;

let level = 1;

/** Niveau de zoom demandé en dernier. */
export function zoomLevel(): number {
  return level;
}

/** Applique `next` (borné, arrondi au dixième) s'il change. */
export async function setZoomLevel(next: number): Promise<void> {
  const target =
    Math.round(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, next)) * 10) / 10;
  if (target === level) return;
  const previous = level;
  // Mis à jour tout de suite : un second appui part du niveau demandé.
  level = target;
  if (!isTauri()) return;
  try {
    await getCurrentWebview().setZoom(target);
  } catch (cause) {
    console.error("setZoom failed", cause);
    if (level === target) level = previous;
  }
}

export const zoomIn = () => setZoomLevel(level + ZOOM_STEP);
export const zoomOut = () => setZoomLevel(level - ZOOM_STEP);
export const zoomReset = () => setZoomLevel(1);
