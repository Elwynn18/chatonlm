// Accueil de l'Atelier (propre à ChatonLM), sur le modèle de l'accueil de la
// Transcription : ce que fait l'outil, le bouton « New project », les
// réglages de l'outil (repliés), puis les projets, du plus récent au plus
// ancien : titre, stratégie, état, phase, parties faites, rang dans la file,
// date. Une carte ouvre la vue du projet.

import { PlusIcon } from "@heroicons/react/20/solid";
import { Link } from "@tanstack/react-router";
import { useId } from "react";
import type { ProjectSummary } from "@/bindings/ProjectSummary";
import { PRIMARY_BUTTON } from "@/components/study/ui";
import {
  useWorkshopList,
  useWorkshopParts,
} from "@/components/workshop/queries";
import { StatePill, StrategyPill } from "@/components/workshop/ui";
import { WorkshopSettingsPanel } from "@/components/workshop/WorkshopSettingsPanel";
import { useT } from "@/i18n";
import { formatTimeAgo } from "@/i18n/format";
import { errorMessage } from "@/lib/ipc";
import { isPartDone } from "@/lib/workshop";

export function WorkshopHome() {
  const t = useT();
  return (
    <main
      data-testid="workshop-home"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-6 pt-2">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <p className="max-w-xl text-sm/6 text-zinc-500 select-none dark:text-zinc-400">
            {t("workshop.home.description")}
          </p>
          <Link
            to="/workshop/new"
            className={PRIMARY_BUTTON}
            data-testid="workshop-new"
          >
            <PlusIcon aria-hidden="true" className="h-4 w-4" />
            {t("workshop.home.newProject")}
          </Link>
        </div>
        <WorkshopSettingsPanel />
        <Projects />
      </div>
    </main>
  );
}

function Projects() {
  const t = useT();
  const headingId = useId();
  const list = useWorkshopList();
  if (list.isPending) return null;
  if (list.isError) {
    return (
      <p role="alert" className="text-sm text-red-600 dark:text-red-400">
        {t("workshop.home.listError", { message: errorMessage(list.error) })}
      </p>
    );
  }
  return (
    <section aria-labelledby={headingId} data-testid="workshop-projects">
      <h2
        id={headingId}
        className="mb-2 px-1 font-rounded text-sm font-medium text-neutral-500 dark:text-neutral-400"
      >
        {t("workshop.home.projects")}
      </h2>
      {list.data.length === 0 ? (
        <p
          data-testid="workshop-empty"
          className="px-1 text-sm text-neutral-500 dark:text-neutral-400"
        >
          {t("workshop.home.empty")}
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {list.data.map((project) => (
            <ProjectCard key={project.id} project={project} />
          ))}
        </ul>
      )}
    </section>
  );
}

function ProjectCard({ project }: { project: ProjectSummary }) {
  const t = useT();
  return (
    <li
      data-testid="workshop-project"
      data-project={project.id}
      data-state={project.state}
      className="rounded-2xl border border-neutral-200 bg-white hover:border-neutral-300 dark:border-neutral-700/70 dark:bg-neutral-800/60 dark:hover:border-neutral-600"
    >
      <Link
        to="/workshop/$projectId"
        params={{ projectId: String(project.id) }}
        draggable={false}
        className="flex flex-col gap-1.5 rounded-2xl px-4 py-3 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
      >
        <span className="flex items-baseline justify-between gap-3">
          <span className="min-w-0 truncate font-medium text-neutral-900 dark:text-white">
            {project.title}
          </span>
          <span className="flex-none text-xs text-neutral-500 dark:text-neutral-400">
            {formatTimeAgo(t, project.updatedAt)}
          </span>
        </span>
        <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-neutral-500 dark:text-neutral-400">
          <StatePill state={project.state} />
          <StrategyPill strategy={project.strategy} />
          {project.phase && project.state !== "done" && (
            <span>
              {t("workshop.home.phase", {
                phase: t(`workshop.phases.${project.phase}`),
              })}
            </span>
          )}
          {project.position !== null && (
            <span data-testid="workshop-project-position">
              {t("workshop.home.position", { position: project.position })}
            </span>
          )}
        </span>
        <PartsGauge id={project.id} />
      </Link>
    </li>
  );
}

/** Jauge simple des parties finies (rien tant qu'il n'y a pas de plan). */
function PartsGauge({ id }: { id: number }) {
  const t = useT();
  const parts = useWorkshopParts(id);
  const leaves = (parts.data ?? []).filter((part) => part.state !== "split");
  if (leaves.length === 0) return null;
  const done = leaves.filter((part) => isPartDone(part.state)).length;
  const total = leaves.length;
  return (
    <span className="flex items-center gap-2">
      <span
        role="progressbar"
        aria-label={t("workshop.home.partsLabel")}
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={done}
        aria-valuetext={t("workshop.home.parts", { done, total })}
        className="block h-1.5 max-w-48 flex-1 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <span
          className="block h-full rounded-full bg-accent"
          style={{ width: `${(done / total) * 100}%` }}
        />
      </span>
      <span
        aria-hidden="true"
        className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400"
      >
        {t("workshop.home.parts", { done, total })}
      </span>
    </span>
  );
}
