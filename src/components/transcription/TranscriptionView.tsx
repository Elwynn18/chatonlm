// Une transcription (outil Transcription, propre à ChatonLM) : titre
// (renommer), fiche (source, chaîne, durée, modèle, langue), supprimer
// après confirmation ; pendant le travail, l'avancement et le texte au fil
// de l'eau (l'état revient au rechargement de la page : `transcription_status`
// rend tous les passages) ; une fois finie, le texte et ses exports, puis le
// document de lecture (Synthétiser). Un échec dit sa raison, d'après son code.
// Finie mais incomplète (fichier lu en partie) : l'avertissement du moteur.
// Étapes et fin annoncées aux lecteurs d'écran (région « status »). Renommée
// pendant le travail, elle montre aussitôt son nouveau titre.

import {
  ArrowLeftIcon,
  ArrowTopRightOnSquareIcon,
  ExclamationTriangleIcon,
  PencilIcon,
  TrashIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import type { TranscriptionItem } from "@/bindings/TranscriptionItem";
import type { TranscriptionJob } from "@/bindings/TranscriptionJob";
import type { TranscriptionRequest } from "@/bindings/TranscriptionRequest";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { PRIMARY_BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import {
  ErrorNotice,
  JobProgress,
} from "@/components/transcription/JobProgress";
import { clock, isRunning } from "@/components/transcription/model";
import {
  refreshTranscription,
  transcriptionKey,
  transcriptionListKey,
  transcriptionStatusKey,
  useTranscription,
  useTranscriptionStatus,
} from "@/components/transcription/queries";
import { SynthesisPanel } from "@/components/transcription/SynthesisPanel";
import {
  Paragraphs,
  TranscriptText,
} from "@/components/transcription/TranscriptText";
import { type TFunction, useT } from "@/i18n";
import { languageName } from "@/i18n/format";
import { AUTO_LANGUAGE } from "@/lib/dictation";
import {
  errorMessage,
  openExternal,
  transcriptionDelete,
  transcriptionRename,
  transcriptionStart,
} from "@/lib/ipc";

const READING = "mx-auto flex w-full max-w-3xl flex-col gap-5 p-6 pt-2";

export function TranscriptionView({ id }: { id: string }) {
  const t = useT();
  const navigate = useNavigate();
  const client = useQueryClient();
  const status = useTranscriptionStatus();
  const detail = useTranscription(id);
  // Horodatage montré : même choix pour le texte au fil de l'eau et fini.
  const [timestamps, setTimestamps] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  /** Titre donné par « Rename » pendant ce passage sur la page. */
  const [renamed, setRenamed] = useState<{ id: string; title: string } | null>(
    null,
  );
  const job = status.data?.job?.id === id ? status.data.job : null;
  const running = isRunning(job);
  const item = detail.data?.item ?? null;

  const retry = (request: TranscriptionRequest) => {
    setRetryError(null);
    transcriptionStart(request).then(
      (next) => {
        void client.invalidateQueries({ queryKey: transcriptionStatusKey });
        void client.invalidateQueries({ queryKey: transcriptionListKey });
        void navigate({
          to: "/transcription/$transcriptionId",
          params: { transcriptionId: next },
        });
      },
      (cause: unknown) =>
        setRetryError(
          t("transcription.form.startError", { message: errorMessage(cause) }),
        ),
    );
  };

  // Pendant le travail, la fiche de la vidéo arrive par l'état (sauf
  // titre donné entre-temps) ; ensuite, le titre enregistré (renommé ou non).
  const title =
    (running
      ? (renamed?.id === id ? renamed.title : null) || job?.title || item?.title
      : item?.title || job?.title) ||
    job?.origin ||
    "";
  const segments = detail.data?.segments ?? [];
  // Échec ou annulation vus pendant ce lancement : l'état de la
  // transcription ; sinon celui de l'historique.
  const ended = job && !running && job.stage !== "done" ? job : null;
  // Finie mais incomplète (fichier lu en partie) : texte du moteur.
  const warning = running
    ? undefined
    : (item?.warning ?? (job?.stage === "done" ? job.warning : undefined));

  return (
    <main
      data-testid="transcription-view"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className={READING}>
        <p
          role="status"
          className="sr-only"
          data-testid="transcription-announce"
        >
          {announce(t, job)}
        </p>
        <Link
          to="/transcription"
          className="inline-flex w-fit items-center gap-1 rounded text-sm text-neutral-600 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-300 dark:hover:text-white"
        >
          <ArrowLeftIcon aria-hidden="true" className="h-4 w-4" />
          {t("transcription.page.back")}
        </Link>

        {(item || job) && (
          <Header
            id={id}
            title={title}
            item={item}
            job={job}
            running={running}
            onRenamed={(next) => setRenamed({ id, title: next })}
            onDeleted={() => void navigate({ to: "/transcription" })}
          />
        )}

        {running && job && (
          <>
            <JobProgress job={job} />
            <section
              aria-label={t("transcription.job.liveText")}
              data-testid="transcription-live"
            >
              <h2 className="mb-2 font-rounded text-sm font-medium text-neutral-500 dark:text-neutral-400">
                {t("transcription.job.liveText")}
              </h2>
              {job.segments.length === 0 ? (
                <p className="text-sm text-neutral-500 dark:text-neutral-400">
                  {t("transcription.job.waiting")}
                </p>
              ) : (
                <Paragraphs
                  segments={job.segments}
                  timestamps={timestamps}
                  live
                />
              )}
            </section>
          </>
        )}

        {ended?.error && (
          <ErrorNotice
            error={ended.error}
            onRetry={() => retry(retryOf(ended))}
          />
        )}
        {ended?.stage === "cancelled" && (
          <div className="flex flex-wrap items-center gap-3">
            <p
              data-testid="transcription-cancelled"
              className="text-sm text-neutral-600 dark:text-neutral-300"
            >
              {t("transcription.job.cancelled")}
            </p>
            <button
              type="button"
              className={QUIET_BUTTON}
              onClick={() => retry(retryOf(ended))}
            >
              {t("transcription.job.retry")}
            </button>
          </div>
        )}
        {retryError && (
          <p role="alert" className="text-sm text-red-600 dark:text-red-400">
            {retryError}
          </p>
        )}

        {!running &&
          item &&
          item.state !== "done" &&
          item.state !== "running" && (
            <p
              data-testid="transcription-partial"
              className="rounded-xl bg-neutral-100 px-3 py-2 text-sm text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200"
            >
              {t(`transcription.page.partial.${item.state}`)}
            </p>
          )}

        {warning && (
          <p
            data-testid="transcription-warning"
            data-code={warning.code}
            className="flex items-start gap-2 rounded-xl bg-neutral-100 px-3 py-2 text-sm text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200"
          >
            <ExclamationTriangleIcon
              aria-hidden="true"
              className="mt-0.5 h-4 w-4 flex-none text-amber-700 dark:text-amber-400"
            />
            {warning.message}
          </p>
        )}

        {!running && item && segments.length > 0 && (
          <>
            <TranscriptText
              id={id}
              title={item.title}
              segments={segments}
              timestamps={timestamps}
              onTimestamps={setTimestamps}
            />
            <SynthesisPanel id={id} title={item.title} />
          </>
        )}

        {!job && detail.isError && (
          <p role="alert" className="text-sm text-red-600 dark:text-red-400">
            {t("transcription.page.loadError", {
              message: errorMessage(detail.error),
            })}
          </p>
        )}
      </div>
    </main>
  );
}

/**
 * Même demande : même source, même modèle, même langue. Sans langue (« Detect
 * automatically », échec avant la détection), « auto » : une demande sans
 * langue prendrait celle de la dictée.
 */
function retryOf(job: TranscriptionJob): TranscriptionRequest {
  return {
    source:
      job.source === "url"
        ? { kind: "url", url: job.origin }
        : { kind: "file", path: job.origin },
    ...(job.model ? { model: job.model } : {}),
    language: job.language || AUTO_LANGUAGE,
  };
}

/**
 * Annonce aux lecteurs d'écran : l'étape en cours (sans les chiffres, qui
 * changent sans cesse), puis la fin ou l'annulation (un échec l'est déjà
 * par son alerte).
 */
function announce(t: TFunction, job: TranscriptionJob | null): string {
  switch (job?.stage) {
    case "preparing":
      return t("transcription.job.preparing");
    case "fetching":
    case "downloading":
    case "decoding":
    case "transcribing":
      return t(`transcription.job.stages.${job.stage}`);
    case "done":
      return t("transcription.job.finished");
    case "cancelled":
      return t("transcription.job.cancelled");
    default:
      return "";
  }
}

function Header({
  id,
  title,
  item,
  job,
  running,
  onRenamed,
  onDeleted,
}: {
  id: string;
  title: string;
  item: TranscriptionItem | null;
  job: TranscriptionJob | null;
  running: boolean;
  onRenamed: (title: string) => void;
  onDeleted: () => void;
}) {
  const t = useT();
  const client = useQueryClient();
  const inputId = useId();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  // Champ ouvert par « Rename » : le focus y va, texte sélectionné.
  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [editing]);
  const channel = item?.channel || job?.channel || "";
  const duration = item?.durationS ?? job?.durationS ?? null;
  const language = item?.language ?? job?.language ?? null;
  const model = item?.model || job?.model || "";
  const page = item?.url || job?.url || "";

  const rename = (event: FormEvent) => {
    event.preventDefault();
    const next = draft.trim();
    if (next === "" || next === title) {
      setEditing(false);
      return;
    }
    setError(null);
    transcriptionRename(id, next).then(
      () => {
        setEditing(false);
        onRenamed(next);
        refreshTranscription(client, id);
      },
      (cause: unknown) =>
        setError(
          t("transcription.page.renameError", { message: errorMessage(cause) }),
        ),
    );
  };
  const remove = () => {
    setDeleting(false);
    setError(null);
    transcriptionDelete(id).then(
      () => {
        client.removeQueries({ queryKey: transcriptionKey(id) });
        void client.invalidateQueries({ queryKey: transcriptionListKey });
        onDeleted();
      },
      (cause: unknown) =>
        setError(
          t("transcription.delete.error", { message: errorMessage(cause) }),
        ),
    );
  };

  return (
    <header className="flex flex-col gap-2">
      {editing ? (
        <form onSubmit={rename} className="flex flex-wrap items-center gap-2">
          <label htmlFor={inputId} className="sr-only">
            {t("transcription.page.renameLabel")}
          </label>
          <input
            id={inputId}
            ref={inputRef}
            value={draft}
            maxLength={300}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") setEditing(false);
            }}
            data-testid="transcription-title-input"
            className="min-w-0 flex-1 rounded-xl border border-neutral-300 bg-white px-3 py-1.5 font-rounded text-xl text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-white"
          />
          <button type="submit" className={PRIMARY_BUTTON}>
            {t("transcription.page.save")}
          </button>
          <button
            type="button"
            className={QUIET_BUTTON}
            onClick={() => setEditing(false)}
          >
            {t("common.actions.cancel")}
          </button>
        </form>
      ) : (
        <div className="flex items-start justify-between gap-3">
          <h2
            data-testid="transcription-title"
            className="min-w-0 font-rounded text-xl font-medium break-words text-neutral-900 dark:text-white"
          >
            {title}
          </h2>
          {item && (
            <div className="flex flex-none gap-1">
              <button
                type="button"
                className={QUIET_BUTTON}
                onClick={() => {
                  setDraft(title);
                  setEditing(true);
                }}
                data-testid="transcription-rename"
              >
                <PencilIcon aria-hidden="true" className="h-4 w-4" />
                {t("transcription.page.rename")}
              </button>
              {!running && (
                <button
                  type="button"
                  className={QUIET_BUTTON}
                  onClick={() => setDeleting(true)}
                  data-testid="transcription-delete"
                >
                  <TrashIcon aria-hidden="true" className="h-4 w-4" />
                  {t("transcription.page.delete")}
                </button>
              )}
            </div>
          )}
        </div>
      )}
      <p
        data-testid="transcription-meta"
        className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-neutral-500 dark:text-neutral-400"
      >
        {channel && <span>{t("transcription.job.by", { channel })}</span>}
        {duration !== null && (
          <span className="tabular-nums">{clock(duration)}</span>
        )}
        {model && <span>{model}</span>}
        {language && language !== "auto" && (
          <span>{languageName(t.lang, language)}</span>
        )}
        {page && (
          <button
            type="button"
            className="inline-flex items-center gap-1 rounded text-accent-fg underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
            onClick={() => void openExternal(page)}
          >
            {t("transcription.page.openSource")}
            <ArrowTopRightOnSquareIcon
              aria-hidden="true"
              className="h-3.5 w-3.5"
            />
          </button>
        )}
      </p>
      {error && (
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
      {deleting && (
        <ConfirmDialog
          title={t("transcription.delete.title", { title })}
          message={t("transcription.delete.message")}
          confirmLabel={t("common.actions.delete")}
          onConfirm={remove}
          onCancel={() => setDeleting(false)}
        />
      )}
    </header>
  );
}
