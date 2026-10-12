/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Downloading.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named export; Heroicons download tray (outline, same
 * width) instead of the SF Symbols glyph; the unit is capped at TB (the
 * official one has none past TB); the bar is an ARIA progressbar (name,
 * percentage and sizes); texts translated into English, Spanish and French
 * (src/i18n), numbers and units written the way of the language, still
 * without thousands separators like the official ones ("1000 KB").
 */

import { ArrowDownTrayIcon } from "@heroicons/react/24/outline";
import { type TFunction, useT } from "@/i18n";
import { formatPercent, SIZE_UNITS, sizeUnit } from "@/i18n/format";

const K = 1024;

/** Index de l'unité d'une taille, plafonné à TB. */
function unitIndex(bytes: number): number {
  if (bytes <= 0) return 0;
  return Math.min(
    Math.floor(Math.log(bytes) / Math.log(K)),
    SIZE_UNITS.length - 1,
  );
}

/**
 * Taille lisible, base 1024, dans l'unité d'index `i` (celle du total, pour
 * la partie téléchargée). Une décimale en GB et TB, aucune sinon ; pas de
 * séparateur des milliers (« 1000 KB », comme l'officiel), mais la virgule
 * décimale et les unités de la langue (« 1,5 Go »).
 */
function formatBytes(t: TFunction, bytes: number, i: number): string {
  const unit = SIZE_UNITS[i];
  const decimals = unit === "GB" || unit === "TB" ? 1 : 0;
  const value = new Intl.NumberFormat(t.lang, {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
    useGrouping: false,
  }).format(bytes / Math.pow(K, i));
  return t("common.size", { value, unit: sizeUnit(t.lang, unit) });
}

export function Downloading({
  completed,
  total,
}: {
  completed: number;
  total: number;
}) {
  const t = useT();
  const percentage = total > 0 ? (completed / total) * 100 : 0;
  const unit = unitIndex(total);
  const progress = t("chat.download.progress", {
    completed: formatBytes(t, completed, unit),
    total: formatBytes(t, total, unit),
    percent: formatPercent(t.lang, Math.floor(percentage)),
  });

  return (
    <div className="my-4 max-w-xs rounded-xl">
      <div className="mb-2 flex flex-col text-neutral-800 dark:text-neutral-200">
        <div className="mb-0.5 flex items-center">
          {/* Trait d'environ 1,2 px, comme l'icône du sélecteur de modèles. */}
          <ArrowDownTrayIcon className="absolute h-3.5 w-3.5" strokeWidth={2} />

          <div className="ml-6">{t("chat.download.title")}</div>
        </div>
        <div className="ml-6 text-sm text-neutral-500">{progress}</div>
      </div>
      <div
        role="progressbar"
        aria-label={t("chat.download.title")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.floor(percentage)}
        aria-valuetext={progress}
        className="relative ml-6 h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <div
          className="absolute top-0 left-0 h-full rounded-full bg-accent"
          style={{
            width: `${percentage}%`,
          }}
        />
      </div>
    </div>
  );
}
