// Carte d'un dépôt (grilles du ModelHub) : lien vers sa fiche.

import { ArrowDownTrayIcon, HeartIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import type { HubModel } from "@/bindings/HubModel";
import {
  CapabilityIcons,
  FitPill,
  InstalledBadge,
  ModelAvatar,
  TrustLine,
} from "@/components/hub/ModelBits";
import { CARD, CARD_LINK, CHIP, SKELETON } from "@/components/hub/styles";
import { useT } from "@/i18n";
import { formatTimeAgo } from "@/i18n/format";
import {
  familyOf,
  formatContext,
  formatCount,
  formatParameters,
  repoParams,
} from "@/lib/hub";

export function ModelCard({
  model,
  installed,
  gpu,
}: {
  model: HubModel;
  /** Une version de ce dépôt est installée. */
  installed: boolean;
  /** La machine a un GPU (texte de la pastille) ; null : pas encore lue. */
  gpu: boolean | null;
}) {
  const t = useT();
  const updated = model.lastModified
    ? formatTimeAgo(t, model.lastModified)
    : "";
  return (
    <Link
      to="/hub/model/$author/$repo"
      params={repoParams(model.id)}
      draggable={false}
      data-testid="model-card"
      className={`${CARD_LINK} flex h-full flex-col p-4 select-none`}
    >
      <div className="flex items-start gap-3">
        <ModelAvatar
          name={model.name}
          colorKey={model.family ?? familyOf(model.name) ?? model.author}
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h4
              className="min-w-0 truncate font-medium text-neutral-900 dark:text-white"
              title={model.name}
            >
              {model.name}
            </h4>
            {installed && <InstalledBadge />}
          </div>
          <TrustLine author={model.author} trust={model.trust} />
        </div>
      </div>

      <div className="mt-3 flex min-h-6 flex-wrap items-center gap-1.5">
        {model.parameters !== null && (
          <span className={CHIP} title={t("hub.model.facts.parameters")}>
            {formatParameters(t, model.parameters)}
          </span>
        )}
        {model.contextLength !== null && (
          <span className={CHIP} title={t("hub.card.contextLength")}>
            {formatContext(t, model.contextLength)}
          </span>
        )}
        <CapabilityIcons capabilities={model.capabilities} />
      </div>

      {/* Chiffres puis pastille, toujours sur deux lignes : même pied pour
          toutes les cartes de la grille. */}
      <div className="mt-auto flex flex-col items-start gap-2 pt-4 text-xs text-neutral-500 dark:text-neutral-400">
        <span className="flex items-center gap-3">
          <span
            className="inline-flex items-center gap-1"
            title={t("hub.card.downloadsHint", { count: model.downloads })}
          >
            <ArrowDownTrayIcon aria-hidden="true" className="h-3.5 w-3.5" />
            {formatCount(t, model.downloads)}
          </span>
          <span
            className="inline-flex items-center gap-1"
            title={t("hub.card.likesHint", { count: model.likes })}
          >
            <HeartIcon aria-hidden="true" className="h-3.5 w-3.5" />
            {formatCount(t, model.likes)}
          </span>
          {updated && <span title={t("hub.card.lastUpdated")}>{updated}</span>}
        </span>
        <FitPill fit={model.fit} size={model.estimatedSize} gpu={gpu} />
      </div>
    </Link>
  );
}

/** Carte en cours de chargement. */
export function ModelCardSkeleton() {
  return (
    <div aria-hidden="true" className={`${CARD} flex h-40 flex-col p-4`}>
      <div className="flex items-center gap-3">
        <div className={`${SKELETON} h-11 w-11 rounded-xl`} />
        <div className="flex-1 flex flex-col gap-2">
          <div className={`${SKELETON} h-4 w-2/3`} />
          <div className={`${SKELETON} h-3 w-1/3`} />
        </div>
      </div>
      <div className={`${SKELETON} mt-4 h-5 w-1/2`} />
      <div className={`${SKELETON} mt-auto h-4 w-full`} />
    </div>
  );
}

/** Grille des cartes : 1, 2 puis 3 colonnes selon la largeur. */
export function ModelGrid({
  models,
  installedRepos,
  gpu,
  dimmed = false,
}: {
  models: readonly HubModel[];
  /** Dépôts dont une version est installée, en minuscules. */
  installedRepos: ReadonlySet<string>;
  gpu: boolean | null;
  /** Données précédentes pendant le chargement d'un nouveau filtre. */
  dimmed?: boolean;
}) {
  return (
    <ul
      className={`grid gap-3 transition-opacity sm:grid-cols-2 xl:grid-cols-3 ${dimmed ? "opacity-60" : ""}`}
      aria-busy={dimmed || undefined}
    >
      {models.map((model) => (
        <li key={model.id}>
          <ModelCard
            model={model}
            installed={installedRepos.has(model.id.toLowerCase())}
            gpu={gpu}
          />
        </li>
      ))}
    </ul>
  );
}

/** Grille de squelettes. */
export function SkeletonGrid({ count = 6 }: { count?: number }) {
  const t = useT();
  return (
    <div
      className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3"
      role="status"
      aria-label={t("hub.discover.loading")}
    >
      {Array.from({ length: count }, (_, index) => (
        <ModelCardSkeleton key={index} />
      ))}
    </div>
  );
}
