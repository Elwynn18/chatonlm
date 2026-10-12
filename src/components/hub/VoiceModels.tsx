// Onglet Voices du ModelHub (propre à ChatonLM) : voix de la lecture à voix
// haute des réponses, téléchargées par ChatonLM lui-même (Ollama ne sait pas
// parler), sur le modèle de l'onglet Transcription (SpeechModels.tsx). Une
// carte par voix (une par langue : « French · Estelle ») : taille, licence
// du modèle (Pocket TTS de Kyutai, CC BY 4.0 : œuvre d'origine citée et
// changements dits, exporté en ONNX et quantifié par ChatonLM), crédit de la
// voix (lié à son enregistrement) et sa licence (liée à son texte), puis
// Download (progression, Cancel) et suppression confirmée. Sans
// ONNX Runtime, la carte le dit, sans Download (suppression gardée). Aussi la
// ligne d'un téléchargement de l'onglet Downloads (section « Voices »).

import { TrashIcon, XCircleIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import { useRef, useState } from "react";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { VoiceModelInfo } from "@/bindings/VoiceModelInfo";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ExternalLink } from "@/components/ExternalLink";
import { ModelAvatar } from "@/components/hub/ModelBits";
import {
  DownloadRowView,
  ROW_TITLE,
  SpeechProgress,
} from "@/components/hub/SpeechModels";
import { CommandError } from "@/components/hub/Status";
import {
  CARD,
  ICON_BUTTON,
  PAGE,
  PRIMARY_BUTTON,
  QUIET_LINK,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import {
  useVoiceActions,
  useVoiceDownloads,
  useVoiceModels,
} from "@/hooks/useReadAloud";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import { isActiveSpeechDownload } from "@/lib/dictation";
import { errorMessage } from "@/lib/ipc";
import { VOICE_RUNTIME_NOTES, voiceName, voiceTitle } from "@/lib/readAloud";

/** Licence Creative Commons Attribution : son texte officiel. */
const CC_BY_URL = "https://creativecommons.org/licenses/by/4.0/";

const LINK =
  "rounded-sm underline underline-offset-2 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-white";

/** « auteur/dépôt » d'une page de Hugging Face, sinon l'adresse sans schéma. */
function pagePath(url: string): string {
  return url.replace(/^https?:\/\/(huggingface\.co\/)?/, "").replace(/\/$/, "");
}

export function VoiceModels() {
  const t = useT();
  const catalog = useVoiceModels();
  const { downloads } = useVoiceDownloads();

  if (catalog.isPending) {
    return (
      <div className={PAGE} role="status" aria-label={t("hub.voices.loading")}>
        <div aria-hidden="true" className="mt-5 flex flex-col gap-3">
          {[0, 1, 2].map((row) => (
            <div key={row} className={`${SKELETON} h-28 rounded-2xl`} />
          ))}
        </div>
      </div>
    );
  }
  if (catalog.isError) {
    return (
      <CommandError
        message={errorMessage(catalog.error)}
        onRetry={() => void catalog.refetch()}
      />
    );
  }
  return (
    <div className={PAGE}>
      <p className="pt-4 text-sm text-neutral-600 dark:text-neutral-300">
        {t("hub.voices.intro")}{" "}
        <Link
          to="/settings"
          search={{ section: "readAloud" }}
          className={QUIET_LINK}
        >
          {t("hub.voices.settingsLink")}
        </Link>
      </p>
      <ul
        aria-label={t("hub.voices.title")}
        className="mt-4 flex flex-col gap-3"
      >
        {catalog.data.map((voice) => (
          <VoiceCard
            key={voice.id}
            voice={voice}
            download={downloads.find((entry) => entry.id === voice.id) ?? null}
          />
        ))}
      </ul>
    </div>
  );
}

function VoiceCard({
  voice,
  download,
}: {
  voice: VoiceModelInfo;
  download: SpeechDownload | null;
}) {
  const t = useT();
  const { download: start, cancel, remove } = useVoiceActions();
  const [confirming, setConfirming] = useState(false);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const cardRef = useRef<HTMLLIElement>(null);
  // Download ou Cancel remplacés : le focus va au titre de la carte.
  useFocusRescue(cardRef, titleRef);
  const title = voiceTitle(t, voice);
  const active = download !== null && isActiveSpeechDownload(download);
  const failed =
    !voice.installed && download?.state === "failed"
      ? (download.error ?? t("hub.downloadState.failed"))
      : null;
  const error =
    (start.isError ? errorMessage(start.error) : null) ??
    (cancel.isError ? errorMessage(cancel.error) : null) ??
    (remove.isError ? errorMessage(remove.error) : null);
  const runtime = voice.runtimeProblem;
  const ccBy = voice.license.startsWith("CC BY");

  return (
    <li
      ref={cardRef}
      data-testid="voice-model"
      aria-labelledby={`voice-${voice.id}`}
      aria-busy={remove.isPending || undefined}
      className={`${CARD} p-4 transition-opacity ${remove.isPending ? "opacity-50" : ""}`}
    >
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <ModelAvatar name={voiceName(voice.voice)} colorKey={voice.id} />
        <div className="min-w-0 flex-1">
          <h3
            ref={titleRef}
            id={`voice-${voice.id}`}
            tabIndex={-1}
            className="rounded-md font-medium text-neutral-900 outline-none dark:text-white"
          >
            {title}
          </h3>
          <p className="mt-1.5 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400">
            <span>{formatBytes(t, voice.sizeBytes)}</span>
            <span aria-hidden="true">·</span>
            <span>
              {t("hub.voices.modelLicense")}{" "}
              <ExternalLink
                href={ccBy ? CC_BY_URL : voice.licenseUrl}
                className={LINK}
              >
                {voice.license}
              </ExternalLink>
            </span>
            <span aria-hidden="true">·</span>
            <span>
              {/* Crédit de la voix (exigé par CC BY) : son enregistrement
                  et le texte de sa licence. */}
              <Trans
                k="hub.voices.voice"
                params={{
                  credit: voice.voiceCredit,
                  license: voice.voiceLicense,
                }}
                tags={{
                  source: (chunks) => (
                    <ExternalLink href={voice.voiceSourceUrl} className={LINK}>
                      {chunks}
                    </ExternalLink>
                  ),
                  license: (chunks) => (
                    <ExternalLink href={voice.voiceLicenseUrl} className={LINK}>
                      {chunks}
                    </ExternalLink>
                  ),
                }}
              />
            </span>
          </p>
          {/* CC BY : l'œuvre d'origine, ses auteurs et nos changements. */}
          <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
            <Trans
              k="hub.voices.basedOn"
              params={{
                source: pagePath(voice.licenseUrl),
                license: voice.license,
              }}
              tags={{
                link: (chunks) => (
                  <ExternalLink href={voice.licenseUrl} className={LINK}>
                    {chunks}
                  </ExternalLink>
                ),
              }}
            />
          </p>
          {runtime && (
            <p className="mt-2 text-xs break-words text-amber-700 dark:text-amber-400">
              {t(VOICE_RUNTIME_NOTES[runtime.kind], { detail: runtime.detail })}
            </p>
          )}
          {active && (
            <div className="mt-3">
              <SpeechProgress download={download} name={title} />
            </div>
          )}
          {failed && (
            <p
              role="alert"
              className="mt-2 flex items-start gap-1.5 text-xs break-words text-red-600 dark:text-red-400"
            >
              <XCircleIcon aria-hidden="true" className="h-4 w-4 flex-none" />
              {failed}
            </p>
          )}
          {error && (
            <p
              role="alert"
              className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
            >
              {error}
            </p>
          )}
        </div>
        {/* Une clé par action : un bouton qui en remplace un autre est un
            nouveau bouton (le focus va au titre, useFocusRescue). */}
        <div className="flex flex-none items-center gap-1.5">
          {active ? (
            <button
              key="cancel"
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(voice.id)}
            >
              {t("common.actions.cancel")}
            </button>
          ) : voice.installed ? (
            <button
              key="delete"
              type="button"
              className={ICON_BUTTON}
              aria-label={t("hub.actions.deleteNamed", { name: title })}
              title={t("common.actions.delete")}
              disabled={remove.isPending}
              onClick={() => setConfirming(true)}
            >
              <TrashIcon aria-hidden="true" className="h-5 w-5" />
            </button>
          ) : runtime ? null : (
            <button
              key="download"
              type="button"
              className={PRIMARY_BUTTON}
              disabled={start.isPending}
              onClick={() => start.mutate(voice.id)}
            >
              {t("hub.actions.download")}
            </button>
          )}
        </div>
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("hub.actions.deleteConfirm", { name: title })}
          message={t("hub.voices.deleteText")}
          confirmLabel={t("common.actions.delete")}
          onConfirm={() => {
            // La corbeille va disparaître : le focus va au titre de la carte.
            titleRef.current?.focus();
            setConfirming(false);
            remove.mutate(voice.id);
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </li>
  );
}

/**
 * Téléchargement d'une voix (onglet Downloads), titré comme sa carte quand
 * le catalogue est lu ; voix supprimée depuis : « No longer installed »,
 * Download again.
 */
export function VoiceDownloadRow({
  download,
  voice,
  removed = false,
}: {
  download: SpeechDownload;
  /** La voix du catalogue, une fois lu. */
  voice?: VoiceModelInfo;
  /** Téléchargée puis supprimée. */
  removed?: boolean;
}) {
  const t = useT();
  const actions = useVoiceActions();
  const name = voice ? voiceTitle(t, voice) : download.name;
  return (
    <DownloadRowView
      testId="voice-download"
      download={download}
      removed={removed}
      title={
        <Link to="/hub/voices" className={ROW_TITLE}>
          {name}
        </Link>
      }
      name={name}
      avatar={voice ? voiceName(voice.voice) : download.name}
      colorKey={download.id}
      actions={actions}
    />
  );
}
