// Lecture des champs des réglages de l'image (propre à ChatonLM ; panneau :
// src/components/ImageSettingsButton.tsx) : entiers bornés, CFG à virgule ou
// à point, CFG écrite dans la langue.

import type { Lang } from "@/i18n";

/** Plus grande graine (u32 du moteur). */
export const MAX_SEED = 4_294_967_295;
export const MAX_CFG = 30;

/** CFG écrite dans la langue (deux décimales au plus : « 4 », « 3,5 »). */
export function cfgText(lang: Lang, value: number): string {
  return new Intl.NumberFormat(lang, {
    maximumFractionDigits: 2,
    useGrouping: false,
  }).format(value);
}

/** Entier de `text` entre `min` et `max` ; vide : `null` ; sinon `undefined`. */
export function parseWhole(
  text: string,
  min: number,
  max: number,
): number | null | undefined {
  const trimmed = text.trim();
  if (trimmed === "") return null;
  if (!/^\d+$/.test(trimmed)) return undefined;
  const value = Number(trimmed);
  return value >= min && value <= max ? value : undefined;
}

/**
 * Nombre décimal de `text` (virgule ou point) entre 0 et [`MAX_CFG`] ;
 * vide : `null` ; illisible ou hors bornes : `undefined`.
 */
export function parseCfg(text: string): number | null | undefined {
  const trimmed = text.trim().replace(",", ".");
  if (trimmed === "") return null;
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return undefined;
  const value = Number(trimmed);
  return value <= MAX_CFG ? value : undefined;
}
