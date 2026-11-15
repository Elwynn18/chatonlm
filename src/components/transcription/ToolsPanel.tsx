// Outil Transcription (propre à ChatonLM) : yt-dlp et Deno, téléchargés à
// la demande pour lire une vidéo du web (rapport 105, § 6). Jamais livrés
// avec l'app : avant le premier téléchargement, une boîte demande l'accord
// explicite (licences de yt-dlp, de ses composants dont certains sous GPL
// ou LGPL, et de Deno ; responsabilité de l'utilisateur) ; puis
// progression, « Cancel », mise à jour proposée et retrait.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { ToolsStatus } from "@/bindings/ToolsStatus";
import type { ToolStatus } from "@/bindings/ToolStatus";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import {
  isToolBusy,
  toolsDownloadBytes,
} from "@/components/transcription/model";
import { toolsStatusKey } from "@/components/transcription/queries";
import { type TFunction, useT } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import {
  errorMessage,
  openExternal,
  toolsCancel,
  toolsRemove,
  toolsUpdate,
} from "@/lib/ipc";

/**
 * Licences des composants embarqués dans le programme de yt-dlp (le même
 * fichier que `_internal/THIRD_PARTY_LICENSES.txt` de l'archive installée).
 */
export const YTDLP_COMPONENT_LICENSES =
  "https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt";

const MUTED = "text-sm text-neutral-600 dark:text-neutral-300";
const LINK =
  "rounded text-sm font-medium text-accent-fg underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg";

const tool = (status: ToolsStatus, id: ToolStatus["tool"]) =>
  status.tools.find((item) => item.tool === id);

/**
 * État des outils sous le champ d'adresse : non pris en charge, absents
 * (proposition), en cours d'installation, installés (versions, mise à jour,
 * retrait), avec la dernière erreur.
 */
export function ToolsPanel({
  status,
  onInstall,
}: {
  status: ToolsStatus;
  /** Ouvre la boîte d'accord. */
  onInstall: () => void;
}) {
  const t = useT();
  const client = useQueryClient();
  const [actionError, setActionError] = useState<string | null>(null);
  const run = (action: () => Promise<void>) => {
    setActionError(null);
    action().then(
      () => void client.invalidateQueries({ queryKey: toolsStatusKey }),
      (cause: unknown) => setActionError(errorMessage(cause)),
    );
  };
  if (!status.supported) {
    return (
      <p data-testid="transcription-tools" className={`mt-3 ${MUTED}`}>
        {t("transcription.tools.unsupported")}
      </p>
    );
  }
  const busy = status.tools.filter(isToolBusy);
  const errors = status.tools
    .map((item) => item.error?.message)
    .filter((message): message is string => !!message);
  let body;
  if (busy.length > 0) {
    body = (
      <div data-testid="transcription-tools-progress">
        {busy.map((item) => (
          <ToolProgress key={item.tool} tool={item} />
        ))}
        <button
          type="button"
          className={`mt-2 ${BUTTON}`}
          onClick={() => run(toolsCancel)}
        >
          {t("transcription.tools.cancel")}
        </button>
      </div>
    );
  } else if (!status.ready) {
    body = (
      <div>
        <p className={MUTED}>
          {t("transcription.tools.needed", {
            size: formatBytes(t, toolsDownloadBytes(status)),
          })}
        </p>
        <button
          type="button"
          className={`mt-2 ${BUTTON}`}
          onClick={onInstall}
          data-testid="transcription-tools-install"
        >
          {t("transcription.tools.install")}
        </button>
      </div>
    );
  } else {
    const ytdlp = tool(status, "ytDlp");
    const deno = tool(status, "deno");
    const updates = status.tools.filter(
      (item) => item.phase === "updateAvailable",
    );
    body = (
      <div className="flex flex-col gap-1">
        <p className={MUTED} data-testid="transcription-tools-installed">
          {t("transcription.tools.installed", {
            ytdlp: ytdlp?.version ?? "",
            deno: deno?.version ?? "",
          })}
        </p>
        {updates.map((item) => (
          <p key={item.tool} className={MUTED}>
            {t("transcription.tools.updateAvailable", {
              name: item.name,
              version: item.latest ?? "",
            })}
          </p>
        ))}
        <div className="flex flex-wrap gap-2">
          {updates.length > 0 && (
            <button
              type="button"
              className={BUTTON}
              disabled={status.inUse}
              onClick={() => run(toolsUpdate)}
            >
              {t("transcription.tools.update")}
            </button>
          )}
          <button
            type="button"
            className={QUIET_BUTTON}
            onClick={() => void openExternal(YTDLP_COMPONENT_LICENSES)}
          >
            {t("transcription.tools.componentLicenses")}
          </button>
          <button
            type="button"
            className={QUIET_BUTTON}
            disabled={status.inUse}
            onClick={() => run(toolsRemove)}
          >
            {t("transcription.tools.remove")}
          </button>
        </div>
      </div>
    );
  }
  return (
    <div
      data-testid="transcription-tools"
      className="mt-3 rounded-xl bg-neutral-50 px-3 py-2.5 dark:bg-neutral-900"
    >
      {body}
      {errors.map((message) => (
        <p
          key={message}
          role="alert"
          className="mt-2 text-sm break-words text-red-600 dark:text-red-400"
        >
          {message}
        </p>
      ))}
      {actionError && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {t("transcription.tools.actionError", { message: actionError })}
        </p>
      )}
    </div>
  );
}

