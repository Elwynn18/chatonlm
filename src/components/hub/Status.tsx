// États des données de Hugging Face (contrat 37 § 9) : bandeau au-dessus de
// données anciennes, ou état plein quand rien n'est en cache ; décompte de la
// limite de requêtes, repris à chaque réponse (`updatedAt`) : une nouvelle
// limite de même durée, données en cache inchangées, le relance aussi ;
// erreur interne d'une commande.

import {
  ArrowPathIcon,
  ClockIcon,
  ExclamationTriangleIcon,
  SignalSlashIcon,
} from "@heroicons/react/24/outline";
import { type ReactNode, useEffect, useState } from "react";
import type { HubProblem } from "@/bindings/HubProblem";
import type { HubStatus } from "@/bindings/HubStatus";
import { SECONDARY_BUTTON } from "@/components/hub/styles";
import { type TFunction, useT } from "@/i18n";
import { formatTimeAgo } from "@/i18n/format";

/** Secondes restantes avant la fin de la limite de requêtes, décomptées. */
function useCountdown(seconds: number): number {
  const [left, setLeft] = useState(seconds);
  useEffect(() => {
    if (left <= 0) return;
    const timer = setTimeout(() => setLeft((value) => value - 1), 1000);
    return () => clearTimeout(timer);
  }, [left]);
  return Math.max(0, left);
}

function RetryButton({
  onRetry,
  disabled = false,
}: {
  onRetry: () => void;
  disabled?: boolean;
}) {
  const t = useT();
  return (
    <button
      type="button"
      onClick={onRetry}
      disabled={disabled}
      className={`${SECONDARY_BUTTON} flex-none`}
    >
      <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
      {t("common.actions.retry")}
    </button>
  );
}

function ProblemIcon({
  problem,
  className,
}: {
  problem: HubProblem;
  className: string;
}) {
  switch (problem.kind) {
    case "offline":
      return <SignalSlashIcon aria-hidden="true" className={className} />;
    case "rateLimited":
      return <ClockIcon aria-hidden="true" className={className} />;
    default:
      return (
        <ExclamationTriangleIcon aria-hidden="true" className={className} />
      );
  }
}

/**
 * Texte du bandeau quand des données anciennes restent affichées. `saved` :
 * date relative de l'enregistrement (« 3 days ago »), ou « earlier ».
 */
function staleText(
  t: TFunction,
  problem: HubProblem,
  saved: string,
  seconds: number,
) {
  switch (problem.kind) {
    case "offline":
      return t("hub.status.stale.offline", { saved });
    case "unavailable":
      return problem.incident
        ? t("hub.status.stale.incident", { saved })
        : t("hub.status.stale.unavailable", { saved });
    case "rateLimited":
      return t("hub.status.rateLimited", { seconds });
    case "notFound":
      return t("hub.status.stale.notFound", { saved });
    case "restricted":
      return t("hub.status.restricted");
    case "failed":
      return t("hub.status.stale.failed", { message: problem.message, saved });
  }
}

/**
 * Bandeau au-dessus de données anciennes (`status.problem` avec
 * `fetchedAt`) ; rien quand les données sont à jour. `updatedAt` : heure de
 * la réponse (dataUpdatedAt de la requête).
 */
export function StaleBanner({
  status,
  updatedAt,
  onRetry,
}: {
  status: HubStatus;
  updatedAt: number;
  onRetry: () => void;
}) {
  const t = useT();
  const { problem, fetchedAt } = status;
  if (!problem) return null;
  return (
    <StaleBannerBody
      // Nouvelle réponse, même identique (partage structurel de React
      // Query) : décompte repris de zéro.
      key={`${problem.kind}-${problem.kind === "rateLimited" ? problem.retryAfter : 0}-${fetchedAt}-${updatedAt}`}
      problem={problem}
      saved={
        fetchedAt === null
          ? t("hub.status.earlier")
          : formatTimeAgo(t, fetchedAt)
      }
      onRetry={onRetry}
    />
  );
}

function StaleBannerBody({
  problem,
  saved,
  onRetry,
}: {
  problem: HubProblem;
  saved: string;
  onRetry: () => void;
}) {
  const t = useT();
  const seconds = useCountdown(
    problem.kind === "rateLimited" ? problem.retryAfter : 0,
  );
  return (
    <div
      role="status"
      data-testid="hub-banner"
      className="mb-5 flex flex-wrap items-center gap-3 rounded-2xl border border-neutral-200 bg-neutral-50 px-4 py-2.5 text-sm text-neutral-700 dark:border-neutral-800 dark:bg-neutral-800/40 dark:text-neutral-200"
    >
      <ProblemIcon
        problem={problem}
        className="h-5 w-5 flex-none text-neutral-500"
      />
      <p className="min-w-0 flex-1">{staleText(t, problem, saved, seconds)}</p>
      <RetryButton
        onRetry={onRetry}
        disabled={problem.kind === "rateLimited" && seconds > 0}
      />
    </div>
  );
}

