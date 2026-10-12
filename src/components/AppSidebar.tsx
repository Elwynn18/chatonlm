/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/AppSidebar.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: no "Apps" section, router links, Heroicons icons;
 * a "ModelHub" entry (ChatonLM's own Hugging Face catalog) between Chat and
 * Settings, with a thin progress bar and "Downloading {n} · {pct}%" under it
 * while downloads are active (link to the Downloads tab); no AppSidebar
 * component: every page shows the chat history (ChatSidebar), which includes
 * this navigation; texts translated into English, Spanish and French
 * (src/i18n); a "Proofread" entry (ChatonLM's own text correction page)
 * between Chat and ModelHub; a "Study" entry (ChatonLM's own generated
 * courses) between Proofread and ModelHub.
 */

import {
  AcademicCapIcon,
  Cog6ToothIcon,
  CubeIcon,
  DocumentCheckIcon,
  PencilSquareIcon,
} from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import { useDownloads } from "@/hooks/useDownloads";
import { useT } from "@/i18n";
import { formatPercent } from "@/i18n/format";
import { downloadPercent, isActiveDownload } from "@/lib/hub";

/** Page de l'app dont l'entrée est surlignée. */
export type AppSection = "chat" | "proofread" | "study" | "hub" | "settings";

export function AppNavigation({ current }: { current: AppSection }) {
  const t = useT();
  const itemClass = (section: AppSection) =>
    // Entrée courante : aplat roux, texte encre (dans les deux thèmes).
    `flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left text-sm ${
      current === section
        ? "bg-accent text-neutral-900"
        : "text-neutral-700 hover:bg-neutral-200 dark:text-neutral-100 dark:hover:bg-neutral-800"
    }`;

  return (
    <div className="flex flex-col gap-0.5">
      <Link
        to="/c/$chatId"
        params={{ chatId: "new" }}
        className={itemClass("chat")}
        draggable={false}
      >
        <PencilSquareIcon className="h-5 w-5" />
        <span className="truncate">{t("chat.nav.chat")}</span>
      </Link>
      <Link
        to="/proofread"
        className={itemClass("proofread")}
        draggable={false}
      >
        <DocumentCheckIcon className="h-5 w-5" />
        <span className="truncate">{t("chat.nav.proofread")}</span>
      </Link>
      <Link to="/study" className={itemClass("study")} draggable={false}>
        <AcademicCapIcon className="h-5 w-5" />
        <span className="truncate">{t("chat.nav.study")}</span>
      </Link>
      <Link to="/hub" className={itemClass("hub")} draggable={false}>
        <CubeIcon className="h-5 w-5" />
        <span className="truncate">{t("chat.nav.hub")}</span>
      </Link>
      <DownloadIndicator />
      <Link to="/settings" className={itemClass("settings")} draggable={false}>
        <Cog6ToothIcon className="h-5 w-5" />
        <span className="truncate">{t("chat.nav.settings")}</span>
      </Link>
    </div>
  );
}

/**
 * Sous ModelHub, pendant des téléchargements : nombre et pourcentage cumulé
 * (octets reçus sur octets connus), barre fine ; lien vers Downloads. La
 * barre latérale (w-48) laisse ~115 px au texte, moins que « Downloading 2 ·
 * 100% » en 11 px : le nombre et le pourcentage (à droite, jamais tronqué)
 * tiennent seuls, le séparateur n'est lu que par les lecteurs d'écran.
 */
function DownloadIndicator() {
  const t = useT();
  const { downloads } = useDownloads();
  const active = downloads.filter(isActiveDownload);
  if (active.length === 0) return null;
  const known = active.filter((download) => download.total > 0);
  const percent = downloadPercent(
    known.reduce((sum, download) => sum + download.completed, 0),
    known.reduce((sum, download) => sum + download.total, 0),
  );
  const shownPercent = formatPercent(t.lang, percent);
  const text = t("chat.nav.downloadingStatus", {
    count: active.length,
    percent: shownPercent,
  });
  return (
    <Link
      to="/hub/downloads"
      draggable={false}
      data-testid="download-indicator"
      className="mb-1 ml-8 block rounded-md py-1 pr-1 pl-2 text-[11px] text-neutral-500 hover:bg-neutral-200 dark:text-neutral-400 dark:hover:bg-neutral-800"
    >
      <span className="flex items-baseline gap-1">
        <span className="min-w-0 flex-1 truncate">
          {t("chat.nav.downloading", { count: active.length })}
        </span>
        <span className="sr-only"> · </span>
        <span className="flex-none tabular-nums">{shownPercent}</span>
      </span>
      <span
        role="progressbar"
        aria-label={t("chat.nav.downloads")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={text}
        className="mt-1 block h-1 overflow-hidden rounded-full bg-neutral-300 dark:bg-neutral-700"
      >
        <span
          className="block h-full rounded-full bg-accent transition-[width] duration-300"
          style={{ width: `${percent}%` }}
        />
      </span>
    </Link>
  );
}
