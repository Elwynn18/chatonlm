import { useCompactChat } from "@/hooks/useCompactChat";
import { useT } from "@/i18n";

/**
 * Action sous l'erreur « contexte plein » (code `contextFull`, propre à
 * ChatonLM) : compacter la conversation tout de suite, comme « Compact now »
 * du panneau de la jauge (même compactage, partagé). Son échec s'affiche
 * en rouge sous le bouton.
 */
export function ContextFullActions({
  chatId,
  disabled = false,
}: {
  chatId: string;
  /** Réponse ou compactage en cours, Ollama perdu, rien à compacter. */
  disabled?: boolean;
}) {
  const t = useT();
  const { compact, compacting, error, hasModel } = useCompactChat(chatId);
  return (
    <div className="flex flex-col items-start gap-2">
      <button
        type="button"
        onClick={() => void compact()}
        disabled={disabled || compacting || !hasModel}
        className="rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200"
      >
        {t(compacting ? "chat.context.compacting" : "chat.context.compactNow")}
      </button>
      {error !== null && (
        <p className="text-xs break-words text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
