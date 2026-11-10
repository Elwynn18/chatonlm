// Faux moteur de la reprise des modèles d'Ollama pour le simulateur (essais
// vitest seulement, lot L5b). `model_import_scan` rend les dossiers réglés
// pour l'origine (`sources`) ; `model_import_start` vérifie le dossier (de
// cette origine) et les noms, pose la reprise en cours (`running`, octets
// des modèles nommés) et la note (`starts`), sans événement : l'essai la
// fait avancer par `progress()` puis la termine par `finish()`, chacun
// suivi de l'événement `model-import` (état complet, comme le moteur) ;
// une reprise déjà en cours est refusée (message `BUSY`).
// `model_import_cancel` est seulement noté (`cancels`) : l'essai joue la
// fin (`finish({ cancelled: true })`). `model_import_leftovers` rend
// `leftovers` ; `model_import_cleanup` rend `cleanup` et vide `leftovers`.
// Une panne d'une commande se force par `failures`.

import type { AppError } from "@/bindings/AppError";
import type { CleanupReport } from "@/bindings/CleanupReport";
import type { ImportCandidate } from "@/bindings/ImportCandidate";
import type { ImportOrigin } from "@/bindings/ImportOrigin";
import type { ImportProgress } from "@/bindings/ImportProgress";
import type { ImportReport } from "@/bindings/ImportReport";
import type { ImportSource } from "@/bindings/ImportSource";
import type { ImportState } from "@/bindings/ImportState";
import type { Leftovers } from "@/bindings/Leftovers";
import { MODEL_IMPORT_EVENT } from "@/lib/ipc";
import type { Args, Simulator } from "@/test/simulator";

/** Dossier des modèles de l'Ollama intégré de ChatonLM 0.9. */
export const BUILTIN_OLLAMA_DIR =
  "/home/user/.local/share/com.chatonlm.chatonlm/ollama/models";

/** Dossier des modèles de l'Ollama de la machine (service système). */
export const MACHINE_OLLAMA_DIR = "/usr/share/ollama/.ollama/models";

/** Refus d'une seconde reprise (texte du moteur, déjà traduit). */
export const BUSY =
  "Models are already being imported or moved. Wait until it's done.";

const GIB = 1024 ** 3;

/** Modèle d'un dossier d'Ollama, prêt à reprendre sauf `fields`. */
export function candidate(
  name: string,
  fields: Partial<ImportCandidate> = {},
): ImportCandidate {
  return {
    name,
    aliases: [],
    id: null,
    sizeBytes: 1 * GIB,
    status: "ready",
    problem: null,
    suggestion: null,
    vision: false,
    noTemplate: false,
    ...fields,
  };
}

/** Dossier d'un Ollama (lisible, même disque, non partagé sauf `fields`). */
export function source(
  origin: ImportOrigin,
  models: ImportCandidate[],
  fields: Partial<ImportSource> = {},
): ImportSource {
  return {
    origin,
    dir: origin === "builtin" ? BUILTIN_OLLAMA_DIR : MACHINE_OLLAMA_DIR,
    readable: true,
    sameDrive: origin === "builtin",
    shared: false,
    freeBytes: 120 * GIB,
    models,
    ...fields,
  };
}

/** Une reprise lancée par l'écran. */
export interface ImportStart {
  origin: ImportOrigin;
  dir: string;
  names: string[];
}

export interface FakeModelImport {
  /** Rendu par model_import_state ; émis par `emit()`. */
  state: ImportState;
  /** Dossiers rendus par model_import_scan, par origine. */
  sources: Record<ImportOrigin, ImportSource[]>;
  /** Rendu par model_import_leftovers (null : rien à effacer). */
  leftovers: Leftovers | null;
  /** Rendu par model_import_cleanup (puis `leftovers` vidé). */
  cleanup: CleanupReport;
  /** Reprises lancées, dans l'ordre. */
  starts: ImportStart[];
  /** Nombre d'appels à model_import_cancel. */
  cancels: number;
  /** Erreur (panne, refus) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Émet « model-import » avec l'état du moment. */
  emit(): Promise<void>;
  /** La reprise en cours avance (`fields`), puis « model-import ». */
  progress(fields: Partial<ImportProgress>): Promise<void>;
  /**
   * La reprise en cours finit : bilan (tous les modèles repris par défaut,
   * complété par `fields`), plus rien en cours, puis « model-import ».
   */
  finish(fields?: Partial<ImportReport>): Promise<void>;
}

