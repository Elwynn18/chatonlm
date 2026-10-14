// Jauge du contexte d'une conversation (propre à ChatonLM ; l'officiel n'a
// rien de tel) : petit diagramme en secteur près du bouton d'envoi, part
// utilisée en roux. Un clic ouvre un panneau : jetons utilisés sur la taille
// du contexte, pourcentage, règle du compactage automatique et bouton
// « Compact now » (useCompactChat). Échap et un clic dehors le ferment, le
// focus revient au bouton s'il était dans le panneau.

import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { ContextUsage } from "@/bindings/ContextUsage";
import { useCompactChat } from "@/hooks/useCompactChat";
import { useT } from "@/i18n";
import { formatInteger, formatPercent } from "@/i18n/format";
import {
  AUTO_COMPACT_PERCENT,
  FULL_FRACTION,
  sectorPath,
  usedFraction,
  usedPercent,
} from "@/lib/contextGauge";
import { isEscapeKey } from "@/lib/shortcuts";

function Pie({ fraction }: { fraction: number }) {
  return (
    <svg viewBox="0 0 20 20" className="h-5 w-5" aria-hidden="true">
      <circle
        cx="10"
        cy="10"
        r="8"
        strokeWidth="1.5"
        className="fill-neutral-100 stroke-neutral-400 dark:fill-neutral-800"
      />
      {fraction >= FULL_FRACTION ? (
        <circle
          data-testid="context-full-disk"
          cx="10"
          cy="10"
          r="8"
          className="fill-accent"
        />
      ) : (
        fraction > 0 && (
          <path
            data-testid="context-sector"
            d={sectorPath(fraction)}
            className="fill-accent"
          />
        )
      )}
    </svg>
  );
}

export function ContextGauge({
  chatId,
  usage,
  compactDisabled,
}: {
  chatId: string;
  usage: ContextUsage;
  /**
   * « Compact now » impossible : réponse ou compactage automatique en cours,
   * Ollama perdu, rien à compacter.
   */
  compactDisabled: boolean;
}) {
  const t = useT();
  const { compact, compacting, error, hasModel } = useCompactChat(chatId);
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const titleId = useId();
  const fraction = usedFraction(usage);
  const percent = formatPercent(t.lang, usedPercent(usage));
  const disabled = compactDisabled || compacting || !hasModel;

  /** Ferme ; le focus revient au bouton s'il était dans le panneau. */
  const close = useCallback(() => {
    if (panelRef.current?.contains(document.activeElement)) {
      buttonRef.current?.focus();
    }
    setOpen(false);
  }, []);

  // Ouverture : le focus va au panneau (annoncé comme une boîte).
  useEffect(() => {
    if (open) panelRef.current?.focus();
  }, [open]);

  // Clic dehors, Échap (consommée : elle n'arrête pas la réponse).
  useEffect(() => {
    if (!open) return;
    const onMouseDown = (event: MouseEvent) => {
      if (!wrapperRef.current?.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isEscapeKey(event) || event.defaultPrevented) return;
      event.preventDefault();
      buttonRef.current?.focus();
      setOpen(false);
    };
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, close]);

  return (
    <div
      ref={wrapperRef}
      className="relative"
      onBlur={(event) => {
        // Focus parti ailleurs (Tab...), pas dans le vide.
        const to = event.relatedTarget;
        if (open && to && !wrapperRef.current?.contains(to)) setOpen(false);
      }}
    >
      <button
        ref={buttonRef}
        type="button"
        aria-label={t("chat.context.gauge", { percent })}
        title={t("chat.context.gauge", { percent })}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => (open ? close() : setOpen(true))}
        className="flex h-9 w-9 cursor-pointer items-center justify-center rounded-full border border-transparent bg-neutral-200 focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-neutral-700"
      >
        <Pie fraction={fraction} />
      </button>

      {open && (
        <div
          ref={panelRef}
          id={panelId}
          role="dialog"
          aria-labelledby={titleId}
          tabIndex={-1}
          className="absolute right-0 bottom-full mb-2 w-72 rounded-2xl border border-neutral-100 bg-white p-4 text-sm text-neutral-800 shadow-xl shadow-black/5 outline-none dark:border-neutral-600/40 dark:bg-neutral-800 dark:text-white"
        >
          <h2 id={titleId} className="font-medium">
            {t("chat.context.title")}
          </h2>
          <p className="mt-1 text-neutral-700 dark:text-neutral-200">
            {t("chat.context.usage", {
              used: formatInteger(t.lang, usage.used),
              size: formatInteger(t.lang, usage.size),
              percent,
            })}
          </p>
          <p className="mt-2 text-xs text-neutral-500 dark:text-neutral-400">
            {t("chat.context.autoHint", {
              threshold: formatPercent(t.lang, AUTO_COMPACT_PERCENT),
            })}
          </p>
          <button
            type="button"
            onClick={() => {
              // Le bouton se désactive : le focus reste dans le panneau.
              panelRef.current?.focus();
              void compact();
            }}
            disabled={disabled}
            className="mt-3 rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200"
          >
            {t(
              compacting
                ? "chat.context.compacting"
                : "chat.context.compactNow",
            )}
          </button>
          {error !== null && (
            <p
              role="alert"
              className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
            >
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
