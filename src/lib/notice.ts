// Annonce brève, montrée en bas de la fenêtre quelle que soit la page
// (propre à ChatonLM) : « Your data has been erased. » après « Erase my
// data », qui quitte les réglages pour une nouvelle conversation. Le texte
// est une clé des catalogues, traduite au rendu (src/components/Notice.tsx) ;
// une annonce remplace la précédente.

import { useSyncExternalStore } from "react";
import type { TKey, TParams } from "@/i18n";

/** Annonce en cours. */
export interface Notice {
  /** Change à chaque annonce : la même phrase, de nouveau, est annoncée. */
  id: number;
  /** Phrases, dans l'ordre. */
  lines: { key: TKey; params?: TParams }[];
}

let current: Notice | null = null;
let next = 0;
const listeners = new Set<() => void>();

function changed() {
  for (const listener of listeners) listener();
}

/** Montre une annonce (remplace la précédente). */
export function showNotice(lines: Notice["lines"]) {
  next += 1;
  current = { id: next, lines };
  changed();
}

/** Retire l'annonce `id`, si c'est encore elle. */
export function hideNotice(id: number) {
  if (current?.id !== id) return;
  current = null;
  changed();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Annonce en cours, ou null. */
export function useNotice(): Notice | null {
  return useSyncExternalStore(subscribe, () => current);
}

/** Essais : plus d'annonce (entre deux essais, rien n'est plus affiché). */
export function resetNoticeForTests() {
  current = null;
}
