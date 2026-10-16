// Onglet Images du ModelHub (propre à ChatonLM ; contrats
// .tools/research/83-image-models-contract.md et
// 85-image-hub-clarity-contract.md) : modèles de la génération d'images,
// choisis librement sur Hugging Face et téléchargés par ChatonLM avec les
// pièces annexes de leur famille (encodeur de texte, décodeur). D'abord
// « Comment ça marche » en trois étapes, la carte graphique de la machine
// (neutre) et ce qu'elle ne permet pas (moteur absent ; processeur sans
// AVX2 : génération impossible ; pas de carte graphique : lent), puis « Your
// image models » (installés, en cours, en échec : Use / In use, corbeille
// avec confirmation, progression et Cancel, Try again et Dismiss), les
// recommandations non installées (Download), enfin la recherche sur Hugging
// Face (paramètre `q` de l'adresse, mis à jour après une pause de frappe ;
// sans texte, les dépôts les plus téléchargés), dont chaque carte ouvre la
// fiche du dépôt (ImageRepo.tsx).
// Une carte de modèle dit, pour un débutant : son nom (recommandation, sinon
// famille) et sa variante (infobulle), à quoi il sert (phrase de la famille),
// puis des repères alignés (vitesse, mémoire de la carte avec « tient /
// dépasse », place sur le disque, usage permis) ; fichiers, licences exactes
// et dépôt sont repliés dans « Details ». La recherche ne montre que les
// dépôts pris en charge ; une ligne dit combien sont masqués et permet de les
// voir (paramètre `showUnsupported`), en bas, en retrait, avec leur raison.
// Un modèle non censuré porte un badge ; la recherche ne les rend qu'avec le
// réglage « Show uncensored models » (moteur). Aussi la ligne d'un
// téléchargement de l'onglet Downloads (section « Image models »).

