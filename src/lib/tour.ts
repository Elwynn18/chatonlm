// Géométrie de la visite guidée du premier démarrage : rectangle des éléments
// montrés, place de la bulle à côté. Logique pure, essayée sans navigateur.

/** Rectangle en pixels CSS de la fenêtre. */
export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Marge mise en lumière autour des éléments montrés. */
export const SPOT_MARGIN = 6;
/** Écart entre la zone mise en lumière et la bulle, et bord de la fenêtre. */
export const BUBBLE_GAP = 12;

/**
 * Rectangle qui englobe les éléments affichés que désignent `selectors`
 * (un élément absent ou sans taille est ignoré) ; null si aucun n'est là.
 */
export function targetBox(
  selectors: readonly string[],
  root: ParentNode = document,
): Box | null {
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const selector of selectors) {
    const rect = root.querySelector(selector)?.getBoundingClientRect();
    if (!rect || rect.width === 0 || rect.height === 0) continue;
    left = Math.min(left, rect.left);
    top = Math.min(top, rect.top);
    right = Math.max(right, rect.right);
    bottom = Math.max(bottom, rect.bottom);
  }
  if (left === Infinity) return null;
  return { left, top, width: right - left, height: bottom - top };
}

/** `box` agrandi de `margin` de chaque côté. */
export function grown(box: Box, margin: number): Box {
  return {
    left: box.left - margin,
    top: box.top - margin,
    width: box.width + 2 * margin,
    height: box.height + 2 * margin,
  };
}

const clamp = (value: number, min: number, max: number) =>
  Math.max(min, Math.min(value, max));

/**
 * Coin haut gauche de la bulle : à droite de la zone si elle y tient (entrée
 * de la barre latérale), sinon dessous, sinon dessus, sinon par-dessus, en
 * bas ; toujours entière dans la fenêtre quand celle-ci est assez grande.
 */
export function bubblePlace(
  spot: Box,
  bubble: { width: number; height: number },
  viewport: { width: number; height: number },
): { left: number; top: number } {
  const maxLeft = Math.max(
    BUBBLE_GAP,
    viewport.width - bubble.width - BUBBLE_GAP,
  );
  const maxTop = Math.max(
    BUBBLE_GAP,
    viewport.height - bubble.height - BUBBLE_GAP,
  );
  const right = spot.left + spot.width + BUBBLE_GAP;
  if (right + bubble.width + BUBBLE_GAP <= viewport.width) {
    return { left: right, top: clamp(spot.top, BUBBLE_GAP, maxTop) };
  }
  const left = clamp(
    spot.left + (spot.width - bubble.width) / 2,
    BUBBLE_GAP,
    maxLeft,
  );
  const below = spot.top + spot.height + BUBBLE_GAP;
  if (below + bubble.height + BUBBLE_GAP <= viewport.height) {
    return { left, top: below };
  }
  const above = spot.top - BUBBLE_GAP - bubble.height;
  if (above >= BUBBLE_GAP) return { left, top: above };
  return { left, top: maxTop };
}

/** Vrai si deux rectangles sont les mêmes au demi-pixel près. */
export function sameBox(a: Box | null, b: Box | null): boolean {
  if (a === null || b === null) return a === b;
  return (
    Math.abs(a.left - b.left) < 0.5 &&
    Math.abs(a.top - b.top) < 0.5 &&
    Math.abs(a.width - b.width) < 0.5 &&
    Math.abs(a.height - b.height) < 0.5
  );
}
