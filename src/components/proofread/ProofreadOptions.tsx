// Menu « Options » de la page Proofread (propre à ChatonLM) : un bouton à
// côté du choix du modèle ouvre un petit panneau de trois interrupteurs,
// enregistrés aussitôt dans les réglages (`proofreadKeepOrder`,
// `proofreadFormatting`, `proofreadDigits`) et lus par le moteur à chaque
// correction : page, raccourci global et VS Code. Échap et un clic dehors le
// ferment ; le focus revient au bouton s'il était dans le panneau.

import { AdjustmentsHorizontalIcon } from "@heroicons/react/20/solid";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { Settings } from "@/bindings/Settings";
import { Switch } from "@/components/settings/Row";
import { useSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { errorMessage } from "@/lib/ipc";
import { isEscapeKey } from "@/lib/shortcuts";

type OptionKey =
  "proofreadKeepOrder" | "proofreadFormatting" | "proofreadDigits";

const OPTIONS = [
  {
    key: "proofreadKeepOrder",
    label: "proofread.options.keepOrder.label",
    description: "proofread.options.keepOrder.description",
  },
  {
    key: "proofreadFormatting",
    label: "proofread.options.formatting.label",
    description: "proofread.options.formatting.description",
  },
  {
    key: "proofreadDigits",
    label: "proofread.options.digits.label",
    description: "proofread.options.digits.description",
  },
] as const satisfies readonly {
  key: OptionKey;
  label: string;
  description: string;
}[];

const BUTTON =
  "inline-flex h-8 flex-none items-center gap-1.5 rounded-lg border border-neutral-300 bg-white px-2.5 text-sm text-neutral-900 outline-none hover:bg-neutral-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100 dark:hover:bg-neutral-700";

export function ProofreadOptions() {
  const t = useT();
  const { settings, setSettings } = useSettings();
  const [open, setOpen] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const titleId = useId();
  const noteId = useId();
  const baseId = useId();

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

  // Clic dehors, Échap (consommée : elle n'arrête pas une correction en
  // cours, écoutée par la page sur window, après document).
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

  const save = (key: OptionKey, value: boolean) => {
    setSaveError(null);
    const patch: Partial<Pick<Settings, OptionKey>> = { [key]: value };
    setSettings(patch).catch((cause: unknown) =>
      setSaveError(errorMessage(cause)),
    );
  };

  return (
    <div
      ref={wrapperRef}
      className="relative flex-none"
      onBlur={(event) => {
        // Focus parti ailleurs (Tab...), pas dans le vide.
        const to = event.relatedTarget;
        if (open && to && !wrapperRef.current?.contains(to)) setOpen(false);
      }}
    >
      <button
        ref={buttonRef}
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        disabled={!settings}
        onClick={() => (open ? close() : setOpen(true))}
        className={BUTTON}
      >
        <AdjustmentsHorizontalIcon
          aria-hidden="true"
          className="h-4 w-4 text-neutral-500 dark:text-neutral-400"
        />
        {t("proofread.options.button")}
      </button>

      {open && settings && (
        <div
          ref={panelRef}
          id={panelId}
          role="dialog"
          aria-labelledby={titleId}
          aria-describedby={noteId}
          tabIndex={-1}
          className="absolute top-full right-0 z-20 mt-2 w-80 max-w-[calc(100vw-2rem)] rounded-2xl border border-neutral-100 bg-white p-4 text-sm text-neutral-800 shadow-xl shadow-black/5 outline-none dark:border-neutral-600/40 dark:bg-neutral-800 dark:text-white"
        >
          <h2 id={titleId} className="font-medium">
            {t("proofread.options.title")}
          </h2>
          <ul className="mt-3 flex flex-col gap-3">
            {OPTIONS.map(({ key, label, description }) => {
              const descriptionId = `${baseId}-${key}`;
              return (
                <li
                  key={key}
                  className="flex items-start justify-between gap-3"
                >
                  <div className="min-w-0">
                    <p className="font-medium">{t(label)}</p>
                    <p
                      id={descriptionId}
                      className="mt-0.5 text-xs/5 text-neutral-500 dark:text-neutral-400"
                    >
                      {t(description)}
                    </p>
                  </div>
                  <Switch
                    checked={settings[key]}
                    onChange={(value) => save(key, value)}
                    label={t(label)}
                    describedBy={descriptionId}
                  />
                </li>
              );
            })}
          </ul>
          <p
            id={noteId}
            className="mt-3 border-t border-neutral-100 pt-3 text-xs/5 text-neutral-500 dark:border-neutral-700 dark:text-neutral-400"
          >
            {t("proofread.options.note")}
          </p>
          {saveError !== null && (
            <p
              role="alert"
              className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
            >
              {t("proofread.options.saveError", { message: saveError })}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
