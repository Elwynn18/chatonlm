// Curseur d'autonomie d'un nouveau projet de l'Atelier (propre à ChatonLM),
// à la façon de Claude Code : quatre niveaux (Plan, Ask, Accept actions,
// Autonomous) en pastilles (boutons radio natifs, flèches du clavier), une
// ligne qui explique le niveau choisi (annoncée quand il change, Maj+Tab
// compris), et le rappel du raccourci (Échap puis Maj+Tab rend à Maj+Tab
// son rôle habituel, une fois). Le niveau actif est en aplat roux,
// texte encre ; son focus clavier est un contour décalé (visible sur l'aplat).

import { useId } from "react";
import type { Autonomy } from "@/bindings/Autonomy";
import { useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import { AUTONOMY_LEVELS } from "@/lib/workshop";

const KBD =
  "rounded border border-neutral-300 bg-neutral-50 px-1 font-sans text-xs text-neutral-700 dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-200";

export function AutonomyPicker({
  value,
  onChange,
  helpId,
  shortcutId,
}: {
  value: Autonomy;
  onChange: (level: Autonomy) => void;
  /** Identifiant de la ligne d'aide (décrit aussi la zone de la tâche). */
  helpId: string;
  /** Identifiant du rappel du raccourci (décrit aussi la zone de la tâche). */
  shortcutId: string;
}) {
  const t = useT();
  const name = useId();
  return (
    <fieldset
      className="min-w-0"
      data-testid="workshop-autonomy"
      data-level={value}
    >
      <legend className="mb-1.5 text-sm font-medium text-neutral-700 dark:text-neutral-300">
        {t("workshop.autonomy.label")}
      </legend>
      <div className="inline-flex flex-wrap gap-1 rounded-full bg-neutral-100 p-1 dark:bg-neutral-900">
        {AUTONOMY_LEVELS.map((level) => (
          <label
            key={level}
            data-testid="workshop-autonomy-level"
            data-level={level}
            className={
              level === value
                ? "cursor-pointer rounded-full bg-accent px-3 py-1 text-sm font-medium text-neutral-900 has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent-fg"
                : "cursor-pointer rounded-full px-3 py-1 text-sm text-neutral-600 hover:text-neutral-900 has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent-fg dark:text-neutral-300 dark:hover:text-white"
            }
          >
            <input
              type="radio"
              name={name}
              className="sr-only"
              checked={level === value}
              onChange={() => onChange(level)}
            />
            {t(`workshop.autonomy.levels.${level}`)}
          </label>
        ))}
      </div>
      <p
        id={helpId}
        role="status"
        data-testid="workshop-autonomy-help"
        className="mt-2 text-sm text-neutral-600 dark:text-neutral-300"
      >
        {t(`workshop.autonomy.help.${value}`)}
      </p>
      <p
        id={shortcutId}
        data-testid="workshop-autonomy-shortcut"
        className="mt-1 text-xs text-neutral-500 dark:text-neutral-400"
      >
        <Trans
          k="workshop.autonomy.shortcut"
          tags={{ kbd: (chunks) => <kbd className={KBD}>{chunks}</kbd> }}
        />
      </p>
    </fieldset>
  );
}
