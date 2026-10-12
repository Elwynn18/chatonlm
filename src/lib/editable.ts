// Cible d'une touche : champ où l'on tape, ou élément qu'Espace active.
// Code propre à ChatonLM, sorti de la zone de saisie du chat (ChatForm.tsx)
// pour la barre du mode vocal, qui suit les mêmes règles.

/** Champ où l'on tape (ou élément dedans) : la frappe lui revient. */
export function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.isContentEditable ||
    target.closest("input, textarea, [contenteditable='true']") !== null
  );
}

/** Éléments qu'Espace active : la touche leur revient (pas aux liens, qu'elle
 * n'active pas). */
const ACTIVATED_BY_SPACE = [
  "button",
  "summary",
  "select",
  "[role='button']",
  "[role='menuitem']",
  "[role='option']",
  "[role='switch']",
  "[role='checkbox']",
  "[role='tab']",
].join(", ");

export function activatedBySpace(target: EventTarget | null): boolean {
  return (
    target instanceof Element && target.closest(ACTIVATED_BY_SPACE) !== null
  );
}
