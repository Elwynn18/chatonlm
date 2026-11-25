// Vue d'un projet de l'Atelier (propre à ChatonLM), l'écran principal
// (conception 109 § 8.1) : en-tête (titre, stratégie, état, Pause / Resume,
// Stop, menu ⋯ : ouvrir le dossier, supprimer avec ou sans les fichiers),
// frise des phases, « I need you » quand une question attend, plan à gauche
// et partie choisie à droite, ligne d'activité avec le chaton, jauge,
// rapport à la fin, journal repliable, tâche. Tenue à jour par
// « workshop-event » (gabarit de l'outil, workshop.lazy.tsx).

import {
  ArrowLeftIcon,
  ChevronDownIcon,
  EllipsisHorizontalIcon,
  PauseIcon,
  PlayIcon,
  StopIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  type FocusEvent,
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ProjectDetail } from "@/bindings/ProjectDetail";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import { ActivityLine, Gauge } from "@/components/workshop/ActivityLine";
import { EventLog } from "@/components/workshop/EventLog";
import { NeedYou } from "@/components/workshop/NeedYou";
import { PartPanel } from "@/components/workshop/PartPanel";
import { PhaseTimeline } from "@/components/workshop/PhaseTimeline";
import { PlanPanel } from "@/components/workshop/PlanPanel";
import {
  useWorkshopCriteria,
  useWorkshopEvents,
  useWorkshopParts,
  useWorkshopProject,
  workshopEventsKey,
  workshopListKey,
  workshopPartsKey,
  workshopProjectKey,
} from "@/components/workshop/queries";
import { ReportView } from "@/components/workshop/ReportView";
import { StatePill, StrategyPill } from "@/components/workshop/ui";
import { useT } from "@/i18n";
import {
  errorMessage,
  isAppError,
  workshopCancel,
  workshopDelete,
  workshopOpenFile,
  workshopOpenFolder,
  workshopPause,
  workshopResume,
} from "@/lib/ipc";
import {
  isFinished,
  isPartBusy,
  lastPlan,
  partName,
  phaseMarks,
} from "@/lib/workshop";

/** Boutons de l'en-tête : `aria-disabled` pendant un appel (le focus reste). */
const HEADER_BUTTON = `${BUTTON} aria-disabled:cursor-default aria-disabled:opacity-50`;

/**
 * Vue du projet `id` ; null (adresse dont le numéro n'en est pas un) :
 * « projet introuvable », sans rien demander au moteur.
 */
export function ProjectView({ id }: { id: number | null }) {
  const t = useT();
  if (id === null) {
    return (
      <Frame>
        <p
          role="alert"
          data-testid="workshop-project-error"
          className="text-sm text-red-600 dark:text-red-400"
        >
          {t("workshop.view.unknown")}
        </p>
      </Frame>
    );
  }
  return <LoadedProject id={id} />;
}

function LoadedProject({ id }: { id: number }) {
  const t = useT();
  const project = useWorkshopProject(id);
  if (project.isPending) return <Frame />;
  if (project.isError) {
    const gone = isAppError(project.error) && project.error.kind === "notFound";
    return (
      <Frame>
        <p
          role="alert"
          data-testid="workshop-project-error"
          className="text-sm text-red-600 dark:text-red-400"
        >
          {gone
            ? t("workshop.view.notFound")
            : t("workshop.view.loadError", {
                message: errorMessage(project.error),
              })}
        </p>
      </Frame>
    );
  }
  return (
    <Frame>
      <Project detail={project.data} />
    </Frame>
  );
}

function Frame({ children }: { children?: ReactNode }) {
  const t = useT();
  return (
    <main
      data-testid="workshop-project"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-4 p-6 pt-2">
        <Link
          to="/workshop"
          className={`${QUIET_BUTTON} self-start`}
          data-testid="workshop-back"
        >
          <ArrowLeftIcon aria-hidden="true" className="h-4 w-4" />
          {t("workshop.view.back")}
        </Link>
        {children}
      </div>
    </main>
  );
}

