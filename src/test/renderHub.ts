// Rendu du ModelHub pour les tests : simulateur, faux backend (ModelHub
// compris, src/test/hub.ts) et app à l'adresse donnée.

import { fakeBackend, type FakeBackend } from "@/test/backend";
import type { FakeHub } from "@/test/hub";
import { renderApp } from "@/test/renderApp";
import { installSimulator, type Simulator } from "@/test/simulator";

export interface HubSetup {
  simulator: Simulator;
  backend: FakeBackend;
  hub: FakeHub;
  app: ReturnType<typeof renderApp>;
}

/**
 * Affiche l'app à `path` (« /hub », « /hub/model/a/b »...) ; `prepare`
 * règle le faux backend avant le premier rendu.
 */
export function renderHub(
  path: string,
  prepare?: (setup: { backend: FakeBackend; hub: FakeHub }) => void,
  options: Parameters<typeof fakeBackend>[1] = {},
): HubSetup {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, options);
  prepare?.({ backend, hub: backend.hub });
  const app = renderApp(path);
  return { simulator, backend, hub: backend.hub, app };
}

/** Arguments des appels d'une commande du ModelHub. */
export function hubCalls(simulator: Simulator, command: string) {
  return simulator.callsTo(command);
}
