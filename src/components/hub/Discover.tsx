// Page Découvrir du ModelHub (propre à ChatonLM) : recherche, machine,
// filtres, puis selon l'adresse l'accueil (rubriques et familles), la liste
// complète d'une rubrique ou d'une famille, ou les résultats d'une recherche
// sur Hugging Face. L'adresse porte l'état (q, family, section, filtres) ;
// le champ de recherche la met à jour 400 ms après la dernière frappe.
//
// Écart au contrat (37 § 10) : « Search Hugging Face for “{label}” » suit la
// liste d'une famille seulement, pas celle d'une rubrique (Trending, For
// coding...) : chercher le nom d'une rubrique sur Hugging Face ne donnerait
// rien d'utile.

import {
  ChevronLeftIcon,
  ChevronRightIcon,
  FunnelIcon,
  MagnifyingGlassIcon,
} from "@heroicons/react/24/outline";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  Fragment,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import type { HubFamily } from "@/bindings/HubFamily";
import type { HubFilters } from "@/bindings/HubFilters";
import type { HubScope } from "@/bindings/HubScope";
import type { HubSection } from "@/bindings/HubSection";
import {
  FilterChips,
  MachinePill,
  SearchField,
} from "@/components/hub/Filters";
import { ModelGrid, SkeletonGrid } from "@/components/hub/ModelCard";
import {
  CommandError,
  EmptyState,
  ProblemState,
  StaleBanner,
} from "@/components/hub/Status";
import {
  PAGE,
  QUIET_LINK,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import {
  useHubBrowse,
  useHubHardware,
  useHubHome,
  useHubInstalled,
  useHubSearch,
} from "@/hooks/useHub";
import { useT } from "@/i18n";
import {
  filtersFromParams,
  GRADIENT_CLASS,
  gradientStyle,
  hasGpu,
  type HubParams,
  SEARCH_DEBOUNCE_MS,
  searchText,
  usesParam,
} from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import { overlayOpen } from "@/lib/overlay";
import { isSearchShortcut } from "@/lib/shortcuts";

/** Cible où « / » s'écrit (champ, zone éditable) : pas de raccourci. */
function typingIn(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target.closest("input, textarea, select, [contenteditable='true']") !==
        null)
  );
}

export function Discover({ params }: { params: HubParams }) {
  const t = useT();
  const navigate = useNavigate({ from: "/hub/" });
  const update = useCallback(
    (patch: Partial<HubParams>) =>
      void navigate({
        search: (previous) => ({ ...previous, ...patch }),
        replace: true,
      }),
    [navigate],
  );

  const query = searchText(params.q ?? "");
  const searching = query !== "";
  const filters = useMemo(
    () => filtersFromParams(params, searching),
    [params, searching],
  );

  // Texte du champ : local (frappe fluide), envoyé à l'adresse après une
  // pause ; repris de l'adresse quand elle change d'ailleurs (lien
  // « Search Hugging Face for »).
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
      update({ q: target || undefined });
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text, query, update]);
  const clearSearch = () => {
    setText("");
    setSync((current) => ({ ...current, pushed: "" }));
    update({ q: undefined });
  };

  // « / » : aller au champ de recherche, sauf pendant une saisie.
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isSearchShortcut(event)) return;
      if (typingIn(event.target) || overlayOpen()) return;
      event.preventDefault();
      inputRef.current?.focus();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const hardware = useHubHardware();
  const installed = useHubInstalled();
  const installedRepos = useMemo(
    () =>
      new Set(
        (installed.data ?? []).flatMap((model) =>
          model.repo ? [model.repo.toLowerCase()] : [],
        ),
      ),
    [installed.data],
  );
  // Machine pas encore lue : pastilles attendues (null).
  const gpu = hardware.isPending ? null : hasGpu(hardware.data);
  const shared = { filters, installedRepos, gpu };

  return (
    <div className={PAGE}>
      <header className="pt-4">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div className="min-w-0">
            <h2 className="font-rounded text-2xl font-semibold tracking-tight text-neutral-900 dark:text-white">
              {t("hub.discover.title")}
            </h2>
            <p className="mt-1 text-sm text-neutral-500 dark:text-neutral-400">
              {t("hub.discover.subtitle")}
            </p>
          </div>
          {!hardware.isError && (
            <MachinePill
              hardware={hardware.isSuccess ? hardware.data : undefined}
            />
          )}
        </div>
        <div className="mt-5">
          <SearchField
            value={text}
            onChange={setText}
            onClear={clearSearch}
            inputRef={inputRef}
          />
        </div>
        <div className="mt-4">
          <FilterChips
            value={filters}
            searching={searching}
            canFit={hardware.data != null}
            onChange={(next) =>
              update({
                uses: usesParam(next.uses),
                size: next.size ?? undefined,
                license: next.openLicense ? "open" : undefined,
                verified: next.verifiedOnly || undefined,
              })
            }
          />
        </div>
      </header>

      <div className="mt-8">
        {searching ? (
          <SearchView key="search" query={query} {...shared} />
        ) : params.family ? (
          <BrowseView
            key="family"
            scope={{ kind: "family", id: params.family }}
            {...shared}
          />
        ) : params.section ? (
          <BrowseView
            key="section"
            scope={{ kind: "section", id: params.section }}
            {...shared}
          />
        ) : (
          <HomeView {...shared} />
        )}
      </div>
    </div>
  );
}

