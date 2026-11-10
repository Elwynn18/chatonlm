// Faux moteur llama.cpp pour le simulateur (essais vitest seulement, lot
// L5b) : état du moteur (Vulkan, NVIDIA GeForce RTX 3090, llama.cpp v0.9.0 par
// défaut), dossier des journaux, dossier des modèles et son changement.
// `engine_status`, `open_engine_logs` et `engine_models_dir` rendent ce qui
// est réglé ; `pick_models_folder` prend le prochain choix de `picks` (vide :
// annulé, null) ; `change_engine_models_dir` émet sur son canal `total` puis
// `progress` (octets des modèles s'ils sont déplacés), change le dossier et
// rend son bilan (`moved`), ou, `hold` vrai, attend que l'essai le termine
// (`held` : arguments, canal, `finish`, `fail`). Une panne d'une commande se
// force par `failures` (erreur du moteur, par nom de commande). Un
// changement accepté décharge les modèles (`onAccepted`), comme le moteur.

import type { AppError } from "@/bindings/AppError";
import type { EngineModelsDir } from "@/bindings/EngineModelsDir";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { FolderMoved } from "@/bindings/FolderMoved";
import type { Args, FakeChannel, Simulator } from "@/test/simulator";

/** Dossier des modèles par défaut (`<données>/models`). */
export const DEFAULT_MODELS_DIR =
  "/home/user/.local/share/com.chatonlm.chatonlm/models";

/** Dossier des journaux du moteur. */
export const ENGINE_LOGS_DIR =
  "/home/user/.local/share/com.chatonlm.chatonlm/logs";

/** Carte graphique du faux moteur. */
export const ENGINE_GPU = "NVIDIA GeForce RTX 3090";

/** Version de llama.cpp livrée (`packaging/llama.env`). */
export const ENGINE_VERSION = "v0.9.0+chatonlm.1b94fcb0b0db";

const GIB = 1024 ** 3;

/** Changement du dossier retenu par l'essai (`hold`). */
export interface HeldChange {
  args: { path: string; moveModels: boolean };
  /** Canal de progression de la commande. */
  channel: FakeChannel;
  /** Termine le changement (bilan par défaut complété par `moved`). */
  finish(moved?: Partial<FolderMoved>): void;
  /** Échec du moteur (refus, panne). */
  fail(error: AppError): void;
}

export interface FakeEngine {
  /** Rendu par engine_status. */
  status: EngineStatus;
  /** Rendu par engine_models_dir ; changé par un changement réussi. */
  modelsDir: EngineModelsDir;
  /** Prochains choix du sélecteur de dossier (vide : annulé). */
  picks: (string | null)[];
  /** Bilan d'un changement réussi, en plus de celui par défaut. */
  moved: Partial<FolderMoved>;
  /** Vrai : change_engine_models_dir attend l'essai (`held`). */
  hold: boolean;
  /** Changements retenus, dans l'ordre. */
  held: HeldChange[];
  /** Erreur (panne, refus) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /**
   * Appelé quand un changement est accepté : le moteur décharge alors
   * lui-même les modèles (faux backend : `running` vidé). Jamais sur un
   * refus.
   */
  onAccepted: (() => void) | null;
}

/** Bilan d'un changement : dossier, modèles renommés ou copiés. */
function movedTo(
  path: string,
  dir: EngineModelsDir,
  move: boolean,
  overrides: Partial<FolderMoved>,
): FolderMoved {
  return {
    path,
    renamed: 0,
    copied: move ? dir.models : 0,
    cleanupFailed: false,
    ...overrides,
  };
}

export function fakeEngine(simulator: Simulator): FakeEngine {
  const engine: FakeEngine = {
    status: {
      available: true,
      version: ENGINE_VERSION,
      backend: "vulkan",
      gpuName: ENGINE_GPU,
      cpuFallback: false,
      logsDir: ENGINE_LOGS_DIR,
    },
    modelsDir: {
      path: DEFAULT_MODELS_DIR,
      isDefault: true,
      freeBytes: 120 * GIB,
      models: 3,
      bytes: 9.5 * GIB,
    },
    picks: [],
    moved: {},
    hold: false,
    held: [],
    failures: new Map(),
    onAccepted: null,
  };

  function failure(command: string) {
    const error = engine.failures.get(command);
    if (error) throw error;
  }

  /** Le dossier change : modèles gardés s'ils sont déplacés. */
  function applyChange(path: string, move: boolean) {
    engine.modelsDir = {
      ...engine.modelsDir,
      path,
      isDefault: path === DEFAULT_MODELS_DIR,
      ...(move ? {} : { models: 0, bytes: 0 }),
    };
  }

  simulator.handle("engine_status", () => {
    failure("engine_status");
    return engine.status;
  });

  simulator.handle("open_engine_logs", () => {
    failure("open_engine_logs");
    return null;
  });

  simulator.handle("engine_models_dir", () => {
    failure("engine_models_dir");
    return engine.modelsDir;
  });

  simulator.handle("pick_models_folder", () => {
    failure("pick_models_folder");
    return engine.picks.length > 0 ? engine.picks.shift() : null;
  });

  simulator.handle("change_engine_models_dir", async (args: Args) => {
    failure("change_engine_models_dir");
    const path = args.path;
    const moveModels = args.moveModels;
    if (typeof path !== "string" || typeof moveModels !== "boolean") {
      throw simulator.fail(
        `change_engine_models_dir: bad arguments ${JSON.stringify(args)}`,
      );
    }
    const channel = simulator.channel(args.onEvent);
    const before = engine.modelsDir;
    engine.onAccepted?.();
    if (engine.hold) {
      return new Promise<FolderMoved>((resolve, reject) => {
        engine.held.push({
          args: { path, moveModels },
          channel,
          finish(moved = {}) {
            applyChange(path, moveModels);
            resolve(movedTo(path, before, moveModels, moved));
          },
          fail: reject,
        });
      });
    }
    if (moveModels && before.bytes > 0) {
      await channel.send({ kind: "total", bytes: before.bytes });
      await channel.send({ kind: "progress", done: before.bytes });
    }
    applyChange(path, moveModels);
    return movedTo(path, before, moveModels, engine.moved);
  });

  return engine;
}
