// Fiche d'un dépôt de modèles d'images (ModelHub, propre à ChatonLM ;
// contrat .tools/research/83-image-models-contract.md § 6) : famille,
// licence (liée à la page du dépôt), téléchargements, puis la colonne du
// téléchargement : choix du fichier (boutons radio : quantification, taille,
// « Recommended », « Installed »), pièces annexes de la famille (rôle,
// dépôt et fichier, taille, licence, « déjà sur cet ordinateur »), total à
// télécharger, place libre et Download, désactivé avec sa raison visible
// (dépôt non pris en charge ou restreint, modèle non censuré sans le
// réglage, processeur, place) ; pendant le téléchargement du fichier
// choisi, sa progression et Cancel ; installé, un lien vers l'onglet.
// Hugging Face injoignable : fiche du cache avec son bandeau, sinon l'état
// plein (Retry, sauf dépôt introuvable ou restreint).

import {
  ArrowTopRightOnSquareIcon,
  ChevronLeftIcon,
  XCircleIcon,
} from "@heroicons/react/24/outline";
import { Link, useCanGoBack, useRouter } from "@tanstack/react-router";
import { type ReactNode, type RefObject, useId, useRef, useState } from "react";
import type { ImageComponentRole } from "@/bindings/ImageComponentRole";
import type { ImageMachine } from "@/bindings/ImageMachine";
import type { ImageRepoDetail } from "@/bindings/ImageRepoDetail";
import type { ImageRepoUnsupported } from "@/bindings/ImageRepoUnsupported";
import { ExternalLink } from "@/components/ExternalLink";
import { MachineNotice, UncensoredBadge } from "@/components/hub/ImageModels";
import { InstalledBadge, ModelAvatar } from "@/components/hub/ModelBits";
import {
  RecommendedBadge,
  SpeechProgress,
} from "@/components/hub/SpeechModels";
import {
  CommandError,
  ProblemState,
  StaleBanner,
} from "@/components/hub/Status";
import {
  CARD,
  CHIP,
  PAGE,
  PRIMARY_BUTTON,
  QUIET_LINK,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import {
  useImageActions,
  useImageDownloads,
  useImageMachine,
  useImageRepo,
  useShowUncensoredImages,
} from "@/hooks/useImageModels";
import { type TKey, useT } from "@/i18n";
import { formatBytes, formatTimeAgo } from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import { isActiveSpeechDownload } from "@/lib/dictation";
import { formatCount, huggingFaceUrl } from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";

/** Marge gardée sur le disque en plus du téléchargement (moteur : 100 Mio). */
const SPACE_MARGIN_BYTES = 100 * 1024 ** 2;

const UNSUPPORTED: Record<ImageRepoUnsupported["kind"], TKey> = {
  unknownFamily: "hub.images.repo.unsupported.unknownFamily",
  gated: "hub.images.repo.unsupported.gated",
  noUsableFile: "hub.images.repo.unsupported.noUsableFile",
  unsupportedFamily: "hub.images.repo.unsupported.unsupportedFamily",
};

const ROLES: Record<ImageComponentRole, TKey> = {
  model: "hub.images.repo.roles.model",
  textEncoder: "hub.images.repo.roles.textEncoder",
  vae: "hub.images.repo.roles.vae",
};

const LINK =
  "rounded-sm underline underline-offset-2 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-white";

export function ImageRepo({ id }: { id: string }) {
  const query = useImageRepo(id);
  const retry = () => void query.refetch();

  let body;
  if (query.isPending) {
    body = <RepoSkeleton />;
  } else if (query.isError) {
    body = <CommandError message={errorMessage(query.error)} onRetry={retry} />;
  } else if (query.data.problem && !query.data.stale) {
    const { problem } = query.data;
    // Introuvable ou restreint : réessayer n'y changerait rien.
    const final = problem.kind === "notFound" || problem.kind === "restricted";
    body = (
      <ProblemState
        problem={problem}
        updatedAt={query.dataUpdatedAt}
        onRetry={final ? undefined : retry}
      >
        <HuggingFaceLink id={id} />
      </ProblemState>
    );
  } else {
    body = (
      <>
        <StaleBanner
          status={{ problem: query.data.problem, fetchedAt: null }}
          updatedAt={query.dataUpdatedAt}
          onRetry={retry}
        />
        <RepoDetails detail={query.data} />
      </>
    );
  }
  return (
    <div className={PAGE}>
      <BackLink />
      {body}
    </div>
  );
}

/** Retour à la page précédente (recherche comprise), sinon à l'onglet. */
function BackLink() {
  const t = useT();
  const router = useRouter();
  const canGoBack = useCanGoBack();
  if (canGoBack) {
    return (
      <button
        type="button"
        onClick={() => router.history.back()}
        className={`${QUIET_LINK} -ml-1 mb-4 px-1`}
      >
        <ChevronLeftIcon aria-hidden="true" className="h-4 w-4" />
        {t("hub.model.back")}
      </button>
    );
  }
  return (
    <Link to="/hub/images" className={`${QUIET_LINK} -ml-1 mb-4 px-1`}>
      <ChevronLeftIcon aria-hidden="true" className="h-4 w-4" />
      {t("hub.tabs.images")}
    </Link>
  );
}

function HuggingFaceLink({ id }: { id: string }) {
  const t = useT();
  return (
    <ExternalLink href={huggingFaceUrl(id)} className={SECONDARY_BUTTON}>
      {t("hub.actions.viewOnHuggingFace")}
      <ArrowTopRightOnSquareIcon aria-hidden="true" className="h-4 w-4" />
    </ExternalLink>
  );
}

function RepoDetails({ detail }: { detail: ImageRepoDetail }) {
  const t = useT();
  const machine = useImageMachine();
  const { summary } = detail;
  const titleRef = useRef<HTMLHeadingElement>(null);
  return (
    <article aria-labelledby="image-repo-title">
      <header className="flex flex-wrap items-center gap-4">
        <ModelAvatar
          name={summary.name}
          colorKey={summary.family?.id ?? summary.author}
          size="lg"
        />
        <div className="min-w-0 flex-1">
          <h2
            ref={titleRef}
            id="image-repo-title"
            tabIndex={-1}
            className="rounded-md outline-none font-rounded text-2xl font-semibold tracking-tight break-words text-neutral-900 dark:text-white"
          >
            {summary.name}
          </h2>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-neutral-500 dark:text-neutral-400">
            <span>{summary.author}</span>
            {summary.uncensored && <UncensoredBadge />}
          </div>
        </div>
        <HuggingFaceLink id={summary.id} />
      </header>
      {machine.data && <MachineNotice machine={machine.data} />}

      <div className="mt-8 grid gap-6 lg:grid-cols-[minmax(0,1fr)_24rem]">
        <section aria-label={t("hub.model.details")} className="min-w-0">
          <Facts detail={detail} />
          {!summary.openLicense && (
            <p className="mt-3 text-sm text-amber-700 dark:text-amber-400">
              {t("hub.images.notOpen")}
            </p>
          )}
        </section>
        <aside
          aria-label={t("hub.images.repo.files")}
          className="lg:sticky lg:top-4 lg:self-start"
        >
          <DownloadPanel
            detail={detail}
            machine={machine.data}
            titleRef={titleRef}
          />
        </aside>
      </div>
    </article>
  );
}

function Facts({ detail }: { detail: ImageRepoDetail }) {
  const t = useT();
  const { summary } = detail;
  const facts: [string, ReactNode][] = [
    [
      t("hub.images.repo.family"),
      summary.family?.name ?? t("hub.images.notSupported"),
    ],
    [
      t("hub.model.facts.license"),
      <ExternalLink href={huggingFaceUrl(summary.id)} className={LINK}>
        {summary.license ?? t("hub.images.noLicense")}
      </ExternalLink>,
    ],
    [t("hub.model.facts.downloads"), formatCount(t, summary.downloads)],
  ];
  if (summary.lastModified) {
    facts.push([
      t("hub.model.facts.updated"),
      formatTimeAgo(t, summary.lastModified),
    ]);
  }
  return (
    <dl
      className={`${CARD} grid grid-cols-2 gap-x-6 gap-y-4 p-5 sm:grid-cols-4`}
    >
      {facts.map(([label, value]) => (
        <div key={label} className="min-w-0">
          <dt className="text-xs text-neutral-500 dark:text-neutral-400">
            {label}
          </dt>
          <dd className="mt-0.5 truncate text-sm font-medium text-neutral-900 dark:text-white">
            {value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function DownloadPanel({
  detail,
  machine,
  titleRef,
}: {
  detail: ImageRepoDetail;
  machine: ImageMachine | undefined;
  /** Titre de la fiche : reçoit le focus d'un bouton remplacé. */
  titleRef: RefObject<HTMLHeadingElement | null>;
}) {
  const t = useT();
  const { summary, files, components } = detail;
  const showUncensored = useShowUncensoredImages();
  const { downloads } = useImageDownloads();
  const { download: start, cancel } = useImageActions();
  const panelRef = useRef<HTMLDivElement>(null);
  // Download remplacé par la progression et Cancel, Cancel par Download :
  // le focus va au titre de la fiche, pas au corps de la page (un bouton
  // jamais relabellisé sous le doigt).
  useFocusRescue(panelRef, titleRef);
  const legendId = useId();
  const reasonId = useId();
  const [choice, setChoice] = useState<string | null>(
    () => (files.find((file) => file.recommended) ?? files[0])?.file ?? null,
  );
  // Fiche relue sans le fichier choisi : le conseillé, sinon le premier.
  const chosen =
    files.find((file) => file.file === choice) ??
    files.find((file) => file.recommended) ??
    files[0] ??
    null;
  const modelId = chosen ? `${summary.id}/${chosen.file}` : null;
  const download = downloads.find((entry) => entry.id === modelId) ?? null;
  const active = download !== null && isActiveSpeechDownload(download);
  const missing = components.filter((component) => !component.present);
  const total =
    (chosen && !chosen.installed ? chosen.sizeBytes : 0) +
    missing.reduce((sum, component) => sum + component.sizeBytes, 0);
  const free = machine?.freeBytes ?? null;

  let reason: ReactNode = null;
  if (detail.unsupported) {
    reason = t(UNSUPPORTED[detail.unsupported.kind]);
  } else if (!chosen) {
    reason = t(UNSUPPORTED.noUsableFile);
  } else if (summary.uncensored && showUncensored === false) {
    reason = (
      <Trans
        k="hub.images.repo.reason.uncensored"
        tags={{
          link: (chunks) => (
            <Link
              to="/settings"
              search={{ section: "models" }}
              className={LINK}
            >
              {chunks}
            </Link>
          ),
        }}
      />
    );
  } else if (machine && machine.missingCpuFeatures.length > 0) {
    reason = t("hub.images.repo.reason.cpu", {
      features: machine.missingCpuFeatures.join(", "),
    });
  } else if (free !== null && total + SPACE_MARGIN_BYTES > free) {
    reason = t("hub.images.repo.reason.space", {
      needed: formatBytes(t, total),
      free: formatBytes(t, free),
    });
  }
  const failed =
    download?.state === "failed" && chosen && !chosen.installed
      ? (download.error ?? t("hub.downloadState.failed"))
      : null;
  const error = start.isError
    ? errorMessage(start.error)
    : cancel.isError
      ? errorMessage(cancel.error)
      : null;

  return (
    <div ref={panelRef} className={`${CARD} flex flex-col gap-5 p-5`}>
      {files.length > 0 && (
        <fieldset aria-labelledby={legendId}>
          <legend
            id={legendId}
            className="font-rounded text-base font-medium text-neutral-900 dark:text-white"
          >
            {t("hub.images.repo.files")}
          </legend>
          <div className="mt-3 flex flex-col gap-2">
            {files.map((file) => {
              const checked = chosen?.file === file.file;
              return (
                <label
                  key={file.file}
                  data-testid="image-file"
                  className={`flex cursor-pointer items-start gap-3 rounded-xl border px-3 py-2 ${
                    checked
                      ? "border-accent-fg bg-neutral-100 dark:bg-neutral-800"
                      : "border-neutral-200 dark:border-neutral-800"
                  }`}
                >
                  <input
                    type="radio"
                    name={`image-file-${summary.id}`}
                    value={file.file}
                    checked={checked}
                    onChange={() => {
                      // L'erreur d'un autre fichier ne vaut plus.
                      start.reset();
                      setChoice(file.file);
                    }}
                    className="mt-1 h-4 w-4 flex-none accent-neutral-900 dark:accent-white"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-1.5">
                      <span className="text-sm font-medium text-neutral-900 dark:text-white">
                        {file.quant ?? file.file}
                      </span>
                      <span className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
                        {formatBytes(t, file.sizeBytes)}
                      </span>
                      {file.recommended && <RecommendedBadge />}
                      {file.installed && <InstalledBadge />}
                    </span>
                    {file.quant !== null && (
                      <span className="block truncate font-mono text-xs text-neutral-500 dark:text-neutral-400">
                        {file.file}
                      </span>
                    )}
                  </span>
                </label>
              );
            })}
          </div>
        </fieldset>
      )}

      {components.length > 0 && (
        <section aria-labelledby="image-components">
          <h3
            id="image-components"
            className="font-rounded text-base font-medium text-neutral-900 dark:text-white"
          >
            {t("hub.images.repo.components")}
          </h3>
          <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
            {t("hub.images.repo.componentsHint")}
          </p>
          <ul className="mt-3 flex flex-col gap-2">
            {components.map((component) => (
              <li
                key={`${component.repo}/${component.file}`}
                data-testid="image-component"
                className="rounded-xl border border-neutral-200 px-3 py-2 dark:border-neutral-700"
              >
                <span className="flex flex-wrap items-center gap-1.5">
                  <span className="text-sm font-medium text-neutral-900 dark:text-white">
                    {t(ROLES[component.role])}
                  </span>
                  <span className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
                    {formatBytes(t, component.sizeBytes)}
                  </span>
                  {component.present && (
                    <span className={CHIP}>{t("hub.images.repo.present")}</span>
                  )}
                </span>
                <span className="mt-0.5 block font-mono text-xs break-all text-neutral-500 dark:text-neutral-400">
                  <ExternalLink
                    href={huggingFaceUrl(component.repo)}
                    className={LINK}
                  >
                    {component.repo}
                  </ExternalLink>
                  /{component.file}
                </span>
                <span className="block text-xs text-neutral-500 dark:text-neutral-400">
                  {component.license
                    ? `${t("hub.images.license")} ${component.license}`
                    : t("hub.images.noLicense")}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      <div className="flex flex-col gap-1 text-sm text-neutral-700 dark:text-neutral-200">
        <p data-testid="image-total" className="tabular-nums">
          {t("hub.images.repo.total", { size: formatBytes(t, total) })}
        </p>
        {free !== null && (
          <p
            data-testid="image-free"
            className="text-xs text-neutral-500 tabular-nums dark:text-neutral-400"
          >
            {t("hub.images.repo.freeSpace", { size: formatBytes(t, free) })}
          </p>
        )}
      </div>

      {active ? (
        <div className="flex flex-col gap-3">
          <SpeechProgress download={download} name={chosen?.file ?? ""} />
          <div>
            <button
              key="cancel"
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(download.id)}
            >
              {t("common.actions.cancel")}
            </button>
          </div>
        </div>
      ) : chosen?.installed ? (
        <div className="flex flex-col items-start gap-2">
          <p className="text-sm text-green-800 dark:text-green-400">
            {t("hub.images.repo.installed")}
          </p>
          <Link to="/hub/images" className={SECONDARY_BUTTON}>
            {t("hub.images.repo.yourModels")}
          </Link>
        </div>
      ) : (
        <div className="flex flex-col items-start gap-2">
          <button
            key="download"
            type="button"
            className={PRIMARY_BUTTON}
            disabled={reason !== null || start.isPending}
            aria-describedby={reason !== null ? reasonId : undefined}
            onClick={() => {
              if (!chosen) return;
              start.mutate({
                repo: summary.id,
                file: chosen.file,
                revision: detail.revision,
              });
            }}
          >
            {t("hub.actions.download")}
          </button>
          {reason !== null && (
            <p
              id={reasonId}
              data-testid="image-download-reason"
              className="text-xs text-neutral-600 dark:text-neutral-300"
            >
              {reason}
            </p>
          )}
        </div>
      )}
      {failed && (
        <p
          role="alert"
          className="flex items-start gap-1.5 text-xs break-words text-red-600 dark:text-red-400"
        >
          <XCircleIcon aria-hidden="true" className="h-4 w-4 flex-none" />
          {failed}
        </p>
      )}
      {error && (
        <p
          role="alert"
          className="text-xs break-words text-red-600 dark:text-red-400"
        >
          {error}
        </p>
      )}
    </div>
  );
}

function RepoSkeleton() {
  const t = useT();
  return (
    <div role="status" aria-label={t("hub.model.loading")}>
      <div className="flex items-center gap-4" aria-hidden="true">
        <div className={`${SKELETON} h-16 w-16 rounded-2xl`} />
        <div className="flex flex-1 flex-col gap-2">
          <div className={`${SKELETON} h-6 w-1/3`} />
          <div className={`${SKELETON} h-3 w-1/5`} />
        </div>
      </div>
      <div
        className="mt-8 grid gap-6 lg:grid-cols-[minmax(0,1fr)_24rem]"
        aria-hidden="true"
      >
        <div className={`${SKELETON} h-28 rounded-2xl`} />
        <div className={`${SKELETON} h-72 rounded-2xl`} />
      </div>
    </div>
  );
}
