import { useT } from "@/i18n";

/**
 * Actions sous l'erreur « Web search requires an Ollama account » (code
 * propre à ChatonLM) : se connecter dans les réglages, ou relancer le message
 * sans recherche web. Compte relié depuis (`signedIn`) : « Retry » remplace
 * « Sign In » et relance avec la recherche web. Sans `onRetry` (pas de
 * message à relancer), seul Sign In est proposé.
 */
export function WebSearchErrorActions({
  onSignIn,
  onRetry,
  signedIn = false,
  retryDisabled = false,
}: {
  onSignIn: () => void;
  /** Relance du message, avec ou sans recherche web. */
  onRetry?: (webSearch: boolean) => void;
  signedIn?: boolean;
  /** Réponse en cours, Ollama perdu : pas de relance. */
  retryDisabled?: boolean;
}) {
  const t = useT();
  const primary =
    "rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";
  return (
    <div className="flex flex-wrap items-center gap-2">
      {signedIn && onRetry ? (
        <button
          type="button"
          onClick={() => onRetry(true)}
          disabled={retryDisabled}
          className={primary}
        >
          {t("common.actions.retry")}
        </button>
      ) : (
        <button type="button" onClick={onSignIn} className={primary}>
          {t("chat.webSearch.signIn")}
        </button>
      )}
      {onRetry && (
        <button
          type="button"
          onClick={() => onRetry(false)}
          disabled={retryDisabled}
          className="rounded-full border border-neutral-300 px-3 py-1 text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          {t("chat.webSearch.withoutWebSearch")}
        </button>
      )}
    </div>
  );
}
