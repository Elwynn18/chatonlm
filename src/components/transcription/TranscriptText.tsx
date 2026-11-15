// Outil Transcription (propre à ChatonLM) : texte d'une transcription, par
// paragraphes, horodatage affichable ou masquable ; copie, et
// enregistrement en .txt, .md ou .srt (texte produit par le moteur,
// `transcription_export`, puis boîte « Enregistrer sous »).

import { useId, useState } from "react";
import type { TranscriptionExportFormat } from "@/bindings/TranscriptionExportFormat";
import type { TranscriptionSegment } from "@/bindings/TranscriptionSegment";
import { CopyButton } from "@/components/CopyButton";
import { QUIET_BUTTON } from "@/components/study/ui";
import {
  clock,
  copyText,
  isLong,
  paragraphs,
  utf8,
} from "@/components/transcription/model";
import { useT } from "@/i18n";
import { errorMessage, saveExport, transcriptionExport } from "@/lib/ipc";

const FORMATS: TranscriptionExportFormat[] = ["txt", "md", "srt"];

/** Paragraphes, horodatés à la demande. */
export function Paragraphs({
  segments,
  timestamps,
  live = false,
}: {
  segments: readonly TranscriptionSegment[];
  timestamps: boolean;
  live?: boolean;
}) {
  const long = isLong(segments);
  return (
    <div
      data-testid="transcription-text"
      aria-live={live ? "polite" : undefined}
      className="flex flex-col gap-3 text-base/7 text-neutral-900 dark:text-neutral-100"
    >
      {paragraphs(segments).map((paragraph) => (
        <p key={paragraph.start} data-testid="transcription-paragraph">
          {timestamps && (
            <span
              data-testid="transcription-stamp"
              className="mr-2 text-sm text-neutral-500 tabular-nums dark:text-neutral-400"
            >
              {clock(paragraph.start, long)}
            </span>
          )}
          {paragraph.text}
        </p>
      ))}
    </div>
  );
}

/** Texte fini, avec ses outils (horodatage, copie, enregistrement). */
export function TranscriptText({
  id,
  title,
  segments,
  timestamps,
  onTimestamps,
}: {
  id: string;
  title: string;
  segments: readonly TranscriptionSegment[];
  timestamps: boolean;
  onTimestamps: (shown: boolean) => void;
}) {
  const t = useT();
  const toggleId = useId();
  const [saved, setSaved] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const save = (format: TranscriptionExportFormat) => {
    setSaved(null);
    setSaveError(null);
    transcriptionExport(id, format, timestamps)
      .then((text) => saveExport(utf8(text), format, title))
      .then(
        (path) => setSaved(path),
        (cause: unknown) => setSaveError(errorMessage(cause)),
      );
  };
  return (
    <section
      aria-label={t("transcription.page.text")}
      data-testid="transcription-transcript"
      className="flex flex-col gap-4"
    >
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-neutral-200 pb-3 dark:border-neutral-700">
        <label
          htmlFor={toggleId}
          className="flex cursor-pointer items-center gap-2 text-sm text-neutral-700 dark:text-neutral-300"
        >
          <input
            id={toggleId}
            type="checkbox"
            className="accent-[var(--color-accent-fg)]"
            checked={timestamps}
            onChange={(event) => onTimestamps(event.target.checked)}
            data-testid="transcription-timestamps"
          />
          {t("transcription.page.timestamps")}
        </label>
        <CopyButton
          content={copyText(segments, timestamps)}
          showLabels
          title={t("transcription.page.copy")}
          className="text-neutral-600 hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-300 dark:hover:bg-neutral-800 dark:hover:text-white"
        />
        <div
          role="group"
          aria-label={t("transcription.page.saveAs")}
          className="flex flex-wrap items-center gap-1"
        >
          <span className="text-sm text-neutral-500 dark:text-neutral-400">
            {t("transcription.page.saveAs")}
          </span>
          {FORMATS.map((format) => (
            <button
              key={format}
              type="button"
              className={QUIET_BUTTON}
              onClick={() => save(format)}
              data-testid={`transcription-save-${format}`}
            >
              {t(`transcription.formats.${format}`)}
            </button>
          ))}
        </div>
      </div>
      {saved && (
        <p
          role="status"
          data-testid="transcription-saved"
          className="text-sm break-all text-neutral-600 dark:text-neutral-300"
        >
          {t("transcription.page.saved", { path: saved })}
        </p>
      )}
      {saveError && (
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          {t("transcription.page.saveError", { message: saveError })}
        </p>
      )}
      <Paragraphs segments={segments} timestamps={timestamps} />
    </section>
  );
}
