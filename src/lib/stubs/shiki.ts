// Remplace l'entrée principale de « shiki » (tous les langages, moteur WASM)
// que Streamdown importe pour son propre bloc de code, jamais affiché :
// ChatonLM surcharge `pre` et `code` et colore avec shiki/core (lib/highlighter).
// Seuls les noms importés par Streamdown 1.4.0 sont fournis.
export const bundledLanguages = Object.freeze(Object.create(null));

export function createHighlighter(): Promise<never> {
  return Promise.reject(new Error("the full Shiki bundle is not included"));
}
