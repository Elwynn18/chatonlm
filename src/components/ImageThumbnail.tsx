/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ImageThumbnail.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: the image comes from the backend through a
 * chatonlm:// address instead of a blob URL built from its bytes; no address
 * (message still being saved) shows an empty square of the same size; not
 * draggable (its address would land in the message field).
 */

import { useState } from "react";

interface ImageThumbnailProps {
  /** Adresse de l'image ; null : carré vide en attendant. */
  src: string | null;
  alt: string;
  className?: string;
}

export function ImageThumbnail({
  src,
  alt,
  className = "w-16 h-16 object-cover rounded-md select-none",
}: ImageThumbnailProps) {
  // Adresse en échec : une nouvelle adresse retente.
  const [failedSrc, setFailedSrc] = useState<string | null>(null);

  if (src === null) {
    return (
      <div
        data-testid="thumbnail-placeholder"
        className={`rounded-md bg-neutral-50 dark:bg-neutral-600/50 ${className}`}
      />
    );
  }

  if (failedSrc === src) {
    return (
      <div
        role="img"
        aria-label={alt}
        className={`flex items-center justify-center rounded-md bg-neutral-50 dark:bg-neutral-600/50 ${className}`}
      >
        <svg
          className="h-4 w-4 text-neutral-500"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          aria-hidden="true"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={1.5}
            d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z"
          />
        </svg>
      </div>
    );
  }

  return (
    <img
      src={src}
      alt={alt}
      draggable={false}
      className={className}
      onError={() => setFailedSrc(src)}
    />
  );
}
