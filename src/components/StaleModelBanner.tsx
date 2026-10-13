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
 * why, Update trying again; the installed models are compared with their
 * registry by the Rust backend (src/hooks/useOutdatedModels.ts).
 */

import { XMarkIcon } from "@heroicons/react/20/solid";
import type { ModelInfo } from "@/bindings/ModelInfo";
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
import { shortModelName, withTag } from "@/lib/modelNames";

const NOTHING: readonly InstalledVersion[] = [];

/**
 * Bandeau au-dessus de la zone de saisie quand le registre a une version plus
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
  model: ModelInfo | null;
  isStreaming: boolean;
}) {
  const t = useT();
  const outdated = useOutdatedModels(
    model?.digest ? [{ model: model.model, version: model.digest }] : NOTHING,
  );
  const { dismissed, dismiss } = useDismissedOutdated();
  const { downloads } = useDownloads();
  const { start } = useDownloadActions();
  if (!model || !listsModel(outdated, model.model)) return null;
  if (listsModel(dismissed, model.model)) return null;

  // Nom que la file des téléchargements garde : avec son étiquette.
  const target = withTag(model.model);
  const name = shortModelName(model.model);
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
              onClick={() => dismiss(model.model)}
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
