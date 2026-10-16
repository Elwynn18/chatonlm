// Onglet Downloads du ModelHub : téléchargements pas encore finis (dans
// l'ordre de la file, ceux en pause à leur place), puis terminés (le plus
// récent d'abord) ; progression, Pause ou Resume, « Download first » pour un
// téléchargement en file (il démarre tout de suite, le dernier en cours
// retournant en file), Cancel ; Use in chat et Dismiss, échec avec Try again
// (et, si la clé d'Ollama est
// cachée à son service, « Fix in Settings › General »). Un modèle téléchargé puis
// supprimé (absent des installés relus depuis sa fin) n'est plus « Installed » :
// « No longer installed », Download again. Sous eux, section « Speech
// models » : téléchargements des modèles de transcription de la dictée, avec
// la même règle (catalogue relu sans le modèle : plus installé) ; puis
// section « Voices » : téléchargements des voix de la lecture à voix haute,
// même règle ; enfin section « Image models » : téléchargements des modèles
// d'images, même règle (modèles relus sans lui : plus installé).

import {
  ArrowDownTrayIcon,
  CheckCircleIcon,
  XCircleIcon,
} from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { HubDownload } from "@/bindings/HubDownload";
import { DownloadProgress } from "@/components/hub/DownloadProgress";
import { ImageDownloadRow } from "@/components/hub/ImageModels";
import { ModelAvatar } from "@/components/hub/ModelBits";
import { SpeechDownloadRow } from "@/components/hub/SpeechModels";
import { VoiceDownloadRow } from "@/components/hub/VoiceModels";
import { EmptyState } from "@/components/hub/Status";
import { OllamaHomeLink } from "@/components/settings/OllamaHome";
import {
  CARD,
  CHIP,
  PAGE,
  PRIMARY_BUTTON,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import { useSpeechDownloads, useSpeechModels } from "@/hooks/useDictation";
import { useDownloadActions, useDownloads } from "@/hooks/useDownloads";
import { useImageDownloads, useImageModels } from "@/hooks/useImageModels";
import { useVoiceDownloads, useVoiceModels } from "@/hooks/useReadAloud";
import { isActiveSpeechDownload } from "@/lib/dictation";
import { useHubInstalled, useUseInChat } from "@/hooks/useHub";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { familyOf, isPendingDownload, repoParams } from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import { displayTag, parseModelName } from "@/lib/modelNames";

export function DownloadsList() {
  const t = useT();
  const { downloads, loaded } = useDownloads();
  const speech = useSpeechDownloads();
  const speechModels = useSpeechModels();
  const installed = useHubInstalled();
  const voices = useVoiceDownloads();
  const voiceModels = useVoiceModels();
  const images = useImageDownloads();
  const imageModels = useImageModels();
  if (!loaded || !speech.loaded || !voices.loaded || !images.loaded) {
    return (
      <div
        className={PAGE}
        role="status"
        aria-label={t("hub.downloads.loading")}
      >
        <div
          aria-hidden="true"
          className={`${SKELETON} mt-5 h-24 rounded-2xl`}
        />
      </div>
    );
  }
  if (
    downloads.length === 0 &&
    speech.downloads.length === 0 &&
    voices.downloads.length === 0 &&
    images.downloads.length === 0
  ) {
    return (
      <EmptyState
        icon={<ArrowDownTrayIcon aria-hidden="true" className="h-6 w-6" />}
        title={t("hub.downloads.empty")}
        text={t("hub.downloads.emptyText")}
      >
        <Link to="/hub" className={SECONDARY_BUTTON}>
          {t("hub.actions.browse")}
        </Link>
      </EmptyState>
    );
  }
  const active = downloads.filter(isPendingDownload);
  const finished = downloads
    .filter((download) => !isPendingDownload(download))
    .sort((a, b) => (b.finishedAt ?? 0) - (a.finishedAt ?? 0));
  const installedNames = new Set(
    (installed.data ?? []).map((model) => model.model.toLowerCase()),
  );
  // Fini avant la dernière lecture des installés, et absent de la liste.
  const removed = (download: HubDownload) =>
    download.state === "done" &&
    installed.isSuccess &&
    (download.finishedAt ?? 0) <= installed.dataUpdatedAt &&
    !installedNames.has(download.model.toLowerCase());
  // Fini, catalogue relu depuis (pas en cours de relecture) et modèle absent.
  const speechRemoved = (id: string) =>
    speechModels.data !== undefined &&
    !speechModels.isFetching &&
    !speechModels.data.some((model) => model.id === id && model.installed);
  const speechActive = speech.downloads.filter(isActiveSpeechDownload);
  const speechFinished = speech.downloads.filter(
    (download) => !isActiveSpeechDownload(download),
  );
  // Même règle pour les voix.
  const voiceRemoved = (id: string) =>
    voiceModels.data !== undefined &&
    !voiceModels.isFetching &&
    !voiceModels.data.some((voice) => voice.id === id && voice.installed);
  const voiceActive = voices.downloads.filter(isActiveSpeechDownload);
  const voiceFinished = voices.downloads.filter(
    (download) => !isActiveSpeechDownload(download),
  );
  // Même règle pour les modèles d'images.
  const imageRemoved = (id: string) =>
    imageModels.data !== undefined &&
    !imageModels.isFetching &&
    !imageModels.data.some((model) => model.id === id && model.installed);
  const imageActive = images.downloads.filter(isActiveSpeechDownload);
  const imageFinished = images.downloads.filter(
    (download) => !isActiveSpeechDownload(download),
  );
  return (
    <div className={PAGE}>
      {downloads.length > 0 && (
        <ul
          className="mt-4 flex flex-col gap-3"
          aria-label={t("hub.tabs.downloads")}
        >
          {[...active, ...finished].map((download) => (
            <DownloadRow
              key={download.model}
              download={download}
              removed={removed(download)}
            />
          ))}
        </ul>
      )}
      {speech.downloads.length > 0 && (
        <section aria-labelledby="speech-downloads" className="mt-6">
          <h2
            id="speech-downloads"
            className="font-rounded text-lg font-medium text-neutral-900 dark:text-white"
          >
            {t("hub.speech.title")}
          </h2>
          <ul
            className="mt-3 flex flex-col gap-3"
            aria-label={t("hub.speech.title")}
          >
            {[...speechActive, ...speechFinished].map((download) => (
              <SpeechDownloadRow
                key={download.id}
                download={download}
                removed={
                  download.state === "done" && speechRemoved(download.id)
                }
              />
            ))}
          </ul>
        </section>
      )}
      {voices.downloads.length > 0 && (
        <section aria-labelledby="voice-downloads" className="mt-6">
          <h2
            id="voice-downloads"
            className="font-rounded text-lg font-medium text-neutral-900 dark:text-white"
          >
            {t("hub.voices.title")}
          </h2>
          <ul
            className="mt-3 flex flex-col gap-3"
            aria-label={t("hub.voices.title")}
          >
            {[...voiceActive, ...voiceFinished].map((download) => (
              <VoiceDownloadRow
                key={download.id}
                download={download}
                voice={voiceModels.data?.find(
                  (voice) => voice.id === download.id,
                )}
                removed={download.state === "done" && voiceRemoved(download.id)}
              />
            ))}
          </ul>
        </section>
      )}
      {images.downloads.length > 0 && (
        <section aria-labelledby="image-downloads" className="mt-6">
          <h2
            id="image-downloads"
            className="font-rounded text-lg font-medium text-neutral-900 dark:text-white"
          >
            {t("hub.images.title")}
          </h2>
          <ul
            className="mt-3 flex flex-col gap-3"
            aria-label={t("hub.images.title")}
          >
            {[...imageActive, ...imageFinished].map((download) => (
              <ImageDownloadRow
                key={download.id}
                download={download}
                model={imageModels.data?.find(
                  (model) => model.id === download.id,
                )}
                removed={download.state === "done" && imageRemoved(download.id)}
              />
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function DownloadRow({
  download,
  removed,
}: {
  download: HubDownload;
  /** Téléchargé puis supprimé du serveur. */
  removed: boolean;
}) {
  const t = useT();
  const { start, cancel, dismiss, pause, resume, prioritize } =
    useDownloadActions();
  const openInChat = useUseInChat();
  const [chatError, setChatError] = useState<string | null>(null);
  const name = parseModelName(download.model);
  const repo = download.repo ?? name.repo;
  const tag = download.label ?? name.tag;
  // Pas encore fini : en file, en cours ou en pause.
  const active = isPendingDownload(download);
  const paused = download.state === "paused";
  // Tout est reçu, Ollama vérifie : plus rien à mettre en pause.
  const pausable = active && !paused && download.state !== "finishing";
  const retry = () =>
    start.mutate({
      model: download.model,
      repo: download.repo,
      label: download.label,
    });
  const failed = [start, cancel, pause, resume, prioritize].find(
    (action) => action.isError,
  );
  const error = chatError ?? (failed ? errorMessage(failed.error) : null);

  const title = repo ? (
    <Link
      to="/hub/model/$author/$repo"
      params={repoParams(repo)}
      className="min-w-0 truncate rounded-md font-medium text-neutral-900 underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-white"
    >
      {name.title}
    </Link>
  ) : (
    <span className="min-w-0 truncate font-medium text-neutral-900 dark:text-white">
      {name.title}
    </span>
  );

  return (
    <li
      data-testid="download"
      data-state={download.state}
      className={`${CARD} p-4`}
    >
      <div className="flex items-start gap-4">
        <ModelAvatar
          name={name.title}
          colorKey={familyOf(name.title) ?? name.publisher ?? name.title}
        />
        <div className="min-w-0 flex-1">
          <div
            className="flex min-w-0 items-center gap-2"
            title={download.model}
          >
            {title}
            {tag && (
              <span className={`${CHIP} flex-none font-mono`}>
                {displayTag(tag)}
              </span>
            )}
          </div>
          <div className="mt-2">
            {active ? (
              <DownloadProgress download={download} name={name.title} />
            ) : (
              <FinishedLine download={download} removed={removed} />
            )}
          </div>
          {error && (
            <p
              role="alert"
              className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
            >
              {error}
            </p>
          )}
        </div>
        <div className="flex flex-none flex-wrap items-center justify-end gap-1.5">
          {download.state === "queued" && (
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={prioritize.isPending}
              onClick={() => prioritize.mutate(download.model)}
            >
              {t("hub.actions.downloadFirst")}
            </button>
          )}
          {pausable && (
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={pause.isPending}
              onClick={() => pause.mutate(download.model)}
            >
              {t("hub.actions.pause")}
            </button>
          )}
          {paused && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              disabled={resume.isPending}
              onClick={() => resume.mutate(download.model)}
            >
              {t("hub.actions.resume")}
            </button>
          )}
          {active && (
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(download.model)}
            >
              {t("common.actions.cancel")}
            </button>
          )}
          {download.state === "done" && !removed && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              onClick={() => {
                setChatError(null);
                openInChat(download.model).catch((cause: unknown) =>
                  setChatError(errorMessage(cause)),
                );
              }}
            >
              {t("hub.actions.useInChat")}
            </button>
          )}
          {(download.state === "failed" ||
            download.state === "cancelled" ||
            removed) && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              disabled={start.isPending}
              onClick={retry}
            >
              {removed
                ? t("hub.actions.downloadAgain")
                : t("hub.actions.tryAgain")}
            </button>
          )}
          {!active && (
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={dismiss.isPending}
              onClick={() => dismiss.mutate(download.model)}
            >
              {t("hub.actions.dismiss")}
            </button>
          )}
        </div>
      </div>
    </li>
  );
}

/** État d'un téléchargement fini. */
function FinishedLine({
  download,
  removed,
}: {
  download: HubDownload;
  removed: boolean;
}) {
  const t = useT();
  switch (download.state) {
    case "done":
      if (removed) {
        return (
          <p className="text-xs text-neutral-500 dark:text-neutral-400">
            {t("hub.downloadState.noLongerInstalled")}
          </p>
        );
      }
      return (
        <p className="flex items-center gap-1.5 text-xs text-green-700 dark:text-green-400">
          <CheckCircleIcon aria-hidden="true" className="h-4 w-4" />
          {t("hub.downloadState.installed")}
          {download.total > 0 && ` · ${formatBytes(t, download.total)}`}
        </p>
      );
    case "failed":
      return (
        <>
          <p
            role="alert"
            className="flex items-start gap-1.5 text-xs break-words text-red-600 dark:text-red-400"
          >
            <XCircleIcon aria-hidden="true" className="h-4 w-4 flex-none" />
            {download.error ?? t("hub.downloadState.failed")}
          </p>
          {/* Clé d'Ollama cachée à son service : la correction. */}
          <OllamaHomeLink
            error={download.error}
            className="mt-1 inline-block text-xs"
          />
        </>
      );
    default:
      return (
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          {t("hub.downloadState.cancelled")}
        </p>
      );
  }
}
