// Places de la pastille qui glisse (src/components/SlidingPill.tsx), gardées
// d'un affichage de la barre à l'autre.

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Dernière place de l'élément actif de chaque barre. */
export const lastBoxes = new Map<string, Box>();

/** Durée du trajet (ms). */
export const PILL_DURATION = 260;

export const PILL_EASING = "cubic-bezier(0.2, 0, 0, 1)";

export function boxOf(element: HTMLElement): Box {
  return {
    left: element.offsetLeft,
    top: element.offsetTop,
    width: element.offsetWidth,
    height: element.offsetHeight,
  };
}

export function sameBox(a: Box, b: Box) {
  return (
    a.left === b.left &&
    a.top === b.top &&
    a.width === b.width &&
    a.height === b.height
  );
}

export function place(pill: HTMLElement, box: Box) {
  pill.style.width = `${box.width}px`;
  pill.style.height = `${box.height}px`;
  pill.style.transform = `translate(${box.left}px, ${box.top}px)`;
}

/** Oublie les places gardées (essais). */
export function resetSlidingPills() {
  lastBoxes.clear();
}
