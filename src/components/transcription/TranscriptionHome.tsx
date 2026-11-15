// Accueil de l'outil Transcription (propre à ChatonLM), sur le modèle de
// l'accueil de Study : en tête, ce que fait l'outil ; la transcription en
// cours (avancement, « Open ») ; le formulaire d'une nouvelle
// transcription ; puis l'historique (ouvrir, supprimer après confirmation).

import { GlobeAltIcon, TrashIcon } from "@heroicons/react/20/solid";
import { FilmIcon } from "@heroicons/react/24/outline";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import type { TranscriptionItem } from "@/bindings/TranscriptionItem";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { BUTTON } from "@/components/study/ui";
import { JobProgress } from "@/components/transcription/JobProgress";
import { clock, isRunning } from "@/components/transcription/model";
import { NewTranscriptionForm } from "@/components/transcription/NewTranscriptionForm";
import {
  transcriptionKey,
  transcriptionListKey,
  useTranscriptions,
  useTranscriptionStatus,
} from "@/components/transcription/queries";
import { useT } from "@/i18n";
import { formatTimeAgo } from "@/i18n/format";
import { errorMessage, transcriptionDelete } from "@/lib/ipc";

export function TranscriptionHome() {
  const t = useT();
  return (
    <main
      data-testid="transcription-home"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-6 pt-2">
        <p className="text-sm/6 text-zinc-500 select-none dark:text-zinc-400">
          {t("transcription.home.description")}
        </p>
        <RunningCard />
        <NewTranscriptionForm />
        <History />
      </div>
    </main>
  );
}

/** Transcription en cours : titre, avancement, lien vers sa page. */
function RunningCard() {
  const t = useT();
  const status = useTranscriptionStatus();
  const job = status.data?.job;
  if (!job || !isRunning(job)) return null;
  const headingId = `running-${job.id}`;
  return (
    <section aria-labelledby={headingId} data-testid="transcription-running">
      <div className="mb-2 flex items-baseline justify-between gap-3 px-1">
        <h2
          id={headingId}
          className="min-w-0 truncate font-rounded text-sm font-medium text-neutral-500 dark:text-neutral-400"
        >
          {t("transcription.home.running")}
          {job.title ? ` · ${job.title}` : ""}
        </h2>
        <Link
          to="/transcription/$transcriptionId"
          params={{ transcriptionId: job.id }}
          className={BUTTON}
        >
          {t("transcription.home.open")}
        </Link>
      </div>
      <JobProgress job={job} />
    </section>
  );
}

/** Historique, du plus récent au plus ancien. */
function History() {
  const t = useT();
  const headingId = useId();
  const list = useTranscriptions();
  const client = useQueryClient();
  const [deleting, setDeleting] = useState<TranscriptionItem | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  if (list.isPending) return null;
  if (list.isError) {
    return (
      <p role="alert" className="text-sm text-red-600 dark:text-red-400">
        {t("transcription.home.listError", {
          message: errorMessage(list.error),
        })}
      </p>
    );
  }
  const confirmDelete = () => {
    const item = deleting;
    setDeleting(null);
    if (!item) return;
    setDeleteError(null);
    transcriptionDelete(item.id).then(
      () => {
        client.removeQueries({ queryKey: transcriptionKey(item.id) });
        void client.invalidateQueries({ queryKey: transcriptionListKey });
      },
      (cause: unknown) =>
        setDeleteError(
          t("transcription.delete.error", { message: errorMessage(cause) }),
        ),
    );
  };
  return (
    <section aria-labelledby={headingId} data-testid="transcription-history">
      <h2
        id={headingId}
        className="mb-2 px-1 font-rounded text-sm font-medium text-neutral-500 dark:text-neutral-400"
      >
        {t("transcription.home.history")}
      </h2>
      {list.data.length === 0 ? (
        <p
          data-testid="transcription-empty"
          className="px-1 text-sm text-neutral-500 dark:text-neutral-400"
        >
          {t("transcription.home.empty")}
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {list.data.map((item) => (
            <HistoryRow key={item.id} item={item} onDelete={setDeleting} />
          ))}
        </ul>
      )}
      {deleteError && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {deleteError}
        </p>
      )}
      {deleting && (
        <ConfirmDialog
          title={t("transcription.delete.title", { title: deleting.title })}
          message={t("transcription.delete.message")}
          confirmLabel={t("common.actions.delete")}
          onConfirm={confirmDelete}
          onCancel={() => setDeleting(null)}
        />
      )}
    </section>
  );
}

function HistoryRow({
  item,
  onDelete,
}: {
  item: TranscriptionItem;
  onDelete: (item: TranscriptionItem) => void;
}) {
  const t = useT();
  const SourceIcon = item.source === "url" ? GlobeAltIcon : FilmIcon;
  const sourceLabel =
    item.source === "url"
      ? t("transcription.history.fromAddress")
      : t("transcription.history.fromFile");
  return (
    <li
      data-testid="transcription-item"
      data-state={item.state}
      className="group relative rounded-2xl border border-neutral-200 bg-white hover:border-neutral-300 dark:border-neutral-700/70 dark:bg-neutral-800/60 dark:hover:border-neutral-600"
    >
      <Link
        to="/transcription/$transcriptionId"
        params={{ transcriptionId: item.id }}
        draggable={false}
        className="flex flex-col gap-1 rounded-2xl px-4 py-3 pr-12 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
      >
        <span className="flex items-baseline justify-between gap-3">
          <span className="min-w-0 truncate font-medium text-neutral-900 dark:text-white">
            {item.title}
          </span>
          <span className="flex-none text-xs text-neutral-500 dark:text-neutral-400">
            {formatTimeAgo(t, item.createdAt)}
          </span>
        </span>
        <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-neutral-500 dark:text-neutral-400">
          <span className="inline-flex items-center gap-1">
            <SourceIcon aria-hidden="true" className="h-3.5 w-3.5" />
            {sourceLabel}
          </span>
          {item.durationS !== null && (
            <span className="tabular-nums">{clock(item.durationS)}</span>
          )}
          {item.state !== "done" && (
            <span data-testid="transcription-item-state">
              {t(`transcription.history.states.${item.state}`)}
            </span>
          )}
          {item.hasSummary && (
            <span className="font-medium text-accent-fg">
              {t("transcription.history.hasSummary")}
            </span>
          )}
        </span>
        {item.preview && (
          <span className="line-clamp-2 text-sm text-neutral-600 dark:text-neutral-300">
            {item.preview}
          </span>
        )}
      </Link>
      <button
        type="button"
        className="absolute top-2.5 right-2.5 rounded-full p-1.5 text-neutral-500 opacity-0 outline-none group-focus-within:opacity-100 group-hover:opacity-100 hover:bg-neutral-100 hover:text-neutral-900 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:hidden dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-white"
        aria-label={t("transcription.history.delete", { title: item.title })}
        title={t("transcription.history.delete", { title: item.title })}
        disabled={item.state === "running"}
        onClick={() => onDelete(item)}
      >
        <TrashIcon aria-hidden="true" className="h-4 w-4" />
      </button>
    </li>
  );
}