/** Titre et texte d'un état sans données. */
function emptyTexts(
  t: TFunction,
  problem: HubProblem,
  seconds: number,
): { title: string; text: string } {
  switch (problem.kind) {
    case "offline":
      return {
        title: t("hub.status.offlineTitle"),
        text: t("hub.status.offlineText"),
      };
    case "unavailable":
      return {
        title: t("hub.status.unavailableTitle"),
        text: problem.incident
          ? t("hub.status.incidentText")
          : t("hub.status.unavailableText"),
      };
    case "rateLimited":
      return {
        title: t("hub.status.rateLimitedTitle"),
        text: t("hub.status.rateLimited", { seconds }),
      };
    case "notFound":
      return {
        title: t("hub.status.notFoundTitle"),
        text: t("hub.status.notFoundText"),
      };
    case "restricted":
      return {
        title: t("hub.status.restrictedTitle"),
        text: t("hub.status.restricted"),
      };
    case "failed":
      return {
        title: t("hub.status.failedTitle"),
        // Message de Hugging Face, tel quel.
        text: problem.message,
      };
  }
}

/**
 * État plein quand aucune donnée n'est disponible. `updatedAt` : heure de la
 * réponse (dataUpdatedAt de la requête).
 */
export function ProblemState({
  problem,
  updatedAt,
  onRetry,
  children,
}: {
  problem: HubProblem;
  updatedAt: number;
  /** Absent : pas de bouton Retry (modèle introuvable...). */
  onRetry?: () => void;
  /** Actions de plus (lien vers Hugging Face...). */
  children?: ReactNode;
}) {
  return (
    <ProblemStateBody
      // Nouvelle réponse : décompte repris de zéro (voir StaleBanner).
      key={`${problem.kind}-${problem.kind === "rateLimited" ? problem.retryAfter : 0}-${updatedAt}`}
      problem={problem}
      onRetry={onRetry}
    >
      {children}
    </ProblemStateBody>
  );
}

function ProblemStateBody({
  problem,
  onRetry,
  children,
}: {
  problem: HubProblem;
  onRetry?: () => void;
  children?: ReactNode;
}) {
  const t = useT();
  const seconds = useCountdown(
    problem.kind === "rateLimited" ? problem.retryAfter : 0,
  );
  const { title, text } = emptyTexts(t, problem, seconds);
  return (
    <EmptyState
      icon={<ProblemIcon problem={problem} className="h-6 w-6" />}
      title={title}
      text={text}
    >
      {onRetry && (
        <RetryButton
          onRetry={onRetry}
          disabled={problem.kind === "rateLimited" && seconds > 0}
        />
      )}
      {children}
    </EmptyState>
  );
}

/** État vide ou en erreur, centré : icône, titre, texte, actions. */
export function EmptyState({
  icon,
  title,
  text,
  children,
}: {
  /** Icône de 24 px, aria-hidden. */
  icon: ReactNode;
  title: string;
  text?: string;
  children?: ReactNode;
}) {
  return (
    <div
      data-testid="hub-empty"
      className="mx-auto flex max-w-md flex-col items-center px-6 py-16 text-center"
    >
      <span className="flex h-12 w-12 items-center justify-center rounded-2xl bg-neutral-100 text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">
        {icon}
      </span>
      <h3 className="mt-4 font-rounded text-lg font-medium text-neutral-900 dark:text-white">
        {title}
      </h3>
      {text && (
        <p className="mt-1 text-sm text-neutral-500 dark:text-neutral-400">
          {text}
        </p>
      )}
      {children && (
        <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
          {children}
        </div>
      )}
    </div>
  );
}

/** Panne interne d'une commande (AppError) : message et Retry. */
export function CommandError({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  const t = useT();
  return (
    <EmptyState
      icon={<ExclamationTriangleIcon aria-hidden="true" className="h-6 w-6" />}
      title={t("hub.status.commandError")}
      text={message}
    >
      <RetryButton onRetry={onRetry} />
    </EmptyState>
  );
}
