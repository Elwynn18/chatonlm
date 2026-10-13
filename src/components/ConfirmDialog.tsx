import {
  type ReactNode,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
} from "react";
import { createPortal } from "react-dom";
import { useT } from "@/i18n";
import { isTabKey } from "@/lib/shortcuts";

/** Bouton d'une confirmation à plusieurs choix. */
export interface DialogAction {
  label: string;
  onSelect: () => void;
}

/**
 * Confirmation dessinée dans la page. Elle remplace window.confirm() de l'app
 * officielle, que WebKitGTK affiche avec le titre « JavaScript - <adresse de
 * la page> ». Comme la boîte native : OK a le focus, Échap annule ; un clic à
 * côté annule aussi. `title` et `confirmLabel` (ModelHub : « Delete {name}? »,
 * bouton « Delete ») sont facultatifs. Avec `actions` (Settings > Model
 * location), ces boutons suivent « Cancel » à la place du bouton de
 * confirmation, le dernier ayant le focus. « OK » et « Cancel » dans la
 * langue affichée. Fermée, elle rend le focus à
 * l'élément qui l'avait à l'ouverture (bouton qui l'a ouverte), s'il est
 * encore dans la page et que le focus n'a pas été mis ailleurs entre-temps
 * (ModelHub : titre de la liste après une suppression).
 */
export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  onConfirm,
  onCancel,
  actions,
}: {
  /** Titre, nom de la boîte pour les lecteurs d'écran. */
  title?: string;
  message: ReactNode;
  /** Bouton de confirmation (« OK ») ; sans effet quand `actions` est donné. */
  confirmLabel?: string;
  onConfirm?: () => void;
  onCancel: () => void;
  actions?: DialogAction[];
}) {
  const t = useT();
  const titleId = useId();
  const messageId = useId();
  const ref = useRef<HTMLDivElement>(null);
  const buttons = actions ?? [
    {
      label: confirmLabel ?? t("chat.dialog.ok"),
      onSelect: () => onConfirm?.(),
    },
  ];

  // Avant que la boîte ne quitte la page (effet de mise en page) : le focus
  // y est encore, sauf si l'action l'a mis ailleurs.
  useLayoutEffect(() => {
    const opener = document.activeElement;
    const dialog = ref.current;
    dialog?.querySelector<HTMLElement>("[data-default]")?.focus();
    return () => {
      const active = document.activeElement;
      const lost =
        active === null ||
        active === document.body ||
        (dialog?.contains(active) ?? false);
      if (lost && opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
      } else if (isTabKey(event)) {
        // Le focus reste dans la boîte.
        const buttons = [
          ...(ref.current?.querySelectorAll<HTMLElement>("button") ?? []),
        ];
        const index = buttons.indexOf(document.activeElement as HTMLElement);
        const step = event.shiftKey ? -1 : 1;
        event.preventDefault();
        buttons[(index + step + buttons.length) % buttons.length]?.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onCancel]);

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/20 p-4 select-none dark:bg-black/50"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
      onContextMenu={(event) => event.preventDefault()}
    >
      <div
        ref={ref}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={title ? titleId : undefined}
        aria-describedby={messageId}
        className="w-full max-w-sm rounded-2xl border border-neutral-200 bg-white p-5 shadow-xl dark:border-neutral-700 dark:bg-neutral-800"
      >
        {title && (
          <h2
            id={titleId}
            className="mb-2 font-rounded text-base font-medium break-words text-neutral-900 dark:text-white"
          >
            {title}
          </h2>
        )}
        <div
          id={messageId}
          className="space-y-2 text-sm text-neutral-800 dark:text-neutral-100"
        >
          {typeof message === "string" ? <p>{message}</p> : message}
        </div>
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-full px-4 py-1.5 text-sm text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-200 dark:hover:bg-neutral-700"
          >
            {t("common.actions.cancel")}
          </button>
          {buttons.map((button, index) => {
            const last = index === buttons.length - 1;
            return (
              <button
                key={button.label}
                type="button"
                data-default={last ? "" : undefined}
                onClick={button.onSelect}
                className={
                  last
                    ? "rounded-full bg-neutral-900 px-4 py-1.5 text-sm text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200"
                    : "rounded-full border border-neutral-300 px-4 py-1.5 text-sm text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:border-neutral-600 dark:text-neutral-100 dark:hover:bg-neutral-700"
                }
              >
                {button.label}
              </button>
            );
          })}
        </div>
      </div>
    </div>,
    document.body,
  );
}
