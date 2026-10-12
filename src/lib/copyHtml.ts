// HTML d'une réponse pour le presse-papiers. Hors de la page, les classes
// Tailwind ne font plus rien : le <pre> du thème sombre (masqué par
// « hidden ») et l'en-tête des blocs de code apparaîtraient au collage, et le
// gras de Streamdown (un <span> à classe) serait perdu.

/** Éléments sans place dans le HTML copié. */
const DROPPED = [
  // En-tête d'un bloc de code (langage, bouton copier).
  "[data-code-block-header]",
  ".copy-button",
  // Zone d'annonce des boutons copier (voir CopyButton).
  "[data-copy-status]",
  // Même code dans le thème sombre : un seul exemplaire suffit.
  'pre[data-theme="dark"]',
  // Rendu visuel de KaTeX ; la version MathML reste.
  ".katex-html",
].join(", ");

/**
 * Clone nettoyé du contenu de `root` : sans les éléments ci-dessus, gras en
 * <strong>, sans attributs class ni data-* (les styles en ligne, couleurs du
 * code, restent), précédé de l'encodage pour les accents.
 */
export function cleanCopyHtml(root: HTMLElement): string {
  const copy = root.cloneNode(true) as HTMLElement;
  for (const element of copy.querySelectorAll(DROPPED)) element.remove();
  for (const span of copy.querySelectorAll('[data-streamdown="strong"]')) {
    const strong = span.ownerDocument.createElement("strong");
    strong.append(...span.childNodes);
    span.replaceWith(strong);
  }
  for (const element of copy.querySelectorAll("*")) {
    for (const { name } of [...element.attributes]) {
      if (name === "class" || name.startsWith("data-")) {
        element.removeAttribute(name);
      }
    }
  }
  return `<meta charset="utf-8">${copy.innerHTML}`;
}
