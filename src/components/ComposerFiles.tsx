/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ChatForm.tsx (attachment strip),
 * commit 42e911bc3d05.
 * Modified for ChatonLM: files are staged by the backend (thumbnails through
 * chatonlm://, file kind from the backend instead of the extension); a
 * pulsing chip for each file still being read; the image warning only when
 * the model's capabilities are known; the remove buttons also appear on
 * keyboard focus; the attachments kept from the message being edited are
 * shown first, from the backend (the official app shows them without data:
 * no thumbnail); no undefined classes (pt, scrollbar-hide); texts translated
 * into English, Spanish and French (src/i18n).
 */

import type { AttachmentInfo } from "@/bindings/AttachmentInfo";
import type { StagedFile } from "@/bindings/StagedFile";
import { ImageThumbnail } from "@/components/ImageThumbnail";
import type { FileError, ReadingFile } from "@/hooks/useComposerFiles";
import { useT } from "@/i18n";
import { attachmentImageUrl, stagedImageUrl } from "@/lib/attachments";

function DocumentIcon({ className }: { className: string }) {
  return (
    <svg
      className={className}
      fill="none"
      viewBox="0 0 24 24"
      stroke="currentColor"
      aria-hidden="true"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={1.5}
        d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
      />
    </svg>
  );
}

function RemoveIcon() {
  return (
    <svg
      className="h-4 w-4"
      fill="none"
      viewBox="0 0 24 24"
      stroke="currentColor"
      aria-hidden="true"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={2}
        d="M6 18L18 6M6 6l12 12"
      />
    </svg>
  );
}

/** Pastille d'une pièce jointe, gardée (message modifié) ou préparée. */
interface Chip {
  key: string;
  filename: string;
  isImage: boolean;
  src: string;
  onRemove: () => void;
}

export function ComposerFiles({
  kept = [],
  onRemoveKept = () => {},
  files,
  reading,
  errors,
  visionMissing,
  onRemove,
  onRemoveError,
}: {
  /** Pièces jointes gardées du message modifié, avant les fichiers ajoutés. */
  kept?: AttachmentInfo[];
  onRemoveKept?: (id: number) => void;
  files: StagedFile[];
  reading: ReadingFile[];
  errors: FileError[];
  /** Le modèle n'a pas la vision (capacités connues) : images en rouge. */
  visionMissing: boolean;
  onRemove: (id: string) => void;
  onRemoveError: (index: number) => void;
}) {
  const t = useT();
  if (
    kept.length === 0 &&
    files.length === 0 &&
    reading.length === 0 &&
    errors.length === 0
  ) {
    return null;
  }
  const chips: Chip[] = [
    ...kept.map((attachment) => ({
      key: `kept-${attachment.id}`,
      filename: attachment.filename,
      isImage: attachment.kind === "image",
      src: attachmentImageUrl(attachment.id),
      onRemove: () => onRemoveKept(attachment.id),
    })),
    ...files.map((file) => ({
      key: file.id,
      filename: file.filename,
      isImage: file.kind === "image",
      src: stagedImageUrl(file.id),
      onRemove: () => onRemove(file.id),
    })),
  ];
  return (
    <div
      data-testid="composer-files"
      className="flex w-full gap-2 overflow-x-auto px-3 pb-3"
    >
      {chips.map((file) => {
        const isImage = file.isImage;
        const isUnsupportedImage = visionMissing && isImage;
        return (
          <div
            key={file.key}
            data-testid="composer-file"
            className={`group flex flex-shrink-0 items-center gap-2 rounded-lg px-3 py-2 transition-colors ${
              isUnsupportedImage
                ? "border border-red-200 bg-red-50 dark:border-red-800 dark:bg-red-900/20"
                : "bg-neutral-50 hover:bg-neutral-100 dark:bg-neutral-700/50 dark:hover:bg-neutral-700"
            }`}
          >
            {isImage ? (
              <ImageThumbnail
                src={file.src}
                alt={file.filename}
                className="h-8 w-8 flex-shrink-0 rounded-md object-cover"
              />
            ) : (
              <DocumentIcon className="h-4 w-4 flex-shrink-0 text-neutral-500" />
            )}
            <div className="flex min-w-0 flex-col">
              <span
                className={`max-w-36 truncate text-sm ${isUnsupportedImage ? "text-red-700 dark:text-red-300" : "text-neutral-700 dark:text-neutral-300"}`}
              >
                {file.filename}
              </span>
              {isUnsupportedImage && (
                <span className="text-xs text-red-600 dark:text-red-400">
                  {t("chat.files.noImages")}
                </span>
              )}
            </div>
            <button
              type="button"
              onClick={file.onRemove}
              className="-mr-1 cursor-pointer text-neutral-500 opacity-0 transition-opacity group-hover:opacity-100 hover:text-neutral-600 focus-visible:opacity-100 dark:hover:text-neutral-300"
              aria-label={t("chat.files.remove", { name: file.filename })}
            >
              <RemoveIcon />
            </button>
          </div>
        );
      })}
      {reading.map((file) => (
        <div
          key={`reading-${file.key}`}
          data-testid="composer-file-reading"
          aria-busy="true"
          className="flex flex-shrink-0 animate-pulse items-center gap-2 rounded-lg bg-neutral-50 px-3 py-2 dark:bg-neutral-700/50"
        >
          <DocumentIcon className="h-4 w-4 flex-shrink-0 text-neutral-500" />
          <span className="max-w-36 truncate text-sm text-neutral-500 dark:text-neutral-400">
            {file.filename}
          </span>
        </div>
      ))}
      {errors.map((fileError, index) => (
        <div
          key={`error-${index}`}
          data-testid="composer-file-error"
          className="group flex flex-shrink-0 items-center gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 dark:border-red-800 dark:bg-red-900/20"
        >
          <DocumentIcon className="h-4 w-4 flex-shrink-0 text-red-500" />
          <span className="max-w-[100px] truncate text-sm text-red-700 dark:text-red-300">
            {fileError.filename}
          </span>
          <span className="text-xs text-red-600 dark:text-red-400">
            • {fileError.error ?? t("chat.files.readError")}
          </span>
          <button
            type="button"
            onClick={() => onRemoveError(index)}
            className="-mr-1 ml-auto text-red-400 opacity-0 transition-opacity group-hover:opacity-100 hover:text-red-600 focus-visible:opacity-100 dark:text-red-500 dark:hover:text-red-300"
            aria-label={t("chat.files.remove", { name: fileError.filename })}
          >
            <RemoveIcon />
          </button>
        </div>
      ))}
    </div>
  );
}
