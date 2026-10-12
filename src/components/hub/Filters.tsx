// Champ de recherche, pastille de la machine et puces de filtres de la page
// Découvrir. Les puces forment une barre d'outils : un seul arrêt de
// tabulation, flèches gauche et droite (Début, Fin) pour aller de l'une à
// l'autre, Entrée ou Espace pour basculer (vrais boutons, aria-pressed).

import {
  CodeBracketIcon,
  CpuChipIcon,
  EyeIcon,
  LightBulbIcon,
  MagnifyingGlassIcon,
  ServerStackIcon,
  WrenchScrewdriverIcon,
  XMarkIcon,
} from "@heroicons/react/24/outline";
import {
  type ComponentType,
  type KeyboardEvent,
  type RefObject,
  useRef,
  useState,
} from "react";
import type { HubHardware } from "@/bindings/HubHardware";
import type { HubSize } from "@/bindings/HubSize";
import type { HubUse } from "@/bindings/HubUse";
import { SKELETON } from "@/components/hub/styles";
import { useT } from "@/i18n";
import { machineHint, machineLabel, SIZES, USES } from "@/lib/hub";

export function SearchField({
  value,
  onChange,
  onClear,
  inputRef,
}: {
  value: string;
  onChange: (value: string) => void;
  /** Échap ou bouton ×. */
  onClear: () => void;
  inputRef: RefObject<HTMLInputElement | null>;
}) {
  const t = useT();
  return (
    <div role="search" className="relative">
      <MagnifyingGlassIcon
        aria-hidden="true"
        className="pointer-events-none absolute top-1/2 left-4 h-5 w-5 -translate-y-1/2 text-neutral-500"
      />
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          if (value) onClear();
          else event.currentTarget.blur();
        }}
        placeholder={t("hub.search.placeholder")}
        aria-label={t("hub.search.placeholder")}
        aria-keyshortcuts="/"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        className="h-12 w-full rounded-2xl border border-neutral-200 bg-white pr-12 pl-11 text-[15px] text-neutral-900 shadow-sm outline-none placeholder:text-neutral-400 focus:border-accent-fg dark:border-neutral-700 dark:bg-neutral-800 dark:text-white dark:placeholder:text-neutral-500"
      />
      <span className="absolute top-1/2 right-3 -translate-y-1/2">
        {value ? (
          <button
            type="button"
            onClick={() => {
              onClear();
              // Le bouton disparaît avec le texte : le focus reste au champ.
              inputRef.current?.focus();
            }}
            aria-label={t("hub.search.clear")}
            title={t("hub.search.clear")}
            className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-neutral-700"
          >
            <XMarkIcon aria-hidden="true" className="h-4 w-4" />
          </button>
        ) : (
          <kbd
            aria-hidden="true"
            className="flex h-6 min-w-6 items-center justify-center rounded-md border border-neutral-200 px-1.5 font-sans text-xs text-neutral-500 dark:border-neutral-700"
          >
            /
          </kbd>
        )}
      </span>
    </div>
  );
}

/**
 * Pastille de la machine du serveur ; infobulle : sens des pastilles des
 * modèles. `undefined` : pas encore connue ; null : serveur distant.
 */
export function MachinePill({
  hardware,
}: {
  hardware: HubHardware | null | undefined;
}) {
  const t = useT();
  if (hardware === undefined) {
    return (
      <span
        aria-hidden="true"
        className={`${SKELETON} h-7 w-56 rounded-full`}
      />
    );
  }
  const remote = hardware === null;
  const Icon = remote ? ServerStackIcon : CpuChipIcon;
  return (
    <span
      data-testid="machine"
      title={remote ? t("hub.machine.remoteHint") : machineHint(t, hardware)}
      className="inline-flex max-w-full items-center gap-1.5 rounded-full border border-neutral-200 bg-white px-3 py-1 text-xs text-neutral-600 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-300"
    >
      <Icon aria-hidden="true" className="h-4 w-4 flex-none text-neutral-500" />
      <span className="truncate">
        {remote ? t("hub.machine.remote") : machineLabel(t, hardware)}
      </span>
    </span>
  );
}

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean }>;

const USE_ICONS: Record<HubUse, Icon> = {
  code: CodeBracketIcon,
  vision: EyeIcon,
  reasoning: LightBulbIcon,
  tools: WrenchScrewdriverIcon,
};

const CHIP_BASE =
  "inline-flex flex-none items-center gap-1.5 rounded-full border px-3 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-1 dark:focus-visible:ring-offset-neutral-900";
