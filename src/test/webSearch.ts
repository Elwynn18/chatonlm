// Faux réglage de la recherche web (moteur choisi, clés) pour le simulateur
// (tests vitest seulement) : web_search_engine, set_web_search_engine,
// forget_web_search_secret, check_web_search_engine. DuckDuckGo par défaut,
// aucune clé ; les clés reçues sont gardées ici (`secrets`) pour que l'essai
// les relise, jamais rendues à la page (comme le moteur). Inscrit par
// src/test/backend.ts (installWebSearch) ; un essai le relit par
// webSearchOf(simulator).

import type { AppError } from "@/bindings/AppError";
import type { WebSearchCheck } from "@/bindings/WebSearchCheck";
import type { WebSearchEngine } from "@/bindings/WebSearchEngine";
import type { WebSearchEngineChoice } from "@/bindings/WebSearchEngineChoice";
import type { Simulator } from "@/test/simulator";

export interface FakeWebSearch {
  engine: WebSearchEngine;
  /** Clés et adresse reçues, par moteur. */
  secrets: Partial<Record<WebSearchEngine, string>>;
  /** Réponse de check_web_search_engine. */
  check: WebSearchCheck;
  /** Erreur rendue par set_web_search_engine (puis effacée). */
  failure: AppError | null;
}

const registry = new WeakMap<Simulator, FakeWebSearch>();

export function webSearchOf(simulator: Simulator): FakeWebSearch {
  const fake = registry.get(simulator);
  if (!fake) throw simulator.fail("installWebSearch was not called");
  return fake;
}

function choice(fake: FakeWebSearch): WebSearchEngineChoice {
  return {
    engine: fake.engine,
    braveConfigured: Boolean(fake.secrets.brave),
    mojeekConfigured: Boolean(fake.secrets.mojeek),
    searxngConfigured: Boolean(fake.secrets.searxng),
  };
}

export function installWebSearch(simulator: Simulator): FakeWebSearch {
  const fake: FakeWebSearch = {
    engine: "duckDuckGo",
    secrets: {},
    check: { ok: true, message: "It works." },
    failure: null,
  };
  simulator.handle("web_search_engine", () => choice(fake));
  simulator.handle("set_web_search_engine", (args) => {
    if (fake.failure) {
      const failure = fake.failure;
      fake.failure = null;
      throw failure;
    }
    const { engine, secret } = args as {
      engine: WebSearchEngine;
      secret: string | null;
    };
    fake.engine = engine;
    if (secret !== null && engine !== "duckDuckGo") {
      fake.secrets[engine] = secret.trim();
    }
    return choice(fake);
  });
  simulator.handle("forget_web_search_secret", (args) => {
    const { engine } = args as { engine: WebSearchEngine };
    delete fake.secrets[engine];
    if (fake.engine === engine) fake.engine = "duckDuckGo";
    return choice(fake);
  });
  simulator.handle("check_web_search_engine", () => ({ ...fake.check }));
  registry.set(simulator, fake);
  return fake;
}
