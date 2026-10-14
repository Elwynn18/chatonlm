import { useT } from "@/i18n";

/**
 * Action sous l'erreur « le modèle répétait sans fin le même texte » (code
 * `loop`, propre à ChatonLM) : relancer le dernier message, comme une
 * modification (la réponse arrêtée est remplacée). Sans `onRetry` (aucun
 * message à relancer), rien n'est proposé.
 */
export function LoopErrorActions({
  onRetry,
  retryDisabled = false,
}: {
  onRetry?: () => void;
  /** Réponse en cours, Ollama perdu : pas de relance. */
  retryDisabled?: boolean;
}) {
  const t = useT();
  if (!onRetry) return null;
  return (
    <button
      type="button"
      onClick={onRetry}
      disabled={retryDisabled}
      className="rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200"
    >
      {t("common.actions.retry")}
    </button>
  );
}
