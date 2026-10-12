// Faux moteur de la correction et des raccourcis du système pour le
// simulateur (tests vitest seulement).
//
// proofread_run : comme le moteur, erreur seule avant le découpage (texte
// vide, trop long, aucun modèle : réglage `proofreadModel`, sinon celui du
// chat), puis `started` (passages coupés aux lignes vides, blancs autour de
// chaque passage dans before/after), et pour chaque passage deux `delta`
// puis `partDone`, enfin `done`. Correction déterministe (table fixe, comme
// le modèle `chatonlm-proofread` du faux serveur) ; Rephrase la complète de
// quelques tournures ; un passage qui contient « KEEP » est gardé tel quel
// (garde-fou). Modèle non installé : `modelNotFound` après `started`. Une
// panne en cours se force par `failAt`. Rien n'est livré tout seul : le
// test fait avancer le travail par `step()` ou `finish()` ; après
// proofread_cancel, `finish()` livre `done { stopped: true }` (passages
// terminés gardés), comme le moteur.
//
// shortcuts_status, shortcuts_bind, shortcuts_configure : statut en
// mémoire ; activer `proofreadShortcut` (update_settings) commence la
// liaison (Binding), la couper la délie ; `bound()` termine une liaison et
// émet « shortcuts-status », `setShortcuts()` force un statut.
//
// Windows (`chooser` « app », à poser par l'essai ; « desktop » par défaut) :
// shortcuts_set_trigger lie la combinaison reçue (libellé « Ctrl+Alt+D »), ou
// la dit tenue par une autre application si elle est dans `takenTriggers`
// (statut `taken`), ou rejette (`failures`) ; shortcuts_reset_trigger rend
// la combinaison par défaut ; shortcuts_recording note ses appels.

import { act } from "@testing-library/react";
import type { AppError } from "@/bindings/AppError";
import type { ProofreadErrorCode } from "@/bindings/ProofreadErrorCode";
import type { ProofreadEvent } from "@/bindings/ProofreadEvent";
import type { ProofreadMode } from "@/bindings/ProofreadMode";
import type { ProofreadPart } from "@/bindings/ProofreadPart";
import type { SelectionResult } from "@/bindings/SelectionResult";
import type { Settings } from "@/bindings/Settings";
import type { ShortcutKeys } from "@/bindings/ShortcutKeys";
import type { ShortcutsStatus } from "@/bindings/ShortcutsStatus";
import type { ShortcutStatus } from "@/bindings/ShortcutStatus";
import type { Args, FakeChannel, Simulator } from "@/test/simulator";

/** Taille maximale d'un texte (MAX_CHARS du moteur). */
export const MAX_CHARS = 100_000;

/** Commandes de repli montrées quand le portail manque. */
export const DICTATE_SHORTCUT_COMMAND = "/usr/bin/chatonlm --dictate";
export const PROOFREAD_COMMAND = "/usr/bin/chatonlm --proofread";

/** Combinaisons par défaut sous Windows. */
export const DEFAULT_TRIGGERS = {
  dictation: "Ctrl+Alt+D",
  proofread: "Ctrl+Alt+C",
} as const;

/** Libellé d'une combinaison saisie, comme le moteur : « Ctrl+Alt+D ». */
export function triggerLabel(keys: ShortcutKeys): string {
  const { keyCode } = keys;
  const key =
    keyCode >= 112 && keyCode <= 135
      ? `F${keyCode - 111}`
      : String.fromCharCode(keyCode);
  return [
    keys.ctrl && "Ctrl",
    keys.alt && "Alt",
    keys.shift && "Shift",
    keys.meta && "Win",
    key,
  ]
    .filter(Boolean)
    .join("+");
}

/** Remplacements de la correction (comme le faux serveur). */
const CORRECTIONS: [RegExp, string][] = [
  [/\bteh\b/g, "the"],
  [/\bTeh\b/g, "The"],
  [/\brecieve\b/g, "receive"],
  [/\ballez au\b/g, "allé au"],
  [/\bcinema\b/g, "cinéma"],
];

/** Tournures réécrites par Rephrase, après la correction. */
const REPHRASINGS: [RegExp, string][] = [
  [/\bvery good\b/g, "excellent"],
  [/\ba lot of\b/g, "many"],
];

/** Texte corrigé (ou reformulé) d'un passage. */
export function fakeCorrection(text: string, mode: ProofreadMode): string {
  let out = text;
  for (const [pattern, replacement] of CORRECTIONS) {
    out = out.replace(pattern, replacement);
  }
  if (mode === "rephrase") {
    for (const [pattern, replacement] of REPHRASINGS) {
      out = out.replace(pattern, replacement);
    }
  }
  return out;
}

/**
 * Passages coupés aux lignes vides ; blancs autour de chaque passage dans
 * before/after. La concaténation redonne le texte.
 */
