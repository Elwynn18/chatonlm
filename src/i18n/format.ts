// Nombres, tailles, dates et noms de langues écrits à la façon de la langue
// affichée (séparateur décimal, espaces des milliers, unités : « 1,5 Go » en
// français). Les fonctions prennent la langue en paramètre : un composant la
// lit par `useLanguage()`, et se rend donc de nouveau quand elle change.

import type { Lang } from "@/bindings/Lang";
import { localized, type TFunction, translate } from "@/i18n";

/** Unités de taille, du plus petit au plus grand (base 1024, comme Ollama). */
export const SIZE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;
export type SizeUnit = (typeof SIZE_UNITS)[number];

/** Symbole d'une unité de taille dans la langue (« Go » en français). */
export function sizeUnit(lang: Lang, unit: SizeUnit): string {
  return translate(lang, `common.units.${unit}`);
}

/**
 * Nombre avec exactement `decimals` décimales, sans milliers groupés (une
 * taille dans une unité imposée, « 1010.0 GB », s'écrit comme avant) :
 * « 5.0 » / « 5,0 ».
 */
export function formatDecimal(
  lang: Lang,
  value: number,
  decimals: number,
): string {
  return new Intl.NumberFormat(lang, {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
    useGrouping: false,
  }).format(value);
}

/** Taille « valeur unité », valeur déjà calculée : « 5,0 Go ». */
export function formatSizeValue(
  lang: Lang,
  value: number,
  decimals: number,
  unit: SizeUnit,
): string {
  return translate(lang, "common.size", {
    value: formatDecimal(lang, value, decimals),
    unit: sizeUnit(lang, unit),
  });
}

/** Entier avec les séparateurs de la langue : « 12,345 » / « 12 345 ». */
export function formatInteger(lang: Lang, value: number): string {
  return new Intl.NumberFormat(lang, { maximumFractionDigits: 0 }).format(
    value,
  );
}

/** Pourcentage entier : « 42% » / « 42 % ». */
export function formatPercent(lang: Lang, percent: number): string {
  return new Intl.NumberFormat(lang, {
    style: "percent",
    maximumFractionDigits: 0,
  }).format(percent / 100);
}

/** Nom d'une langue (code ISO 639-1) dans la langue affichée. */
export function languageName(lang: Lang, code: string): string {
  let name = code;
  try {
    name = new Intl.DisplayNames([lang], { type: "language" }).of(code) ?? code;
  } catch {
    // Code inconnu d'Intl : écrit tel quel.
  }
  return localized(name.charAt(0).toLocaleUpperCase(lang) + name.slice(1));
}

/**
 * Nom d'une langue proposée, écrit dans cette langue (« Español ») : le même
 * quelle que soit la langue affichée, pour qu'on retrouve la sienne.
 */
export function nativeLanguageName(code: Lang): string {
  return languageName(code, code);
}

/** Date et heure courtes : « 6 oct. 2026, 18:40 ». */
export function formatDateTime(lang: Lang, date: Date | number): string {
  return localized(
    new Intl.DateTimeFormat(lang, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(date),
  );
}

/** Heure courte, sans la date : « 18:40 », « 6:40 PM ». */
export function formatTime(lang: Lang, date: Date | number): string {
  return localized(
    new Intl.DateTimeFormat(lang, { timeStyle: "short" }).format(date),
  );
}

/** Durée relative : « il y a 3 jours », « hace 2 horas ». */
export function formatRelative(
  lang: Lang,
  value: number,
  unit: Intl.RelativeTimeFormatUnit,
): string {
  return localized(
    new Intl.RelativeTimeFormat(lang, { numeric: "auto" }).format(value, unit),
  );
}

const K = 1024;

/**
 * Taille lisible, base 1024 comme Ollama : une décimale sous 10 (« 5.0 GB »,
 * « 5,0 Go »), aucune au-delà (« 18 GB », « 850 MB ») ; jamais « 1024 MB ».
 */
export function formatBytes(t: TFunction, bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return formatSizeValue(t.lang, 0, 0, "B");
  }
  let i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(K)),
    SIZE_UNITS.length - 1,
  );
  let value = bytes / K ** i;
  // 1023,7 Mo s'écrirait « 1024 MB » : unité suivante.
  if (value >= 999.5 && i < SIZE_UNITS.length - 1) {
    i += 1;
    value /= K;
  }
  const decimals = i > 0 && value < 9.95 ? 1 : 0;
  return formatSizeValue(t.lang, value, decimals, SIZE_UNITS[i]);
}

/**
 * Date relative : « just now » (moins d'une minute), « 5 minutes ago »,
 * « yesterday », « 3 weeks ago », « last year », dans la langue de `t`.
 * `date` : ISO 8601 ou ms depuis 1970 ; vide si elle est illisible.
 */
export function formatTimeAgo(
  t: TFunction,
  date: string | number,
  now: number = Date.now(),
): string {
  const time = typeof date === "number" ? date : Date.parse(date);
  if (!Number.isFinite(time)) return "";
  const seconds = Math.round((time - now) / 1000);
  const abs = Math.abs(seconds);
  if (abs < 60) return t("common.time.justNow");
  const units: [Intl.RelativeTimeFormatUnit, number, number][] = [
    ["minute", 60, 3600],
    ["hour", 3600, 86400],
    ["day", 86400, 7 * 86400],
    ["week", 7 * 86400, 30 * 86400],
    ["month", 30 * 86400, 365 * 86400],
    ["year", 365 * 86400, Infinity],
  ];
  for (const [unit, size, limit] of units) {
    if (abs < limit) {
      return formatRelative(t.lang, Math.trunc(seconds / size), unit);
    }
  }
  return "";
}