export function fakeModelImport(simulator: Simulator): FakeModelImport {
  const fake: FakeModelImport = {
    state: { running: null, last: null },
    sources: { builtin: [], machine: [] },
    leftovers: null,
    cleanup: { removedFiles: 0, removedBytes: 0, failed: 0 },
    starts: [],
    cancels: 0,
    failures: new Map(),
    emit: () =>
      simulator.emitEvent(MODEL_IMPORT_EVENT, structuredClone(fake.state)),
    async progress(fields) {
      const running = fake.state.running;
      if (!running) throw simulator.fail("progress: no import is running");
      fake.state = { ...fake.state, running: { ...running, ...fields } };
      await fake.emit();
    },
    async finish(fields = {}) {
      const running = fake.state.running;
      if (!running) throw simulator.fail("finish: no import is running");
      const start = fake.starts[fake.starts.length - 1];
      fake.state = {
        running: null,
        last: {
          origin: running.origin,
          dir: running.dir,
          imported: (start?.names ?? []).map((name) => ({
            name,
            id: `@ollama/${name}`,
          })),
          redownload: [],
          failed: [],
          cancelled: false,
          error: null,
          ...fields,
        },
      };
      await fake.emit();
    },
  };

  function failure(command: string) {
    const error = fake.failures.get(command);
    if (error) throw error;
  }

  function originOf(command: string, args: Args): ImportOrigin {
    const origin = args.origin;
    if (origin !== "builtin" && origin !== "machine") {
      throw simulator.fail(`${command}: bad origin ${JSON.stringify(args)}`);
    }
    return origin;
  }

  simulator.handle("model_import_state", () => {
    failure("model_import_state");
    return fake.state;
  });

  simulator.handle("model_import_scan", (args: Args) => {
    failure("model_import_scan");
    return fake.sources[originOf("model_import_scan", args)];
  });

  simulator.handle("model_import_start", (args: Args) => {
    failure("model_import_start");
    const origin = originOf("model_import_start", args);
    const { dir, names } = args;
    if (typeof dir !== "string" || !Array.isArray(names)) {
      throw simulator.fail(
        `model_import_start: bad arguments ${JSON.stringify(args)}`,
      );
    }
    const found = fake.sources[origin].find((s) => s.dir === dir);
    if (!found) {
      throw simulator.fail(`model_import_start: unknown folder ${dir}`);
    }
    if (fake.state.running) throw { kind: "invalid", message: BUSY };
    const chosen = (names as string[]).map((name) => {
      const model = found.models.find((m) => m.name === name);
      if (!model) {
        throw simulator.fail(`model_import_start: unknown model ${name}`);
      }
      return model;
    });
    fake.starts.push({ origin, dir, names: names as string[] });
    fake.state = {
      ...fake.state,
      running: {
        origin,
        dir,
        model: null,
        doneModels: 0,
        totalModels: chosen.length,
        doneBytes: 0,
        totalBytes: chosen.reduce((total, m) => total + m.sizeBytes, 0),
      },
    };
    return null;
  });

  simulator.handle("model_import_cancel", () => {
    failure("model_import_cancel");
    fake.cancels += 1;
    return null;
  });

  simulator.handle("model_import_leftovers", () => {
    failure("model_import_leftovers");
    return fake.leftovers;
  });

  simulator.handle("model_import_cleanup", () => {
    failure("model_import_cleanup");
    if (!fake.leftovers) {
      throw simulator.fail("model_import_cleanup: nothing to clean");
    }
    fake.leftovers = null;
    return fake.cleanup;
  });

  return fake;
}
