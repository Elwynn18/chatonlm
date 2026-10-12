// Texte traduit qui contient des éléments (lien, touche, gras) : le
// catalogue les marque par des balises « <nom>…</nom> » (ou « <nom/> » pour
// un élément sans texte), remplacées par les rendus donnés. Une balise
// inconnue est écrite telle quelle ; pas d'imbrication.

import { Fragment, type ReactNode } from "react";
import { type TKey, type TParams, useT } from "@/i18n";

/** Rendu d'une balise, avec son texte (vide pour « <nom/> »). */
export type TagRenderers = Record<string, (chunks: string) => ReactNode>;

const TAG = /<(\w+)>(.*?)<\/\1>|<(\w+)\/>/gs;

/** Découpe `text` en morceaux de texte et éléments rendus. */
function renderTags(text: string, tags: TagRenderers): ReactNode[] {
  const parts: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(TAG)) {
    const name = match[1] ?? match[3];
    const render = tags[name];
    if (!render) continue;
    if (match.index > last) parts.push(text.slice(last, match.index));
    parts.push(
      <Fragment key={parts.length}>{render(match[2] ?? "")}</Fragment>,
    );
    last = match.index + match[0].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

export function Trans({
  k,
  params,
  tags,
}: {
  k: TKey;
  params?: TParams;
  tags: TagRenderers;
}) {
  const t = useT();
  return <>{renderTags(t(k, params), tags)}</>;
}
