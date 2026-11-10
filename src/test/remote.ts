// Faux moteur des modèles distants pour le simulateur (tests vitest
// seulement) : fournisseurs en mémoire, clés gardées à part et jamais rendues
// (indice : quatre derniers caractères, comme le moteur), vérification d'une
// clé selon `checks` (sinon : acceptée, trois modèles). save_remote_provider
// refuse comme le moteur une adresse « custom » qui n'est pas http(s) et une
// clé avec une espace (erreur « invalid ») ; un champ de clé absent garde la
// clé, vide la retire. Une panne d'une commande se force par `failures`.
// Inscrit par src/test/backend.ts (installRemote) ; un essai le relit par
// remoteOf(simulator).

import type { AppError } from "@/bindings/AppError";
import type { RemoteCheck } from "@/bindings/RemoteCheck";
import type { RemoteProvider } from "@/bindings/RemoteProvider";
import type { RemoteProviderInput } from "@/bindings/RemoteProviderInput";
import type { RemoteProviderKind } from "@/bindings/RemoteProviderKind";
import type { Args, Simulator } from "@/test/simulator";

/** Erreurs du moteur, déjà traduites (anglais des essais). */
export const INVALID_URL =
  "Enter the address of a server compatible with the OpenAI API, starting with http:// or https://.";
export const INVALID_KEY = "This API key isn't valid: it can't contain spaces.";

const ID_BASES: Record<RemoteProviderKind, string> = {
  ollamaCloud: "ollama",
  openRouter: "openrouter",
  mistral: "mistral",
  custom: "custom",
};

const NAMES: Record<RemoteProviderKind, string> = {
  ollamaCloud: "Ollama Cloud",
  openRouter: "OpenRouter",
  mistral: "Mistral",
  custom: "custom",
};

export interface FakeRemote {
  providers: RemoteProvider[];
  /** Clés enregistrées, par fournisseur (jamais rendues à la page). */
  keys: Map<string, string>;
  /** Résultat de check_remote_provider, par fournisseur. */
  checks: Map<string, RemoteCheck>;
  failures: Map<string, AppError>;
  /** Ajoute un fournisseur déjà enregistré. */
  add(
    kind: RemoteProviderKind,
    fields?: Partial<RemoteProvider>,
    key?: string,
  ): RemoteProvider;
}

const registry = new WeakMap<Simulator, FakeRemote>();

/** Faux moteur des modèles distants inscrit sur `simulator`. */
export function remoteOf(simulator: Simulator): FakeRemote {
  const fake = registry.get(simulator);
  if (!fake) throw simulator.fail("installRemote was not called");
  return fake;
}

function hint(key: string | undefined): string | null {
  return key !== undefined && key.length >= 8 ? key.slice(-4) : null;
}

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

export function installRemote(simulator: Simulator): FakeRemote {
  let clock = 1_700_000_000_000;
  const view = (provider: RemoteProvider): RemoteProvider => {
    const key = fake.keys.get(provider.id);
    return {
      ...provider,
      hasKey: key !== undefined && key !== "",
      keyHint: hint(key),
    };
  };
  const freeId = (kind: RemoteProviderKind) => {
    const base = ID_BASES[kind];
    const taken = new Set(fake.providers.map((p) => p.id));
    if (!taken.has(base)) return base;
    for (let n = 2; ; n++)
      if (!taken.has(`${base}-${n}`)) return `${base}-${n}`;
  };
  const fake: FakeRemote = {
    providers: [],
    keys: new Map(),
    checks: new Map(),
    failures: new Map(),
    add(kind, fields = {}, key) {
      clock += 1;
      const provider: RemoteProvider = {
        id: freeId(kind),
        kind,
        name: NAMES[kind],
        baseUrl: kind === "custom" ? "http://192.168.1.2:8080" : null,
        enabled: true,
        hasKey: false,
        keyHint: null,
        createdAt: clock,
        updatedAt: clock,
        ...fields,
      };
      fake.providers.push(provider);
      if (key !== undefined) fake.keys.set(provider.id, key);
      return view(provider);
    },
  };
  const failing = (command: string) => {
    const failure = fake.failures.get(command);
    if (failure) throw failure;
  };
  simulator.handle("remote_providers", () => {
    failing("remote_providers");
    return fake.providers.map(view);
  });
  simulator.handle("save_remote_provider", (args: Args) => {
    failing("save_remote_provider");
    const input = args.input as RemoteProviderInput;
    const existing = fake.providers.find((p) => p.id === input.id);
    const kind = existing?.kind ?? input.kind;
    let baseUrl: string | null = null;
    if (kind === "custom") {
      const given = (input.baseUrl ?? "").trim();
      if (!/^https?:\/\/[^\s/]+/.test(given)) throw invalid(INVALID_URL);
      baseUrl = given.replace(/\/+$/, "").replace(/\/v1$/, "");
    }
    const key = input.apiKey?.trim();
    if (key && /\s/.test(key)) throw invalid(INVALID_KEY);
    clock += 1;
    const addressChanged =
      existing !== undefined &&
      kind === "custom" &&
      existing.baseUrl !== baseUrl;
    const name = input.name.trim() || NAMES[kind];
    let saved: RemoteProvider;
    if (existing) {
      saved = Object.assign(existing, {
        name,
        baseUrl,
        enabled: input.enabled,
        updatedAt: clock,
      });
    } else {
      saved = fake.add(kind, { name, baseUrl, enabled: input.enabled });
      saved = fake.providers.find((p) => p.id === saved.id) ?? saved;
    }
    // Comme le moteur (`providers::save`) : clé vide = retirée ; adresse
    // d'un serveur changée sans nouvelle clé = clé retirée (jamais suivie
    // vers une autre adresse) ; nouvelle clé = remplace.
    if (key === "" || (key === undefined && addressChanged)) {
      fake.keys.delete(saved.id);
    } else if (key !== undefined) fake.keys.set(saved.id, key);
    return view(saved);
  });
  simulator.handle("delete_remote_provider", (args: Args) => {
    failing("delete_remote_provider");
    const id = args.id as string;
    fake.providers = fake.providers.filter((p) => p.id !== id);
    fake.keys.delete(id);
    return null;
  });
  simulator.handle("check_remote_provider", (args: Args) => {
    failing("check_remote_provider");
    const id = args.id as string;
    return (
      fake.checks.get(id) ?? {
        ok: true,
        problem: null,
        status: null,
        models: 3,
      }
    );
  });
  registry.set(simulator, fake);
  return fake;
}
