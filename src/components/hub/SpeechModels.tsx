// Onglet Transcription du ModelHub (étape 17, propre à ChatonLM) : modèles
// de transcription de la dictée, téléchargés par ChatonLM lui-même (Ollama ne
// sait pas transcrire). Une carte par modèle du catalogue : moteur,
// recommandation, description, taille, langues, licence et source liées
// (attribution de Parakeet, CC BY 4.0, visible), puis Download (progression,
// Cancel), Use, « In use » et suppression confirmée. Un modèle que cette
// machine ne peut pas faire tourner (Whisper sans AVX2..., Parakeet sans
// ONNX Runtime) le dit, sans Download ni Use (suppression gardée). Aussi la
// ligne d'un téléchargement de l'onglet Downloads (section « Speech
// models »), dont l'affichage sert aussi aux voix de la lecture à voix haute
// (DownloadRowView, VoiceModels.tsx).

import {
  CheckCircleIcon,
  CheckIcon,
  TrashIcon,
  XCircleIcon,
} from "@heroicons/react/24/outline";
import type { UseMutationResult } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useRef, useState } from "react";
import type { RuntimeProblemKind } from "@/bindings/RuntimeProblemKind";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { SpeechEngine } from "@/bindings/SpeechEngine";
import type { SpeechModelInfo } from "@/bindings/SpeechModelInfo";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ExternalLink } from "@/components/ExternalLink";
import { ModelAvatar } from "@/components/hub/ModelBits";
import { CommandError } from "@/components/hub/Status";
import {
  CARD,
  CHIP,
  ICON_BUTTON,
  PAGE,
  PRIMARY_BUTTON,
  QUIET_LINK,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import {
  useDictationSettings,
  useSpeechActions,
  useSpeechDownloads,
  useSpeechModels,
} from "@/hooks/useDictation";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import { isKey, type TFunction, type TKey, useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import {
  isActiveSpeechDownload,
  speechDownloadPercent,
  speechDownloadText,
} from "@/lib/dictation";
import { errorMessage } from "@/lib/ipc";

const ENGINES: Record<SpeechEngine, string> = {
  parakeet: "Parakeet",
  whisper: "Whisper",
};

/** Explication d'un ONNX Runtime indisponible (Parakeet), par raison. */
const RUNTIME_NOTES: Record<RuntimeProblemKind, TKey> = {
  missing: "hub.speech.runtime.missing",
  incompatible: "hub.speech.runtime.incompatible",
  loadFailed: "hub.speech.runtime.loadFailed",
};

/** Licence Creative Commons Attribution : son texte officiel. */
const CC_BY_URL = "https://creativecommons.org/licenses/by/4.0/";

const LINK =
  "rounded-sm underline underline-offset-2 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-white";

/** « auteur/dépôt » d'une page de Hugging Face, sinon l'adresse sans schéma. */
function pagePath(url: string): string {
  return url.replace(/^https?:\/\/(huggingface\.co\/)?/, "").replace(/\/$/, "");
}

/** Badge « Recommended » (aussi dans l'onglet Polish). */
export function RecommendedBadge() {
  const t = useT();
  return (
    <span className="inline-flex flex-none items-center rounded-md bg-accent px-1.5 text-[11px] leading-5 font-medium text-neutral-900">
      {t("hub.speech.recommended")}
    </span>
  );
}

/** « In use » : modèle réglé pour la dictée (aussi dans l'onglet Polish). */
export function InUse() {
  const t = useT();
  return (
    <span className="inline-flex flex-none items-center gap-1 rounded-full bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
      <CheckIcon aria-hidden="true" className="h-4 w-4" />
      {t("hub.speech.inUse")}
    </span>
  );
}

/** Progression d'un téléchargement (ARIA progressbar, comme DownloadProgress). */
export function SpeechProgress({
  download,
  name,
}: {
  download: SpeechDownload;
  name: string;
}) {
  const t = useT();
  const known = download.totalBytes > 0;
  const percent = speechDownloadPercent(download);
  const text = speechDownloadText(t, download);
  const waiting = download.state === "queued" || !known;
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
          } ${download.state === "verifying" ? "animate-pulse" : ""}`}
          style={waiting ? undefined : { width: `${percent}%` }}
        />
      </div>
      <p className="mt-1.5 text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
        {text}
      </p>
    </div>
  );
}

export function SpeechModels() {
  const t = useT();
  const catalog = useSpeechModels();
  const { downloads } = useSpeechDownloads();
  const { settings } = useDictationSettings();

  if (catalog.isPending) {
    return (
      <div className={PAGE} role="status" aria-label={t("hub.speech.loading")}>
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
        {t("hub.speech.intro")}{" "}
        <Link
          to="/settings"
          search={{ section: "dictation" }}
          className={QUIET_LINK}
        >
          {t("hub.speech.settingsLink")}
        </Link>
      </p>
      <ul
        aria-label={t("hub.speech.title")}
        className="mt-4 flex flex-col gap-3"
      >
        {catalog.data.map((model) => (
          <SpeechModelCard
            key={model.id}
            model={model}
            download={downloads.find((entry) => entry.id === model.id) ?? null}
            inUse={settings?.speechModel === model.id}
          />
        ))}
      </ul>
    </div>
  );
}

function SpeechModelCard({
  model,
  download,
  inUse,
}: {
  model: SpeechModelInfo;
  download: SpeechDownload | null;
  /** Réglé pour la dictée. */
  inUse: boolean;
}) {
  const t = useT();
  const { download: start, cancel, remove } = useSpeechActions();
  const { update } = useDictationSettings();
  const [confirming, setConfirming] = useState(false);
  const [useError, setUseError] = useState<string | null>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const cardRef = useRef<HTMLLIElement>(null);
  // Use, Download ou Cancel remplacés : le focus va au titre de la carte.
  useFocusRescue(cardRef, titleRef);
  const active = download !== null && isActiveSpeechDownload(download);
  const failed =
    !model.installed && download?.state === "failed"
      ? (download.error ?? t("hub.downloadState.failed"))
      : null;
  const error =
    useError ??
    (start.isError ? errorMessage(start.error) : null) ??
    (cancel.isError ? errorMessage(cancel.error) : null) ??
    (remove.isError ? errorMessage(remove.error) : null);
  const ccBy = model.license.startsWith("CC BY");
  // Instructions du processeur qui manquent à ce modèle (noms techniques),
  // ou ONNX Runtime indisponible (Parakeet).
  const runtime = model.runtimeProblem;
  const unsupported = model.missingCpuFeatures.length > 0 || runtime !== null;

  return (
    <li
      ref={cardRef}
      data-testid="speech-model"
      aria-labelledby={`speech-${model.id}`}
      aria-busy={remove.isPending || undefined}
      className={`${CARD} p-4 transition-opacity ${remove.isPending ? "opacity-50" : ""}`}
    >
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <ModelAvatar name={model.name} colorKey={model.engine} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3
              ref={titleRef}
              id={`speech-${model.id}`}
              tabIndex={-1}
              className="rounded-md font-medium text-neutral-900 outline-none dark:text-white"
            >
              {model.name}
            </h3>
            <span className={CHIP}>{ENGINES[model.engine]}</span>
            {model.recommended && <RecommendedBadge />}
          </div>
          <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-300">
            {catalogText(t, model, "description")}
          </p>
          <p className="mt-1.5 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400">
            <span>{formatBytes(t, model.sizeBytes)}</span>
            <span aria-hidden="true">·</span>
            <span>{catalogText(t, model, "languages")}</span>
            <span aria-hidden="true">·</span>
            <span>
              {t("hub.speech.license")}{" "}
              <ExternalLink
                href={ccBy ? CC_BY_URL : model.licenseUrl}
                className={LINK}
              >
                {model.license}
              </ExternalLink>
            </span>
            <span aria-hidden="true">·</span>
            <span>
              {t("hub.speech.source")}{" "}
              <ExternalLink href={model.sourceUrl} className={LINK}>
                {pagePath(model.sourceUrl)}
              </ExternalLink>
            </span>
          </p>
          {ccBy && model.licenseUrl !== model.sourceUrl && (
            // CC BY : l'œuvre d'origine et ses auteurs, cités en clair.
            <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
              <Trans
                k="hub.speech.basedOn"
                params={{
                  source: pagePath(model.licenseUrl),
                  license: model.license,
                }}
                tags={{
                  link: (chunks) => (
                    <ExternalLink href={model.licenseUrl} className={LINK}>
                      {chunks}
                    </ExternalLink>
                  ),
                }}
              />
            </p>
          )}
          {model.missingCpuFeatures.length > 0 && (
            <p className="mt-2 text-xs break-words text-amber-700 dark:text-amber-400">
              {t("hub.speech.unsupported", {
                features: model.missingCpuFeatures.join(", "),
              })}
            </p>
          )}
          {runtime && (
            <p className="mt-2 text-xs break-words text-amber-700 dark:text-amber-400">
              {t(RUNTIME_NOTES[runtime.kind], { detail: runtime.detail })}
            </p>
          )}
          {active && (
            <div className="mt-3">
              <SpeechProgress download={download} name={model.name} />
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
            nouveau bouton (le focus va au titre, useFocusRescue), jamais le
            même relabellisé sous le doigt (Entrée : Cancel devenu Use). */}
        <div className="flex flex-none items-center gap-1.5">
          {active ? (
            <button
              key="cancel"
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(model.id)}
            >
              {t("common.actions.cancel")}
            </button>
          ) : model.installed ? (
            <>
              {inUse ? (
                <InUse />
              ) : unsupported ? null : (
                <button
                  key="use"
                  type="button"
                  className={PRIMARY_BUTTON}
                  onClick={() => {
                    setUseError(null);
                    update({ speechModel: model.id }).catch((cause: unknown) =>
                      setUseError(errorMessage(cause)),
                    );
                  }}
                >
                  {t("hub.speech.use")}
                </button>
              )}
              <button
                key="delete"
                type="button"
                className={ICON_BUTTON}
                aria-label={t("hub.actions.deleteNamed", { name: model.name })}
                title={t("common.actions.delete")}
                disabled={remove.isPending}
                onClick={() => setConfirming(true)}
              >
                <TrashIcon aria-hidden="true" className="h-5 w-5" />
              </button>
            </>
          ) : unsupported ? null : (
            <button
              key="download"
              type="button"
              className={PRIMARY_BUTTON}
              disabled={start.isPending}
              onClick={() => start.mutate(model.id)}
            >
              {t("hub.actions.download")}
            </button>
          )}
        </div>
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("hub.actions.deleteConfirm", { name: model.name })}
          message={
            inUse ? t("hub.speech.deleteTextInUse") : t("hub.speech.deleteText")
          }
          confirmLabel={t("common.actions.delete")}
          onConfirm={() => {
            // La corbeille va disparaître : le focus va au titre de la carte.
            titleRef.current?.focus();
            setConfirming(false);
            remove.mutate(model.id);
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </li>
  );
}

/**
 * Téléchargement d'un modèle de transcription (onglet Downloads) :
 * progression et Cancel, puis état final, Try again et Dismiss ; modèle
 * supprimé depuis : « No longer installed », Download again.
 */
export function SpeechDownloadRow({
  download,
  removed = false,
}: {
  download: SpeechDownload;
  /** Téléchargé puis supprimé. */
  removed?: boolean;
}) {
  const actions = useSpeechActions();
  return (
    <DownloadRowView
      testId="speech-download"
      download={download}
      removed={removed}
      title={
        <Link to="/hub/transcription" className={ROW_TITLE}>
          {download.name}
        </Link>
      }
      name={download.name}
      actions={actions}
    />
  );
}

/** Titre d'une ligne de l'onglet Downloads, lien vers son onglet. */
export const ROW_TITLE =
  "min-w-0 truncate rounded-md font-medium text-neutral-900 underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-white";

/** Commande d'un téléchargement (identifiant du catalogue). */
type DownloadCommand = UseMutationResult<void, Error, string>;

/**
 * Ligne d'un téléchargement géré par ChatonLM (modèle de transcription,
 * voix) : progression et Cancel, puis état final, Try again et Dismiss ;
 * supprimé depuis : « No longer installed », Download again.
 */
export function DownloadRowView({
  testId,
  download,
  removed,
  title,
  name,
  avatar = name,
  colorKey = download.id.split("-")[0],
  actions: { download: start, cancel, dismiss },
}: {
  testId: string;
  download: SpeechDownload;
  /** Téléchargé puis supprimé. */
  removed: boolean;
  /** Titre (lien vers l'onglet de son catalogue). */
  title: ReactNode;
  /** Nom affiché (barre de progression). */
  name: string;
  /** Nom dont l'avatar prend les initiales (`name` par défaut). */
  avatar?: string;
  /** Couleur de l'avatar (celle de la carte du catalogue). */
  colorKey?: string;
  actions: {
    download: DownloadCommand;
    cancel: DownloadCommand;
    dismiss: DownloadCommand;
  };
}) {
  const t = useT();
  const active = isActiveSpeechDownload(download);
  const error =
    (start.isError ? errorMessage(start.error) : null) ??
    (cancel.isError ? errorMessage(cancel.error) : null) ??
    (dismiss.isError ? errorMessage(dismiss.error) : null);
  return (
    <li
      data-testid={testId}
      data-state={download.state}
      className={`${CARD} p-4`}
    >
      <div className="flex items-start gap-4">
        <ModelAvatar name={avatar} colorKey={colorKey} />
        <div className="min-w-0 flex-1">
          {title}
          <div className="mt-2">
            {active ? (
              <SpeechProgress download={download} name={name} />
            ) : download.state === "done" && removed ? (
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("hub.downloadState.noLongerInstalled")}
              </p>
            ) : download.state === "done" ? (
              <p className="flex items-center gap-1.5 text-xs text-green-700 dark:text-green-400">
                <CheckCircleIcon aria-hidden="true" className="h-4 w-4" />
                {t("hub.downloadState.installed")}
                {download.totalBytes > 0 &&
                  ` · ${formatBytes(t, download.totalBytes)}`}
              </p>
            ) : download.state === "failed" ? (
              <p
                role="alert"
                className="flex items-start gap-1.5 text-xs break-words text-red-600 dark:text-red-400"
              >
                <XCircleIcon aria-hidden="true" className="h-4 w-4 flex-none" />
                {download.error ?? t("hub.downloadState.failed")}
              </p>
            ) : (
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("hub.downloadState.cancelled")}
              </p>
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
          {active && (
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(download.id)}
            >
              {t("common.actions.cancel")}
            </button>
          )}
          {(download.state === "failed" ||
            download.state === "cancelled" ||
            removed) && (
            <button
              type="button"
              className={PRIMARY_BUTTON}
              disabled={start.isPending}
              onClick={() => start.mutate(download.id)}
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
              onClick={() => dismiss.mutate(download.id)}
            >
              {t("hub.actions.dismiss")}
            </button>
          )}
        </div>
      </div>
    </li>
  );
}

/**
 * Description ou langues d'un modèle du catalogue du moteur
 * (dictation/models.rs, en anglais), traduites par son identifiant
 * (`hub.speech.catalog`, anglais vérifié par un essai du moteur) ; le texte
 * du moteur pour un modèle que ce catalogue ne connaît pas.
 */
function catalogText(
  t: TFunction,
  model: SpeechModelInfo,
  field: "description" | "languages",
): string {
  const key = `hub.speech.catalog.${model.id.replace(/[^A-Za-z0-9]/g, "_")}.${field}`;
  return isKey(key) ? t(key) : model[field];
}
