// Petits éléments des cartes et de la fiche du ModelHub : avatar généré,
// confiance, capacités, pastille de compatibilité. Pas d'image distante
// (CSP) : l'avatar est fait d'initiales sur un dégradé déterministe.

import {
  CodeBracketIcon,
  EyeIcon,
  LightBulbIcon,
  WrenchScrewdriverIcon,
} from "@heroicons/react/24/outline";
import { CheckBadgeIcon } from "@heroicons/react/20/solid";
import type { ComponentType } from "react";
import type { HubCapabilities } from "@/bindings/HubCapabilities";
import type { HubFit } from "@/bindings/HubFit";
import type { HubTrust } from "@/bindings/HubTrust";
import type { HubUse } from "@/bindings/HubUse";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { fitLabel, GRADIENT_CLASS, gradientStyle, initials } from "@/lib/hub";

const AVATAR_SIZES = {
  sm: "h-9 w-9 rounded-xl text-xs",
  md: "h-11 w-11 rounded-xl text-sm",
  lg: "h-16 w-16 rounded-2xl text-xl",
} as const;

/** Avatar généré : initiales du nom, dégradé de la famille (sinon de l'auteur). */
export function ModelAvatar({
  name,
  colorKey,
  size = "md",
}: {
  name: string;
  colorKey: string;
  size?: keyof typeof AVATAR_SIZES;
}) {
  return (
    <span
      aria-hidden="true"
      style={gradientStyle(colorKey)}
      className={`flex flex-none items-center justify-center font-rounded font-semibold tracking-tight  select-none ${AVATAR_SIZES[size]} ${GRADIENT_CLASS}`}
    >
      {initials(name)}
    </span>
  );
}

/** « auteur · Verified » ou « auteur · Unverified ». */
export function TrustLine({
  author,
  trust,
}: {
  author: string;
  trust: HubTrust;
}) {
  const t = useT();
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-xs text-neutral-500 dark:text-neutral-400">
      <span className="truncate">{author}</span>
      <span aria-hidden="true">·</span>
      {trust === "unverified" ? (
        <UnverifiedBadge />
      ) : (
        <span className="inline-flex flex-none items-center gap-0.5 text-neutral-600 dark:text-neutral-300">
          <CheckBadgeIcon
            aria-hidden="true"
            className="h-3.5 w-3.5 text-accent-fg"
          />
          {t("hub.trust.verified")}
        </span>
      )}
    </span>
  );
}

export function UnverifiedBadge() {
  const t = useT();
  return (
    <span
      title={t("hub.trust.unverifiedHint")}
      className="inline-flex flex-none items-center rounded-md border border-neutral-300 px-1.5 text-[11px] leading-4 font-medium text-neutral-600 dark:border-neutral-600 dark:text-neutral-300"
    >
      {t("hub.trust.unverified")}
    </span>
  );
}

type CapabilityKey = keyof HubCapabilities;

/** Capacités montrées, avec le libellé de l'usage du même nom (filtres). */
const CAPABILITIES: {
  key: CapabilityKey;
  use: HubUse;
  icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
}[] = [
  { key: "tools", use: "tools", icon: WrenchScrewdriverIcon },
  { key: "thinking", use: "reasoning", icon: LightBulbIcon },
  { key: "vision", use: "vision", icon: EyeIcon },
  { key: "code", use: "code", icon: CodeBracketIcon },
];

/** Icônes des capacités, avec infobulle et nom accessible. */
export function CapabilityIcons({
  capabilities,
}: {
  capabilities: HubCapabilities;
}) {
  const t = useT();
  const shown = CAPABILITIES.filter(({ key }) => capabilities[key]);
  if (shown.length === 0) return null;
  return (
    <span className="flex items-center gap-1">
      {shown.map(({ key, use, icon: Icon }) => {
        const label = t(`hub.uses.${use}`);
        return (
          <span
            key={key}
            role="img"
            aria-label={label}
            title={label}
            className="flex h-6 w-6 items-center justify-center rounded-md text-neutral-500 dark:text-neutral-400"
          >
            <Icon aria-hidden className="h-4 w-4" />
          </span>
        );
      })}
    </span>
  );
}

