// En-tête du panneau : modèle choisi (modèles cloud marqués), nouvelle
// conversation, chaton-fantôme (conversation éphémère ; dans une conversation
// éphémère, « Keep this chat », comme dans l'app) et état de la liaison.
import { PencilSquareIcon } from "@heroicons/react/24/outline";
import type { MouseEvent } from "react";
import { GhostCatIcon } from "@/components/icons";
import { useT } from "@/i18n";
import type { Connection, PanelState } from "../shared/protocol";
import { connectionLabel } from "./labels";
import type { Post } from "./vscodeApi";

/** Pastille de l'état : couleur seulement, le nom est dans aria-label. */
function dotClass(connection: Connection): string {
  switch (connection) {
    case "connected":
      return "bg-green-700 dark:bg-green-400";
    case "connecting":
      return "bg-accent";
    case "error":
    case "incompatible":
      return "bg-red-600";
    default:
      return "bg-neutral-400";
  }
}

const ICON_BUTTON =
  "flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full disabled:cursor-default disabled:opacity-50";
const ICON_BUTTON_IDLE = `${ICON_BUTTON} text-neutral-500 hover:bg-neutral-200 dark:text-neutral-400 dark:hover:bg-neutral-700/75`;
const ICON_BUTTON_ON = `${ICON_BUTTON} bg-accent text-neutral-900`;

export function Header({
  state,
  busy,
  post,
  onNewChat,
  onKeep,
}: {
  state: PanelState;
  busy: boolean;
  post: Post;
  /** Nouvelle conversation demandée (le panneau oublie l'ancienne). */
  onNewChat: (ephemeral: boolean) => void;
  /** Garde la conversation éphémère ouverte. */
  onKeep: () => void;
}) {
  const t = useT();
  const connected = state.connection === "connected";
  const status = connectionLabel(t, state.connection);
  const hasModels = state.models.length > 0;
  // Conversation éphémère déjà commencée : le chaton-fantôme la garde.
  const keepable = state.ephemeral && state.chatId !== null;
  const cloud = new Set(state.cloudModels);

  const newChat = (ephemeral: boolean) => (event: MouseEvent) => {
    // Second clic d'un double-clic : le premier a déjà agi (après Keep, le
    // second ouvrirait une nouvelle conversation éphémère).
    if (event.detail > 1) return;
    onNewChat(ephemeral);
  };

  const onGhost = (event: MouseEvent) => {
    if (event.detail > 1) return;
    if (keepable) onKeep();
    else onNewChat(!state.ephemeral);
  };

  return (
    <header className="flex items-center gap-1 border-b border-neutral-200 px-2 py-1.5 dark:border-neutral-700">
      <select
        aria-label={t("vscode.header.model")}
        title={t("vscode.header.model")}
        className="min-w-0 flex-1 cursor-pointer truncate rounded-lg bg-transparent py-1 pr-7 pl-2 text-sm text-neutral-800 hover:bg-neutral-100 disabled:cursor-default disabled:opacity-50 dark:text-neutral-200 dark:hover:bg-neutral-800"
        value={state.model ?? ""}
        disabled={!connected || busy || !hasModels}
        onChange={(event) =>
          post({ type: "selectModel", model: event.target.value })
        }
      >
        {!hasModels && <option value="">{t("vscode.header.noModels")}</option>}
        {hasModels && state.model === null && (
          <option value="" disabled>
            {t("vscode.header.chooseModel")}
          </option>
        )}
        {state.models.map((name) => (
          <option key={name} value={name}>
            {cloud.has(name)
              ? t("vscode.header.cloudModel", { model: name })
              : name}
          </option>
        ))}
      </select>
      <button
        type="button"
        className={ICON_BUTTON_IDLE}
        aria-label={t("vscode.header.newChat")}
        title={t("vscode.header.newChat")}
        disabled={!connected}
        onClick={newChat(false)}
      >
        <PencilSquareIcon className="h-5 w-5" />
      </button>
      <button
        type="button"
        data-testid="ephemeral-button"
        className={state.ephemeral ? ICON_BUTTON_ON : ICON_BUTTON_IDLE}
        aria-pressed={keepable ? undefined : state.ephemeral}
        aria-label={t(
          keepable ? "vscode.header.keepThis" : "vscode.header.ephemeral",
        )}
        title={t(
          keepable
            ? "vscode.header.keepThis"
            : state.ephemeral
              ? "vscode.header.ephemeralOff"
              : "vscode.header.ephemeralOn",
        )}
        disabled={!connected}
        onClick={onGhost}
      >
        <GhostCatIcon className="h-5 w-5" />
      </button>
      <span
        role="img"
        aria-label={status}
        title={status}
        data-connection={state.connection}
        className={`mx-1.5 h-2.5 w-2.5 shrink-0 rounded-full ${dotClass(state.connection)}`}
      />
    </header>
  );
}
