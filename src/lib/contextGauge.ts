// Calculs de la jauge du contexte (propre à ChatonLM, ContextGauge.tsx) :
// part utilisée et tracé du secteur.

import type { ContextUsage } from "@/bindings/ContextUsage";

/** Part au-delà de laquelle le moteur compacte avant l'envoi suivant. */
export const AUTO_COMPACT_PERCENT = 80;

/** Part utilisée, de 0 à 1 (bornée : l'estimation peut dépasser la taille). */
export function usedFraction({ used, size }: ContextUsage): number {
  if (size <= 0) return 0;
  return Math.min(Math.max(used / size, 0), 1);
}

/**
 * Part dessinée en disque plein : au-delà, l'arc SVG dégénère (départ et
 * arrivée confondus) et ne dessine rien.
 */
export const FULL_FRACTION = 0.999;

/**
 * Pourcentage entier utilisé, arrondi vers le bas (79,5 % n'affiche pas le
 * seuil de 80 %, 99,5 % pas 100 %), borné à [0, 100] ; calcul en entiers.
 */
export function usedPercent({ used, size }: ContextUsage): number {
  if (size <= 0) return 0;
  return Math.min(Math.max(Math.floor((used * 100) / size), 0), 100);
}

/** Tracé du secteur de `fraction` (0 à 1), depuis midi, sens horaire. */
export function sectorPath(fraction: number, r = 8, c = 10): string {
  const angle = fraction * 2 * Math.PI;
  const x = c + r * Math.sin(angle);
  const y = c - r * Math.cos(angle);
  const large = fraction > 0.5 ? 1 : 0;
  return `M ${c} ${c} L ${c} ${c - r} A ${r} ${r} 0 ${large} 1 ${x.toFixed(3)} ${y.toFixed(3)} Z`;
}
