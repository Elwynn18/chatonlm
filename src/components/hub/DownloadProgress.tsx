// Progression d'un téléchargement du ModelHub : barre (ARIA progressbar,
// comme Downloading.tsx) et ligne « 2.1 GB of 5.0 GB · 48 MB/s · 1 min left ».
// En pause : barre arrêtée où elle en était, estompée, « Paused · 2.1 GB of
// 5.0 GB ».

import type { HubDownload } from "@/bindings/HubDownload";
import { useT } from "@/i18n";
import { activeText, downloadPercent } from "@/lib/hub";

export function DownloadProgress({
  download,
  name,
}: {
  download: HubDownload;
  /** Nom lisible du modèle (nom de la barre). */
  name: string;
}) {
  const t = useT();
  const known = download.total > 0;
  const percent = downloadPercent(download.completed, download.total);
  const text = activeText(t, download);
  const waiting =
    download.state === "queued" || (download.state === "starting" && !known);
  return (
    <div>
      <div
        role="progressbar"
        aria-label={t("hub.progress.downloading", { name })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={known ? percent : undefined}
        aria-valuetext={text}
        className="relative h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <div
          className={`absolute inset-y-0 left-0 rounded-full bg-accent transition-[width] duration-300 ${
            waiting ? "w-1/4 animate-pulse opacity-40" : ""
          } ${download.state === "paused" ? "opacity-40" : ""}`}
          style={waiting ? undefined : { width: `${percent}%` }}
        />
      </div>
      <p className="mt-1.5 text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
        {text}
      </p>
    </div>
  );
}