import {
  ArrowDownTrayIcon,
  ArrowTopRightOnSquareIcon,
  CpuChipIcon,
  MagnifyingGlassIcon,
  TrashIcon,
  XCircleIcon,
} from "@heroicons/react/24/outline";
import { Link, useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { ImageMachine } from "@/bindings/ImageMachine";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ImageRepoSummary } from "@/bindings/ImageRepoSummary";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ExternalLink } from "@/components/ExternalLink";
import { SearchField } from "@/components/hub/Filters";
import {
  ComponentList,
  Disclosure,
  Facts,
  IMAGE_LINK,
  ImageIcon,
  MemoryFact,
  SizeFact,
  SpeedFact,
  UsageFact,
  VariantChip,
} from "@/components/hub/ImageBits";
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
import { type TKey, useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { isActiveSpeechDownload } from "@/lib/dictation";
import {
  formatCount,
  huggingFaceUrl,
  repoParams,
  SEARCH_DEBOUNCE_MS,
  searchText,
} from "@/lib/hub";
import {
  familyBlurb,
  imageModelLabel,
  imageModelTitle,
  usageClass,
  usageLabel,
} from "@/lib/imageModels";
import { errorMessage } from "@/lib/ipc";
import type { ImageParams } from "@/routes/hub.images.index";

/** Titre d'une section de la page. */
const SECTION_TITLE =
  "font-rounded text-lg font-medium text-neutral-900 dark:text-white";

/**
 * Carte d'un dépôt non pris en charge ou restreint : en retrait (bordure en
 * pointillés, sans fond de carte), textes lisibles (pas d'opacité).
 */
const MUTED_CARD_LINK =
  "block h-full rounded-2xl border border-dashed border-neutral-300 p-4 transition duration-150 outline-none hover:border-neutral-400 focus-visible:ring-2 focus-visible:ring-accent-fg dark:border-neutral-700 dark:hover:border-neutral-500";

/** Étapes de « How it works ». */
const HOW_TO: TKey[] = [
  "hub.images.howTo.download",
  "hub.images.howTo.use",
  "hub.images.howTo.chat",
];

/** La génération est impossible sur cette machine (processeur). */
function cannotGenerate(machine: ImageMachine | undefined): boolean {
  return (machine?.missingCpuFeatures.length ?? 0) > 0;
}

/** Dépôt que ChatonLM peut télécharger (famille connue, accès libre). */
function isSupported(repo: ImageRepoSummary): boolean {
  return repo.family !== null && !repo.gated;
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
 * La carte graphique de la machine (neutre : nom et mémoire, quand elle est
 * connue), puis ce que la machine ne permet pas : moteur absent ;
 * processeur sans les instructions du moteur (génération impossible, en
 * rouge) ; pas de carte graphique (ambre : images faites par le
 * processeur). Rien sinon, ni tant que la machine n'est pas lue.
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
  const card = machine.gpu
    ? [
        machine.gpuName,
        machine.vramBytes !== null ? formatBytes(t, machine.vramBytes) : null,
      ]
        .filter((part) => part)
        .join(" · ")
    : "";
  if (notes.length === 0 && !card) return null;
  return (
    <div className="mt-4 flex flex-col gap-2">
      {card && (
        <p
          data-testid="image-gpu"
          className="flex items-center gap-2 rounded-2xl border border-neutral-200 px-4 py-2.5 text-sm text-neutral-600 dark:border-neutral-700 dark:text-neutral-300"
        >
          <CpuChipIcon
            aria-hidden="true"
            className="h-4 w-4 flex-none text-neutral-400"
          />
          {t("hub.images.machine.gpu", { card })}
        </p>
      )}
      {notes.length > 0 && (
        <div data-testid="image-machine" className="flex flex-col gap-2">
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
      )}
    </div>
  );
}

/** « How it works » : les trois étapes, sur une ligne. */
function HowItWorks() {
  const t = useT();
  const labelId = useId();
  return (
    <div
      data-testid="image-how-to"
      className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm"
    >
      <p id={labelId} className="font-medium text-neutral-900 dark:text-white">
        {t("hub.images.howTo.title")}
      </p>
      <ol
        aria-labelledby={labelId}
        className="flex flex-wrap items-center gap-x-4 gap-y-2 text-neutral-600 dark:text-neutral-300"
      >
        {HOW_TO.map((key, index) => (
          <li key={key} className="flex items-center gap-2">
            <span
              aria-hidden="true"
              className="flex h-5 w-5 flex-none items-center justify-center rounded-full bg-neutral-200 text-xs font-semibold text-neutral-700 tabular-nums dark:bg-neutral-700 dark:text-neutral-100"
            >
              {index + 1}
            </span>
            {t(key)}
          </li>
        ))}
      </ol>
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
        <HowItWorks />
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
  const variantId = useId();
  const title = imageModelTitle(model);
  const label = imageModelLabel(model);
  const blurb = familyBlurb(t, model.family.id);
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
      aria-labelledby={model.quant ? `${titleId} ${variantId}` : titleId}
      aria-busy={remove.isPending || undefined}
      className={`${CARD} p-4 transition-opacity ${remove.isPending ? "opacity-50" : ""}`}
    >
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <ImageIcon />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h4
              ref={titleRef}
              id={titleId}
              tabIndex={-1}
              className="min-w-0 rounded-md font-medium break-words text-neutral-900 outline-none dark:text-white"
            >
              {title}
            </h4>
            {model.quant && (
              <VariantChip variant={model.quant} id={variantId} />
            )}
            {model.recommended && <RecommendedBadge />}
            {model.uncensored && <UncensoredBadge />}
          </div>
          {blurb && (
            <p
              data-testid="image-blurb"
              className="mt-1 text-sm text-neutral-600 dark:text-neutral-300"
            >
              {blurb}
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
                aria-label={t("hub.actions.deleteNamed", { name: label })}
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
      {/* Sous le texte, aligné sur le titre (icône de 44 px et son écart). */}
      <div className="mt-3 sm:pl-15">
        <Facts>
          <SpeedFact
            measured={model.measuredSeconds}
            estimated={model.estimatedSeconds}
            machine={machine}
          />
          <MemoryFact vramBytes={model.vramBytes} machine={machine} />
          <SizeFact model={model} />
          <UsageFact
            usage={model.usage}
            repo={model.repo}
            license={model.license}
          />
        </Facts>
        {active && (
          <div className="mt-3">
            <SpeechProgress download={download} name={label} />
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
        <div className="mt-3">
          <Disclosure label={t("hub.model.details")}>
            <ComponentList
              components={model.components}
              showPresent={!model.installed}
            />
            <ExternalLink
              href={huggingFaceUrl(model.repo)}
              className={`${IMAGE_LINK} mt-3 inline-flex items-center gap-1 text-xs text-neutral-600 dark:text-neutral-300`}
            >
              {t("hub.actions.viewOnHuggingFace")}
              <ArrowTopRightOnSquareIcon
                aria-hidden="true"
                className="h-3.5 w-3.5"
              />
            </ExternalLink>
          </Disclosure>
        </div>
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("hub.actions.deleteConfirm", { name: label })}
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
 * elle change d'ailleurs. Les dépôts non pris en charge sont montrés sur
 * demande (`showUnsupported` dans l'adresse, gardé pendant la frappe).
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
  const showUnsupported = params.showUnsupported === true;
  const setShowUnsupported = (show: boolean) =>
    void navigate({
      search: (previous) => ({
        ...previous,
        showUnsupported: show ? true : undefined,
      }),
      replace: true,
    });
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
  const headingRef = useRef<HTMLHeadingElement>(null);
  const sectionRef = useRef<HTMLElement>(null);
  // « Hide » retiré (liste relue sans les dépôts masqués) : le focus va au
  // titre de la section, pas au corps de la page.
  useFocusRescue(sectionRef, headingRef);
  const heading = query
    ? t("hub.images.results", { query })
    : t("hub.images.popular");

  return (
    <section ref={sectionRef} aria-labelledby={headingId} className="mt-10">
      <SearchField
        value={text}
        onChange={setText}
        onClear={clearSearch}
        inputRef={inputRef}
      />
      <h3
        ref={headingRef}
        id={headingId}
        tabIndex={-1}
        className={`${SECTION_TITLE} mt-6 mb-3 rounded-md outline-none`}
      >
        {heading}
      </h3>
      <SearchResults
        query={query}
        showUnsupported={showUnsupported}
        onShowUnsupported={setShowUnsupported}
      />
    </section>
  );
}

function SearchResults({
  query,
  showUnsupported,
  onShowUnsupported,
}: {
  query: string;
  showUnsupported: boolean;
  onShowUnsupported: (show: boolean) => void;
}) {
  const t = useT();
  const search = useImageSearch(query, showUnsupported);
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
  const result = search.data;
  const supported = result.repos.filter(isSupported);
  const unsupported = result.repos.filter((repo) => !isSupported(repo));
  const hidden = result.hiddenUnsupported;
  // Ligne « N masqués · Show », ou « montrés · Hide » : même élément d'un
  // état à l'autre (le bouton garde le focus).
  const toggle =
    showUnsupported || hidden > 0 ? (
      <p
        data-testid="image-hidden"
        className="mt-4 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400"
      >
        <span>
          {showUnsupported
            ? t("hub.images.unsupportedShown")
            : t("hub.images.hidden", { count: hidden })}
        </span>
        <span aria-hidden="true">·</span>
        <button
          type="button"
          onClick={() => onShowUnsupported(!showUnsupported)}
          className={`${IMAGE_LINK} font-medium text-neutral-700 dark:text-neutral-200`}
        >
          {showUnsupported ? t("hub.images.hide") : t("hub.images.show")}
        </button>
      </p>
    ) : null;
  if (result.repos.length === 0) {
    if (result.problem) {
      return (
        <ProblemState
          problem={result.problem}
          updatedAt={search.dataUpdatedAt}
          onRetry={retry}
        />
      );
    }
    return (
      <>
        <EmptyState
          icon={<MagnifyingGlassIcon aria-hidden="true" className="h-6 w-6" />}
          title={
            hidden > 0
              ? t("hub.images.noCompatible")
              : t("hub.images.noResults")
          }
        />
        {toggle}
      </>
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
      {supported.length > 0 && (
        <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {supported.map((repo) => (
            <li key={repo.id}>
              <RepoCard repo={repo} />
            </li>
          ))}
        </ul>
      )}
      {toggle}
      {unsupported.length > 0 && (
        <ul
          aria-label={t("hub.images.unsupportedShown")}
          className="mt-3 grid gap-3 sm:grid-cols-2 xl:grid-cols-3"
        >
          {unsupported.map((repo) => (
            <li key={repo.id}>
              <RepoCard repo={repo} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Carte d'un dépôt trouvé, vers sa fiche : nom et auteur, famille et sa
 * phrase, usage permis, téléchargements ; non pris en charge ou restreint :
 * en retrait, avec sa raison.
 */
function RepoCard({ repo }: { repo: ImageRepoSummary }) {
  const t = useT();
  const supported = isSupported(repo);
  const blurb = repo.family ? familyBlurb(t, repo.family.id) : null;
  return (
    <Link
      to="/hub/images/$author/$repo"
      params={repoParams(repo.id)}
      data-testid="image-repo-card"
      data-supported={supported}
      draggable={false}
      className={supported ? `${CARD_LINK} h-full p-4` : MUTED_CARD_LINK}
    >
      <p
        className={`truncate font-medium ${
          supported
            ? "text-neutral-900 dark:text-white"
            : "text-neutral-600 dark:text-neutral-300"
        }`}
      >
        {repo.name}
      </p>
      <p className="truncate text-xs text-neutral-500 dark:text-neutral-400">
        {repo.author}
      </p>
      {supported && repo.family ? (
        <>
          <p className="mt-3 flex flex-wrap items-center gap-1.5 text-sm font-medium text-neutral-800 dark:text-neutral-100">
            <span>{repo.family.name}</span>
            {repo.uncensored && <UncensoredBadge />}
          </p>
          {blurb && (
            <p className="mt-1 line-clamp-2 text-xs text-neutral-600 dark:text-neutral-300">
              {blurb}
            </p>
          )}
        </>
      ) : (
        <p
          data-testid="image-repo-reason"
          className="mt-3 text-xs text-neutral-600 dark:text-neutral-300"
        >
          {repo.gated
            ? t("hub.images.repoCard.restricted")
            : t("hub.images.repoCard.unsupported")}
        </p>
      )}
      <p className="mt-3 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400">
        {supported && (
          <>
            <span className={usageClass(repo.usage)}>
              {usageLabel(t, repo.usage)}
            </span>
            <span aria-hidden="true">·</span>
          </>
        )}
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
 * carte (nom et variante) quand les modèles sont lus, lié à la fiche de son
 * dépôt ; modèle supprimé depuis : « No longer installed », Download again.
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
  const name = model ? imageModelLabel(model) : download.name;
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
