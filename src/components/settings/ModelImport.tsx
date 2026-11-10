// Settings › Models, propre à ChatonLM : carte « Models from Ollama » (lot
// L5b). Reprend, sans les retélécharger, les modèles de l'Ollama intégré aux
// versions 0.9 de ChatonLM (`builtin` : liens physiques sur le même disque,
// copies sinon ; copies seulement si son dossier est aussi celui de l'Ollama
// de la machine) et ceux de l'Ollama de la machine (`machine` : copies,
// rien n'y change). Chaque origine se cherche à la demande (« Look for
// models ») ; chaque dossier trouvé liste ses modèles : prêts (case cochée
// par défaut), déjà là, à retélécharger (format propre à Ollama : bouton
// « Download » de l'équivalent sur Hugging Face, suivi comme au ModelHub ;
// un échec dit sa raison, « Try again »),
// laissés (raison traduite par code), abandonnés après deux échecs (« Try
// again »). Pendant la reprise (événement `model-import`, suivi de toute
// page par useModelImportEvents, route racine) : modèle en
// cours, modèles faits, octets copiés, « Stop » ; ensuite son bilan. Une
// fois l'intégré repris, son ancien dossier peut être effacé : place et
// fichiers, modèles qui disparaîtront nommés avant la confirmation, puis
// bilan du ménage. « Ollama » est nommé ici : c'est le logiciel d'où
// viennent les modèles.

import {
  ArrowPathIcon,
  InformationCircleIcon,
} from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import type { CleanupReport } from "@/bindings/CleanupReport";
import type { ImportCandidate } from "@/bindings/ImportCandidate";
import type { ImportOrigin } from "@/bindings/ImportOrigin";
import type { ImportProgress } from "@/bindings/ImportProgress";
import type { ImportReport } from "@/bindings/ImportReport";
import type { ImportSource } from "@/bindings/ImportSource";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DownloadProgress } from "@/components/hub/DownloadProgress";
import {
  findDownload,
  useDownloadActions,
  useDownloads,
} from "@/hooks/useDownloads";
import {
  modelImportLeftoversKey,
  modelImportScansKey,
  modelImportStateKey,
  useImportState,
} from "@/hooks/useModelImport";
import { type TKey, useT } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import { isPendingDownload } from "@/lib/hub";
import {
  errorMessage,
  modelImportCancel,
  modelImportCleanup,
  modelImportLeftovers,
  modelImportScan,
  modelImportStart,
} from "@/lib/ipc";
import {
  candidateStatusText,
  copiesFiles,
  importPercent,
  isSelectable,
  lacksSpace,
  neededBytes,
  selectedNames,
  selectionKey,
} from "@/lib/modelImport";

const scansKey = modelImportScansKey;
const leftoversKey = modelImportLeftoversKey;
const scanKey = (origin: ImportOrigin) => [...scansKey, origin] as const;

const ORIGINS: readonly ImportOrigin[] = ["builtin", "machine"];

const ORIGIN_TITLES: Record<ImportOrigin, TKey> = {
  builtin: "settings.modelImport.origins.builtin.title",
  machine: "settings.modelImport.origins.machine.title",
};
const ORIGIN_DESCRIPTIONS: Record<ImportOrigin, TKey> = {
  builtin: "settings.modelImport.origins.builtin.description",
  machine: "settings.modelImport.origins.machine.description",
};
const REPORT_TITLES: Record<ImportOrigin, TKey> = {
  builtin: "settings.modelImport.report.titleBuiltin",
  machine: "settings.modelImport.report.titleMachine",
};

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const SMALL_MUTED =
  "text-xs break-words text-neutral-600 dark:text-neutral-300";
const ERROR = "text-xs break-words text-red-600 dark:text-red-400";
const BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const PRIMARY =
  "inline-flex flex-none items-center gap-1.5 rounded-lg bg-neutral-900 px-3 py-1 text-sm/6 font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-800";
