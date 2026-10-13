// Colonne « Choose a version » de la fiche : versions en boutons radio
// (flèches du clavier natives ; indisponibles grisées avec leur raison),
// puis selon la version choisie Download, progression et Cancel, Installed
// et Use in chat, ou l'échec et Try again ; nom Ollama à copier, autres
// sources du même modèle. Un téléchargement fini ne vaut « Installed » que
// s'il est plus récent que la dernière lecture des modèles installés : un
// modèle supprimé depuis redevient téléchargeable.

import { CheckCircleIcon, LockClosedIcon } from "@heroicons/react/24/outline";
import { ArrowDownTrayIcon } from "@heroicons/react/20/solid";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import type { HubQuant } from "@/bindings/HubQuant";
import type { HubRepo } from "@/bindings/HubRepo";
import { CopyButton } from "@/components/CopyButton";
import { ExternalLink } from "@/components/ExternalLink";
import { DownloadProgress } from "@/components/hub/DownloadProgress";
import { FitPill, Footprint } from "@/components/hub/ModelBits";
import {
  CARD,
  PRIMARY_BUTTON,
  SECONDARY_BUTTON,
} from "@/components/hub/styles";
import {
  findDownload,
  useDownloadActions,
  useDownloads,
} from "@/hooks/useDownloads";
import { useHubHardware, useHubInstalled, useUseInChat } from "@/hooks/useHub";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import {
  formatContext,
  hasGpu,
  huggingFaceUrl,
  isActiveDownload,
  repoParams,
} from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import { displayTag } from "@/lib/modelNames";

/** Versions montrées avant « Show all ». */
const VISIBLE_VERSIONS = 6;

