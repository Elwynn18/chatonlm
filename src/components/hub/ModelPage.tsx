// Fiche d'un dépôt (ModelHub, propre à ChatonLM) : en-tête, faits, capacités,
// README, et la colonne « Choose a version » (versions, téléchargement,
// progression, Use in chat, nom Ollama, autres sources). Un README en échec
// a son Retry ; celui du bandeau le relit aussi.

import {
  ArrowPathIcon,
  ArrowTopRightOnSquareIcon,
  ChevronLeftIcon,
} from "@heroicons/react/24/outline";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useCanGoBack, useRouter } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { HubReadme } from "@/bindings/HubReadme";
import type { HubRepo } from "@/bindings/HubRepo";
import { ExternalLink } from "@/components/ExternalLink";
import {
  CapabilityChips,
  ModelAvatar,
  TrustLine,
} from "@/components/hub/ModelBits";
import { Readme } from "@/components/hub/Readme";
import {
  CommandError,
  ProblemState,
  StaleBanner,
} from "@/components/hub/Status";
import {
  CARD,
  PAGE,
  QUIET_LINK,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import { VersionPanel } from "@/components/hub/VersionPanel";
import { hubReadmeKey, useHubReadme, useHubRepo } from "@/hooks/useHub";
import { useT } from "@/i18n";
import { formatTimeAgo, languageName } from "@/i18n/format";
import {
  familyOf,
  formatContext,
  formatCount,
  formatParameters,
  huggingFaceUrl,
} from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";

export function ModelPage({ id }: { id: string }) {
  const query = useHubRepo(id);
  const queryClient = useQueryClient();
  const retry = () => {
    void query.refetch();
    // README en échec : relu avec la fiche.
    void queryClient.refetchQueries({
      queryKey: hubReadmeKey(id),
      predicate: ({ state }) =>
        state.status === "error" ||
        (state.data as HubReadme | undefined)?.problem != null,
    });
  };

  let body;
  if (query.isPending) {
    body = <ModelSkeleton />;
  } else if (query.isError) {
    body = <CommandError message={errorMessage(query.error)} onRetry={retry} />;
  } else if (!query.data.repo) {
    const problem = query.data.status.problem ?? { kind: "notFound" as const };
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
          status={query.data.status}
          updatedAt={query.dataUpdatedAt}
          onRetry={retry}
        />
        <RepoDetails repo={query.data.repo} />
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

/** Retour à la page précédente (adresse et filtres compris), sinon à Découvrir. */
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
    <Link to="/hub" className={`${QUIET_LINK} -ml-1 mb-4 px-1`}>
      <ChevronLeftIcon aria-hidden="true" className="h-4 w-4" />
      {t("hub.tabs.discover")}
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

function RepoDetails({ repo }: { repo: HubRepo }) {
  const t = useT();
  const { model } = repo;
  return (
    <article aria-labelledby="model-title">
      <header className="flex flex-wrap items-center gap-4">
        <ModelAvatar
          name={model.name}
          colorKey={model.family ?? familyOf(model.name) ?? model.author}
          size="lg"
        />
        <div className="min-w-0 flex-1">
          <h2
            id="model-title"
            className="font-rounded text-2xl font-semibold tracking-tight break-words text-neutral-900 dark:text-white"
          >
            {model.name}
          </h2>
          <div className="mt-1">
            <TrustLine author={model.author} trust={model.trust} />
          </div>
        </div>
        <HuggingFaceLink id={model.id} />
      </header>

      {/* Deux lignes à gauche (détails, puis About) le long de la colonne des
          versions : la première à sa hauteur, About en haut de la seconde,
          sans s'étirer quand la colonne de droite est plus haute. */}
      <div className="mt-8 grid gap-6 lg:grid-cols-[minmax(0,1fr)_21rem] lg:grid-rows-[auto_1fr]">
        <section
          aria-label={t("hub.model.details")}
          className="min-w-0 flex flex-col gap-6"
        >
          <Facts repo={repo} />
          {(model.capabilities.tools ||
            model.capabilities.thinking ||
            model.capabilities.vision ||
            model.capabilities.code) && (
            <div>
              <CapabilityChips capabilities={model.capabilities} />
              <p className="mt-2 text-xs text-neutral-500 dark:text-neutral-400">
                {t("hub.model.announced")}
              </p>
            </div>
          )}
        </section>

        <aside
          aria-label={t("hub.versions.title")}
          className="lg:sticky lg:top-4 lg:col-start-2 lg:row-span-2 lg:row-start-1 lg:self-start"
        >
          <VersionPanel repo={repo} />
        </aside>

        <div className="min-w-0 self-start">
          <About repo={repo} />
        </div>
      </div>
    </article>
  );
}

function Facts({ repo }: { repo: HubRepo }) {
  const t = useT();
  const { model } = repo;
  const facts: [string, ReactNode][] = [];
  if (model.parameters !== null) {
    facts.push([
      t("hub.model.facts.parameters"),
      formatParameters(t, model.parameters),
    ]);
  }
  if (model.contextLength !== null) {
    facts.push([
      t("hub.model.facts.context"),
      t("hub.model.facts.contextValue", {
        context: formatContext(t, model.contextLength),
      }),
    ]);
  }
  if (model.architecture) {
    facts.push([t("hub.model.facts.architecture"), model.architecture]);
  }
  if (model.license) facts.push([t("hub.model.facts.license"), model.license]);
  facts.push([t("hub.model.facts.downloads"), formatCount(t, model.downloads)]);
  if (model.lastModified) {
    facts.push([
      t("hub.model.facts.updated"),
      formatTimeAgo(t, model.lastModified),
    ]);
  }
  if (repo.languages.length > 0) {
    const names = repo.languages.map((code) => languageName(t.lang, code));
    facts.push([
      t("hub.model.facts.languages"),
      <span title={names.join(", ")}>
        {names.slice(0, 3).join(", ")}
        {names.length > 3 && ` +${names.length - 3}`}
      </span>,
    ]);
  }
  return (
    <dl
      className={`${CARD} grid grid-cols-2 gap-x-6 gap-y-4 p-5 sm:grid-cols-3`}
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

function About({ repo }: { repo: HubRepo }) {
  const t = useT();
  const id = repo.model.id;
  const readme = useHubReadme(id, repo.sha ?? "main");
  let content;
  if (readme.isPending) {
    content = (
      <div aria-hidden="true" className="flex flex-col gap-2">
        <div className={`${SKELETON} h-4 w-1/2`} />
        <div className={`${SKELETON} h-3 w-full`} />
        <div className={`${SKELETON} h-3 w-5/6`} />
      </div>
    );
  } else if (readme.isError || !readme.data.text) {
    const failed = readme.isError || readme.data.problem !== null;
    content = (
      <div className="flex flex-wrap items-center gap-3">
        <p className="min-w-0 flex-1 text-sm text-neutral-500 dark:text-neutral-400">
          {failed ? t("hub.model.readmeFailed") : t("hub.model.noReadme")}{" "}
          <ExternalLink
            href={huggingFaceUrl(id)}
            className="underline underline-offset-2"
          >
            {t("hub.actions.viewOnHuggingFace")}
          </ExternalLink>
        </p>
        {failed && (
          <button
            type="button"
            className={`${SECONDARY_BUTTON} flex-none`}
            onClick={() => void readme.refetch()}
          >
            <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
            {t("common.actions.retry")}
          </button>
        )}
      </div>
    );
  } else {
    content = <Readme markdown={readme.data.text} repoId={id} />;
  }
  return (
    <section
      aria-labelledby="about-title"
      className={`${CARD} min-w-0 p-5 lg:col-start-1`}
    >
      <h3
        id="about-title"
        className="mb-3 font-rounded text-lg font-medium text-neutral-900 dark:text-white"
      >
        {t("hub.model.about")}
      </h3>
      {content}
    </section>
  );
}

function ModelSkeleton() {
  const t = useT();
  return (
    <div role="status" aria-label={t("hub.model.loading")}>
      <div className="flex items-center gap-4" aria-hidden="true">
        <div className={`${SKELETON} h-16 w-16 rounded-2xl`} />
        <div className="flex-1 flex flex-col gap-2">
          <div className={`${SKELETON} h-6 w-1/3`} />
          <div className={`${SKELETON} h-3 w-1/5`} />
        </div>
      </div>
      <div
        className="mt-8 grid gap-6 lg:grid-cols-[minmax(0,1fr)_21rem]"
        aria-hidden="true"
      >
        <div className={`${SKELETON} h-40 rounded-2xl`} />
        <div className={`${SKELETON} h-72 rounded-2xl`} />
      </div>
    </div>
  );
}
