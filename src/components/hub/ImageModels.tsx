// Onglet Images du ModelHub (propre à ChatonLM ; contrat
// .tools/research/83-image-models-contract.md) : modèles de la génération
// d'images, choisis librement sur Hugging Face et téléchargés par ChatonLM
// avec les pièces annexes de leur famille (encodeur de texte, décodeur).
// D'abord ce que la machine permet (moteur absent ; processeur sans AVX2 :
// génération impossible ; pas de carte graphique : lent), puis « Your image
// models » (installés, en cours, en échec : Use / In use, corbeille avec
// confirmation, progression et Cancel, Try again et Dismiss), les
// recommandations non installées (Download, taille qui reste à
// télécharger), enfin la recherche sur Hugging Face (paramètre `q` de
// l'adresse, mis à jour après une pause de frappe ; sans texte, les dépôts
// les plus téléchargés), dont chaque carte ouvre la fiche du dépôt
// (ImageRepo.tsx). Un modèle non censuré porte un badge ; la recherche ne les
// rend qu'avec le réglage « Show uncensored models » (moteur). Aussi la ligne
// d'un téléchargement de l'onglet Downloads (section « Image models »).

import {
  ArrowDownTrayIcon,
  MagnifyingGlassIcon,
  TrashIcon,
  XCircleIcon,
} from "@heroicons/react/24/outline";
import { Link, useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { ImageMachine } from "@/bindings/ImageMachine";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ImageRepoSummary } from "@/bindings/ImageRepoSummary";
import type { ImageSearchResult } from "@/bindings/ImageSearchResult";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ExternalLink } from "@/components/ExternalLink";
import { SearchField } from "@/components/hub/Filters";
import { ModelAvatar } from "@/components/hub/ModelBits";
import {
  DownloadRowView,
  InUse,
  RecommendedBadge,
  ROW_TITLE,
  SpeechProgress,
} from "@/components/hub/SpeechModels";
import {
  CommandError,
  EmptyState,
  ProblemState,
  StaleBanner,
} from "@/components/hub/Status";
import {
  CARD,
  CARD_LINK,
  CHIP,
  ICON_BUTTON,
  PAGE,
  PRIMARY_BUTTON,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import {
  splitImageModelId,
  useImageActions,
  useImageDownloads,
  useImageMachine,
  useImageModels,
  useImageSearch,
} from "@/hooks/useImageModels";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { isActiveSpeechDownload } from "@/lib/dictation";
import {
  formatCount,
  huggingFaceUrl,
  repoParams,
  SEARCH_DEBOUNCE_MS,
  searchText,
} from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import type { ImageParams } from "@/routes/hub.images.index";

const LINK =
  "rounded-sm underline underline-offset-2 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-white";

/** Titre d'une section de la page. */
const SECTION_TITLE =
  "font-rounded text-lg font-medium text-neutral-900 dark:text-white";

/** La génération est impossible sur cette machine (processeur). */
function cannotGenerate(machine: ImageMachine | undefined): boolean {
  return (machine?.missingCpuFeatures.length ?? 0) > 0;
}

/** Badge « Uncensored » (modèle montré seulement avec le réglage). */
export function UncensoredBadge() {
  const t = useT();
  return (
    <span
      title={t("hub.images.uncensoredHint")}
      className="inline-flex flex-none items-center rounded-md border border-amber-600 px-1.5 text-[11px] leading-4 font-medium text-amber-700 dark:border-amber-400 dark:text-amber-400"
    >
      {t("hub.images.uncensored")}
    </span>
  );
}

/**
 * Ce que la machine permet : moteur absent ; processeur sans les
 * instructions du moteur (génération impossible, en rouge) ; pas de carte
 * graphique (ambre : images faites par le processeur). Rien sinon, ni tant
 * que la machine n'est pas lue.
 */
export function MachineNotice({ machine }: { machine: ImageMachine }) {
  const t = useT();
  const notes: { key: string; text: string; error: boolean }[] = [];
  if (!machine.engineAvailable) {
    notes.push({
      key: "engine",
      text: t("hub.images.machine.noEngine"),
      error: true,
    });
  }
  if (machine.missingCpuFeatures.length > 0) {
    notes.push({
      key: "cpu",
      text: t("hub.images.machine.missingCpu", {
        features: machine.missingCpuFeatures.join(", "),
      }),
      error: true,
    });
  } else if (machine.engineAvailable && !machine.gpu) {
    notes.push({
      key: "gpu",
      text: t("hub.images.machine.noGpu"),
      error: false,
    });
  }
  if (notes.length === 0) return null;
  return (
    <div data-testid="image-machine" className="mt-4 flex flex-col gap-2">
      {notes.map((note) => (
        <p
          key={note.key}
          data-note={note.key}
          className={`rounded-2xl border px-4 py-2.5 text-sm ${
            note.error
              ? "border-red-600/40 text-red-600 dark:border-red-400/40 dark:text-red-400"
              : "border-amber-600/40 text-amber-700 dark:border-amber-400/40 dark:text-amber-400"
          }`}
        >
          {note.text}
        </p>
      ))}
    </div>
  );
}

export function ImageModels({ params }: { params: ImageParams }) {
  const t = useT();
  const machine = useImageMachine();
  return (
    <div className={PAGE}>
      <header className="pt-4">
        <h2 className="font-rounded text-2xl font-semibold tracking-tight text-neutral-900 dark:text-white">
          {t("hub.images.title")}
        </h2>
        <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-300">
          {t("hub.images.intro")}
        </p>
        {machine.data && <MachineNotice machine={machine.data} />}
      </header>
      <ModelLists machine={machine.data} />
      <SearchBlock params={params} />
    </div>
  );
}

/** Le téléchargement de ce modèle le range dans « Your image models ». */
function keepsInYours(download: SpeechDownload | null): boolean {
  return (
    download !== null &&
    (isActiveSpeechDownload(download) || download.state === "failed")
  );
}

/** « Your image models », puis les recommandations non installées. */
function ModelLists({ machine }: { machine: ImageMachine | undefined }) {
  const t = useT();
  const models = useImageModels();
  const { downloads } = useImageDownloads();
  if (models.isPending) {
    return (
      <div
        role="status"
        aria-label={t("hub.images.loading")}
        className="mt-8 flex flex-col gap-3"
      >
        {[0, 1].map((row) => (
          <div
            key={row}
            aria-hidden="true"
            className={`${SKELETON} h-28 rounded-2xl`}
          />
        ))}
      </div>
    );
  }
  if (models.isError) {
    return (
      <CommandError
        message={errorMessage(models.error)}
        onRetry={() => void models.refetch()}
      />
    );
  }
  const downloadOf = (id: string) =>
    downloads.find((entry) => entry.id === id) ?? null;
  const yours = models.data.filter(
    (model) =>
      model.installed ||
      !model.recommended ||
      keepsInYours(downloadOf(model.id)),
  );
  const recommended = models.data.filter((model) => !yours.includes(model));
  const list = (entries: ImageModelInfo[], label: string) => (
    <ul aria-label={label} className="mt-3 flex flex-col gap-3">
      {entries.map((model) => (
        <ImageModelCard
          key={model.id}
          model={model}
          download={downloadOf(model.id)}
          machine={machine}
        />
      ))}
    </ul>
  );
  return (
    <>
      {yours.length > 0 && (
        <section aria-labelledby="image-yours" className="mt-8">
          <h3 id="image-yours" className={SECTION_TITLE}>
            {t("hub.images.yours")}
          </h3>
          {list(yours, t("hub.images.yours"))}
        </section>
      )}
      {recommended.length > 0 && (
        <section aria-labelledby="image-recommended" className="mt-8">
          <h3 id="image-recommended" className={SECTION_TITLE}>
            {t("hub.images.recommended")}
          </h3>
          {list(recommended, t("hub.images.recommended"))}
        </section>
      )}
    </>
  );
}

function ImageModelCard({
  model,
  download,
  machine,
}: {
  model: ImageModelInfo;
  download: SpeechDownload | null;
  machine: ImageMachine | undefined;
}) {
  const t = useT();
  const {
    download: start,
    retry,
    cancel,
    dismiss,
    remove,
    use,
  } = useImageActions();
  const [confirming, setConfirming] = useState(false);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const cardRef = useRef<HTMLLIElement>(null);
  // Use, Download, Cancel ou Try again remplacés : le focus va au titre.
  useFocusRescue(cardRef, titleRef);
  const titleId = useId();
  const active = download !== null && isActiveSpeechDownload(download);
  const failed =
    !model.installed && download?.state === "failed"
      ? (download.error ?? t("hub.downloadState.failed"))
      : null;
  // Seule l'erreur de la dernière action reste : chaque action efface celle
  // des autres (une erreur d'avant une action réussie ne reste pas).
  const actions = [start, retry, cancel, dismiss, remove, use];
  const only = (current: { reset: () => void }) => {
    for (const action of actions) if (action !== current) action.reset();
  };
  const error = actions.find((action) => action.isError)?.error;
  // Processeur sans les instructions du moteur : ni Download ni Use.
  const blocked = cannotGenerate(machine);
  // Retire le téléchargement fini, échoué ou annulé (et la ligne d'un modèle
  // jamais installé qui n'est pas une recommandation).
  const dismissButton = (
    <button
      key="dismiss"
      type="button"
      className={SECONDARY_BUTTON}
      disabled={dismiss.isPending}
      onClick={() => {
        only(dismiss);
        dismiss.mutate(model.id);
      }}
    >
      {t("hub.actions.dismiss")}
    </button>
  );

  return (
    <li
      ref={cardRef}
      data-testid="image-model"
      aria-labelledby={titleId}
      aria-busy={remove.isPending || undefined}
      className={`${CARD} p-4 transition-opacity ${remove.isPending ? "opacity-50" : ""}`}
    >
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <ModelAvatar name={model.name} colorKey={model.family.id} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h4
              ref={titleRef}
              id={titleId}
              tabIndex={-1}
              className="min-w-0 rounded-md font-medium break-words text-neutral-900 outline-none dark:text-white"
            >
              {model.name}
            </h4>
            {/* Famille, sauf quand elle donne déjà son nom (recommandation). */}
            {model.family.name !== model.name && (
              <span className={CHIP}>{model.family.name}</span>
            )}
            {model.recommended && <RecommendedBadge />}
            {model.uncensored && <UncensoredBadge />}
          </div>
          <p className="mt-1.5 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400">
            <span>
              {model.installed
                ? formatBytes(t, model.sizeBytes)
                : t("hub.images.toDownload", {
                    size: formatBytes(t, model.missingBytes),
                  })}
            </span>
            <span aria-hidden="true">·</span>
            <span>
              {model.license
                ? `${t("hub.images.license")} ${model.license}`
                : t("hub.images.noLicense")}
            </span>
            <span aria-hidden="true">·</span>
            <span className="min-w-0 break-all">
              {t("hub.images.source")}{" "}
              <ExternalLink href={huggingFaceUrl(model.repo)} className={LINK}>
                {model.repo}
              </ExternalLink>
            </span>
          </p>
          {!model.openLicense && (
            <p className="mt-1 text-xs text-amber-700 dark:text-amber-400">
              {t("hub.images.notOpen")}
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
              {errorMessage(error)}
            </p>
          )}
        </div>
        {/* Une clé par action : un bouton qui en remplace un autre est un
            nouveau bouton (le focus va au titre, useFocusRescue). */}
        <div className="flex flex-none flex-wrap items-center justify-end gap-1.5">
          {active ? (
            <button
              key="cancel"
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => {
                only(cancel);
                cancel.mutate(model.id);
              }}
            >
              {t("common.actions.cancel")}
            </button>
          ) : model.installed ? (
            <>
              {model.selected ? (
                <InUse />
              ) : blocked ? null : (
                <button
                  key="use"
                  type="button"
                  className={PRIMARY_BUTTON}
                  disabled={use.isPending}
                  onClick={() => {
                    only(use);
                    use.mutate(model.id);
                  }}
                >
                  {t("hub.images.use")}
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
          ) : failed ? (
            <>
              {!blocked && (
                <button
                  key="retry"
                  type="button"
                  className={PRIMARY_BUTTON}
                  disabled={retry.isPending}
                  onClick={() => {
                    only(retry);
                    retry.mutate(model.id);
                  }}
                >
                  {t("hub.actions.tryAgain")}
                </button>
              )}
              {dismissButton}
            </>
          ) : (
            <>
              {!blocked && (
                <button
                  key="download"
                  type="button"
                  className={PRIMARY_BUTTON}
                  disabled={start.isPending}
                  onClick={() => {
                    only(start);
                    start.mutate({
                      repo: model.repo,
                      file: model.file,
                      revision: model.revision,
                    });
                  }}
                >
                  {t("hub.actions.download")}
                </button>
              )}
              {/* Ligne jamais installée qui n'est pas une recommandation
                  (après Cancel, ou d'avant un redémarrage) : on peut
                  l'oublier, même quand le processeur bloque. */}
              {!model.recommended && dismissButton}
            </>
          )}
        </div>
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("hub.actions.deleteConfirm", { name: model.name })}
          message={
            model.selected
              ? t("hub.images.deleteTextInUse")
              : t("hub.images.deleteText")
          }
          confirmLabel={t("common.actions.delete")}
          onConfirm={() => {
            // La corbeille va disparaître : le focus va au titre de la carte.
            titleRef.current?.focus();
            setConfirming(false);
            only(remove);
            remove.mutate(model.id);
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </li>
  );
}

/**
 * Recherche sur Hugging Face : texte local (frappe fluide), envoyé à
 * l'adresse après une pause (comme Découvrir) ; repris de l'adresse quand
 * elle change d'ailleurs.
 */
function SearchBlock({ params }: { params: ImageParams }) {
  const t = useT();
  const navigate = useNavigate({ from: "/hub/images/" });
  const update = useCallback(
    (q: string | undefined) =>
      void navigate({
        search: (previous) => ({ ...previous, q }),
        replace: true,
      }),
    [navigate],
  );
  const query = searchText(params.q ?? "");
  const urlText = params.q ?? "";
  const [text, setText] = useState(urlText);
  const [sync, setSync] = useState({ url: urlText, pushed: urlText });
  if (sync.url !== urlText) {
    setSync({ url: urlText, pushed: sync.pushed });
    if (urlText !== sync.pushed) setText(urlText);
  }
  useEffect(() => {
    const target = searchText(text);
    if (target === query) return;
    const timer = setTimeout(() => {
      setSync((current) => ({ ...current, pushed: target }));
      update(target || undefined);
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text, query, update]);
  const clearSearch = () => {
    setText("");
    setSync((current) => ({ ...current, pushed: "" }));
    update(undefined);
  };
  const inputRef = useRef<HTMLInputElement>(null);
  const headingId = useId();
  const heading = query
    ? t("hub.images.results", { query })
    : t("hub.images.popular");

  return (
    <section aria-labelledby={headingId} className="mt-10">
      <SearchField
        value={text}
        onChange={setText}
        onClear={clearSearch}
        inputRef={inputRef}
      />
      <h3 id={headingId} className={`${SECTION_TITLE} mt-6 mb-3`}>
        {heading}
      </h3>
      <SearchResults query={query} />
    </section>
  );
}

function SearchResults({ query }: { query: string }) {
  const t = useT();
  const search = useImageSearch(query);
  const retry = () => void search.refetch();
  if (search.isPending) {
    return (
      <div
        role="status"
        aria-label={t("hub.images.searchLoading")}
        className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3"
      >
        {[0, 1, 2].map((card) => (
          <div
            key={card}
            aria-hidden="true"
            className={`${SKELETON} h-28 rounded-2xl`}
          />
        ))}
      </div>
    );
  }
  if (search.isError) {
    return (
      <CommandError message={errorMessage(search.error)} onRetry={retry} />
    );
  }
  const result: ImageSearchResult = search.data;
  if (result.repos.length === 0) {
    return result.problem ? (
      <ProblemState
        problem={result.problem}
        updatedAt={search.dataUpdatedAt}
        onRetry={retry}
      />
    ) : (
      <EmptyState
        icon={<MagnifyingGlassIcon aria-hidden="true" className="h-6 w-6" />}
        title={t("hub.images.noResults")}
      />
    );
  }
  return (
    <div
      className={`transition-opacity ${search.isPlaceholderData ? "opacity-60" : ""}`}
      aria-busy={search.isPlaceholderData || undefined}
    >
      <StaleBanner
        status={{
          problem: result.stale ? result.problem : null,
          fetchedAt: null,
        }}
        updatedAt={search.dataUpdatedAt}
        onRetry={retry}
      />
      <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {result.repos.map((repo) => (
          <li key={repo.id}>
            <RepoCard repo={repo} />
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Carte d'un dépôt trouvé : vers sa fiche. */
function RepoCard({ repo }: { repo: ImageRepoSummary }) {
  const t = useT();
  return (
    <Link
      to="/hub/images/$author/$repo"
      params={repoParams(repo.id)}
      data-testid="image-repo-card"
      draggable={false}
      className={`${CARD_LINK} h-full p-4`}
    >
      <div className="flex items-start gap-3">
        <ModelAvatar
          name={repo.name}
          colorKey={repo.family?.id ?? repo.author}
        />
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-neutral-900 dark:text-white">
            {repo.name}
          </p>
          <p className="truncate text-xs text-neutral-500 dark:text-neutral-400">
            {repo.author}
          </p>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <span className={CHIP}>
          {repo.family ? repo.family.name : t("hub.images.notSupported")}
        </span>
        {repo.gated && (
          <span className={CHIP}>{t("hub.images.restricted")}</span>
        )}
        {repo.uncensored && <UncensoredBadge />}
      </div>
      <p className="mt-2 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400">
        <span>{repo.license ?? t("hub.images.noLicense")}</span>
        <span aria-hidden="true">·</span>
        <span
          className="inline-flex items-center gap-1"
          title={t("hub.images.downloads", { count: repo.downloads })}
        >
          <ArrowDownTrayIcon aria-hidden="true" className="h-3.5 w-3.5" />
          {formatCount(t, repo.downloads)}
        </span>
      </p>
    </Link>
  );
}

/**
 * Téléchargement d'un modèle d'images (onglet Downloads), titré comme sa
 * carte quand les modèles sont lus, lié à la fiche de son dépôt ; modèle
 * supprimé depuis : « No longer installed », Download again.
 */
export function ImageDownloadRow({
  download,
  model,
  removed = false,
}: {
  download: SpeechDownload;
  /** Le modèle, une fois les modèles lus. */
  model?: ImageModelInfo;
  /** Téléchargé puis supprimé. */
  removed?: boolean;
}) {
  const { retry, cancel, dismiss } = useImageActions();
  const name = model?.name ?? download.name;
  const repo = model?.repo ?? splitImageModelId(download.id).repo;
  return (
    <DownloadRowView
      testId="image-download"
      download={download}
      removed={removed}
      title={
        <Link
          to="/hub/images/$author/$repo"
          params={repoParams(repo)}
          className={ROW_TITLE}
        >
          {name}
        </Link>
      }
      name={name}
      colorKey={model?.family.id ?? download.id}
      actions={{ download: retry, cancel, dismiss }}
    />
  );
}
