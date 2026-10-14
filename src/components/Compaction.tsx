// Compactage du contexte dans la liste des messages (propre à ChatonLM ;
// l'officiel n'a rien de tel). Séparateur après le dernier message résumé :
// une ligne et un bouton « Context compacted » qui déplie le résumé envoyé au
// modèle (tout l'historique reste affiché). Ligne « Compacting
// conversation… » pendant un compactage automatique, à la place des trois
// points, avec l'allure de « Loading model... » (LoadingModel.tsx).

import {
  ArrowsPointingInIcon,
  ChevronRightIcon,
} from "@heroicons/react/24/outline";
import { useId, useState } from "react";
import type { Compaction } from "@/bindings/Compaction";
import { useT } from "@/i18n";

export function CompactionSeparator({
  compaction,
}: {
  compaction: Compaction;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const summaryId = useId();
  return (
    <div data-testid="compaction" className="mb-8 flex flex-col">
      <div className="flex items-center gap-3">
        <div
          aria-hidden="true"
          className="h-px flex-1 bg-neutral-300 dark:bg-neutral-700"
        />
        <button
          type="button"
          aria-expanded={open}
          aria-controls={open ? summaryId : undefined}
          onClick={() => setOpen((value) => !value)}
          className="flex cursor-pointer items-center gap-1 rounded-full px-2 py-1 text-xs text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200"
        >
          <ChevronRightIcon
            aria-hidden="true"
            className={`h-3 w-3 transition-transform ${open ? "rotate-90" : ""}`}
            strokeWidth={2}
          />
          {t(
            compaction.automatic
              ? "chat.context.compactedAuto"
              : "chat.context.compacted",
          )}
        </button>
        <div
          aria-hidden="true"
          className="h-px flex-1 bg-neutral-300 dark:bg-neutral-700"
        />
      </div>
      {open && (
        <div
          id={summaryId}
          data-testid="compaction-summary"
          className="mt-3 text-sm break-words whitespace-pre-wrap text-neutral-600 dark:text-neutral-300"
        >
          {compaction.summary}
        </div>
      )}
    </div>
  );
}

export function CompactingConversation() {
  const t = useT();
  return (
    <div
      data-testid="compacting"
      className="mt-2 mb-4 flex text-neutral-800 dark:text-neutral-200"
    >
      <span className="relative flex items-center">
        <ArrowsPointingInIcon
          aria-hidden="true"
          className="absolute top-1/2 -left-1 h-5 w-5 -translate-y-1/2"
        />
        <span className="ml-6">{t("chat.context.compactingConversation")}</span>
      </span>
    </div>
  );
}
