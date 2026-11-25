// Plan d'un projet de l'Atelier (propre à ChatonLM) : les parties dans
// l'ordre, chacune avec son état, son essai et ses mots ; introduction et
// conclusion « écrites à la fin » ; sous-parties d'un redécoupage sous leur
// parent. La version d'un seul jet est une seule partie (« One-go
// version »). Un clic ouvre la partie dans le panneau de droite ; la partie
// au travail est marquée (halo qui pulse trois fois puis se fige). La ligne
// choisie est un élément actif (aplat roux, texte encre, contour décalé au
// focus) ; son nom est tout son texte (titre, état, essai, « écrite à la
// fin », mots), lu tel quel.

import { useId } from "react";
import type { WorkshopPartSummary } from "@/bindings/WorkshopPartSummary";
import { PartMark } from "@/components/workshop/ui";
import { useT } from "@/i18n";
import { isPartBusy, wordsText } from "@/lib/workshop";

export function PlanPanel({
  parts,
  selected,
  onSelect,
  loading,
}: {
  parts: readonly WorkshopPartSummary[];
  /** Partie montrée à droite. */
  selected: number | null;
  onSelect: (id: number) => void;
  loading: boolean;
}) {
  const t = useT();
  const headingId = useId();
  const direct = parts.length === 1 && parts[0].id === 0;
  return (
    <section
      aria-labelledby={headingId}
      data-testid="workshop-plan"
      className="min-w-0 rounded-2xl border border-neutral-200 bg-white p-4 dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <h3
        id={headingId}
        className="mb-2 text-xs font-medium tracking-wide text-neutral-500 uppercase dark:text-neutral-400"
      >
        {t("workshop.plan.title")}
      </h3>
      {parts.length === 0 ? (
        !loading && (
          <p className="text-sm text-neutral-500 dark:text-neutral-400">
            {t("workshop.plan.empty")}
          </p>
        )
      ) : (
        <ul className="flex flex-col gap-0.5">
          {parts.map((part) => (
            <PartRow
              key={part.id}
              part={part}
              selected={selected === part.id}
              onSelect={onSelect}
            />
          ))}
        </ul>
      )}
      {direct && (
        <p className="mt-3 text-xs text-neutral-500 dark:text-neutral-400">
          {t("workshop.plan.directHint")}
        </p>
      )}
    </section>
  );
}

/** Virgule entendue seulement : sépare les morceaux du nom d'une ligne. */
const SEP = <span className="sr-only">, </span>;

function PartRow({
  part,
  selected,
  onSelect,
}: {
  part: WorkshopPartSummary;
  selected: boolean;
  onSelect: (id: number) => void;
}) {
  const t = useT();
  const title = part.id === 0 ? t("workshop.plan.direct") : part.title;
  const state = t(`workshop.partStates.${part.state}`);
  const busy = isPartBusy(part.state);
  const last = part.kind === "intro" || part.kind === "conclusion";
  return (
    <li className={part.parent !== null ? "pl-6" : undefined}>
      <button
        type="button"
        data-testid="workshop-part-row"
        data-part={part.id}
        data-state={part.state}
        aria-pressed={selected}
        onClick={() => onSelect(part.id)}
        className={
          selected
            ? "flex w-full items-start gap-2.5 rounded-xl bg-accent px-2 py-1.5 text-left text-neutral-900 outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent-fg"
            : "flex w-full items-start gap-2.5 rounded-xl px-2 py-1.5 text-left outline-none hover:bg-neutral-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent-fg dark:hover:bg-neutral-700/60"
        }
      >
        <span className="pt-0.5">
          <PartMark state={part.state} />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span
            className={`truncate text-sm ${busy ? "font-semibold" : ""} ${
              selected
                ? "text-neutral-900"
                : busy
                  ? "text-neutral-900 dark:text-white"
                  : "text-neutral-800 dark:text-neutral-100"
            }`}
          >
            {title}
          </span>
          {SEP}
          <span
            className={
              selected
                ? "flex flex-wrap gap-x-2 text-xs text-neutral-900"
                : "flex flex-wrap gap-x-2 text-xs text-neutral-500 dark:text-neutral-400"
            }
          >
            <span>{state}</span>
            {part.attempt > 0 && (
              <span>
                {SEP}
                {t("workshop.plan.attempt", { number: part.attempt + 1 })}
              </span>
            )}
            {last && (
              <span>
                {SEP}
                {t("workshop.plan.writtenLast")}
              </span>
            )}
          </span>
        </span>
        {SEP}
        <span
          className={
            selected
              ? "flex-none pt-0.5 text-xs text-neutral-900 tabular-nums"
              : "flex-none pt-0.5 text-xs text-neutral-500 tabular-nums dark:text-neutral-400"
          }
        >
          {/* Court à l'écran (« 120/300 »), en mots pour la lecture. */}
          <span aria-hidden="true">
            {part.targetWords !== null
              ? t("workshop.plan.words", {
                  words: part.words,
                  target: part.targetWords,
                })
              : t("workshop.plan.wordsWritten", { words: part.words })}
          </span>
          <span className="sr-only">
            {part.targetWords !== null
              ? t("workshop.part.wordsOf", {
                  words: part.words,
                  target: part.targetWords,
                })
              : wordsText(t, part.words)}
          </span>
        </span>
      </button>
    </li>
  );
}
