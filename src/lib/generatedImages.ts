// Images créées par l'outil `generate_image` (propre à ChatonLM ; contrat
// .tools/research/84-image-chat-contract.md) : lecture de l'appel et de son
// résultat, cadre d'affichage commun à l'aperçu et à l'image finie.
import type { CSSProperties } from "react";
import type { ToolCall } from "@/bindings/ToolCall";

/** Outil d'images de ChatonLM (src-tauri/src/chat/images.rs). */
export const IMAGE_TOOL = "generate_image";

/** Côté le plus long d'une image affichée (pixels CSS). */
const DISPLAY_MAX = 512;

/** Bornes et pas d'un côté demandé, comme le moteur (chat/images.rs). */
const MIN_SIDE = 256;
const MAX_SIDE = 1536;
const SIDE_STEP = 16;

/** Description demandée par un appel `generate_image` ("" sans elle). */
export function imagePrompt(call: ToolCall): string {
  try {
    const args: unknown = JSON.parse(call.arguments);
    const prompt =
      typeof args === "object" && args !== null
        ? (args as Record<string, unknown>).prompt
        : undefined;
    return typeof prompt === "string" ? prompt : "";
  } catch {
    return "";
  }
}

/**
 * Côté demandé par l'argument `key` de l'appel, comme le moteur : nombre ou
 * texte d'un nombre, arrondi au multiple de 16 le plus proche, borné à
 * [256, 1536] ; null s'il est absent ou illisible (le moteur prend alors le
 * côté par défaut du modèle, que la page ne connaît pas).
 */
export function requestedSide(call: ToolCall, key: string): number | null {
  let value: unknown;
  try {
    const args: unknown = JSON.parse(call.arguments);
    value =
      typeof args === "object" && args !== null
        ? (args as Record<string, unknown>)[key]
        : undefined;
  } catch {
    return null;
  }
  const side =
    typeof value === "number"
      ? value
      : typeof value === "string" && value.trim() !== ""
        ? Number(value.trim())
        : NaN;
  if (!Number.isFinite(side)) return null;
  const rounded = Math.round(side / SIDE_STEP) * SIDE_STEP;
  return Math.min(MAX_SIDE, Math.max(MIN_SIDE, rounded));
}

/**
 * Cadre d'une image de `width` × `height` pixels : ses proportions, au plus
 * DISPLAY_MAX de côté, jamais plus large que la colonne.
 */
export function imageFrame(width: number, height: number): CSSProperties {
  const w = Math.max(1, width);
  const h = Math.max(1, height);
  const scale = Math.min(1, DISPLAY_MAX / Math.max(w, h));
  return {
    aspectRatio: `${w} / ${h}`,
    width: `${Math.round(w * scale)}px`,
    maxWidth: "100%",
  };
}

/** Résultat d'un appel `generate_image` réussi (JSON du moteur). */
export interface ImageResult {
  attachmentId: number;
  width: number;
  height: number;
  /** Nom du modèle d'images ("" s'il manque). */
  model: string;
}

/** Résultat lu ; null s'il n'a pas la forme attendue. */
export function parseImageResult(text: string | null): ImageResult | null {
  if (text === null) return null;
  try {
    const value: unknown = JSON.parse(text);
    if (typeof value !== "object" || value === null) return null;
    const { attachmentId, width, height, model } = value as Record<
      string,
      unknown
    >;
    if (
      typeof attachmentId !== "number" ||
      typeof width !== "number" ||
      typeof height !== "number"
    ) {
      return null;
    }
    return {
      attachmentId,
      width,
      height,
      model: typeof model === "string" ? model : "",
    };
  } catch {
    return null;
  }
}
