// Réglages de l'Atelier (propre à ChatonLM), repliés sous l'accueil : dossier
// des projets (sélecteur natif ; « par défaut » : Documents/ChatonLM),
// recherche web permise ou non, autonomie proposée aux nouveaux projets.
// Chaque changement est enregistré aussitôt par le moteur ; pendant
// l'enregistrement, les contrôles sont `aria-disabled` (pas `disabled`, qui
// ferait perdre le focus) et ignorent un nouveau changement. « Use the
// default » disparaît une fois appliqué : le focus passe à « Choose… ».

import { ChevronDownIcon } from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";
import type { Autonomy } from "@/bindings/Autonomy";
import type { WorkshopSettingsPatch } from "@/bindings/WorkshopSettingsPatch";
import { BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import {
  useWorkshopSettings,
  workshopSettingsKey,
} from "@/components/workshop/queries";
import { useT } from "@/i18n";
import { errorMessage, workshopPickDir, workshopSetSettings } from "@/lib/ipc";
import { AUTONOMY_LEVELS } from "@/lib/workshop";

const SELECT =
  "h-8 max-w-64 min-w-0 rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 aria-disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const ARIA_DISABLED = "aria-disabled:cursor-default aria-disabled:opacity-50";

export function WorkshopSettingsPanel() {
  const t = useT();
  const ids = { panel: useId(), web: useId(), autonomy: useId() };
  const [open, setOpen] = useState(false);
  const settings = useWorkshopSettings();
  const client = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const chooseRef = useRef<HTMLButtonElement>(null);

  const save = (patch: WorkshopSettingsPatch, after?: () => void) => {
    if (busy) return;
    setError(null);
    setBusy(true);
    workshopSetSettings(patch).then(
      (saved) => {
        setBusy(false);
        client.setQueryData(workshopSettingsKey, saved);
        after?.();
      },
      (cause: unknown) => {
        setBusy(false);
        setError(
          t("workshop.settings.saveError", { message: errorMessage(cause) }),
        );
      },
    );
  };

  const choose = () => {
    if (busy) return;
    setError(null);
    workshopPickDir().then(
      (dir) => {
        if (dir !== null) save({ dir });
      },
      (cause: unknown) =>
        setError(
          t("workshop.settings.saveError", { message: errorMessage(cause) }),
        ),
    );
  };

  const data = settings.data;
  return (
    <section
      data-testid="workshop-settings"
      className="rounded-2xl border border-neutral-200 bg-white dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <h2>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={ids.panel}
          data-testid="workshop-settings-toggle"
          onClick={() => setOpen((value) => !value)}
          className="flex w-full items-center justify-between gap-3 rounded-2xl px-4 py-3 text-left font-rounded text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-50 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          {t("workshop.settings.title")}
          <ChevronDownIcon
            aria-hidden="true"
            className={`h-4 w-4 flex-none text-neutral-500 transition-transform motion-reduce:transition-none dark:text-neutral-400 ${open ? "rotate-180" : ""}`}
          />
        </button>
      </h2>
      {open && (
        <div
          id={ids.panel}
          className="flex flex-col gap-4 border-t border-neutral-200 px-4 py-4 dark:border-neutral-700/70"
        >
          {settings.isError && (
            <p role="alert" className="text-sm text-red-600 dark:text-red-400">
              {t("workshop.settings.loadError", {
                message: errorMessage(settings.error),
              })}
            </p>
          )}
          {data && (
            <>
              <div className="flex flex-col gap-1.5">
                <span className="text-sm font-medium text-neutral-700 dark:text-neutral-300">
                  {t("workshop.settings.folder")}
                </span>
                {data.dir === "" ? (
                  <p className="text-sm text-neutral-600 dark:text-neutral-300">
                    {t("workshop.settings.noFolder")}
                  </p>
                ) : (
                  <p className="flex flex-wrap items-center gap-2 text-sm text-neutral-800 dark:text-neutral-100">
                    <span
                      data-testid="workshop-settings-dir"
                      className="min-w-0 font-mono text-xs break-all"
                    >
                      {data.dir}
                    </span>
                    {!data.custom && (
                      <span className="rounded-full bg-neutral-100 px-2 py-0.5 text-xs text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300">
                        {t("workshop.settings.default")}
                      </span>
                    )}
                  </p>
                )}
                <div className="flex flex-wrap gap-2">
                  <button
                    ref={chooseRef}
                    type="button"
                    className={`${BUTTON} ${ARIA_DISABLED}`}
                    aria-disabled={busy || undefined}
                    onClick={choose}
                    data-testid="workshop-settings-choose"
                  >
                    {t("workshop.settings.choose")}
                  </button>
                  {data.custom && (
                    <button
                      type="button"
                      className={`${QUIET_BUTTON} ${ARIA_DISABLED}`}
                      aria-disabled={busy || undefined}
                      data-testid="workshop-settings-default"
                      onClick={() =>
                        save({ dir: "" }, () => chooseRef.current?.focus())
                      }
                    >
                      {t("workshop.settings.useDefault")}
                    </button>
                  )}
                </div>
              </div>
              <div className="flex items-start gap-3">
                <input
                  id={ids.web}
                  type="checkbox"
                  className={`mt-1 h-4 w-4 flex-none accent-accent-fg ${ARIA_DISABLED}`}
                  checked={data.web}
                  aria-disabled={busy || undefined}
                  data-testid="workshop-settings-web"
                  onChange={(event) => save({ web: event.target.checked })}
                />
                <label htmlFor={ids.web} className="flex flex-col">
                  <span className="text-sm font-medium text-neutral-700 dark:text-neutral-300">
                    {t("workshop.settings.web")}
                  </span>
                  <span className="text-sm text-neutral-500 dark:text-neutral-400">
                    {t("workshop.settings.webHint")}
                  </span>
                </label>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <label
                  htmlFor={ids.autonomy}
                  className="text-sm font-medium text-neutral-700 dark:text-neutral-300"
                >
                  {t("workshop.settings.autonomy")}
                </label>
                <select
                  id={ids.autonomy}
                  className={SELECT}
                  value={data.autonomy}
                  aria-disabled={busy || undefined}
                  data-testid="workshop-settings-autonomy"
                  onChange={(event) =>
                    save({ autonomy: event.target.value as Autonomy })
                  }
                >
                  {AUTONOMY_LEVELS.map((level) => (
                    <option key={level} value={level}>
                      {t(`workshop.autonomy.levels.${level}`)}
                    </option>
                  ))}
                </select>
              </div>
            </>
          )}
          {error && (
            <p role="alert" className="text-sm text-red-600 dark:text-red-400">
              {error}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
