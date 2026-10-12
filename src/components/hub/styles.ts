// Classes partagées du ModelHub : mêmes gris, arrondis et boutons que le
// reste de l'app (ConfirmDialog, Settings).

/** Carte : bordure fine, fond de page. */
export const CARD =
  "rounded-2xl border border-neutral-200 bg-white dark:border-neutral-700 dark:bg-neutral-900";

/** Carte cliquable : survol discret (soulevée, ombre). */
export const CARD_LINK = `${CARD} block transition duration-150 outline-none hover:-translate-y-0.5 hover:border-neutral-300 hover:shadow-md focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:border-neutral-600`;

/** Bouton principal (Download, Use in chat). */
export const PRIMARY_BUTTON =
  "inline-flex items-center justify-center gap-1.5 rounded-full bg-neutral-900 px-4 py-1.5 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-900";

/** Bouton secondaire (Retry, Cancel, Dismiss). */
export const SECONDARY_BUTTON =
  "inline-flex items-center justify-center gap-1.5 rounded-full border border-neutral-200 bg-white px-3.5 py-1.5 text-sm text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800";

/** Bouton d'icône seule (suppression...). */
export const ICON_BUTTON =
  "inline-flex h-8 w-8 items-center justify-center rounded-full text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-800 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100";

/** Lien discret dans le texte (See all, Back...). */
export const QUIET_LINK =
  "inline-flex items-center gap-1 rounded-md text-sm text-neutral-500 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:text-white";

/** Pastille neutre (paramètres, contexte, étiquette). */
export const CHIP =
  "inline-flex items-center gap-1 rounded-md bg-neutral-100 px-1.5 py-0.5 text-xs font-medium text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300";

/** Bloc de squelette (chargement). */
export const SKELETON =
  "animate-pulse rounded-md bg-neutral-200 dark:bg-neutral-800";

/** Contenu des pages du ModelHub : centré, largeur maximale. */
export const PAGE = "mx-auto w-full max-w-6xl px-6 pt-2 pb-10";

/** Barre d'onglets en pilules (ModelHub, Settings). */
export const TAB_BAR =
  "inline-flex rounded-xl bg-neutral-100 p-1 dark:bg-neutral-800";

/**
 * Onglet : vrai lien. Focus : le contour décalé de la feuille de style
 * (src/index.css), visible aussi sur l'onglet courant, en aplat roux.
 */
export const TAB =
  "inline-flex items-center gap-1.5 rounded-lg px-3 py-1 text-sm";

/** Onglet au repos. */
export const TAB_IDLE =
  "text-neutral-600 hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-white";

/** Onglet courant : aplat roux, texte encre (dans les deux thèmes). */
export const TAB_CURRENT = "bg-accent text-neutral-900";
