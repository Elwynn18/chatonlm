/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/FileUpload.tsx (drop zone overlay),
 * commit 42e911bc3d05.
 * Modified for ChatonLM: covers the whole window, above dialogs and notices
 * (files are accepted anywhere, from native drops; rendered in document.body,
 * out of the composer's stacking context); the bg-opacity utilities, which do
 * nothing in Tailwind 4, are left out (same look as the official overlay);
 * text translated into English, Spanish and French (src/i18n).
 */

import { DocumentPlusIcon } from "@heroicons/react/24/outline";
import { createPortal } from "react-dom";
import { useT } from "@/i18n";

export function DropOverlay() {
  const t = useT();
  return createPortal(
    <div
      data-testid="drop-overlay"
      className="pointer-events-none fixed inset-0 z-[9999]"
    >
      <div className="absolute inset-0 bg-neutral-500/5 transition-opacity duration-200 dark:bg-neutral-400/10" />
      <div className="absolute inset-0 flex items-center justify-center bg-neutral-50 dark:bg-neutral-900">
        <div className="mx-4 max-w-sm rounded-2xl border border-neutral-200/50 bg-white/90 p-12 text-center shadow-2xl backdrop-blur-xl dark:border-neutral-700 dark:bg-neutral-800/90">
          <DocumentPlusIcon className="mx-auto mb-2 h-8 w-8 text-black dark:text-white" />
          <p className="text-sm leading-relaxed font-medium text-neutral-500 dark:text-neutral-400">
            {t("chat.drop.hint")}
          </p>
        </div>
      </div>
    </div>,
    document.body,
  );
}
