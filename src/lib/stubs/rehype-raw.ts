// Remplace rehype-raw (et parse5), cité par les greffons par défaut de
// Streamdown : ChatonLM n'interprète jamais le HTML brut des réponses.
export default function rehypeRaw(): never {
  throw new Error("raw HTML is not supported");
}
