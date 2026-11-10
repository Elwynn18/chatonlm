// Escargot (propre à ChatonLM) : le modèle tourne en partie sur le
// processeur, faute de mémoire graphique, ou tout entier après l'échec de la
// carte graphique, et ses réponses en sont plus lentes. Montré près du
// sélecteur de modèle de la zone de saisie et dans la liste des modèles
// chargés de l'onglet Models. La raison est dans l'étiquette (lue, et en
// infobulle).

import { useT } from "@/i18n";
import { formatPercent } from "@/i18n/format";
import { shortModelName } from "@/lib/modelNames";
import type { SlowRun } from "@/lib/runningModels";

/** Escargot au trait, à la façon des icônes Heroicons (20 × 20). */
function SnailIcon({ className }: { className: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      {/* Coquille et sa spirale. */}
      <circle cx="9.5" cy="12" r="6.5" />
      <path d="M9.5 12a1.8 1.8 0 1 1 1.8 1.8a3.6 3.6 0 0 1-3.6-3.6" />
      {/* Corps, tête et cornes. */}
      <path d="M1.5 19.5h16a3.5 3.5 0 0 0 3.5-3.5v-3" />
      <path d="M21 13l-2.5-4.5M21 13l2-4.5" />
    </svg>
  );
}

export function SlowModelSnail({
  model,
  run,
  className = "",
}: {
  model: string;
  /** Part sur le processeur, ou repli après l'échec de la carte. */
  run: SlowRun;
  className?: string;
}) {
  const t = useT();
  const name = shortModelName(model);
  const label =
    run.kind === "fallback"
      ? t("chat.slowModelFallback", { model: name })
      : t("chat.slowModel", {
          percent: formatPercent(t.lang, run.cpu),
          model: name,
        });
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      data-testid="slow-model"
      className={`flex flex-none items-center justify-center text-amber-700 dark:text-amber-400 ${className}`}
    >
      <SnailIcon className="h-6 w-6" />
    </span>
  );
}