interface ViewProps {
  filters: HubFilters;
  installedRepos: ReadonlySet<string>;
  gpu: boolean | null;
}

/** Aucun modèle avec ces filtres : proposition de les retirer. */
function NoMatch() {
  const t = useT();
  const navigate = useNavigate({ from: "/hub/" });
  return (
    <EmptyState
      icon={<FunnelIcon aria-hidden="true" className="h-6 w-6" />}
      title={t("hub.discover.noMatch")}
    >
      <button
        type="button"
        className={SECONDARY_BUTTON}
        onClick={() =>
          void navigate({
            search: (previous) => ({
              ...previous,
              uses: undefined,
              size: undefined,
              license: undefined,
              verified: undefined,
            }),
            replace: true,
          })
        }
      >
        {t("hub.filters.reset")}
      </button>
    </EmptyState>
  );
}

function HomeView({ filters, installedRepos, gpu }: ViewProps) {
  const home = useHubHome(filters);
  const retry = () => void home.refetch();
  if (home.isPending) return <HomeSkeleton />;
  if (home.isError) {
    return <CommandError message={errorMessage(home.error)} onRetry={retry} />;
  }
  const { sections, families, status } = home.data;
  if (sections.length === 0 && families.length === 0) {
    return status.problem ? (
      <ProblemState
        problem={status.problem}
        updatedAt={home.dataUpdatedAt}
        onRetry={retry}
      />
    ) : (
      <NoMatch />
    );
  }
  const familiesBlock = families.length > 0 && <Families families={families} />;
  return (
    <div
      className={`transition-opacity ${home.isPlaceholderData ? "opacity-60" : ""}`}
      aria-busy={home.isPlaceholderData || undefined}
    >
      <StaleBanner
        status={status}
        updatedAt={home.dataUpdatedAt}
        onRetry={retry}
      />
      {sections.map((section, index) => (
        <Fragment key={section.id}>
          <SectionBlock
            section={section}
            installedRepos={installedRepos}
            gpu={gpu}
          />
          {/* Familles après les deux premières rubriques : une entrée
              colorée sans attendre la fin de la page. */}
          {index === 1 && familiesBlock}
        </Fragment>
      ))}
      {sections.length < 2 && familiesBlock}
    </div>
  );
}

