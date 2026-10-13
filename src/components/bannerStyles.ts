// Classes des bandeaux placés au-dessus de la zone de saisie (Ollama ne
// répond plus, nouvelle version d'un modèle) : même boîte, mêmes boutons.

/** Boîte du bandeau, alignée sur la zone de saisie. */
export const BANNER_BOX =
  "mx-auto w-full max-w-[768px] rounded-2xl border border-neutral-200 bg-white px-4 py-2.5 text-sm text-neutral-700 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200";

/** Action principale (Retry, Update). */
export const BANNER_PRIMARY =
  "min-w-20 rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-60 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";

/** Action secondaire (Open log). */
export const BANNER_SECONDARY =
  "rounded-full border border-neutral-300 px-3 py-1 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-700";

/** Bouton d'icône seule (fermer le bandeau). */
export const BANNER_ICON_BUTTON =
  "inline-flex h-7 w-7 flex-none items-center justify-center rounded-full text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-800 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-neutral-100";

/** Ligne d'erreur sous le texte du bandeau. */
export const BANNER_ERROR =
  "mt-1.5 text-xs break-words text-red-600 dark:text-red-400";
