// Outil Transcription (propre à ChatonLM) : « Summarize » (Synthétiser) une
// transcription en un document de lecture. Modèle « Auto » par défaut (le
// moteur prend le plus grand modèle prêt, sinon celui du chat), ou un modèle
// installé choisi comme dans Study et Proofread ; le document s'écrit en
// flux (Markdown rendu par le composant du chat), « Stop » l'arrête ; fini,
// il s'enregistre en .md, .docx ou .pdf (src/lib/export) et peut être
// réécrit. Un PDF qui refuse des caractères le dit et propose le .docx.

import { DocumentTextIcon } from "@heroicons/react/24/outline";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState, useSyncExternalStore } from "react";
import { StreamingMarkdownContent } from "@/components/StreamingMarkdownContent";
import { BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import {
  synthesisKey,
  useSynthesis,
  useTranscription,
} from "@/components/transcription/queries";
import { useModels } from "@/hooks/useModels";
import { useT } from "@/i18n";
import { formatDateTime } from "@/i18n/format";
import {
  errorMessage,
  transcriptionSynthesisCancel,
  transcriptionSynthesize,
} from "@/lib/ipc";
import { ExportError, saveDocument } from "@/lib/export";
import type { ExportKind } from "@/lib/ipc";
import { shortModelName } from "@/lib/modelNames";

const KINDS: ExportKind[] = ["md", "docx", "pdf"];

const SELECT =
  "h-8 max-w-64 min-w-0 rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";

// Modèle choisi (vide = « Auto ») : gardé pour la séance, d'une
// transcription à l'autre.
let chosenModel = "";
const modelListeners = new Set<() => void>();
function setChosenModel(value: string) {
  chosenModel = value;
  for (const listener of modelListeners) listener();
}
function useChosenModel(): string {
  return useSyncExternalStore(
    (listener) => {
      modelListeners.add(listener);
      return () => modelListeners.delete(listener);
    },
    () => chosenModel,
  );
}

export function SynthesisPanel({ id, title }: { id: string; title: string }) {
  const t = useT();
  const modelId = useId();
  const client = useQueryClient();
  const detail = useTranscription(id);
  const { synthesis, otherRunning } = useSynthesis(id);
  const models = useModels();
  const model = useChosenModel();
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const summary = detail.data?.summary ?? null;
  const running = synthesis?.stage === "running";
  const names = (models.data ?? []).map((m) => m.id);
  const noModel = models.data !== undefined && names.length === 0;

  const start = () => {
    setStarting(true);
    setError(null);
    transcriptionSynthesize(id, model || undefined).then(
      () => {
        setStarting(false);
        void client.invalidateQueries({ queryKey: synthesisKey });
      },
      (cause: unknown) => {
        setStarting(false);
        setError(
          t("transcription.synthesis.error", { message: errorMessage(cause) }),
        );
      },
    );
  };
  const stop = () => {
    transcriptionSynthesisCancel(id).catch((cause: unknown) =>
      setError(
        t("transcription.synthesis.error", { message: errorMessage(cause) }),
      ),
    );
  };

  return (
    <section
      aria-labelledby={`${modelId}-title`}
      data-testid="transcription-synthesis"
      data-stage={synthesis?.stage ?? (summary ? "saved" : "none")}
      className="flex flex-col gap-3 border-t border-neutral-200 pt-5 dark:border-neutral-700"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2
          id={`${modelId}-title`}
          className="flex items-center gap-2 font-rounded text-lg font-medium text-neutral-900 dark:text-white"
        >
          <DocumentTextIcon
            aria-hidden="true"
            className="h-5 w-5 text-accent-fg"
          />
          {t("transcription.synthesis.title")}
        </h2>
        <div className="flex items-center gap-2">
          <label
            htmlFor={modelId}
            className="text-sm text-neutral-700 dark:text-neutral-300"
          >
            {t("transcription.synthesis.model")}
          </label>
          <select
            id={modelId}
            className={SELECT}
            value={model}
            disabled={running}
            onChange={(event) => setChosenModel(event.target.value)}
            data-testid="transcription-synthesis-model"
          >
            <option value="">{t("transcription.synthesis.auto")}</option>
            {model !== "" && !names.includes(model) && (
              <option value={model}>
                {t("transcription.synthesis.notInstalled", {
                  name: shortModelName(model),
                })}
              </option>
            )}
            {names.map((name) => (
              <option key={name} value={name}>
                {shortModelName(name)}
              </option>
            ))}
          </select>
        </div>
      </div>

      {running ? (
        <>
          <div className="flex flex-wrap items-center gap-3">
            <p
              aria-busy="true"
              data-testid="transcription-synthesis-phase"
              className="text-sm text-neutral-600 motion-safe:animate-pulse dark:text-neutral-300"
            >
              {/* Notes d'une longue transcription d'abord (rien à montrer),
                  puis le document, qui s'écrit en flux. */}
              {synthesis.markdown
                ? t("transcription.synthesis.writing")
                : t("transcription.synthesis.reading")}
            </p>
            <button
              type="button"
              className={BUTTON}
              onClick={stop}
              data-testid="transcription-synthesis-stop"
            >
              {t("transcription.synthesis.stop")}
            </button>
          </div>
          {synthesis.markdown && (
            <Document markdown={synthesis.markdown} streaming />
          )}
        </>
      ) : (
        <>
          {!summary && (
            <p className="text-sm text-neutral-600 dark:text-neutral-300">
              {t("transcription.synthesis.description")}
            </p>
          )}
          {synthesis?.stage === "cancelled" && (
            <p
              data-testid="transcription-synthesis-cancelled"
              className="text-sm text-neutral-600 dark:text-neutral-300"
            >
              {t("transcription.synthesis.cancelled")}
            </p>
          )}
          {synthesis?.stage === "error" && synthesis.error && (
            <div
              role="alert"
              data-testid="transcription-synthesis-error"
              data-code={synthesis.error.code}
              className="rounded-xl bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950/40 dark:text-red-200"
            >
              <p className="font-medium">
                {t(`transcription.synthesis.codes.${synthesis.error.code}`)}
              </p>
              <p className="break-words">{synthesis.error.message}</p>
              {synthesis.error.code === "modelNotFound" && (
                <Link to="/hub" className={`mt-2 ${BUTTON}`}>
                  {t("transcription.errors.openHub")}
                </Link>
              )}
            </div>
          )}
          {noModel && (
            <p className="text-sm text-neutral-600 dark:text-neutral-300">
              {t("transcription.synthesis.noModel")}{" "}
              <Link
                to="/hub"
                className="rounded font-medium text-accent-fg underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
              >
                {t("transcription.errors.openHub")}
              </Link>
            </p>
          )}
          {otherRunning && (
            <p className="text-sm text-neutral-600 dark:text-neutral-300">
              {t("transcription.synthesis.busy")}
            </p>
          )}
          <div>
            <button
              type="button"
              className={summary ? QUIET_BUTTON : PRIMARY_BUTTON}
              disabled={starting || noModel || otherRunning}
              onClick={start}
              data-testid="transcription-synthesize"
            >
              {summary
                ? t("transcription.synthesis.again")
                : t("transcription.synthesis.start")}
            </button>
          </div>
          {summary && (
            <>
              <DocumentExports
                markdown={summary.markdown}
                title={title}
                date={summary.at}
              />
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("transcription.synthesis.madeWith", {
                  model: shortModelName(summary.model),
                  date: formatDateTime(t.lang, summary.at),
                })}
              </p>
              <Document markdown={summary.markdown} />
            </>
          )}
        </>
      )}
      {error && (
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </section>
  );
}

function Document({
  markdown,
  streaming = false,
}: {
  markdown: string;
  streaming?: boolean;
}) {
  return (
    <div
      data-testid="transcription-document"
      className="rounded-2xl border border-neutral-200 bg-white px-5 py-4 dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <StreamingMarkdownContent content={markdown} isStreaming={streaming} />
    </div>
  );
}

/** Enregistrer le document en .md, .docx ou .pdf. */
function DocumentExports({
  markdown,
  title,
  date,
}: {
  markdown: string;
  title: string;
  date: number;
}) {
  const t = useT();
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [offerDocx, setOfferDocx] = useState(false);
  const save = (kind: ExportKind) => {
    setSaved(null);
    setError(null);
    setOfferDocx(false);
    // Titre du document : son « # » d'ouverture ; celui de la
    // transcription nomme le fichier. docx et pdfmake sont chargés au
    // premier export de leur format (src/lib/export).
    saveDocument(markdown, kind, t, { name: title, date: new Date(date) }).then(
      (path) => setSaved(path),
      (cause: unknown) => {
        if (cause instanceof ExportError) {
          // Message déjà traduit (caractères refusés par le PDF).
          setError(cause.message);
          setOfferDocx(cause.code === "unsupportedCharacters");
        } else {
          setError(
            t("transcription.page.saveError", { message: errorMessage(cause) }),
          );
        }
      },
    );
  };
  return (
    <div className="flex flex-col gap-2">
      <div
        role="group"
        aria-label={t("transcription.page.saveAs")}
        className="flex flex-wrap items-center gap-1"
      >
        <span className="text-sm text-neutral-500 dark:text-neutral-400">
          {t("transcription.page.saveAs")}
        </span>
        {KINDS.map((kind) => (
          <button
            key={kind}
            type="button"
            className={QUIET_BUTTON}
            onClick={() => save(kind)}
            data-testid={`transcription-document-${kind}`}
          >
            {t(`transcription.formats.${kind}`)}
          </button>
        ))}
      </div>
      {saved && (
        <p
          role="status"
          data-testid="transcription-document-saved"
          className="text-sm break-all text-neutral-600 dark:text-neutral-300"
        >
          {t("transcription.page.saved", { path: saved })}
        </p>
      )}
      {error && (
        <div role="alert" className="flex flex-wrap items-center gap-2">
          <p className="text-sm text-red-600 dark:text-red-400">{error}</p>
          {offerDocx && (
            <button
              type="button"
              className={BUTTON}
              onClick={() => save("docx")}
            >
              {t("transcription.synthesis.useDocx")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
