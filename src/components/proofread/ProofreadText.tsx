// Texte d'une correction (page Proofread, propre à ChatonLM) : passages
// terminés avec leurs marques (mots supprimés barrés en rouge discret, mots
// ajoutés surlignés en vert discret ; « deleted » et « inserted » lus par les
// lecteurs d'écran), passage en cours en flux, passages à venir atténués.
//
// Les éléments marqués `data-skip` (mots supprimés, textes cachés) ne font
// pas partie du texte final : sans eux, le texte du conteneur est exactement
// le texte final (position du curseur au clic, copie d'une sélection :
// `finalOffset` et `finalTextOf` de src/lib/proofread.ts). Une espace
// remplacée par une autre (segment « space », invisible) est montrée comme
// le texte gardé.

import type { ReactNode } from "react";
import { useT } from "@/i18n";
import type { PartView } from "@/lib/proofread";

export function ProofreadText({ views }: { views: readonly PartView[] }) {
  const t = useT();
  return views.map((view, index) => {
    let body: ReactNode;
    switch (view.state) {
      case "done":
        body = view.segments.map((segment, i) =>
          segment.kind === "same" || segment.kind === "space" ? (
            segment.text
          ) : segment.kind === "del" ? (
            <del
              key={i}
              data-skip=""
              className="mr-0.5 rounded-sm bg-red-500/10 px-px text-red-600 decoration-red-600/70 dark:bg-red-400/15 dark:text-red-300 dark:decoration-red-300/70"
            >
              <span className="sr-only">{t("proofread.review.deleted")} </span>
              {segment.text}
            </del>
          ) : (
            <ins
              key={i}
              className="rounded-sm bg-green-100 px-px text-green-900 no-underline dark:bg-green-400/20 dark:text-green-100"
            >
              <span data-skip="" className="sr-only">
                {t("proofread.review.inserted")}{" "}
              </span>
              {segment.text}
            </ins>
          ),
        );
        break;
      case "streaming":
        body = (
          <span
            data-state="streaming"
            className="text-neutral-900 dark:text-neutral-100"
          >
            {view.text}
            <span
              aria-hidden="true"
              data-skip=""
              className="ml-0.5 inline-block h-4 w-1.5 animate-pulse rounded-sm bg-neutral-400 align-text-bottom dark:bg-neutral-500"
            />
          </span>
        );
        break;
      case "pending":
        body = (
          <span data-state="pending" className="text-neutral-500">
            {view.text}
          </span>
        );
        break;
    }
    return (
      <span key={index} data-part={index}>
        {view.before}
        {body}
        {view.after}
      </span>
    );
  });
}