const CHIP =
  "inline-flex items-center rounded-md bg-neutral-100 px-1.5 py-0.5 text-xs font-medium text-neutral-600 dark:bg-neutral-700 dark:text-neutral-200";
const PANEL =
  "rounded-lg border border-neutral-200 p-3 dark:border-neutral-700";

/** Une réponse peut arriver après la sortie de la page. */
function useMounted() {
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  return mounted;
}

/** Carte « Models from Ollama » de Settings › Models. */
export function ModelImport() {
  const t = useT();
  const titleId = useId();
  const state = useImportState();
  const running = state?.running ?? null;
  const busy = running !== null;
  return (
    <section
      aria-labelledby={titleId}
      data-testid="model-import"
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="space-y-4 p-4">
        <div>
          <h2
            id={titleId}
            className="text-sm/6 font-medium text-zinc-950 dark:text-white"
          >
            {t("settings.modelImport.title")}
          </h2>
          <p className={MUTED}>{t("settings.modelImport.description")}</p>
        </div>
        {running && <RunningView progress={running} />}
        {!running && state?.last && <ReportView report={state.last} />}
        <LeftoversView busy={busy} />
        {ORIGINS.map((origin) => (
          <OriginView key={origin} origin={origin} busy={busy} />
        ))}
      </div>
    </section>
  );
}

