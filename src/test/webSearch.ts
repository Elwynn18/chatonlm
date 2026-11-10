// Faux état de la recherche web (relais) pour le simulateur (tests vitest
// seulement) : web_search_status rend `status` (recherche coupée par défaut :
// « off », comme le moteur quand le réglage est coupé), ou l'erreur de
// `failure`. Inscrit par src/test/backend.ts (installWebSearch) ; un essai le
// relit par webSearchOf(simulator).

import type { AppError } from "@/bindings/AppError";
import type { WebSearchStatus } from "@/bindings/WebSearchStatus";
import type { Simulator } from "@/test/simulator";

export interface FakeWebSearch {
  status: WebSearchStatus;
  failure: AppError | null;
  /** Lectures de web_search_status. */
  reads: number;
}

/** État d'un relais qui répond, `remaining` recherches restantes sur 50. */
export function okStatus(remaining: number): WebSearchStatus {
  return {
    state: remaining > 0 ? "ok" : "limit",
    dailyLimit: 50,
    remainingToday: remaining,
    resetsAt: 1_700_006_400_000,
    retryAt: null,
  };
}

const registry = new WeakMap<Simulator, FakeWebSearch>();

export function webSearchOf(simulator: Simulator): FakeWebSearch {
  const fake = registry.get(simulator);
  if (!fake) throw simulator.fail("installWebSearch was not called");
  return fake;
}

export function installWebSearch(simulator: Simulator): FakeWebSearch {
  const fake: FakeWebSearch = {
    status: {
      state: "off",
      dailyLimit: null,
      remainingToday: null,
      resetsAt: null,
      retryAt: null,
    },
    failure: null,
    reads: 0,
  };
  simulator.handle("web_search_status", () => {
    fake.reads += 1;
    if (fake.failure) throw fake.failure;
    return { ...fake.status };
  });
  registry.set(simulator, fake);
  return fake;
}
