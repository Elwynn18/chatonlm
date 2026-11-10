// Actions sous une erreur de la recherche web (propre à ChatonLM ; la
// recherche passe par le relais de ChatonLM) : relais injoignable, Retry
// avec la recherche ; quota du jour atteint ou service en pause, relancer
// sans recherche web ; app trop ancienne, Check for updates. Chaque fois que
// l'envoi peut être refait, « Generate without web search » le relance sans
// elle. Le texte de l'erreur, déjà traduit par le moteur, est au-dessus.

import { useT } from "@/i18n";
import type { WebSearchErrorKind } from "@/lib/webSearch";

const PRIMARY =
  "rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";
const SECONDARY =
  "rounded-full border border-neutral-300 px-3 py-1 text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

export function WebSearchErrorActions({
  kind,
  onRetry,
  onCheckUpdates,
  retryDisabled = false,
}: {
  kind: WebSearchErrorKind;
  /** Relance du message, avec ou sans recherche web ; absente : rien à
   * relancer (page d'une nouvelle conversation). */
  onRetry?: (webSearch: boolean) => void;
  /** Ouvre la carte des mises à jour (Settings › General). */
  onCheckUpdates?: () => void;
  /** Réponse en cours, moteur indisponible : pas de relance. */
  retryDisabled?: boolean;
}) {
  const t = useT();
  // Sans recherche : action principale quand la recherche ne reviendra pas
  // tout de suite (quota, pause), secondaire sinon.
  const withoutFirst = kind === "webSearchLimit" || kind === "webSearchPaused";
  return (
    <div className="flex flex-wrap items-center gap-2">
      {kind === "webSearchOutdated" && onCheckUpdates && (
        <button type="button" onClick={onCheckUpdates} className={PRIMARY}>
          {t("chat.webSearch.checkUpdates")}
        </button>
      )}
      {kind === "webSearchUnavailable" && onRetry && (
        <button
          type="button"
          onClick={() => onRetry(true)}
          disabled={retryDisabled}
          className={PRIMARY}
        >
          {t("common.actions.retry")}
        </button>
      )}
      {onRetry && (
        <button
          type="button"
          onClick={() => onRetry(false)}
          disabled={retryDisabled}
          className={withoutFirst ? PRIMARY : SECONDARY}
        >
          {t("chat.webSearch.withoutWebSearch")}
        </button>
      )}
    </div>
  );
}