const CHIP_OFF =
  "border-neutral-200 bg-white text-neutral-700 hover:bg-neutral-100 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-300 dark:hover:bg-neutral-800";
// Filtre actif : aplat roux, texte encre (dans les deux thèmes).
const CHIP_ON = "border-accent bg-accent text-neutral-900";

interface Chip {
  key: string;
  label: string;
  icon?: Icon;
  pressed: boolean;
  toggle: () => void;
  /** Début d'un groupe : séparateur avant la puce. */
  group?: boolean;
}

export interface FilterState {
  uses: HubUse[];
  size: HubSize | null;
  openLicense: boolean;
  verifiedOnly: boolean;
}

/**
 * Puces de filtres ; `searching` ajoute « Verified only », `canFit` (machine
 * connue) « Fits my machine ». Reset filters quand un filtre est posé.
 */
export function FilterChips({
  value,
  onChange,
  searching,
  canFit,
}: {
  value: FilterState;
  onChange: (value: FilterState) => void;
  searching: boolean;
  canFit: boolean;
}) {
  const t = useT();
  const chips: Chip[] = [
    ...USES.map((use, index): Chip => ({
      key: use,
      label: t(`hub.uses.${use}`),
      icon: USE_ICONS[use],
      pressed: value.uses.includes(use),
      toggle: () =>
        onChange({
          ...value,
          uses: value.uses.includes(use)
            ? value.uses.filter((u) => u !== use)
            : USES.filter((u) => u === use || value.uses.includes(u)),
        }),
      group: index === 0,
    })),
    ...SIZES.filter((size) => size !== "fits" || canFit).map(
      (size, index): Chip => ({
        key: size,
        label: t(`hub.sizes.${size}`),
        pressed: value.size === size,
        toggle: () =>
          onChange({ ...value, size: value.size === size ? null : size }),
        group: index === 0,
      }),
    ),
    {
      key: "license",
      label: t("hub.filters.openLicense"),
      pressed: value.openLicense,
      toggle: () => onChange({ ...value, openLicense: !value.openLicense }),
      group: true,
    },
    ...(searching
      ? [
          {
            key: "verified",
            label: t("hub.filters.verifiedOnly"),
            pressed: value.verifiedOnly,
            toggle: () =>
              onChange({ ...value, verifiedOnly: !value.verifiedOnly }),
            group: true,
          },
        ]
      : []),
  ];
  const filtered =
    value.uses.length > 0 ||
    value.size !== null ||
    value.openLicense ||
    (searching && value.verifiedOnly);

  const [focusIndex, setFocusIndex] = useState(0);
  const current = Math.min(focusIndex, chips.length - 1);
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);

  const onKeyDown = (event: KeyboardEvent) => {
    const last = chips.length - 1;
    const next =
      event.key === "ArrowRight"
        ? current === last
          ? 0
          : current + 1
        : event.key === "ArrowLeft"
          ? current === 0
            ? last
            : current - 1
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : null;
    if (next === null) return;
    event.preventDefault();
    // onFocus de la puce en fait l'arrêt de tabulation.
    buttons.current[next]?.focus();
  };

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <div
        role="toolbar"
        aria-label={t("hub.filters.label")}
        onKeyDown={onKeyDown}
        className="flex flex-wrap items-center gap-1.5"
      >
        {chips.map((chip, index) => {
          const Icon = chip.icon;
          return (
            <span key={chip.key} className="flex items-center gap-1.5">
              {chip.group && index > 0 && (
                <span
                  aria-hidden="true"
                  className="mx-1 h-5 w-px bg-neutral-200 dark:bg-neutral-700"
                />
              )}
              <button
                ref={(element) => {
                  buttons.current[index] = element;
                }}
                type="button"
                aria-pressed={chip.pressed}
                tabIndex={index === current ? 0 : -1}
                onFocus={() => setFocusIndex(index)}
                onClick={chip.toggle}
                className={`${CHIP_BASE} ${chip.pressed ? CHIP_ON : CHIP_OFF}`}
              >
                {Icon && <Icon aria-hidden className="h-4 w-4" />}
                {chip.label}
              </button>
            </span>
          );
        })}
      </div>
      {filtered && (
        <button
          type="button"
          onClick={() =>
            onChange({
              uses: [],
              size: null,
              openLicense: false,
              verifiedOnly: false,
            })
          }
          className="rounded-md px-1 text-sm text-neutral-500 underline-offset-2 outline-none hover:text-neutral-900 hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:text-white"
        >
          {t("hub.filters.reset")}
        </button>
      )}
    </div>
  );
}