function SectionBlock({
  section,
  installedRepos,
  gpu,
}: {
  section: HubSection;
  installedRepos: ReadonlySet<string>;
  gpu: boolean | null;
}) {
  const t = useT();
  const headingId = useId();
  const title = t(`hub.sections.${section.id}`);
  return (
    <section aria-labelledby={headingId} className="mb-10">
      <div className="mb-3 flex items-baseline justify-between gap-3">
        <h3
          id={headingId}
          className="font-rounded text-lg font-medium text-neutral-900 dark:text-white"
        >
          {title}
        </h3>
        <Link
          to="/hub"
          search={(previous) => ({
            ...previous,
            section: section.id,
            family: undefined,
          })}
          aria-label={t("hub.discover.seeAllLabel", { section: title })}
          className={QUIET_LINK}
        >
          {t("hub.discover.seeAll")}
          <ChevronRightIcon aria-hidden="true" className="h-3.5 w-3.5" />
        </Link>
      </div>
      <ModelGrid
        models={section.models}
        installedRepos={installedRepos}
        gpu={gpu}
      />
    </section>
  );
}

function Families({ families }: { families: readonly HubFamily[] }) {
  const t = useT();
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="mb-10">
      <h3
        id={headingId}
        className="mb-3 font-rounded text-lg font-medium text-neutral-900 dark:text-white"
      >
        {t("hub.discover.families")}
      </h3>
      <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
        {families.map((family) => (
          <li key={family.id}>
            <FamilyTile family={family} />
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Tuile d'une famille : dégradé doux, grande initiale en filigrane. */
function FamilyTile({ family }: { family: HubFamily }) {
  const t = useT();
  return (
    <Link
      to="/hub"
      search={(previous) => ({
        ...previous,
        family: family.id,
        section: undefined,
      })}
      draggable={false}
      style={gradientStyle(family.id)}
      className={`relative flex h-24 flex-col justify-end overflow-hidden rounded-2xl p-4  transition duration-150 outline-none select-none hover:-translate-y-0.5 hover:shadow-md focus-visible:ring-2 focus-visible:ring-accent-fg ${GRADIENT_CLASS}`}
    >
      <span
        aria-hidden="true"
        className="pointer-events-none absolute -top-3 -right-1 font-rounded text-7xl font-bold opacity-10"
      >
        {family.label.charAt(0).toUpperCase()}
      </span>
      <span className="relative truncate font-rounded text-base font-semibold">
        {family.label}
      </span>
      <span className="relative text-xs opacity-75">
        {t("hub.discover.modelCount", { count: family.count })}
      </span>
    </Link>
  );
}

function HomeSkeleton() {
  const t = useT();
  return (
    <div role="status" aria-label={t("hub.discover.loading")}>
      {[0, 1].map((block) => (
        <div key={block} className="mb-10" aria-hidden="true">
          <div className={`${SKELETON} mb-4 h-5 w-40`} />
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {[0, 1, 2].map((card) => (
              <div
                key={card}
                className="h-40 rounded-2xl border border-neutral-200 p-4 dark:border-neutral-800"
              >
                <div className="flex items-center gap-3">
                  <div className={`${SKELETON} h-11 w-11 rounded-xl`} />
                  <div className="flex-1 flex flex-col gap-2">
                    <div className={`${SKELETON} h-4 w-2/3`} />
                    <div className={`${SKELETON} h-3 w-1/3`} />
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

/** Liste complète d'une rubrique ou d'une famille. */
function BrowseView({
  scope,
  filters,
  installedRepos,
  gpu,
}: ViewProps & { scope: HubScope }) {
  const t = useT();
  const list = useHubBrowse(scope, filters);
  // Libellé d'une famille : celui de l'accueil (déjà en cache d'ordinaire).
  const home = useHubHome(filters, scope.kind === "family");
  const familyLabel =
    scope.kind === "family"
      ? (home.data?.families.find((family) => family.id === scope.id)?.label ??
        scope.id.charAt(0).toUpperCase() + scope.id.slice(1))
      : null;
  const title =
    scope.kind === "section" ? t(`hub.sections.${scope.id}`) : familyLabel!;
  const retry = () => void list.refetch();
  const models = list.data?.models ?? [];

  let body;
  if (list.isPending) {
    body = <SkeletonGrid />;
  } else if (list.isError) {
    body = <CommandError message={errorMessage(list.error)} onRetry={retry} />;
  } else if (models.length === 0) {
    body = list.data.status.problem ? (
      <ProblemState
        problem={list.data.status.problem}
        updatedAt={list.dataUpdatedAt}
        onRetry={retry}
      />
    ) : (
      <NoMatch />
    );
  } else {
    body = (
      <>
        <StaleBanner
          status={list.data.status}
          updatedAt={list.dataUpdatedAt}
          onRetry={retry}
        />
        <ModelGrid
          models={models}
          installedRepos={installedRepos}
          gpu={gpu}
          dimmed={list.isPlaceholderData}
        />
      </>
    );
  }

  return (
    <section aria-label={title}>
      <Link
        to="/hub"
        search={(previous) => ({
          ...previous,
          family: undefined,
          section: undefined,
        })}
        className={`${QUIET_LINK} -ml-1 px-1`}
      >
        <ChevronLeftIcon aria-hidden="true" className="h-4 w-4" />
        {t("hub.discover.allModels")}
      </Link>
      <div className="mt-2 mb-4 flex items-baseline gap-3">
        <h3 className="font-rounded text-xl font-medium text-neutral-900 dark:text-white">
          {title}
        </h3>
        {list.isSuccess && models.length > 0 && (
          <span className="text-sm text-neutral-500 dark:text-neutral-400">
            {t("hub.discover.modelCount", { count: models.length })}
          </span>
        )}
      </div>
      {body}
      {/* Famille seulement : voir l'en-tête du fichier. */}
      {familyLabel && (
        <div className="mt-8 flex justify-center">
          <Link
            to="/hub"
            search={(previous) => ({
              ...previous,
              q: familyLabel,
              family: undefined,
              section: undefined,
            })}
            className={SECONDARY_BUTTON}
          >
            <MagnifyingGlassIcon aria-hidden="true" className="h-4 w-4" />
            {t("hub.discover.searchFor", { query: familyLabel })}
          </Link>
        </div>
      )}
    </section>
  );
}

/** Résultats d'une recherche, page par page (« Load more »). */
function SearchView({
  query,
  filters,
  installedRepos,
  gpu,
}: ViewProps & { query: string }) {
  const t = useT();
  const search = useHubSearch(query, filters);
  const pages = search.data?.pages ?? [];
  const models = pages.flatMap((page) => page.models);
  const status = pages.at(-1)?.status ?? null;
  const retry = () => void search.refetch();

  let body;
  if (search.isPending) {
    body = <SkeletonGrid />;
  } else if (search.isError && pages.length === 0) {
    body = (
      <CommandError message={errorMessage(search.error)} onRetry={retry} />
    );
  } else if (models.length === 0) {
    body = status?.problem ? (
      <ProblemState
        problem={status.problem}
        updatedAt={search.dataUpdatedAt}
        onRetry={retry}
      />
    ) : (
      <EmptyState
        icon={<MagnifyingGlassIcon aria-hidden="true" className="h-6 w-6" />}
        title={t("hub.discover.noResults")}
      />
    );
  } else {
    body = (
      <>
        {status && (
          <StaleBanner
            status={status}
            updatedAt={search.dataUpdatedAt}
            onRetry={retry}
          />
        )}
        <ModelGrid
          models={models}
          installedRepos={installedRepos}
          gpu={gpu}
          dimmed={search.isPlaceholderData}
        />
        {search.isFetchNextPageError && (
          <p
            role="alert"
            className="mt-4 text-center text-sm text-red-600 dark:text-red-400"
          >
            {errorMessage(search.error)}
          </p>
        )}
        {search.hasNextPage && (
          <div className="mt-6 flex justify-center">
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={search.isFetchingNextPage}
              onClick={() => void search.fetchNextPage()}
            >
              {search.isFetchingNextPage
                ? t("hub.discover.loadingMore")
                : t("hub.discover.loadMore")}
            </button>
          </div>
        )}
      </>
    );
  }

  const heading = t("hub.discover.results", { query });
  return (
    <section aria-label={heading}>
      <h3 className="mb-4 font-rounded text-xl font-medium text-neutral-900 dark:text-white">
        {heading}
      </h3>
      {body}
    </section>
  );
}
