// Atelier (propre à ChatonLM) : petits éléments communs aux pages (pastilles
// d'état et de stratégie, marque de l'état d'une partie). Couleurs du thème
// seulement : aplat roux avec texte encre pour ce qui travaille, vert doux
// pour ce qui est fini, rouge des erreurs ; l'état est toujours dit en texte.

import {
  CheckIcon,
  ExclamationTriangleIcon,
  MinusIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import type { BrickState } from "@/bindings/BrickState";
import type { ProjectState } from "@/bindings/ProjectState";
import type { Strategy } from "@/bindings/Strategy";
import { useT } from "@/i18n";

const PILL =
  "inline-flex flex-none items-center rounded-full px-2 py-0.5 text-xs font-medium";

/** Teinte de la pastille d'un état (deux chaînes exclusives, jamais mêlées). */
const STATE_TONES: Record<ProjectState, string> = {
  queued:
    "border border-neutral-300 text-neutral-600 dark:border-neutral-600 dark:text-neutral-300",
  running: "bg-accent text-neutral-900",
  waiting: "bg-accent text-neutral-900",
  yielding:
    "border border-neutral-300 text-neutral-600 dark:border-neutral-600 dark:text-neutral-300",
  paused:
    "bg-neutral-100 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200",
  done: "bg-green-100 text-green-800 dark:bg-green-800 dark:text-green-100",
  failed: "bg-red-50 text-red-700 dark:bg-red-950/40 dark:text-red-300",
  cancelled:
    "bg-neutral-100 text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300",
};

/** État d'un projet en pastille. */
export function StatePill({ state }: { state: ProjectState }) {
  const t = useT();
  return (
    <span
      data-testid="workshop-state"
      data-state={state}
      className={`${PILL} ${STATE_TONES[state]}`}
    >
      {t(`workshop.states.${state}`)}
    </span>
  );
}

/** Stratégie d'un projet en pastille (« à choisir » tant qu'elle ne l'est pas). */
export function StrategyPill({ strategy }: { strategy: Strategy | null }) {
  const t = useT();
  return (
    <span
      data-testid="workshop-strategy"
      className={`${PILL} bg-neutral-100 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200`}
    >
      {strategy
        ? t(`workshop.strategies.${strategy}`)
        : t("workshop.home.noStrategy")}
    </span>
  );
}

/**
 * Marque ronde de l'état d'une partie (décorative : l'état est dit par le
 * texte de la ligne). La partie au travail a un halo qui pulse trois fois
 * puis se fige (jamais d'animation sans fin sur une page qu'on lit).
 */
export function PartMark({ state }: { state: BrickState }) {
  const base =
    "flex h-5 w-5 flex-none items-center justify-center rounded-full border-2";
  switch (state) {
    case "done":
      return (
        <span
          aria-hidden="true"
          className={`${base} border-green-700 bg-green-100 text-green-800 dark:border-green-400 dark:bg-green-800 dark:text-green-100`}
        >
          <CheckIcon className="h-3 w-3" />
        </span>
      );
    case "toReview":
      return (
        <span
          aria-hidden="true"
          className={`${base} border-accent-fg text-accent-fg`}
        >
          <ExclamationTriangleIcon className="h-3 w-3" />
        </span>
      );
    case "refused":
      return (
        <span
          aria-hidden="true"
          className={`${base} border-red-600 text-red-600 dark:border-red-400 dark:text-red-400`}
        >
          <XMarkIcon className="h-3 w-3" />
        </span>
      );
    case "split":
      return (
        <span
          aria-hidden="true"
          className={`${base} border-neutral-400 text-neutral-500 dark:border-neutral-500 dark:text-neutral-400`}
        >
          <MinusIcon className="h-3 w-3" />
        </span>
      );
    case "running":
    case "checking":
    case "fixing":
      return (
        <span
          aria-hidden="true"
          data-busy=""
          className={`${base} border-accent-fg bg-accent motion-safe:animate-study-pulse`}
        />
      );
    case "pending":
      return (
        <span
          aria-hidden="true"
          className={`${base} border-neutral-300 dark:border-neutral-600`}
        />
      );
  }
}
