import {
  BANNER_BOX as BOX,
  BANNER_ERROR as ERROR,
  BANNER_SECONDARY as SECONDARY,
} from "@/components/bannerStyles";
import { GhostCatIcon } from "@/components/icons";
import { useKeepChat } from "@/hooks/useEphemeral";
import { useT } from "@/i18n";
import { errorMessage } from "@/lib/ipc";

/**
 * Avis d'une conversation éphémère au-dessus de la zone de saisie (propre à
 * ChatonLM) : elle n'est pas dans l'historique et sera effacée en la
 * quittant ; une fois créée (`chatId`), « Keep » la garde (focus rendu à la
 * zone de saisie).
 */
export function EphemeralBanner({ chatId }: { chatId?: string }) {
  const t = useT();
  const keep = useKeepChat();
  return (
    <div data-testid="ephemeral-banner" className={BOX}>
      <div className="flex items-center justify-between gap-3">
        <span className="flex items-center gap-2">
          <GhostCatIcon className="h-4 w-4 flex-none" />
          {t("chat.ephemeral.notice")}
        </span>
        {chatId !== undefined && (
          <button
            type="button"
            onClick={() =>
              keep.mutate(chatId, {
                // Le bandeau (et ce bouton) disparaît : le focus va à la zone
                // de saisie plutôt que de retomber sur la page.
                onSuccess: () =>
                  document
                    .querySelector<HTMLTextAreaElement>(
                      "[data-tour=composer] textarea",
                    )
                    ?.focus(),
              })
            }
            disabled={keep.isPending}
            className={SECONDARY}
          >
            {t("chat.ephemeral.keep")}
          </button>
        )}
      </div>
      {keep.error && (
        <p role="alert" className={ERROR}>
          {t("chat.ephemeral.keepFailed", {
            error: errorMessage(keep.error),
          })}
        </p>
      )}
    </div>
  );
}
