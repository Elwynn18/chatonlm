// Ligne d'activité d'un projet de l'Atelier (propre à ChatonLM), avec le
// chaton : ce qui se passe en ce moment, en langage clair, traduit depuis les
// codes du moteur (activité, état du projet, erreur) ; jamais un texte du
// moteur tel quel. Région `role="status"` : annoncée quand la phrase change.
// Sous elle, la jauge : modèle utilisé, temps de travail, mots écrits,
// parties faites ; pas d'estimation du temps restant (fausse précision).

import { useEffect, useState } from "react";
import type { ProjectSummary } from "@/bindings/ProjectSummary";
import type { WorkshopEvent } from "@/bindings/WorkshopEvent";
import type { WorkshopPartSummary } from "@/bindings/WorkshopPartSummary";
import { Logo } from "@/components/Logo";
import { useT } from "@/i18n";
import { shortModelName } from "@/lib/modelNames";
import {
  activityLine,
  durationText,
  elapsedMs,
  isPartDone,
  wordsText,
} from "@/lib/workshop";

export function ActivityLine({
  project,
  events,
  name,
}: {
  project: ProjectSummary;
  events: readonly WorkshopEvent[];
  name: (id: number) => string;
}) {
  const t = useT();
  const failed = project.state === "failed";
  return (
    <div
      data-testid="workshop-activity"
      data-state={project.state}
      className="flex items-center gap-3 rounded-2xl border border-neutral-200 bg-white px-4 py-3 dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <Logo size={32} className="flex-none" />
      <p
        role="status"
        aria-label={t("workshop.activity.label")}
        data-testid="workshop-activity-text"
        className={
          failed
            ? "min-w-0 text-sm font-medium text-red-600 dark:text-red-400"
            : "min-w-0 text-sm font-medium text-neutral-800 dark:text-neutral-100"
        }
      >
        {activityLine(t, project, events, name)}
      </p>
    </div>
  );
}

/**
 * Heure courante, relue toutes les 30 s tant que `active` (temps de
 * travail d'un projet qui calcule) ; figée sinon.
 */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

export function Gauge({
  project,
  events,
  parts,
}: {
  project: ProjectSummary;
  events: readonly WorkshopEvent[];
  parts: readonly WorkshopPartSummary[];
}) {
  const t = useT();
  const working = project.state === "running" || project.state === "yielding";
  const now = useNow(working);
  const leaves = parts.filter((part) => part.state !== "split");
  const done = leaves.filter((part) => isPartDone(part.state)).length;
  const words = leaves.reduce((sum, part) => sum + part.words, 0);
  const model = project.modelUsed || project.model;
  const total = leaves.length;
  const items = [
    {
      key: "model",
      label: t("workshop.gauge.model"),
      value: model ? shortModelName(model) : t("workshop.new.modelAuto"),
    },
    {
      key: "elapsed",
      label: t("workshop.gauge.elapsed"),
      value: durationText(t, elapsedMs(events, working, now)),
    },
    {
      key: "words",
      label: t("workshop.gauge.words"),
      value: wordsText(t, words),
    },
    {
      key: "parts",
      label: t("workshop.gauge.parts"),
      value: t("workshop.gauge.partsValue", { done, total }),
    },
  ];
  return (
    <div className="flex flex-col gap-2 px-1">
      {total > 0 && (
        <span
          role="progressbar"
          aria-label={t("workshop.gauge.parts")}
          aria-valuemin={0}
          aria-valuemax={total}
          aria-valuenow={done}
          aria-valuetext={t("workshop.home.parts", { done, total })}
          data-testid="workshop-gauge-bar"
          className="block h-1.5 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
        >
          <span
            className="block h-full rounded-full bg-accent motion-safe:transition-[width]"
            style={{ width: `${(done / total) * 100}%` }}
          />
        </span>
      )}
      <dl
        aria-label={t("workshop.gauge.label")}
        data-testid="workshop-gauge"
        className="flex flex-wrap gap-x-6 gap-y-2 text-sm"
      >
        {items.map((item) => (
          <div
            key={item.key}
            data-testid={`workshop-gauge-${item.key}`}
            className="flex min-w-0 items-baseline gap-1.5"
          >
            <dt className="text-xs text-neutral-500 dark:text-neutral-400">
              {item.label}
            </dt>
            <dd className="min-w-0 truncate text-neutral-800 tabular-nums dark:text-neutral-100">
              {item.value}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}