/** Capacités en pastilles avec libellé (fiche). */
export function CapabilityChips({
  capabilities,
}: {
  capabilities: HubCapabilities;
}) {
  const t = useT();
  const shown = CAPABILITIES.filter(({ key }) => capabilities[key]);
  return (
    <ul className="flex flex-wrap gap-2" aria-label={t("hub.capabilities")}>
      {shown.map(({ key, use, icon: Icon }) => (
        <li
          key={key}
          className="inline-flex items-center gap-1.5 rounded-full border border-neutral-200 px-3 py-1 text-sm text-neutral-700 dark:border-neutral-700 dark:text-neutral-200"
        >
          <Icon aria-hidden className="h-4 w-4 text-neutral-500" />
          {t(`hub.uses.${use}`)}
        </li>
      ))}
    </ul>
  );
}

const FIT_COLORS: Record<HubFit, string> = {
  fits: "bg-green-100 text-green-800 dark:bg-green-500/10 dark:text-green-400",
  partial:
    "bg-amber-100 text-amber-800 dark:bg-amber-400/10 dark:text-amber-400",
  tooLarge: "bg-zinc-200 text-zinc-600 dark:bg-white/5 dark:text-zinc-400",
};

/**
 * Pastille de compatibilité (« Fits your GPU ») ; rien pour une machine
 * inconnue (serveur distant : `fit` null), ni tant que la machine n'est pas
 * lue (`gpu` null), pour ne pas montrer un texte qui changerait aussitôt. Les
 * tailles sont dites à côté, par Footprint.
 */
export function FitPill({
  fit,
  gpu,
  title,
}: {
  fit: HubFit | null;
  gpu: boolean | null;
  title?: string;
}) {
  const t = useT();
  if (gpu === null || fit === null) return null;
  return (
    <span
      data-fit={fit}
      title={title}
      className={`inline-flex flex-none items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium whitespace-nowrap ${FIT_COLORS[fit]}`}
    >
      <span
        aria-hidden="true"
        className="h-1.5 w-1.5 rounded-full bg-current opacity-80"
      />
      {fitLabel(t, fit, gpu)}
    </span>
  );
}

/**
 * Les deux poids d'un modèle, dits à part : ce qu'il faut télécharger (la
 * place prise sur le disque), puis la mémoire qu'il lui faut pour tourner
 * (« VRAM » avec une carte graphique, « Memory » sans). La mémoire n'est
 * pas montrée tant qu'elle n'est pas connue (serveur distant, machine pas
 * encore lue). `estimated` : chiffres estimés (cartes des listes).
 */
export function Footprint({
  download,
  memory,
  gpu,
  memoryHint,
  estimated = false,
}: {
  /** Octets à télécharger ; null : inconnu, rien n'est montré. */
  download: number | null;
  /** Mémoire nécessaire, en octets ; null : inconnue. */
  memory: number | null;
  gpu: boolean | null;
  /** Précision de la mémoire (contexte compté) ; à défaut, le texte commun. */
  memoryHint?: string;
  estimated?: boolean;
}) {
  const t = useT();
  if (download === null) return null;
  const hint = (text: string) =>
    estimated ? `${text}\n${t("hub.footprint.estimated")}` : text;
  const value =
    "font-medium text-neutral-700 tabular-nums dark:text-neutral-200";
  return (
    <span
      data-testid="footprint"
      className="inline-flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400"
    >
      <span title={hint(t("hub.footprint.downloadHint"))}>
        {t("hub.footprint.download")}{" "}
        <span className={value}>{formatBytes(t, download)}</span>
      </span>
      {memory !== null && gpu !== null && (
        <>
          <span aria-hidden="true">·</span>
          <span
            title={hint(
              memoryHint ??
                t(gpu ? "hub.footprint.vramHint" : "hub.footprint.memoryHint"),
            )}
          >
            {t(gpu ? "hub.footprint.vram" : "hub.footprint.memory")}{" "}
            <span className={value}>{formatBytes(t, memory)}</span>
          </span>
        </>
      )}
    </span>
  );
}

/** Badge « Installed ». */
export function InstalledBadge() {
  const t = useT();
  return (
    <span className="inline-flex flex-none items-center rounded-md bg-neutral-900 px-1.5 text-[11px] leading-5 font-medium text-white dark:bg-white dark:text-neutral-900">
      {t("hub.installedBadge")}
    </span>
  );
}
