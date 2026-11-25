// Frise des phases d'un projet de l'Atelier (propre à ChatonLM), reprise du
// parcours à bulles de l'Étude (LevelPath) mis à l'horizontale : Collect →
// Delivery. Phase faite : coche verte ; en cours : bulle rousse (halo qui
// pulse trois fois puis se fige, seulement si le projet travaille) ; à
// venir : bulle vide. L'état est dit en texte (nom lu), jamais par la seule
// couleur.

import { CheckIcon } from "@heroicons/react/20/solid";
import type { Phase } from "@/bindings/Phase";
import { useT } from "@/i18n";
import { PHASES, type PhaseMark } from "@/lib/workshop";

export function PhaseTimeline({
  marks,
  working,
}: {
  marks: Record<Phase, PhaseMark>;
  /** Le projet calcule : la phase en cours a son halo. */
  working: boolean;
}) {
  const t = useT();
  return (
    <ol
      aria-label={t("workshop.phaseMarks.label")}
      data-testid="workshop-phases"
      className="flex w-full overflow-x-auto pb-1"
    >
      {PHASES.map((phase, index) => {
        const mark = marks[phase];
        const last = index === PHASES.length - 1;
        return (
          <li
            key={phase}
            data-testid="workshop-phase"
            data-phase={phase}
            data-mark={mark}
            aria-current={mark === "current" ? "step" : undefined}
            className="relative flex min-w-[5.5rem] flex-1 flex-col items-center gap-1.5"
          >
            {!last && (
              <span
                aria-hidden="true"
                className={`absolute top-3 left-1/2 h-0.5 w-full ${
                  mark === "done"
                    ? "bg-green-700 dark:bg-green-400"
                    : "bg-neutral-200 dark:bg-neutral-700"
                }`}
              />
            )}
            <Bubble mark={mark} working={working} />
            <span
              aria-hidden="true"
              className={
                mark === "current"
                  ? "px-1 text-center text-xs font-semibold text-neutral-900 dark:text-white"
                  : "px-1 text-center text-xs text-neutral-600 dark:text-neutral-300"
              }
            >
              {t(`workshop.phases.${phase}`)}
            </span>
            <span className="sr-only">
              {t("workshop.phaseMarks.item", {
                phase: t(`workshop.phases.${phase}`),
                status: t(`workshop.phaseMarks.${mark}`),
              })}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

function Bubble({ mark, working }: { mark: PhaseMark; working: boolean }) {
  const base =
    "relative z-10 flex h-6 w-6 flex-none items-center justify-center rounded-full border-2";
  if (mark === "done") {
    return (
      <span
        aria-hidden="true"
        className={`${base} border-green-700 bg-green-100 text-green-800 dark:border-green-400 dark:bg-green-800 dark:text-green-100`}
      >
        <CheckIcon className="h-3.5 w-3.5" />
      </span>
    );
  }
  if (mark === "current") {
    return (
      <span
        aria-hidden="true"
        className={
          working
            ? `${base} border-accent-fg bg-accent motion-safe:animate-study-pulse`
            : `${base} border-accent-fg bg-accent`
        }
      />
    );
  }
  return (
    <span
      aria-hidden="true"
      className={`${base} border-neutral-300 bg-white dark:border-neutral-600 dark:bg-neutral-900`}
    />
  );
}
