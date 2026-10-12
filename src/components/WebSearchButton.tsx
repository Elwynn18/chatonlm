/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/WebSearchButton.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named function without forwardRef (no Tab cycle to
 * take part in); Heroicons v2 globe instead of the inline Heroicons v1 path;
 * a labelled toggle button (aria-pressed), of type "button"; texts
 * translated into English, Spanish and French (src/i18n).
 */

import { GlobeAltIcon } from "@heroicons/react/24/outline";
import { useT } from "@/i18n";

export function WebSearchButton({
  isVisible,
  isActive,
  onToggle,
}: {
  isVisible?: boolean;
  isActive: boolean;
  onToggle: () => void;
}) {
  const t = useT();
  if (!isVisible) return null;

  return (
    <button
      type="button"
      aria-label={t("chat.webSearch.label")}
      aria-pressed={isActive}
      title={t(isActive ? "chat.webSearch.disable" : "chat.webSearch.enable")}
      onClick={onToggle}
      className={`flex h-9 w-9 cursor-pointer items-center justify-center rounded-full border border-transparent whitespace-nowrap transition-all select-none focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg ${
        isActive
          ? "bg-accent text-neutral-900"
          : "bg-neutral-200 text-neutral-500 dark:bg-neutral-700 dark:text-neutral-400"
      }`}
    >
      <GlobeAltIcon aria-hidden="true" className="h-5 w-5" strokeWidth={2} />
    </button>
  );
}
