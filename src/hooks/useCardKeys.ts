// Lecteur de cours (onglet Study) : touches de la carte montrée (Entrée,
// chiffres), depuis toute la page.

import { useEffect, useRef } from "react";
import { overlayOpen } from "@/lib/overlay";

/** Champ où la frappe va au texte (pas un bouton radio). */
export function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement) return true;
  if (target instanceof HTMLSelectElement) return true;
  if (target instanceof HTMLInputElement) {
    return !["radio", "checkbox", "button", "submit", "reset"].includes(
      target.type,
    );
  }
  return false;
}

/** Entrée et Espace activent déjà le bouton ou le lien qui a le focus. */
function activatedByKey(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest("a[href], button, summary, [role='button']") !== null
  );
}

export interface CardKeys {
  /** Entrée hors d'un champ et d'un bouton : action principale de la carte. */
  onEnter?: () => void;
  /** Chiffre 1 à 9 hors d'un champ : choix d'une option. */
  onDigit?: (digit: number) => void;
}

/**
 * Touches de la carte montrée, depuis toute la page (pas une boîte ni un
 * menu ouvert, ni un champ de saisie, ni une touche déjà prise). Une touche
 * tenue (répétition) n'agit qu'une fois : Entrée tenue ne fait pas défiler
 * les cartes, ni n'active le bouton qui vient de prendre le focus
 * (« Continue » après « Check » : le retour se lit d'abord).
 */
export function useCardKeys(keys: CardKeys) {
  const keysRef = useRef(keys);
  useEffect(() => {
    keysRef.current = keys;
  });
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || overlayOpen()) return;
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      if (isTextField(event.target)) return;
      if (event.repeat) {
        if (event.key === "Enter") event.preventDefault();
        return;
      }
      const { onEnter, onDigit } = keysRef.current;
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        if (!onEnter || activatedByKey(event.target)) return;
        event.preventDefault();
        onEnter();
      } else if (/^[1-9]$/.test(event.key) && onDigit && !event.shiftKey) {
        event.preventDefault();
        onDigit(Number(event.key));
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