function ToolProgress({ tool }: { tool: ToolStatus }) {
  const t = useT();
  const percent =
    tool.totalBytes > 0
      ? Math.min(100, (tool.completedBytes / tool.totalBytes) * 100)
      : 0;
  const text = toolPhaseText(t, tool);
  return (
    <div className="mb-1.5">
      <p className={MUTED}>{text}</p>
      {tool.phase === "downloading" && (
        <div
          role="progressbar"
          aria-label={t("transcription.tools.progress")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.floor(percent)}
          aria-valuetext={formatPercent(t.lang, Math.floor(percent))}
          className="mt-1 h-1.5 max-w-xs overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
        >
          <div
            className="h-full rounded-full bg-accent"
            style={{ width: `${percent}%` }}
          />
        </div>
      )}
    </div>
  );
}

function toolPhaseText(t: TFunction, tool: ToolStatus): string {
  switch (tool.phase) {
    case "downloading":
      return t("transcription.tools.downloading", {
        name: tool.name,
        done: formatBytes(t, tool.completedBytes),
        total: formatBytes(t, tool.totalBytes),
      });
    case "verifying":
      return t("transcription.tools.verifying", { name: tool.name });
    case "installing":
      return t("transcription.tools.installing", { name: tool.name });
    default:
      return t("transcription.tools.fetching");
  }
}

/**
 * Accord avant le premier téléchargement des outils (rapport 105, § 6,
 * point 3) : d'où ils viennent et leurs licences, composants sous GPL ou
 * LGPL, responsabilité de l'utilisateur ; liens vers les licences.
 */
export function ToolsConsent({
  status,
  onAccept,
  onCancel,
}: {
  status: ToolsStatus;
  onAccept: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const ytdlp = tool(status, "ytDlp");
  const deno = tool(status, "deno");
  return (
    <ConfirmDialog
      title={t("transcription.tools.consent.title")}
      message={
        <div data-testid="transcription-consent" className="space-y-2">
          <p>
            {t("transcription.tools.consent.download", {
              size: formatBytes(t, toolsDownloadBytes(status)),
            })}
          </p>
          <p>{t("transcription.tools.consent.components")}</p>
          <p>{t("transcription.tools.consent.responsibility")}</p>
          <p>{t("transcription.tools.consent.workFile")}</p>
          <ul className="flex flex-col items-start gap-1">
            {ytdlp && (
              <li>
                <button
                  type="button"
                  className={LINK}
                  onClick={() => void openExternal(ytdlp.licenseUrl)}
                >
                  {t("transcription.tools.consent.ytdlpLicense")}
                </button>
              </li>
            )}
            <li>
              <button
                type="button"
                className={LINK}
                onClick={() => void openExternal(YTDLP_COMPONENT_LICENSES)}
              >
                {t("transcription.tools.componentLicenses")}
              </button>
            </li>
            {deno && (
              <li>
                <button
                  type="button"
                  className={LINK}
                  onClick={() => void openExternal(deno.licenseUrl)}
                >
                  {t("transcription.tools.consent.denoLicense")}
                </button>
              </li>
            )}
          </ul>
        </div>
      }
      actions={[
        {
          label: t("transcription.tools.consent.accept"),
          onSelect: onAccept,
        },
      ]}
      onCancel={onCancel}
    />
  );
}
