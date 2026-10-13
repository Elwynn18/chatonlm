// Barre latérale ouverte ou fermée. Comme dans l'app officielle : fermée au
// lancement, état conservé d'une page à l'autre pendant la session, mais pas
// d'un lancement à l'autre. Tenu hors des composants : chaque page a sa propre
// mise en page (SidebarLayout), et la visite guidée du premier démarrage
// ouvre la barre de la page affichée.

import { useSyncExternalStore } from "react";

let open = false;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Ouvre ou ferme la barre latérale de toutes les pages. */
export function setSidebarOpen(next: boolean) {
  if (next === open) return;
  open = next;
  for (const listener of listeners) listener();
}

/** État de la barre latérale ; le composant suit ses changements. */
export function useSidebarOpen(): boolean {
  return useSyncExternalStore(subscribe, () => open);
}