export function fakeSplit(text: string): ProofreadPart[] {
  const parts: ProofreadPart[] = [];
  const chunks = text.split(/(\n[ \t]*\n\s*)/);
  let pending = "";
  for (let i = 0; i < chunks.length; i += 1) {
    const chunk = chunks[i];
    if (i % 2 === 1) {
      pending += chunk;
      continue;
    }
    const match = /^(\s*)([\s\S]*?)(\s*)$/.exec(chunk)!;
    const [, lead, body, trail] = match;
    if (!body) {
      pending += chunk;
      continue;
    }
    if (parts.length > 0) {
      parts[parts.length - 1].after += pending;
      pending = "";
    }
    parts.push({ before: pending + lead, text: body, after: trail });
    pending = "";
  }
  if (parts.length > 0) parts[parts.length - 1].after += pending;
  return parts;
}

export interface FakeJob {
  jobId: string;
  text: string;
  mode: ProofreadMode;
  model: string;
  /** Évènements prévus, dans l'ordre. */
  readonly planned: ProofreadEvent[];
  /** Évènements livrés. */
  readonly delivered: ProofreadEvent[];
  cancelled: boolean;
  /** Fini : `done` ou `error` livré, commande résolue. */
  finished: boolean;
  /** Livre les `count` prochains évènements prévus. */
  step(count?: number): Promise<void>;
  /** Livre la fin (après Stop : `done { stopped: true }` seulement). */
  finish(): Promise<void>;
  /**
   * Résout la commande sans livrer l'évènement final (Tauri n'ordonne pas
   * la réponse et les messages du canal).
   */
  answer(): Promise<void>;
}

export interface FakeProofread {
  /** Réponse de proofread_last_selection. */
  lastSelection: SelectionResult | null;
  /** Travaux reçus, dans l'ordre. */
  jobs: FakeJob[];
  /** Appels de proofread_cancel_all (chargement de la page). */
  cancelAllCalls: number;
  /** Dernier travail reçu (échec du test s'il n'y en a pas). */
  lastJob(): FakeJob;
  /**
   * Panne après `afterParts` passages terminés (null : aucune) ; le code
   * `unreachable` est celui d'Ollama injoignable.
   */
  failAt: {
    afterParts: number;
    code: ProofreadErrorCode;
    message: string;
  } | null;
  /** Statut des raccourcis (shortcuts_status). */
  shortcuts: ShortcutsStatus;
  /** Touches au bout d'une liaison de la correction. */
  boundProofread: ShortcutStatus;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Combinaisons qu'une autre application tient (libellés). */
  takenTriggers: Set<string>;
  /** Appels de shortcuts_recording, dans l'ordre (`active`). */
  recordings: boolean[];
  /** Change le statut des raccourcis puis émet « shortcuts-status ». */
  setShortcuts(patch: Partial<ShortcutsStatus>): Promise<void>;
  /** Fin de la liaison de la correction par le bureau, puis émission. */
  bound(): Promise<void>;
  /** Appelé par update_settings du faux backend (réglage proofreadShortcut). */
  applyShortcut(enabled: boolean): void;
}

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

