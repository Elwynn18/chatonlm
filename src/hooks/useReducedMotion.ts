// Préférence « mouvement réduit » du système (prefers-reduced-motion), suivie
// en direct : les animations CSS passent par la variante motion-safe, ce qui
// est rendu en plus (confettis de l'onglet Study) la lit ici.

import { useSyncExternalStore } from "react";

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

function subscribeMotion(listener: () => void) {
  if (typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(REDUCED_MOTION);
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}

function reducedMotion() {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia(REDUCED_MOTION).matches
  );
}

/** Mouvement réduit demandé par le système (pas de confettis). */
export function useReducedMotion(): boolean {
  return useSyncExternalStore(subscribeMotion, reducedMotion, reducedMotion);
}
