// Pastille rousse qui glisse de l'élément actif au suivant (onglets des
// réglages et du ModelHub, entrées de la barre latérale ; propre à
// ChatonLM, l'officiel passe d'un onglet à l'autre sans transition).
//
// Au repos, rien ne change : l'élément actif porte lui-même son aplat roux
// (`bg-accent`), ce que lisent les essais et la mesure des contrastes. Quand
// il change, la pastille part de la place de l'ancien (gardée par nom, même
// quand la nouvelle page remonte la barre) et glisse jusqu'au nouveau, dont
// l'aplat est masqué le temps du trajet (`data-pill-moving`, voir
// src/index.css), puis elle s'efface. Mouvement réduit : pas de trajet.
//
// La pastille est le premier enfant de la barre (positionnée, `relative`) ;
// les éléments portent `data-pill="<clé>"` et sont positionnés (`relative`,
// peints au-dessus de la pastille, qui les précède), sans intermédiaire
// positionné : leur place est lue par offsetLeft / offsetTop.

import { useLayoutEffect, useRef } from "react";
import { reducedMotion } from "@/hooks/useReducedMotion";
import {
  boxOf,
  lastBoxes,
  PILL_DURATION,
  PILL_EASING,
  place,
  sameBox,
} from "@/lib/slidingPill";

export function SlidingPill({
  name,
  active,
  className = "rounded-lg",
}: {
  /** Nom de la barre (place gardée d'une page à l'autre). */
  name: string;
  /** Clé de l'élément actif (`data-pill`), ou aucun. */
  active: string | null;
  /** Arrondi, celui des éléments. */
  className?: string;
}) {
  const pill = useRef<HTMLSpanElement>(null);

  useLayoutEffect(() => {
    // Le parent, pas une référence de la barre : celle-ci n'est posée
    // qu'après les effets de ses enfants, au premier affichage.
    const ghost = pill.current;
    const root = ghost?.parentElement;
    if (!ghost || !root) return;
    const target =
      active === null
        ? null
        : root.querySelector<HTMLElement>(
            `[data-pill="${CSS.escape(active)}"]`,
          );
    if (!target) {
      lastBoxes.delete(name);
      return;
    }
    const box = boxOf(target);
    const from = lastBoxes.get(name);
    lastBoxes.set(name, box);

    // Barre qui change de forme (fenêtre, zoom, langue) : la place gardée
    // suit, sans trajet.
    const observer =
      typeof ResizeObserver === "function"
        ? new ResizeObserver(() => {
            if (target.isConnected) lastBoxes.set(name, boxOf(target));
          })
        : null;
    observer?.observe(root);

    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = () => {
      delete target.dataset.pillMoving;
      ghost.style.transition = "none";
      ghost.style.opacity = "0";
    };
    if (from && !sameBox(from, box) && !reducedMotion()) {
      target.dataset.pillMoving = "";
      ghost.style.transition = "none";
      place(ghost, from);
      ghost.style.opacity = "1";
      // Départ peint avant le trajet.
      void ghost.offsetWidth;
      ghost.style.transition = ["transform", "width", "height"]
        .map((property) => `${property} ${PILL_DURATION}ms ${PILL_EASING}`)
        .join(", ");
      place(ghost, box);
      timer = setTimeout(finish, PILL_DURATION + 40);
    }
    return () => {
      observer?.disconnect();
      if (timer !== undefined) {
        clearTimeout(timer);
        finish();
      }
    };
  }, [active, name]);

  return (
    <span
      ref={pill}
      aria-hidden="true"
      data-testid={`sliding-pill-${name}`}
      className={`pointer-events-none absolute top-0 left-0 bg-accent opacity-0 ${className}`}
    />
  );
}
