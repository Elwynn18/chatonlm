// Ouverture des liens dans le navigateur, et l'avis affiché quand elle échoue
// (pas de navigateur par défaut, xdg-open absent...). L'avis vit hors de
// React, comme les flux (streams.ts) : un seul composant l'affiche
// (LinkErrorNotice), quel que soit le lien cliqué.
import { errorMessage, openExternal } from "@/lib/ipc";

/** Un second clic sur la même adresse dans ce délai est ignoré. */
export const REPEAT_MS = 1000;

export type LinkProblem =
  | { id: number; url: string; kind: "open"; detail: string }
  | { id: number; url: string; kind: "copy" };

/**
 * Adresse à ouvrir dans le navigateur, normalisée, ou null : seuls http et
 * https sont ouverts (localhost compris, un clic explicite) ; vide, relatif,
 * ancre, mailto: et le reste ne sont pas des liens.
 */
export function webUrl(href: string | undefined): string | null {
  if (!href) return null;
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.href
      : null;
  } catch {
    return null;
  }
}

let problem: LinkProblem | null = null;
let nextId = 1;
let lastOpen: { url: string; at: number } | null = null;
const listeners = new Set<() => void>();

function set(value: LinkProblem | null) {
  problem = value;
  for (const listener of listeners) listener();
}

export function subscribeLinkProblem(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Avis affiché, ou null. */
export function linkProblem(): LinkProblem | null {
  return problem;
}

/** Ferme l'avis `id` (s'il n'a pas déjà été remplacé par un autre). */
export function dismissLinkProblem(id: number) {
  if (problem?.id === id) set(null);
}

/**
 * Ouvre `url` (déjà vérifiée par webUrl) dans le navigateur, sans attendre :
 * xdg-open peut mettre 2 s à répondre. En cas d'échec, l'avis donne la raison
 * et l'adresse. La promesse ne rejette jamais.
 */
export function openLink(url: string): Promise<void> {
  const now = Date.now();
  // Double clic, ou clic répété pendant que le navigateur démarre.
  if (lastOpen?.url === url && now - lastOpen.at < REPEAT_MS) {
    return Promise.resolve();
  }
  lastOpen = { url, at: now };
  return openExternal(url).then(
    () => {
      // Le navigateur s'ouvre : un ancien avis d'échec n'a plus lieu d'être.
      if (problem?.kind === "open") set(null);
    },
    (error: unknown) => {
      set({ id: nextId++, url, kind: "open", detail: errorMessage(error) });
    },
  );
}

/** La copie d'une adresse a échoué : l'avis la montre, à sélectionner. */
export function reportCopyFailure(url: string) {
  set({ id: nextId++, url, kind: "copy" });
}

/** Pour les tests : remet le magasin à zéro (composants démontés avant). */
export function resetLinks() {
  problem = null;
  lastOpen = null;
}
