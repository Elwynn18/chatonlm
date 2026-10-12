// Essais des traductions : textes d'un écran rendu qui ne viennent pas des
// catalogues (écrits en dur). Sous `usePseudoLanguage()`, chaque texte
// traduit est entouré de « ⟦ ⟧ » (src/i18n) ; ce qui reste hors de ces
// marques, et qui n'est pas un contenu donné par l'essai (`allow` : noms de
// modèles, messages, chemins...), est un texte oublié.

import { act } from "@testing-library/react";
import { setPseudoForTests } from "@/i18n";

/** Textes traduits marqués, jusqu'à la fin de l'essai (src/test/setup.ts). */
export function usePseudoLanguage() {
  act(() => setPseudoForTests(true));
}

/** Attributs lus ou montrés à l'utilisateur. */
const TEXT_ATTRIBUTES = [
  "aria-label",
  "aria-description",
  "aria-valuetext",
  "aria-roledescription",
  "title",
  "placeholder",
  "alt",
];

/** Retire les textes marqués et les contenus permis ; garde ce qui a des lettres. */
function leftover(text: string, allow: readonly string[]): string | null {
  let rest = text.replace(/⟦[^⟦⟧]*⟧/g, " ");
  // Marques imbriquées (texte traduit dans un autre) : de l'intérieur.
  while (/⟦[^⟦⟧]*⟧/.test(rest)) rest = rest.replace(/⟦[^⟦⟧]*⟧/g, " ");
  for (const allowed of [...allow].sort((a, b) => b.length - a.length)) {
    if (allowed) rest = rest.split(allowed).join(" ");
  }
  rest = rest.replace(/\s+/g, " ").trim();
  return /\p{L}{2,}/u.test(rest) ? rest : null;
}

/**
 * Textes non traduits de `root` : son texte (zones masquées comprises) et
 * ses attributs lus à l'utilisateur. Vide si tout vient des catalogues ou
 * de `allow`.
 */
export function untranslatedTexts(
  root: Element = document.body,
  allow: readonly string[] = [],
): string[] {
  const found: string[] = [];
  // Texte de chaque élément sans enfant élément, puis des autres morceaux de
  // texte : une phrase traduite coupée par une balise (<Trans>) reste
  // entière dans le texte de son parent.
  const text = leftover(root.textContent ?? "", allow);
  if (text) found.push(text);
  for (const element of root.querySelectorAll("*")) {
    for (const name of TEXT_ATTRIBUTES) {
      const value = element.getAttribute(name);
      if (value === null) continue;
      const rest = leftover(value, allow);
      if (rest) found.push(`[${name}] ${rest}`);
    }
  }
  return found;
}
