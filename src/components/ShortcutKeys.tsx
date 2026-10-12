// Touches d'un raccourci en pastilles <kbd> (propre à ChatonLM) : onglet
// « Keyboard shortcuts » des réglages et page Proofread. Les modificateurs
// et touches nommées sont traduits (« Maj », « Entrée »...) ; plusieurs
// combinaisons sont réunies par le texte traduit « … or … » (« Ctrl+Maj+Z ou
// Ctrl+Y »).

import { Fragment, type ReactNode } from "react";
import { useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import { keyLabel, type ShortcutCombo } from "@/lib/shortcuts";

export const KBD =
  "inline-flex min-w-6 items-center justify-center rounded-md border border-neutral-300 bg-neutral-50 px-1.5 py-0.5 font-sans text-xs font-medium text-neutral-800 shadow-sm dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-100";

/** Une combinaison : pastilles séparées par « + ». */
function Combo({ combo }: { combo: ShortcutCombo }) {
  const t = useT();
  return combo.map((key, index) => (
    <Fragment key={index}>
      {index > 0 && <span>+</span>}
      <kbd className={KBD}>{keyLabel(t, key)}</kbd>
    </Fragment>
  ));
}

export function ShortcutKeys({ keys }: { keys: readonly ShortcutCombo[] }) {
  return (
    <span className="inline-flex flex-wrap items-center gap-1 text-xs text-neutral-500">
      {keys
        .map((combo, index): ReactNode => <Combo key={index} combo={combo} />)
        .reduce((first, second) => (
          <Trans
            k="settings.shortcuts.either"
            tags={{ first: () => first, second: () => second }}
          />
        ))}
    </span>
  );
}
