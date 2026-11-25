// Journal d'un projet de l'Atelier (propre à ChatonLM), replié par défaut :
// les événements numérotés, chacun en clair (phase, partie, vérifications,
// question, avertissement…), avec leur heure (et leur date quand le journal
// couvre plusieurs jours). Aucun texte du moteur tel quel : les codes sont
// traduits par la page.

import { ChevronDownIcon } from "@heroicons/react/20/solid";
import { useId, useState } from "react";
import type { WorkshopEvent } from "@/bindings/WorkshopEvent";
import { useT } from "@/i18n";
import { formatDateTime, formatTime } from "@/i18n/format";
import { eventText, spansDays } from "@/lib/workshop";

export function EventLog({
  events,
  name,
}: {
  events: readonly WorkshopEvent[];
  name: (id: number) => string;
}) {
  const t = useT();
  const listId = useId();
  const [open, setOpen] = useState(false);
  const withDate = spansDays(events);
  return (
    <section
      data-testid="workshop-log"
      className="rounded-2xl border border-neutral-200 bg-white dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <h3>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={listId}
          data-testid="workshop-log-toggle"
          onClick={() => setOpen((value) => !value)}
          className="flex w-full items-center justify-between gap-3 rounded-2xl px-4 py-3 text-left text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-50 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          {t("workshop.log.title")}
          <ChevronDownIcon
            aria-hidden="true"
            className={`h-4 w-4 flex-none text-neutral-500 transition-transform motion-reduce:transition-none dark:text-neutral-400 ${open ? "rotate-180" : ""}`}
          />
        </button>
      </h3>
      {open && (
        <div
          id={listId}
          className="max-h-80 overflow-y-auto border-t border-neutral-200 px-4 py-3 dark:border-neutral-700/70"
        >
          {events.length === 0 ? (
            <p className="text-sm text-neutral-500 dark:text-neutral-400">
              {t("workshop.log.empty")}
            </p>
          ) : (
            <ol className="flex flex-col gap-1">
              {events.map((event) => (
                <li
                  key={event.seq}
                  data-testid="workshop-log-entry"
                  data-type={event.kind.type}
                  className="flex gap-3 text-sm"
                >
                  <span className="w-10 flex-none text-right text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
                    {event.seq}
                  </span>
                  <span
                    data-testid="workshop-log-time"
                    className={`${withDate ? "w-36" : "w-16"} flex-none text-xs text-neutral-500 tabular-nums dark:text-neutral-400`}
                  >
                    {withDate
                      ? formatDateTime(t.lang, event.atMs)
                      : formatTime(t.lang, event.atMs)}
                  </span>
                  <span className="min-w-0 text-neutral-800 dark:text-neutral-100">
                    {eventText(t, event, name)}
                  </span>
                </li>
              ))}
            </ol>
          )}
        </div>
      )}
    </section>
  );
}
