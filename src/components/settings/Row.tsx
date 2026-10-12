// Éléments communs des cartes de réglages (propres à ChatonLM, sortis de
// Settings.tsx pour la carte Dictation) : ligne de réglage, erreur
// d'enregistrement d'une ligne, interrupteur, choix exclusif.

import { type ComponentType, type ReactNode, useId } from "react";
import { useT } from "@/i18n";

/** Ligne de réglage : icône, libellé, description, puis le contrôle. */
export function Row({
  icon: Icon,
  label,
  description,
  descriptionId,
  children,
}: {
  icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  label: ReactNode;
  description: ReactNode;
  descriptionId: string;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-1 items-start space-x-3">
      <Icon
        aria-hidden
        className="mt-1 h-5 w-5 flex-shrink-0 text-black dark:text-neutral-100"
      />
      <div className="w-full min-w-0">
        <div className="text-sm/6 text-zinc-950 dark:text-white">{label}</div>
        <p
          id={descriptionId}
          className="text-sm/6 text-zinc-500 dark:text-zinc-400"
        >
          {description}
        </p>
        {children}
      </div>
    </div>
  );
}

/** Échec de l'enregistrement d'une ligne ; `message` : raison du moteur. */
export function RowError({ message }: { message?: string | null }) {
  const t = useT();
  if (!message) return null;
  return (
    <p
      role="alert"
      className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
    >
      {t("settings.saveError", { message })}
    </p>
  );
}

/**
 * Interrupteur (rôle switch) : un bouton natif, Espace et Entrée compris.
 * `label` : nom lu par les lecteurs d'écran.
 */
export function Switch({
  checked,
  onChange,
  label,
  describedBy,
  disabled = false,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  describedBy?: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={describedBy}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-6 w-10 flex-none cursor-pointer items-center rounded-full transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50 dark:focus-visible:ring-offset-neutral-800 ${
        checked
          ? "bg-neutral-900 dark:bg-white"
          : "bg-neutral-300 dark:bg-neutral-600"
      }`}
    >
      <span
        aria-hidden="true"
        className={`inline-block h-5 w-5 rounded-full shadow-sm transition-transform ${
          checked
            ? "translate-x-[18px] bg-white dark:bg-neutral-900"
            : "translate-x-0.5 bg-white dark:bg-neutral-300"
        }`}
      />
    </button>
  );
}

/**
 * Choix exclusif : boutons radio natifs (flèches du clavier comprises),
 * comme ceux de Settings › General et de la dictée.
 */
export function Choice<T extends string>({
  options,
  value,
  onChange,
  labelledBy,
  describedBy,
}: {
  options: readonly { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
  labelledBy: string;
  describedBy: string;
}) {
  const name = useId();
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className="inline-flex rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-900"
    >
      {options.map((option) => (
        <label key={option.value} className="relative">
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            onChange={() => onChange(option.value)}
            className="peer sr-only"
          />
          <span className="block cursor-pointer rounded-md px-3 py-1 text-sm/6 whitespace-nowrap text-neutral-600 peer-checked:bg-white peer-checked:text-neutral-900 peer-checked:shadow-sm peer-focus-visible:ring-2 peer-focus-visible:ring-accent-fg peer-disabled:cursor-not-allowed peer-disabled:opacity-50 dark:text-neutral-300 dark:peer-checked:bg-neutral-700 dark:peer-checked:text-white">
            {option.label}
          </span>
        </label>
      ))}
    </div>
  );
}
