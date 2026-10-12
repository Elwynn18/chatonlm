// Focus gardé dans une carte dont le bouton focalisé disparaît (Use remplacé
// par « In use », Download par la progression, Cancel à la fin d'un
// téléchargement, Unload d'un modèle déchargé) : il va à `fallback` (titre
// de la carte, ou l'élément que la fonction choisit d'après l'élément
// perdu), pas au corps de la page, où le lecteur d'écran et le clavier
// perdraient leur place.

import { type RefObject, useEffect, useRef } from "react";

export function useFocusRescue(
  container: RefObject<HTMLElement | null>,
  /**
   * Élément qui reçoit le focus ; une fonction le choisit d'après l'élément
   * perdu (détaché, ses ancêtres retirés avec lui restent lisibles).
   */
  fallback:
    RefObject<HTMLElement | null> | ((lost: Element) => HTMLElement | null),
) {
  // Dernier élément de la carte qui a eu le focus, tant qu'il n'est pas allé
  // ailleurs dans la page.
  const lastRef = useRef<Element | null>(null);

  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const onFocusIn = (event: FocusEvent) => {
      lastRef.current = event.target instanceof Element ? event.target : null;
    };
    const onFocusOut = (event: FocusEvent) => {
      // Vers un autre élément de la page : plus rien à garder. Sans
      // destination (fenêtre quittée, élément retiré), on garde.
      const next = event.relatedTarget;
      if (next instanceof Node && !element.contains(next)) {
        lastRef.current = null;
      }
    };
    element.addEventListener("focusin", onFocusIn);
    element.addEventListener("focusout", onFocusOut);
    return () => {
      element.removeEventListener("focusin", onFocusIn);
      element.removeEventListener("focusout", onFocusOut);
    };
  }, [container]);

  // Après chaque rendu : l'élément focalisé a-t-il quitté la page ?
  useEffect(() => {
    const last = lastRef.current;
    if (!last || last.isConnected) return;
    lastRef.current = null;
    const active = document.activeElement;
    if (active !== null && active !== document.body) return;
    const target =
      typeof fallback === "function" ? fallback(last) : fallback.current;
    target?.focus();
  });
}