/** Reprise en cours : modèle, modèles faits, octets, « Stop ». */
function RunningView({ progress }: { progress: ImportProgress }) {
  const t = useT();
  const mounted = useMounted();
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const percent = importPercent(progress);
  const status = progress.model
    ? t("settings.modelImport.running.model", { name: progress.model })
    : t("settings.modelImport.running.preparing");
  const models = t("settings.modelImport.running.models", {
    done: progress.doneModels,
    count: progress.totalModels,
  });
  const bytes =
    progress.totalBytes > 0
      ? t("settings.modelImport.running.bytes", {
          done: formatBytes(t, progress.doneBytes),
          total: formatBytes(t, progress.totalBytes),
        })
      : null;
  const details = [models, bytes, formatPercent(t.lang, percent)]
    .filter(Boolean)
    .join(" · ");

  const stop = async () => {
    setError(null);
    setStopping(true);
    try {
      await modelImportCancel();
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    }
    if (mounted.current) setStopping(false);
  };

  return (
    <div data-testid="model-import-running" className={`${PANEL} space-y-2`}>
      <div className="flex items-start gap-3">
        <p className="flex min-w-0 flex-1 items-center gap-2 text-sm break-words text-neutral-800 dark:text-neutral-100">
          <ArrowPathIcon
            aria-hidden="true"
            className="h-4 w-4 flex-none animate-spin text-neutral-500"
          />
          {/* Le modèle en cours seul est annoncé : les octets changent souvent. */}
          <span aria-live="polite">{status}</span>
        </p>
        <button
          type="button"
          className={BUTTON}
          disabled={stopping}
          onClick={() => void stop()}
        >
          {t("settings.modelImport.running.stop")}
        </button>
      </div>
      <div
        role="progressbar"
        aria-label={t("settings.modelImport.running.label")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={details}
        className="relative h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <div
          className="absolute top-0 left-0 h-full rounded-full bg-accent"
          style={{ width: `${percent}%` }}
        />
      </div>
      <p className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
        {details}
      </p>
      {error && (
        <p role="alert" className={ERROR}>
          {t("settings.modelImport.running.stopError", { message: error })}
        </p>
      )}
    </div>
  );
}

/** Bilan de la dernière reprise. */
function ReportView({ report }: { report: ImportReport }) {
  const t = useT();
  return (
    <div data-testid="model-import-report" className={`${PANEL} space-y-2`}>
      <h3 className="text-sm font-medium text-neutral-900 dark:text-white">
        {t(REPORT_TITLES[report.origin])}
      </h3>
      {report.error && (
        // Échec d'ensemble, dans la langue du moteur.
        <p role="alert" className={ERROR}>
          {report.error}
        </p>
      )}
      {report.cancelled && (
        <p className={SMALL_MUTED}>
          {t("settings.modelImport.report.cancelled")}
        </p>
      )}
      {!report.error &&
        (report.imported.length > 0 ? (
          <div>
            <p className="text-xs text-green-800 dark:text-green-100">
              {t("settings.modelImport.report.imported", {
                count: report.imported.length,
              })}
            </p>
            <ul className="mt-1 space-y-0.5">
              {report.imported.map((model) => (
                <li
                  key={model.id}
                  className="font-mono text-xs break-all text-neutral-700 dark:text-neutral-200"
                >
                  {model.name}
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <p className={SMALL_MUTED}>
            {t("settings.modelImport.report.nothing")}
          </p>
        ))}
      {report.redownload.length > 0 && (
        <div>
          <p className={SMALL_MUTED}>
            {t("settings.modelImport.report.redownload")}
          </p>
          <ul className="mt-1 space-y-2">
            {report.redownload.map((model) => (
              <li key={model.name}>
                <span className="font-mono text-xs break-all text-neutral-700 dark:text-neutral-200">
                  {model.name}
                </span>
                {model.suggestion ? (
                  <SuggestionDownload
                    name={model.name}
                    suggestion={model.suggestion}
                  />
                ) : (
                  <p className={SMALL_MUTED}>
                    {t("settings.modelImport.noSuggestion")}
                  </p>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
      {report.failed.length > 0 && (
        <div>
          <p className={ERROR}>{t("settings.modelImport.report.failed")}</p>
          <ul className="mt-1 space-y-0.5">
            {report.failed.map((model) => (
              <li key={model.name} className={ERROR}>
                {t("settings.modelImport.report.failedItem", {
                  name: model.name,
                  error: model.error,
                })}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/**
 * Équivalent sur Hugging Face d'un modèle à retélécharger : « Download »,
 * puis la progression du ModelHub, puis « Downloaded ».
 */
function SuggestionDownload({
  name,
  suggestion,
}: {
  name: string;
  suggestion: string;
}) {
  const t = useT();
  const { downloads } = useDownloads();
  const { start } = useDownloadActions();
  const download = findDownload(downloads, suggestion);
  // Téléchargement en échec (hors ligne, disque plein) : la raison, et le
  // bouton pour réessayer.
  const failed = download?.state === "failed";
  return (
    <div className="mt-1 space-y-1.5">
      <p className={SMALL_MUTED}>
        {t("settings.modelImport.suggestion", { id: suggestion })}
      </p>
      {download?.state === "done" ? (
        <p className="text-xs text-green-800 dark:text-green-100">
          {t("settings.modelImport.downloaded")}
        </p>
      ) : download && isPendingDownload(download) ? (
        <DownloadProgress download={download} name={suggestion} />
      ) : (
        <button
          type="button"
          className={BUTTON}
          aria-label={t("settings.modelImport.downloadNamed", { name })}
          disabled={start.isPending}
          onClick={() =>
            start.mutate({ model: suggestion, repo: null, label: null })
          }
        >
          {failed
            ? t("settings.modelImport.tryAgain")
            : t("settings.modelImport.download")}
        </button>
      )}
      {failed && !start.error && (
        <p role="alert" className={ERROR}>
          {download.error
            ? t("settings.modelImport.downloadFailed", {
                message: download.error,
              })
            : t("settings.modelImport.downloadFailedPlain")}
        </p>
      )}
      {start.error && (
        <p role="alert" className={ERROR}>
          {t("settings.modelImport.downloadError", {
            message: errorMessage(start.error),
          })}
        </p>
      )}
    </div>
  );
}

/**
 * Ancien dossier de l'Ollama intégré, une fois ses modèles repris : sa place,
 * les modèles qui disparaîtront avec lui, « Delete » (après confirmation),
 * puis le bilan du ménage.
 */
function LeftoversView({ busy }: { busy: boolean }) {
  const t = useT();
  const queryClient = useQueryClient();
  const mounted = useMounted();
  const { data: leftovers } = useQuery({
    queryKey: leftoversKey,
    queryFn: modelImportLeftovers,
  });
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [report, setReport] = useState<CleanupReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  const clean = async () => {
    setConfirming(false);
    setDeleting(true);
    setError(null);
    setReport(null);
    try {
      const done = await modelImportCleanup();
      if (mounted.current) setReport(done);
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    }
    void queryClient.invalidateQueries({ queryKey: leftoversKey });
    void queryClient.invalidateQueries({ queryKey: scansKey });
    if (mounted.current) setDeleting(false);
  };

  return (
    <>
      {leftovers && (
        <div data-testid="model-import-leftovers" className={PANEL}>
          <div className="flex items-start gap-2">
            <InformationCircleIcon
              aria-hidden="true"
              className="mt-0.5 h-4 w-4 flex-none text-neutral-500 dark:text-neutral-400"
            />
            <div className="min-w-0 flex-1 space-y-2">
              <p className="text-sm break-words text-neutral-800 dark:text-neutral-100">
                {t("settings.modelImport.cleanup.notice", {
                  count: leftovers.files,
                  size: formatBytes(t, leftovers.bytes),
                })}{" "}
                {t("settings.modelImport.cleanup.hint")}
              </p>
              <p className="font-mono text-xs break-all text-neutral-600 select-text dark:text-neutral-300">
                {leftovers.dir}
              </p>
              {leftovers.models.length > 0 && (
                <div>
                  <p className={SMALL_MUTED}>
                    {t("settings.modelImport.cleanup.willDisappear")}
                  </p>
                  <ul
                    data-testid="model-import-lost"
                    className="mt-1 space-y-0.5"
                  >
                    {leftovers.models.map((model) => (
                      <li
                        key={model.name}
                        className="font-mono text-xs break-all text-neutral-700 dark:text-neutral-200"
                      >
                        {model.name}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              <button
                type="button"
                className={BUTTON}
                disabled={busy || deleting}
                onClick={() => setConfirming(true)}
              >
                {deleting
                  ? t("settings.modelImport.cleanup.deleting")
                  : t("settings.modelImport.cleanup.delete")}
              </button>
            </div>
          </div>
        </div>
      )}
      {report && (
        <div aria-live="polite" className="space-y-1">
          <p className="text-xs text-green-800 dark:text-green-100">
            {t("settings.modelImport.cleanup.done", {
              count: report.removedFiles,
              size: formatBytes(t, report.removedBytes),
            })}
          </p>
          {report.failed > 0 && (
            <p className={ERROR}>
              {t("settings.modelImport.cleanup.failed", {
                count: report.failed,
              })}
            </p>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className={ERROR}>
          {t("settings.modelImport.cleanup.error", { message: error })}
        </p>
      )}
      {confirming && leftovers && (
        <ConfirmDialog
          title={t("settings.modelImport.cleanup.confirmTitle")}
          message={
            <>
              <p>
                {t("settings.modelImport.cleanup.confirmText", {
                  size: formatBytes(t, leftovers.bytes),
                  dir: leftovers.dir,
                })}
              </p>
              {leftovers.models.length > 0 && (
                <p>
                  {t("settings.modelImport.cleanup.confirmModels", {
                    count: leftovers.models.length,
                    names: leftovers.models
                      .map((model) => model.name)
                      .join(", "),
                  })}
                </p>
              )}
            </>
          }
          confirmLabel={t("settings.modelImport.cleanup.delete")}
          onConfirm={() => void clean()}
          onCancel={() => setConfirming(false)}
        />
      )}
    </>
  );
}

/** Une origine : « Look for models », puis ses dossiers. */
function OriginView({ origin, busy }: { origin: ImportOrigin; busy: boolean }) {
  const t = useT();
  const titleId = useId();
  const [asked, setAsked] = useState(false);
  const scan = useQuery({
    queryKey: scanKey(origin),
    queryFn: () => modelImportScan(origin),
    enabled: asked,
  });
  const looking = asked && scan.isFetching;
  return (
    <section
      aria-labelledby={titleId}
      data-testid={`model-import-${origin}`}
      className="space-y-2 border-t border-neutral-200 pt-4 dark:border-neutral-700"
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h3
            id={titleId}
            className="text-sm/6 font-medium text-zinc-950 dark:text-white"
          >
            {t(ORIGIN_TITLES[origin])}
          </h3>
          <p className={MUTED}>{t(ORIGIN_DESCRIPTIONS[origin])}</p>
        </div>
        <button
          type="button"
          className={BUTTON}
          disabled={looking}
          aria-describedby={titleId}
          onClick={() => {
            if (asked) void scan.refetch();
            else setAsked(true);
          }}
        >
          {looking
            ? t("settings.modelImport.looking")
            : asked
              ? t("settings.modelImport.lookAgain")
              : t("settings.modelImport.look")}
        </button>
      </div>
      {scan.error && (
        <p role="alert" className={ERROR}>
          {t("settings.modelImport.scanError", {
            message: errorMessage(scan.error),
          })}
        </p>
      )}
      {scan.data?.length === 0 && (
        <p className={SMALL_MUTED}>{t("settings.modelImport.noSources")}</p>
      )}
      {scan.data?.map((source) => (
        <SourceView key={source.dir} source={source} busy={busy} />
      ))}
    </section>
  );
}

/** Un dossier d'Ollama : ses modèles, les avis de copie, « Import N models ». */
function SourceView({ source, busy }: { source: ImportSource; busy: boolean }) {
  const t = useT();
  const queryClient = useQueryClient();
  const mounted = useMounted();
  const [unchecked, setUnchecked] = useState<ReadonlySet<string>>(new Set());
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const names = selectedNames(source, unchecked);
  const need = neededBytes(source, names);

  const start = async (list: string[]) => {
    setError(null);
    setStarting(true);
    try {
      await modelImportStart(source.origin, source.dir, list);
      // L'événement suit ; l'état est aussi relu (en cours).
      void queryClient.invalidateQueries({ queryKey: modelImportStateKey });
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    }
    if (mounted.current) setStarting(false);
  };

  const toggle = (name: string, checked: boolean) => {
    const key = selectionKey(source, name);
    setUnchecked((current) => {
      const next = new Set(current);
      if (checked) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  return (
    <div data-testid="model-import-source" className={`${PANEL} space-y-2`}>
      <p className="font-mono text-xs break-all text-neutral-600 select-text dark:text-neutral-300">
        {source.dir}
      </p>
      {!source.readable && (
        <p role="alert" className={ERROR}>
          {t("settings.modelImport.unreadable")}
        </p>
      )}
      {source.shared && (
        <p data-testid="model-import-shared" className={SMALL_MUTED}>
          {t("settings.modelImport.shared")}
        </p>
      )}
      {source.readable && source.models.length === 0 && (
        <p className={SMALL_MUTED}>{t("settings.modelImport.emptyFolder")}</p>
      )}
      {source.models.length > 0 && (
        <ul>
          {source.models.map((candidate) => (
            <CandidateRow
              key={candidate.name}
              candidate={candidate}
              checked={
                isSelectable(candidate) &&
                !unchecked.has(selectionKey(source, candidate.name))
              }
              disabled={busy || starting}
              onToggle={(checked) => toggle(candidate.name, checked)}
              onRetry={() => void start([candidate.name])}
            />
          ))}
        </ul>
      )}
      {names.length > 0 && copiesFiles(source) && (
        <p
          data-testid="model-import-space"
          className={lacksSpace(source, need) ? ERROR : SMALL_MUTED}
        >
          {lacksSpace(source, need) && source.freeBytes !== null
            ? t("settings.modelImport.noSpace", {
                need: formatBytes(t, need),
                free: formatBytes(t, source.freeBytes),
              })
            : source.freeBytes !== null
              ? t("settings.modelImport.copy", {
                  need: formatBytes(t, need),
                  free: formatBytes(t, source.freeBytes),
                })
              : t("settings.modelImport.copyUnknownFree", {
                  need: formatBytes(t, need),
                })}
        </p>
      )}
      {source.models.some(isSelectable) && (
        <button
          type="button"
          className={PRIMARY}
          disabled={names.length === 0 || busy || starting || !source.readable}
          onClick={() => void start(names)}
        >
          {t("settings.modelImport.import", { count: names.length })}
        </button>
      )}
      {error && (
        <p role="alert" className={ERROR}>
          {t("settings.modelImport.startError", { message: error })}
        </p>
      )}
    </div>
  );
}

/** Un modèle d'un dossier d'Ollama. */
function CandidateRow({
  candidate,
  checked,
  disabled,
  onToggle,
  onRetry,
}: {
  candidate: ImportCandidate;
  checked: boolean;
  disabled: boolean;
  onToggle: (checked: boolean) => void;
  onRetry: () => void;
}) {
  const t = useT();
  const id = useId();
  const selectable = isSelectable(candidate);
  const status = candidateStatusText(t, candidate);
  const nameClass =
    "font-mono text-sm break-all text-neutral-900 dark:text-neutral-100";
  return (
    <li
      data-testid="model-import-candidate"
      data-status={candidate.status}
      // Filet entre les lignes, posé sur chacune (les classes « divide » de
      // Tailwind sont refusées par scripts/check-bundle.mjs).
      className="flex items-start gap-3 border-t border-neutral-200 py-2 first:border-t-0 dark:border-neutral-700"
    >
      {selectable ? (
        <input
          id={id}
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(event) => onToggle(event.target.checked)}
          className="mt-1 h-4 w-4 flex-none cursor-pointer accent-accent-fg disabled:cursor-default"
        />
      ) : (
        <span aria-hidden="true" className="h-4 w-4 flex-none" />
      )}
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          {selectable ? (
            <label htmlFor={id} className={`cursor-pointer ${nameClass}`}>
              {candidate.name}
            </label>
          ) : (
            <span className={nameClass}>{candidate.name}</span>
          )}
          {candidate.vision && (
            <span className={CHIP}>{t("settings.modelImport.vision")}</span>
          )}
          <span className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
            {formatBytes(t, candidate.sizeBytes)}
          </span>
        </div>
        {candidate.aliases.length > 0 && (
          <p className={SMALL_MUTED}>
            {t("settings.modelImport.aliases", {
              names: candidate.aliases.join(", "),
            })}
          </p>
        )}
        {status && (
          <p className={candidate.status === "failed" ? ERROR : SMALL_MUTED}>
            {status}
          </p>
        )}
        {candidate.noTemplate && candidate.status !== "skipped" && (
          <p className={SMALL_MUTED}>{t("settings.modelImport.noTemplate")}</p>
        )}
        {candidate.status === "redownload" &&
          (candidate.suggestion ? (
            <SuggestionDownload
              name={candidate.name}
              suggestion={candidate.suggestion}
            />
          ) : (
            <p className={SMALL_MUTED}>
              {t("settings.modelImport.noSuggestion")}
            </p>
          ))}
        {candidate.status === "failed" && (
          <button
            type="button"
            className={`mt-1 ${BUTTON}`}
            disabled={disabled}
            aria-label={t("settings.modelImport.tryAgainNamed", {
              name: candidate.name,
            })}
            onClick={onRetry}
          >
            {t("settings.modelImport.tryAgain")}
          </button>
        )}
      </div>
    </li>
  );
}
