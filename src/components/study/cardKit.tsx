// Lecteur de cours (onglet Study) : cadre commun des cartes (sur-titre,
// titre qui prend le focus au changement de carte, actions) et indications
// discrètes des touches (touches elles-mêmes : src/hooks/useCardKeys.ts).

import type { ReactNode } from "react";
import { CARD, EYEBROW } from "@/components/study/ui";
import type { TKey } from "@/i18n";
import { Trans } from "@/i18n/Trans";

/** Indication discrète d'une touche (« Enter to continue »). */
export function KeyHint({
  k,
  params,
}: {
  k: TKey;
  params?: Record<string, string | number>;
}) {
  return (
    <p className="hidden text-xs text-neutral-500 sm:block">
      <Trans
        k={k}
        params={params}
        tags={{
          kbd: (chunks) => (
            <kbd className="rounded border border-neutral-300 px-1 font-sans text-[11px] dark:border-neutral-600">
              {chunks}
            </kbd>
          ),
        }}
      />
    </p>
  );
}

/**
 * Carte du lecteur : sur-titre, titre (focalisé au changement de carte par
 * le lecteur : `data-card-heading`), contenu, puis actions et indication.
 */
export function CardShell({
  eyebrow,
  title,
  testId,
  children,
  actions,
  hint,
  className = "",
}: {
  eyebrow?: ReactNode;
  title: ReactNode;
  testId: string;
  children?: ReactNode;
  actions?: ReactNode;
  hint?: ReactNode;
  className?: string;
}) {
  return (
    <article
      data-testid={testId}
      className={`${CARD} relative overflow-hidden ${className}`}
    >
      {eyebrow && <p className={EYEBROW}>{eyebrow}</p>}
      <h3
        data-card-heading=""
        tabIndex={-1}
        className="mt-1 font-rounded text-xl font-semibold text-neutral-900 outline-none dark:text-white"
      >
        {title}
      </h3>
      {children && <div className="mt-4">{children}</div>}
      {(actions || hint) && (
        <div className="mt-6 flex flex-wrap items-center justify-between gap-3">
          <div className="flex flex-wrap items-center gap-2">{actions}</div>
          {hint}
        </div>
      )}
    </article>
  );
}
