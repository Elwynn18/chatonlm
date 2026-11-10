/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/DisplayStale.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: ChatonLM's own banner box instead of the official
 * Display component (the one of the "Ollama isn't responding" banner);
 * labelled Dismiss button; the readable name of a Hugging Face model; Update
 * queues the model in the ModelHub downloads (the official app pulls it
 * through an empty chat message), so the banner then says "Updating..." with
 * the progress, and leaves when the download has ended; a failed update says
 * why, Update trying again; the installed models of Hugging Face are compared
 * with their repository by the Rust backend (src/hooks/useOutdatedModels.ts),
 * never a model taken from Ollama nor a remote model.
 */

import { XMarkIcon } from "@heroicons/react/20/solid";
import type { ChatModel } from "@/bindings/ChatModel";
import {
  BANNER_BOX,
  BANNER_ERROR,
  BANNER_ICON_BUTTON,
  BANNER_PRIMARY,
} from "@/components/bannerStyles";
import {
  findDownload,
  useDownloadActions,
  useDownloads,
} from "@/hooks/useDownloads";
import {
  type InstalledVersion,
  listsModel,
  useDismissedOutdated,
  useOutdatedModels,
} from "@/hooks/useOutdatedModels";
import { useT } from "@/i18n";
import { formatPercent } from "@/i18n/format";
import { downloadPercent, isActiveDownload } from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import { shortModelName } from "@/lib/modelNames";

const NOTHING: readonly InstalledVersion[] = [];

/**
 * Bandeau au-dessus de la zone de saisie quand Hugging Face a une version plus
 * récente du modèle choisi (installé) : Update le met en file dans les
 * téléchargements du ModelHub, la croix le ferme pour ce modèle jusqu'à la fin
 * de la session. Update attend la fin de la réponse en cours, comme dans
 * l'officiel. Rien pour un modèle pas encore installé.
 */
export function StaleModelBanner({
  model,
  isStreaming,
}: {
  /** Modèle choisi, s'il est installé. */
  model: ChatModel | null;
  isStreaming: boolean;
}) {
  const t = useT();
  // Seuls les modèles de Hugging Face ont une version à comparer.
  const checked =
    model?.origin === "hf" && model.installedAt !== null ? model : null;
  const outdated = useOutdatedModels(
    checked
      ? [{ model: checked.id, version: String(checked.installedAt) }]
      : NOTHING,
  );
  const { dismissed, dismiss } = useDismissedOutdated();
  const { downloads } = useDownloads();
  const { start } = useDownloadActions();
  if (!checked || !listsModel(outdated, checked.id)) return null;
  if (listsModel(dismissed, checked.id)) return null;

  const target = checked.id;
  const name = shortModelName(target);
  const download = findDownload(downloads, target);

  if (download && isActiveDownload(download)) {
    return (
      <div className="pb-2">
        <div role="status" data-testid="stale-model" className={BANNER_BOX}>
          {download.total > 0
            ? t("chat.staleModel.updatingProgress", {
                model: name,
                percent: formatPercent(
                  t.lang,
                  downloadPercent(download.completed, download.total),
                ),
              })
            : t("chat.staleModel.updating", { model: name })}
        </div>
      </div>
    );
  }

  // Refus de la file (dossier des modèles en cours de changement...), ou
  // téléchargement en échec.
  const failure = start.isError
    ? errorMessage(start.error)
    : download?.state === "failed"
      ? download.error
      : null;
  return (
    <div className="pb-2">
      <div role="status" data-testid="stale-model" className={BANNER_BOX}>
        <div className="flex items-center justify-between gap-3">
          <span className="min-w-0 break-words select-text">
            {t("chat.staleModel.available", { model: name })}
          </span>
          <div className="flex flex-none items-center gap-2">
            <button
              type="button"
              className={BANNER_PRIMARY}
              disabled={isStreaming || start.isPending}
              onClick={() =>
                start.mutate({ model: target, repo: null, label: null })
              }
            >
              {t("chat.staleModel.update")}
            </button>
            <button
              type="button"
              className={BANNER_ICON_BUTTON}
              aria-label={t("chat.staleModel.dismiss")}
              title={t("chat.staleModel.dismiss")}
              onClick={() => dismiss(target)}
            >
              <XMarkIcon aria-hidden="true" className="h-4 w-4" />
            </button>
          </div>
        </div>
        {failure && (
          <p role="alert" className={BANNER_ERROR}>
            {t("chat.staleModel.failed", { message: failure })}
          </p>
        )}
      </div>
    </div>
  );
}
