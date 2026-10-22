import { useNavigate } from "@tanstack/react-router";
import type { MouseEvent } from "react";
import { GhostCatIcon } from "@/components/icons";
import { useKeepChat } from "@/hooks/useEphemeral";
import { useNewChatStream } from "@/hooks/useSendMessage";
import { useT } from "@/i18n";
import { setNewChatEphemeral, useNewChatEphemeral } from "@/lib/ephemeral";

/**
 * Chaton-fantôme en haut à droite de la conversation (propre à ChatonLM) :
 * - nouvelle conversation : bascule (libellé fixe, `aria-pressed`) du mode
 *   éphémère de la conversation que le premier message va créer (inactive
 *   pendant cet envoi) ;
 * - conversation éphémère : en aplat roux, bouton d'action « Keep this chat »
 *   (un double-clic ne garde pas puis n'ouvre pas une nouvelle conversation) ;
 * - conversation ordinaire : ouvre une nouvelle conversation éphémère.
 */
export function EphemeralButton({
  chatId,
  ephemeral,
}: {
  chatId: string;
  /** La conversation ouverte est éphémère (sans effet pour "new"). */
  ephemeral: boolean;
}) {
  const t = useT();
  const navigate = useNavigate();
  const newChatOn = useNewChatEphemeral();
  const creating = useNewChatStream() !== null;
  const keep = useKeepChat();
  const isNew = chatId === "new";
  const pressed = isNew ? newChatOn : ephemeral;
  const label = t(
    isNew
      ? "chat.ephemeral.toggle"
      : ephemeral
        ? "chat.ephemeral.keepThis"
        : "chat.ephemeral.newEphemeral",
  );
  // Infobulle : ce que fait le clic.
  const tooltip = isNew
    ? t(newChatOn ? "chat.ephemeral.turnOff" : "chat.ephemeral.turnOn")
    : label;

  const onClick = (event: MouseEvent) => {
    // Second clic d'un double-clic : le premier a déjà agi (après Keep, le
    // bouton ouvrirait une nouvelle conversation éphémère).
    if (event.detail > 1) return;
    if (isNew) {
      setNewChatEphemeral(!newChatOn);
    } else if (ephemeral) {
      keep.mutate(chatId);
    } else {
      setNewChatEphemeral(true);
      void navigate({ to: "/c/$chatId", params: { chatId: "new" } });
    }
  };

  return (
    <button
      type="button"
      data-testid="ephemeral-button"
      onClick={onClick}
      disabled={(isNew && creating) || keep.isPending}
      aria-pressed={isNew ? newChatOn : undefined}
      aria-label={label}
      title={tooltip}
      className={
        pressed
          ? "flex h-9 w-9 cursor-pointer items-center justify-center rounded-full bg-accent text-neutral-900 disabled:cursor-default disabled:opacity-50"
          : "flex h-9 w-9 cursor-pointer items-center justify-center rounded-full text-neutral-500 hover:bg-neutral-200 disabled:cursor-default disabled:opacity-50 dark:text-neutral-400 dark:hover:bg-neutral-700/75"
      }
    >
      <GhostCatIcon className="h-5 w-5" />
    </button>
  );
}
