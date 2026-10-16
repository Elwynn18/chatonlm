// Petits éléments des pages Images du ModelHub (propre à ChatonLM ; contrat
// .tools/research/85-image-hub-clarity-contract.md § 3) : icône neutre (pas
// d'initiales), nom affiché d'un modèle (recommandation, sinon famille) et
// puce de sa variante, phrase « à quoi il sert » de la famille, repères
// (vitesse, mémoire de la carte avec « tient / dépasse », place sur le
// disque, usage permis), « Details » repliable et liste des fichiers d'un
// modèle. Chiffres approximatifs, donnés pour une image 1024×1024.

import {
  BoltIcon,
  ChevronDownIcon,
  CircleStackIcon,
  CpuChipIcon,
  PhotoIcon,
  ScaleIcon,
} from "@heroicons/react/24/outline";
import {
  type ComponentType,
  createContext,
  type ReactNode,
  useContext,
  useId,
  useState,
} from "react";
import type { ImageComponentInfo } from "@/bindings/ImageComponentInfo";
import type { ImageComponentRole } from "@/bindings/ImageComponentRole";
import type { ImageMachine } from "@/bindings/ImageMachine";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ImageUsage } from "@/bindings/ImageUsage";
import { ExternalLink } from "@/components/ExternalLink";
import { CHIP } from "@/components/hub/styles";
import { type TKey, useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { huggingFaceUrl } from "@/lib/hub";
import { speedFact, usageClass, usageLabel } from "@/lib/imageModels";

/** Lien dans le texte (souligné, focus roux). */
export const IMAGE_LINK =
  "rounded-sm underline underline-offset-2 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-white";

/** Vert doux (« tient sur votre carte ») et ambre (avertissement). */
const PILL =
  "inline-flex flex-none items-center rounded-full px-2 py-0.5 text-xs font-medium whitespace-nowrap";
const PILL_FITS = `${PILL} bg-green-100 text-green-800 dark:bg-green-500/10 dark:text-green-400`;
const PILL_TOO_BIG = `${PILL} bg-amber-100 text-amber-800 dark:bg-amber-400/10 dark:text-amber-400`;

const ICON_SIZES = {
  md: "h-11 w-11 rounded-xl",
  lg: "h-16 w-16 rounded-2xl",
} as const;

/** Icône neutre d'un modèle d'images (à la place des initiales). */
export function ImageIcon({ size = "md" }: { size?: keyof typeof ICON_SIZES }) {
  return (
    <span
      aria-hidden="true"
      className={`flex flex-none items-center justify-center bg-neutral-100 text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400 ${ICON_SIZES[size]}`}
    >
      <PhotoIcon className={size === "lg" ? "h-8 w-8" : "h-6 w-6"} />
    </span>
  );
}

/** Puce de la variante (`Q4_0`, `Q8_0`...), expliquée en infobulle. */
export function VariantChip({ variant, id }: { variant: string; id?: string }) {
  const t = useT();
  return (
    <span id={id} title={t("hub.images.quantHint")} className={CHIP}>
      {variant}
    </span>
  );
}

/**
 * Repères écrits en `span` (dans le libellé d'un bouton radio, où une liste
 * `ul` n'a pas sa place), rôles de liste gardés.
 */
const InlineFacts = createContext(false);

/** Repère : icône, puis son contenu. */
export function Fact({
  icon: Icon,
  name,
  title,
  children,
}: {
  icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  /** Nom du repère (`data-fact`, pour les essais). */
  name: string;
  title?: string;
  children: ReactNode;
}) {
  const inline = useContext(InlineFacts);
  const Item = inline ? "span" : "li";
  return (
    <Item
      role={inline ? "listitem" : undefined}
      data-fact={name}
      className="flex min-w-0 items-start gap-1.5"
    >
      <Icon aria-hidden className="mt-0.5 h-4 w-4 flex-none text-neutral-400" />
      <span
        title={title}
        className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1"
      >
        {children}
      </span>
    </Item>
  );
}

/** Place vide d'un repère inconnu (repères alignés d'une carte à l'autre). */
function EmptyFact({ name }: { name: string }) {
  const inline = useContext(InlineFacts);
  return inline ? null : <li aria-hidden="true" data-fact={name} />;
}

/** Rangée des repères, alignés d'une carte à l'autre. */
export function Facts({
  children,
  className = "gap-y-2 text-sm sm:grid-cols-2 lg:grid-cols-4",
  inline = false,
}: {
  children: ReactNode;
  /** Colonnes, écart vertical et taille du texte. */
  className?: string;
  /** Dans un libellé : écrits en `span`. */
  inline?: boolean;
}) {
  const t = useT();
  const List = inline ? "span" : "ul";
  return (
    <InlineFacts value={inline}>
      <List
        role={inline ? "list" : undefined}
        aria-label={t("hub.images.glance")}
        className={`grid gap-x-6 text-neutral-700 dark:text-neutral-200 ${className}`}
      >
        {children}
      </List>
    </InlineFacts>
  );
}

/** Repère de la vitesse ; une place vide si elle est inconnue (alignement). */
export function SpeedFact(props: {
  measured: number | null;
  estimated: number | null;
  machine: ImageMachine | undefined;
}) {
  const t = useT();
  const speed = speedFact(t, props);
  if (!speed) return <EmptyFact name="speed" />;
  return (
    <Fact icon={BoltIcon} name="speed" title={speed.hint}>
      <span className="tabular-nums">{speed.text}</span>
    </Fact>
  );
}

/**
 * Mémoire de la carte graphique : chiffre, puis « tient sur votre carte »
 * (vert doux) ou « dépasse votre carte : plus lent » (ambre) quand la carte
 * est connue ; chiffre seul sinon.
 */
export function MemoryFact({
  vramBytes,
  machine,
}: {
  vramBytes: number;
  machine: ImageMachine | undefined;
}) {
  const t = useT();
  const card = machine?.gpu ? machine.vramBytes : null;
  return (
    <Fact icon={CpuChipIcon} name="memory" title={t("hub.images.memory.hint")}>
      <span className="tabular-nums">
        {t("hub.images.memory.vram", { size: formatBytes(t, vramBytes) })}
      </span>
      {card !== null &&
        (vramBytes <= card ? (
          <span data-fit="fits" className={PILL_FITS}>
            {t("hub.images.memory.fits", { vram: formatBytes(t, card) })}
          </span>
        ) : (
          <span data-fit="tooBig" className={PILL_TOO_BIG}>
            {t("hub.images.memory.tooBig")}
          </span>
        ))}
    </Fact>
  );
}

/**
 * Repère de l'usage permis ; « Licence à lire » est un lien vers la page du
 * dépôt (où est la licence). Licence exacte en infobulle.
 */
export function UsageFact({
  usage,
  repo,
  license,
}: {
  usage: ImageUsage;
  repo: string;
  license: string | null;
}) {
  const t = useT();
  const hint = license
    ? `${t("hub.images.license")} ${license}`
    : t("hub.images.noLicense");
  return (
    <Fact icon={ScaleIcon} name="usage" title={hint}>
      {usage === "checkLicense" ? (
        <ExternalLink href={huggingFaceUrl(repo)} className={IMAGE_LINK}>
          {usageLabel(t, usage)}
        </ExternalLink>
      ) : (
        <span className={usageClass(usage)}>{usageLabel(t, usage)}</span>
      )}
    </Fact>
  );
}

/**
 * Place : installé, sur le disque ; sinon ce qui reste à télécharger, avec
 * la part déjà présente (pièces partagées avec un autre modèle).
 */
export function SizeFact({ model }: { model: ImageModelInfo }) {
  const t = useT();
  const present = model.sizeBytes - model.missingBytes;
  let text: string;
  if (model.installed) {
    text = t("hub.images.onDisk", { size: formatBytes(t, model.sizeBytes) });
  } else if (present > 0 && model.missingBytes > 0) {
    text = t("hub.images.toDownloadShared", {
      size: formatBytes(t, model.missingBytes),
      present: formatBytes(t, present),
    });
  } else {
    text = t("hub.images.toDownload", {
      size: formatBytes(t, model.missingBytes),
    });
  }
  return (
    <Fact icon={CircleStackIcon} name="size">
      <span className="tabular-nums">{text}</span>
    </Fact>
  );
}

/** « Details » repliable : bouton `aria-expanded`, contenu masqué replié. */
export function Disclosure({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const contentId = useId();
  return (
    <div>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={contentId}
        onClick={() => setOpen((value) => !value)}
        className="inline-flex items-center gap-1 rounded-md text-xs font-medium text-neutral-600 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-300 dark:hover:text-white"
      >
        {label}
        <ChevronDownIcon
          aria-hidden="true"
          className={`h-3.5 w-3.5 transition-transform ${open ? "rotate-180" : ""}`}
        />
      </button>
      <div id={contentId} hidden={!open} className="mt-3">
        {children}
      </div>
    </div>
  );
}

const ROLES: Record<ImageComponentRole, TKey> = {
  model: "hub.images.repo.roles.model",
  textEncoder: "hub.images.repo.roles.textEncoder",
  vae: "hub.images.repo.roles.vae",
};

/**
 * Fichiers d'un modèle : rôle, taille, « déjà sur cet ordinateur »
 * (`showPresent`), dépôt et fichier (dépôt lié), licence exacte.
 */
export function ComponentList({
  components,
  showPresent = true,
}: {
  components: ImageComponentInfo[];
  showPresent?: boolean;
}) {
  const t = useT();
  return (
    <ul className="flex flex-col gap-2">
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
            {showPresent && component.present && (
              <span className={CHIP}>{t("hub.images.repo.present")}</span>
            )}
          </span>
          <span className="mt-0.5 block font-mono text-xs break-all text-neutral-500 dark:text-neutral-400">
            <ExternalLink
              href={huggingFaceUrl(component.repo)}
              className={IMAGE_LINK}
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
  );
}