/** Gestionnaires de proofread_* et shortcuts_*. */
export function fakeProofread(
  simulator: Simulator,
  backend: { settings: Settings; models: string[] },
): FakeProofread {
  const fake: FakeProofread = {
    lastSelection: null,
    jobs: [],
    cancelAllCalls: 0,
    lastJob() {
      const job = fake.jobs.at(-1);
      if (!job) throw simulator.fail("no proofread_run received");
      return job;
    },
    failAt: null,
    shortcuts: {
      dictation: { state: "disabled" },
      proofread: { state: "disabled" },
      canConfigure: true,
      chooser: "desktop",
      dictateCommand: DICTATE_SHORTCUT_COMMAND,
      proofreadCommand: PROOFREAD_COMMAND,
    },
    boundProofread: { state: "bound", trigger: "Ctrl+Alt+C" },
    failures: new Map(),
    takenTriggers: new Set(),
    recordings: [],
    async setShortcuts(patch) {
      fake.shortcuts = { ...fake.shortcuts, ...patch };
      await simulator.emitEvent("shortcuts-status", fake.shortcuts);
    },
    async bound() {
      await fake.setShortcuts({ proofread: fake.boundProofread });
    },
    applyShortcut(enabled) {
      fake.shortcuts = {
        ...fake.shortcuts,
        proofread: enabled ? { state: "binding" } : { state: "disabled" },
      };
    },
  };

  function failure(command: string) {
    const error = fake.failures.get(command);
    if (error) throw error;
  }

  /** Évènements d'un travail, comme le moteur les enverrait. */
  function plan(text: string, mode: ProofreadMode, model: string) {
    const error = (code: ProofreadErrorCode, message: string) =>
      ({ event: "error", code, message }) as const;
    if (!text.trim()) return [error("empty", "There is no text to correct")];
    if ([...text].length > MAX_CHARS) {
      return [error("tooLong", "The text is too long")];
    }
    if (!model) {
      return [error("noModel", "Choose a model for the correction first")];
    }
    const parts = fakeSplit(text);
    const events: ProofreadEvent[] = [{ event: "started", model, parts }];
    if (!backend.models.includes(model)) {
      events.push(error("modelNotFound", `Model "${model}" not found`));
      return events;
    }
    for (let index = 0; index < parts.length; index += 1) {
      if (fake.failAt && fake.failAt.afterParts === index) {
        events.push(error(fake.failAt.code, fake.failAt.message));
        return events;
      }
      const source = parts[index].text;
      const kept = source.includes("KEEP");
      const answer = kept
        ? "Sure! Here is my answer."
        : fakeCorrection(source, mode);
      const half = Math.ceil(answer.length / 2);
      events.push({ event: "delta", index, text: answer.slice(0, half) });
      events.push({ event: "delta", index, text: answer.slice(half) });
      events.push({
        event: "partDone",
        index,
        result: { text: kept ? source : answer, kept },
      });
    }
    events.push({ event: "done", stopped: false });
    return events;
  }

  simulator.handle("proofread_run", (args: Args) => {
    failure("proofread_run");
    const channel: FakeChannel = simulator.channel(args.onEvent);
    const text = args.text as string;
    const mode = args.mode as ProofreadMode;
    const model =
      backend.settings.proofreadModel || backend.settings.selectedModel;
    let resolve: () => void = () => {};
    const answered = new Promise<null>((ok) => {
      resolve = () => ok(null);
    });
    const planned = plan(text, mode, model);
    const job: FakeJob = {
      jobId: args.jobId as string,
      text,
      mode,
      model,
      planned,
      delivered: [],
      cancelled: false,
      finished: false,
      async step(count = 1) {
        for (let i = 0; i < count; i += 1) {
          const event = planned[job.delivered.length];
          if (!event || job.finished) {
            throw simulator.fail(`proofread job ${job.jobId}: no event left`);
          }
          await deliver(event);
        }
      },
      async answer() {
        await act(async () => resolve());
      },
      async finish() {
        if (job.finished) return;
        if (job.cancelled) {
          await deliver({ event: "done", stopped: true });
          return;
        }
        while (!job.finished) await deliver(planned[job.delivered.length]);
      },
    };
    async function deliver(event: ProofreadEvent) {
      job.delivered.push(event);
      await channel.send(event);
      if (event.event === "done" || event.event === "error") {
        job.finished = true;
        resolve();
      }
    }
    fake.jobs.push(job);
    return answered;
  });
  simulator.handle("proofread_cancel", (args: Args) => {
    failure("proofread_cancel");
    const job = fake.jobs.find((j) => j.jobId === args.jobId);
    if (job && !job.finished) job.cancelled = true;
    return null;
  });
  simulator.handle("proofread_cancel_all", () => {
    failure("proofread_cancel_all");
    for (const job of fake.jobs) {
      if (!job.finished) job.cancelled = true;
    }
    fake.cancelAllCalls += 1;
    return null;
  });
  simulator.handle("proofread_last_selection", () => {
    failure("proofread_last_selection");
    return fake.lastSelection;
  });

  simulator.handle("shortcuts_status", () => {
    failure("shortcuts_status");
    return fake.shortcuts;
  });
  simulator.handle("shortcuts_bind", () => {
    failure("shortcuts_bind");
    const wanted = {
      dictation: fake.shortcuts.dictation.state !== "disabled",
      proofread: backend.settings.proofreadShortcut,
    };
    if (!wanted.dictation && !wanted.proofread) {
      throw invalid("No shortcut to set");
    }
    fake.shortcuts = {
      ...fake.shortcuts,
      ...(wanted.dictation && { dictation: { state: "binding" } }),
      ...(wanted.proofread && { proofread: { state: "binding" } }),
    };
    return null;
  });
  simulator.handle("shortcuts_configure", () => {
    failure("shortcuts_configure");
    const { dictation, proofread } = fake.shortcuts;
    if (dictation.state !== "bound" && proofread.state !== "bound") {
      throw invalid("The shortcut is not set yet");
    }
    return null;
  });

  /** Lie `trigger` au raccourci `id`, ou le dit tenu par une autre app. */
  function setTrigger(id: "dictation" | "proofread", trigger: string) {
    const status: ShortcutStatus = fake.takenTriggers.has(trigger)
      ? {
          state: "unavailable",
          problem: "taken",
          reason: `${trigger} is already used by another app`,
        }
      : { state: "bound", trigger };
    fake.shortcuts = { ...fake.shortcuts, [id]: status };
    return fake.shortcuts;
  }
  simulator.handle("shortcuts_set_trigger", (args: Args) => {
    failure("shortcuts_set_trigger");
    const id = args.id as "dictation" | "proofread";
    return setTrigger(id, triggerLabel(args.keys as ShortcutKeys));
  });
  simulator.handle("shortcuts_reset_trigger", (args: Args) => {
    failure("shortcuts_reset_trigger");
    const id = args.id as "dictation" | "proofread";
    return setTrigger(id, DEFAULT_TRIGGERS[id]);
  });
  simulator.handle("shortcuts_recording", (args: Args) => {
    failure("shortcuts_recording");
    fake.recordings.push(args.active as boolean);
    return null;
  });

  return fake;
}