function Project({ detail }: { detail: ProjectDetail }) {
  const t = useT();
  const summary = detail.project;
  const id = summary.id;
  const events = useWorkshopEvents(id);
  const parts = useWorkshopParts(id);
  const criteria = useWorkshopCriteria(id);
  const [selected, setSelected] = useState<number | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const open = (action: () => Promise<void>) => {
    setOpenError(null);
    action().catch((cause: unknown) =>
      setOpenError(
        t("workshop.view.actionError", { message: errorMessage(cause) }),
      ),
    );
  };
  const eventList = useMemo(() => events.data ?? [], [events.data]);
  const partList = useMemo(() => parts.data ?? [], [parts.data]);
  const plan = useMemo(() => lastPlan(eventList), [eventList]);
  const titles = useMemo(
    () => new Map(partList.map((part) => [part.id, part.title])),
    [partList],
  );
  const name = useCallback(
    (partId: number) => partName(t, plan, partId, titles),
    [t, plan, titles],
  );
  const working = summary.state === "running";
  const marks = useMemo(
    () => phaseMarks(eventList, summary.phase, summary.state === "done"),
    [eventList, summary.phase, summary.state],
  );
  // Partie montrée : celle cliquée (si elle existe encore), sinon celle au
  // travail, sinon la première.
  const known = selected !== null && partList.some((p) => p.id === selected);
  const shown = known
    ? selected
    : (partList.find((part) => isPartBusy(part.state))?.id ??
      partList[0]?.id ??
      null);

  return (
    <div
      data-testid="workshop-project-view"
      data-project={id}
      data-state={summary.state}
      className="flex flex-col gap-4"
    >
      <Header detail={detail} />
      <PhaseTimeline marks={marks} working={working} />
      {detail.question && (
        <NeedYou
          key={detail.question.id}
          projectId={id}
          question={detail.question}
        />
      )}
      <div className="grid grid-cols-1 gap-4 md:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
        <PlanPanel
          parts={partList}
          selected={shown}
          onSelect={setSelected}
          loading={parts.isPending}
        />
        <PartPanel projectId={id} partId={shown} criteria={criteria} />
      </div>
      <ActivityLine project={summary} events={eventList} name={name} />
      <Gauge project={summary} events={eventList} parts={partList} />
      {detail.report && (
        <ReportView
          report={detail.report}
          criteria={criteria}
          onOpenFile={() => open(() => workshopOpenFile(id))}
          onOpenFolder={() => open(() => workshopOpenFolder(id))}
        />
      )}
      {openError && (
        <p
          role="alert"
          data-testid="workshop-open-error"
          className="text-sm text-red-600 dark:text-red-400"
        >
          {openError}
        </p>
      )}
      <EventLog events={eventList} name={name} />
      <Task detail={detail} />
    </div>
  );
}

/** En-tête : titre, stratégie, état, actions. */
function Header({ detail }: { detail: ProjectDetail }) {
  const t = useT();
  const navigate = useNavigate();
  const client = useQueryClient();
  const summary = detail.project;
  const id = summary.id;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"stop" | "delete" | null>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);

  const refresh = () => {
    void client.invalidateQueries({ queryKey: workshopProjectKey(id) });
    void client.invalidateQueries({ queryKey: workshopListKey });
    void client.invalidateQueries({ queryKey: workshopEventsKey(id) });
    void client.invalidateQueries({ queryKey: workshopPartsKey(id) });
  };
  const run = (action: () => Promise<void>, after?: () => void) => {
    setBusy(true);
    setError(null);
    action().then(
      () => {
        setBusy(false);
        if (after) after();
        else refresh();
      },
      (cause: unknown) => {
        setBusy(false);
        setError(
          t("workshop.view.actionError", { message: errorMessage(cause) }),
        );
      },
    );
  };
  const remove = (deleteFiles: boolean) => {
    setConfirm(null);
    run(
      () => workshopDelete(id, deleteFiles),
      () => {
        client.removeQueries({ queryKey: workshopProjectKey(id) });
        void client.invalidateQueries({ queryKey: workshopListKey });
        void navigate({ to: "/workshop" });
      },
    );
  };

  const state = summary.state;
  const canPause =
    state === "queued" ||
    state === "running" ||
    state === "yielding" ||
    state === "waiting";
  const canResume = state === "paused" || state === "failed";
  const canStop = !isFinished(state);

  return (
    <header className="flex flex-col gap-2">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex min-w-0 flex-col gap-1.5">
          <h2
            ref={titleRef}
            tabIndex={-1}
            data-testid="workshop-project-title"
            className="font-rounded text-xl font-semibold break-words text-neutral-900 outline-none dark:text-white"
          >
            {summary.title}
          </h2>
          <div className="flex flex-wrap items-center gap-2">
            <StrategyPill strategy={summary.strategy} />
            <StatePill state={state} />
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {(canPause || canResume) && (
            // Un seul bouton (texte et icône changent) : le focus reste
            // dessus quand l'état passe de « en pause » à « en file ».
            <button
              type="button"
              className={HEADER_BUTTON}
              aria-disabled={busy || undefined}
              data-testid={canPause ? "workshop-pause" : "workshop-resume"}
              onClick={() => {
                if (busy) return;
                run(() => (canPause ? workshopPause(id) : workshopResume(id)));
              }}
            >
              {canPause ? (
                <PauseIcon aria-hidden="true" className="h-4 w-4" />
              ) : (
                <PlayIcon aria-hidden="true" className="h-4 w-4" />
              )}
              {canPause ? t("workshop.view.pause") : t("workshop.view.resume")}
            </button>
          )}
          {canStop && (
            <button
              type="button"
              className={HEADER_BUTTON}
              aria-disabled={busy || undefined}
              data-testid="workshop-stop"
              onClick={() => {
                if (!busy) setConfirm("stop");
              }}
            >
              <StopIcon aria-hidden="true" className="h-4 w-4" />
              {t("workshop.view.stop")}
            </button>
          )}
          <MoreMenu
            disabled={busy}
            onOpenFolder={() =>
              run(
                () => workshopOpenFolder(id),
                () => {},
              )
            }
            onDelete={() => setConfirm("delete")}
          />
        </div>
      </div>
      {error && (
        <p
          role="alert"
          data-testid="workshop-action-error"
          className="text-sm text-red-600 dark:text-red-400"
        >
          {error}
        </p>
      )}
      {confirm === "stop" && (
        <ConfirmDialog
          title={t("workshop.view.stopTitle", { title: summary.title })}
          message={t("workshop.view.stopMessage")}
          confirmLabel={t("workshop.view.stopConfirm")}
          destructive
          initialFocus="cancel"
          onConfirm={() => {
            setConfirm(null);
            // Arrêté, le projet perd ses boutons Pause et Stop : le focus va
            // au titre, pas dans le vide.
            run(
              () => workshopCancel(id),
              () => {
                refresh();
                titleRef.current?.focus();
              },
            );
          }}
          onCancel={() => setConfirm(null)}
        />
      )}
      {confirm === "delete" && (
        <ConfirmDialog
          title={t("workshop.view.deleteTitle", { title: summary.title })}
          message={t("workshop.view.deleteMessage", { folder: summary.folder })}
          initialFocus="cancel"
          // « Supprimer aussi les fichiers » (dernier bouton) en rouge.
          destructive
          actions={[
            {
              label: t("workshop.view.deleteKeep"),
              onSelect: () => remove(false),
            },
            {
              label: t("workshop.view.deleteFiles"),
              onSelect: () => remove(true),
            },
          ]}
          onCancel={() => setConfirm(null)}
        />
      )}
    </header>
  );
}