export function VersionPanel({ repo }: { repo: HubRepo }) {
  const t = useT();
  const headingId = useId();
  return (
    <div className={`${CARD} p-4`}>
      <h3
        id={headingId}
        className="font-rounded text-base font-medium text-neutral-900 dark:text-white"
      >
        {t("hub.versions.title")}
      </h3>
      {repo.gated ? (
        <Restricted id={repo.model.id} />
      ) : repo.quants.length === 0 ? (
        <p className="mt-3 text-sm text-neutral-600 dark:text-neutral-300">
          {t("hub.versions.none")}
        </p>
      ) : (
        <Versions repo={repo} headingId={headingId} />
      )}
      {repo.otherSources.length > 0 && (
        <div className="mt-4 border-t border-neutral-200 pt-4 dark:border-neutral-800">
          <h4 className="text-xs text-neutral-500 dark:text-neutral-400">
            {t("hub.versions.otherSources")}
          </h4>
          <ul className="mt-1.5 flex flex-col gap-1">
            {repo.otherSources.map((id) => (
              <li key={id}>
                <Link
                  to="/hub/model/$author/$repo"
                  params={repoParams(id)}
                  className="block truncate rounded-md text-sm text-neutral-700 underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-200"
                >
                  {id}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function Restricted({ id }: { id: string }) {
  const t = useT();
  return (
    <div className="mt-3 flex gap-3 text-sm text-neutral-600 dark:text-neutral-300">
      <LockClosedIcon aria-hidden="true" className="mt-0.5 h-5 w-5 flex-none" />
      <div>
        <p>{t("hub.status.restricted")}</p>
        <ExternalLink
          href={huggingFaceUrl(id)}
          className="mt-2 inline-block underline underline-offset-2"
        >
          {t("hub.actions.viewOnHuggingFace")}
        </ExternalLink>
      </div>
    </div>
  );
}

function Versions({ repo, headingId }: { repo: HubRepo; headingId: string }) {
  const t = useT();
  const { quants } = repo;
  const name = useId();
  const [chosenLabel, setChosenLabel] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const hardware = useHubHardware();
  const gpu = hardware.isPending ? null : hasGpu(hardware.data);
  const installed = useHubInstalled();
  const installedNames = new Set(
    (installed.data ?? []).map((model) => model.model.toLowerCase()),
  );

  const recommended =
    quants.find((quant) => quant.recommended && quant.model) ??
    quants.find((quant) => quant.model) ??
    null;
  const chosen =
    quants.find((quant) => quant.label === chosenLabel && quant.model) ??
    recommended;
  // Fenêtre stable autour de la version conseillée (elle ne bouge pas
  // pendant qu'on choisit au clavier), déplacée seulement si la version
  // choisie (dans la liste complète) en sort.
  const windowAt = (index: number) =>
    Math.max(0, Math.min(index - 2, quants.length - VISIBLE_VERSIONS));
  const chosenIndex = chosen ? quants.indexOf(chosen) : 0;
  let start = windowAt(recommended ? quants.indexOf(recommended) : 0);
  if (chosenIndex < start || chosenIndex >= start + VISIBLE_VERSIONS) {
    start = windowAt(chosenIndex);
  }
  const collapsed = !expanded && quants.length > VISIBLE_VERSIONS;
  const visible = collapsed
    ? quants.slice(start, start + VISIBLE_VERSIONS)
    : quants;
  const context = repo.contextUsed;

  return (
    <>
      <div
        role="radiogroup"
        aria-labelledby={headingId}
        className="mt-3 flex flex-col gap-1.5"
      >
        {visible.map((quant) => (
          <VersionOption
            key={quant.label}
            quant={quant}
            name={name}
            checked={quant === chosen}
            installed={
              quant.model !== null &&
              installedNames.has(quant.model.toLowerCase())
            }
            gpu={gpu}
            context={context}
            onChoose={() => setChosenLabel(quant.label)}
          />
        ))}
      </div>
      {quants.length > VISIBLE_VERSIONS && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
          className="mt-2 rounded-md px-1 text-sm text-neutral-500 underline-offset-2 outline-none hover:text-neutral-900 hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:text-white"
        >
          {expanded
            ? t("hub.versions.showFewer")
            : t("hub.versions.showAll", { count: quants.length })}
        </button>
      )}
      {chosen?.model && (
        <DownloadArea
          key={chosen.model}
          quant={chosen}
          model={chosen.model}
          repoId={repo.model.id}
          installed={installedNames.has(chosen.model.toLowerCase())}
          installedReadAt={installed.dataUpdatedAt}
        />
      )}
    </>
  );
}

function VersionOption({
  quant,
  name,
  checked,
  installed,
  gpu,
  context,
  onChoose,
}: {
  quant: HubQuant;
  name: string;
  checked: boolean;
  installed: boolean;
  gpu: boolean | null;
  context: number | null;
  onChoose: () => void;
}) {
  const t = useT();
  const unavailable = quant.model === null;
  const memoryHint =
    quant.memory === null
      ? undefined
      : context
        ? t("hub.versions.memoryContext", {
            size: formatBytes(t, quant.memory),
            context: formatContext(t, context),
          })
        : t("hub.versions.memory", { size: formatBytes(t, quant.memory) });
  return (
    <label
      data-testid="version"
      title={quant.file}
      className={`flex items-start gap-3 rounded-xl border px-3 py-2.5 transition-colors ${
        checked
          ? "border-accent-fg bg-neutral-100 dark:bg-neutral-800"
          : "border-neutral-200 dark:border-neutral-800"
      } ${
        unavailable
          ? "cursor-not-allowed opacity-50"
          : "cursor-pointer hover:bg-neutral-100 dark:hover:bg-neutral-800/60"
      }`}
    >
      <input
        type="radio"
        name={name}
        value={quant.label}
        checked={checked}
        disabled={unavailable}
        onChange={onChoose}
        className="mt-1 h-4 w-4 flex-none accent-neutral-900 dark:accent-white"
      />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="min-w-0 truncate font-mono text-sm font-medium text-neutral-900 dark:text-white">
            {displayTag(quant.label)}
          </span>
          {quant.recommended && (
            <span className="flex-none rounded-md bg-accent px-1.5 text-[11px] leading-5 font-medium text-neutral-900">
              {t("hub.versions.recommended")}
            </span>
          )}
          {installed && (
            <span
              role="img"
              aria-label={t("hub.versions.installed")}
              title={t("hub.versions.installed")}
            >
              <CheckCircleIcon
                aria-hidden="true"
                className="h-4 w-4 flex-none text-neutral-500"
              />
            </span>
          )}
        </span>
        {/* Les deux poids, dits à part : à télécharger, puis en mémoire. */}
        <span className="mt-1 block">
          <Footprint
            download={quant.downloadSize}
            memory={quant.issue ? null : quant.memory}
            gpu={gpu}
            memoryHint={memoryHint}
          />
        </span>
        <span className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-neutral-500 dark:text-neutral-400">
          {quant.issue ? (
            t(`hub.issues.${quant.issue}`)
          ) : (
            <>
              {t(`hub.tiers.${quant.tier}`)}
              <FitPill fit={quant.fit} gpu={gpu} title={memoryHint} />
            </>
          )}
        </span>
      </span>
    </label>
  );
}

/** Ce que permet la version choisie : télécharger, suivre, utiliser. */
function DownloadArea({
  quant,
  model,
  repoId,
  installed,
  installedReadAt,
}: {
  quant: HubQuant;
  model: string;
  repoId: string;
  installed: boolean;
  /** Dernière lecture réussie des modèles installés (0 : jamais). */
  installedReadAt: number;
}) {
  const t = useT();
  const { downloads } = useDownloads();
  const download = findDownload(downloads, model);
  const { start, cancel, resume } = useDownloadActions();
  const openInChat = useUseInChat();
  const [chatError, setChatError] = useState<string | null>(null);
  const label = displayTag(quant.label);
  // Fini après la dernière lecture des installés (relue à la fin) : déjà
  // installé. Fini avant : la liste fait foi (supprimé depuis).
  const done =
    installed ||
    (download?.state === "done" &&
      (download.finishedAt ?? 0) > installedReadAt);

  const startDownload = () =>
    start.mutate({ model, repo: repoId, label: quant.label });

  let action;
  if (download?.state === "paused") {
    // En pause (onglet Downloads) : où il en est, Resume, Cancel.
    action = (
      <div className="flex flex-col gap-3">
        <DownloadProgress download={download} name={label} />
        <button
          type="button"
          className={`${PRIMARY_BUTTON} w-full`}
          disabled={resume.isPending}
          onClick={() => resume.mutate(model)}
        >
          {t("hub.actions.resume")}
        </button>
        <button
          type="button"
          className={`${SECONDARY_BUTTON} w-full`}
          disabled={cancel.isPending}
          onClick={() => cancel.mutate(model)}
        >
          {t("common.actions.cancel")}
        </button>
      </div>
    );
  } else if (download && isActiveDownload(download)) {
    action = (
      <div className="flex flex-col gap-3">
        <DownloadProgress download={download} name={label} />
        <button
          type="button"
          className={`${SECONDARY_BUTTON} w-full`}
          disabled={cancel.isPending}
          onClick={() => cancel.mutate(model)}
        >
          {t("common.actions.cancel")}
        </button>
      </div>
    );
  } else if (done) {
    action = (
      <div className="flex flex-col gap-3">
        <p className="flex items-center gap-1.5 text-sm font-medium text-green-700 dark:text-green-400">
          <CheckCircleIcon aria-hidden="true" className="h-5 w-5" />
          {t("hub.versions.installed")}
        </p>
        <button
          type="button"
          className={`${PRIMARY_BUTTON} w-full`}
          onClick={() => {
            setChatError(null);
            openInChat(model).catch((error: unknown) =>
              setChatError(errorMessage(error)),
            );
          }}
        >
          {t("hub.actions.useInChat")}
        </button>
        {chatError && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {chatError}
          </p>
        )}
      </div>
    );
  } else if (download?.state === "failed") {
    action = (
      <div className="flex flex-col gap-3">
        <p
          role="alert"
          className="text-sm break-words text-red-600 dark:text-red-400"
        >
          {download.error ?? t("hub.downloadState.failed")}
        </p>
        <button
          type="button"
          className={`${PRIMARY_BUTTON} w-full`}
          disabled={start.isPending}
          onClick={startDownload}
        >
          {t("hub.actions.tryAgain")}
        </button>
      </div>
    );
  } else {
    action = (
      <div className="flex flex-col gap-2">
        <button
          type="button"
          className={`${PRIMARY_BUTTON} w-full py-2`}
          disabled={start.isPending}
          onClick={startDownload}
        >
          <ArrowDownTrayIcon aria-hidden="true" className="h-4 w-4" />
          {start.isPending
            ? t("hub.progress.starting")
            : t("hub.versions.download", {
                size: formatBytes(t, quant.downloadSize),
              })}
        </button>
        {start.isError && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {errorMessage(start.error)}
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="mt-4 flex flex-col gap-4">
      {action}
      <div className="border-t border-neutral-200 pt-3 dark:border-neutral-800">
        <div className="text-xs text-neutral-500 dark:text-neutral-400">
          {t("hub.versions.ollamaName")}
        </div>
        <div className="mt-1 flex items-center gap-1">
          <code
            data-testid="ollama-name"
            title={model}
            className="allow-context-menu min-w-0 flex-1 truncate rounded-md bg-neutral-100 px-2 py-1 font-mono text-xs text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200"
          >
            {model}
          </code>
          <CopyButton
            content={model}
            title={t("hub.versions.copyOllamaName")}
            className="text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800"
          />
        </div>
      </div>
    </div>
  );
}