/**
 * Menu ⋯ : ouvrir le dossier, supprimer. Échap le ferme. Simple bouton qui
 * déplie une liste de boutons (pas de rôle « menu », donc pas
 * d'`aria-haspopup`).
 */
function MoreMenu({
  disabled,
  onOpenFolder,
  onDelete,
}: {
  disabled: boolean;
  onOpenFolder: () => void;
  onDelete: () => void;
}) {
  const t = useT();
  const menuId = useId();
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const close = (focus: boolean) => {
    setOpen(false);
    if (focus) buttonRef.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      close(true);
    }
  };
  const onBlur = (event: FocusEvent<HTMLDivElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
      setOpen(false);
    }
  };
  const item =
    "block w-full rounded-lg px-3 py-1.5 text-left text-sm text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:bg-neutral-100 dark:text-neutral-100 dark:hover:bg-neutral-700 dark:focus-visible:bg-neutral-700";
  return (
    <div className="relative" onKeyDown={onKeyDown} onBlur={onBlur}>
      <button
        ref={buttonRef}
        type="button"
        className={`${HEADER_BUTTON} px-2`}
        aria-disabled={disabled || undefined}
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        aria-label={t("workshop.view.more")}
        title={t("workshop.view.more")}
        data-testid="workshop-more"
        onClick={() => {
          if (!disabled) setOpen((value) => !value);
        }}
      >
        <EllipsisHorizontalIcon aria-hidden="true" className="h-4 w-4" />
      </button>
      {open && (
        <div
          id={menuId}
          className="absolute right-0 z-20 mt-1 w-56 rounded-xl border border-neutral-200 bg-white p-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-800"
        >
          <button
            type="button"
            className={item}
            data-testid="workshop-open-folder"
            onClick={() => {
              close(true);
              onOpenFolder();
            }}
          >
            {t("workshop.view.openFolder")}
          </button>
          <button
            type="button"
            className={`${item} text-red-600 dark:text-red-400`}
            data-testid="workshop-delete"
            onClick={() => {
              close(false);
              onDelete();
            }}
          >
            {t("workshop.view.delete")}
          </button>
        </div>
      )}
    </div>
  );
}

/** La tâche donnée et les pièces jointes, repliées (même bouton que le
 * journal : chevron à droite, pas le triangle natif de `<details>`). */
function Task({ detail }: { detail: ProjectDetail }) {
  const t = useT();
  const bodyId = useId();
  const [open, setOpen] = useState(false);
  return (
    <section
      data-testid="workshop-task-detail"
      className="rounded-2xl border border-neutral-200 bg-white dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <h3>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={bodyId}
          data-testid="workshop-task-toggle"
          onClick={() => setOpen((value) => !value)}
          className="flex w-full items-center justify-between gap-3 rounded-2xl px-4 py-3 text-left text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-50 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          {t("workshop.view.task")}
          <ChevronDownIcon
            aria-hidden="true"
            className={`h-4 w-4 flex-none text-neutral-500 transition-transform motion-reduce:transition-none dark:text-neutral-400 ${open ? "rotate-180" : ""}`}
          />
        </button>
      </h3>
      {open && (
        <div
          id={bodyId}
          className="border-t border-neutral-200 px-4 py-3 dark:border-neutral-700/70"
        >
          <p className="max-h-64 overflow-y-auto text-sm break-words whitespace-pre-wrap text-neutral-800 dark:text-neutral-100">
            {detail.task}
          </p>
          {detail.attachments.length > 0 && (
            <p className="mt-2 text-xs text-neutral-500 dark:text-neutral-400">
              {t("workshop.view.attachments", {
                names: detail.attachments.join(", "),
              })}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
