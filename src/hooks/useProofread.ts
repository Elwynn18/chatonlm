// État de la page Proofread (propre à ChatonLM), dans un magasin de module lu
// par useSyncExternalStore : il survit à la navigation (une correction
// continue hors de la page, son résultat attend le retour), pas au
// redémarrage de l'app.
//
// Phases : « edit » (zone de texte, `draft`), « running » (évènements du
// moteur appliqués au travail `run`), « review » (texte final montré avec
// ses marques ; `draft` garde le texte d'avant, pour Undo et pour le Ctrl+Z
// natif de la zone de texte quand la frappe y revient). Un arrêt ou une
// erreur garde les passages terminés ; s'il n'y en a aucun, la page revient
// à l'édition du texte d'avant (erreur montrée sous la zone).
//
// Undo de la revue se défait : la revue annulée (`undone`) revient par Redo
// tant que le texte d'avant n'a pas été modifié. La sélection corrigée par
// le raccourci global (« Show ») ne fait rien perdre : ce que la page
// contenait (brouillon, revue, ou travail en cours arrêté avec ses passages
// terminés) est mis de côté (`saved`) et rétabli par « Back to your text ».

import { Channel } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import type { PartResult } from "@/bindings/PartResult";
import type { ProofreadErrorCode } from "@/bindings/ProofreadErrorCode";
import type { ProofreadEvent } from "@/bindings/ProofreadEvent";
import type { ProofreadMode } from "@/bindings/ProofreadMode";
import type { ProofreadPart } from "@/bindings/ProofreadPart";
import type { SelectionResult } from "@/bindings/SelectionResult";
import { errorMessage, proofreadCancel, proofreadRun } from "@/lib/ipc";
import { assembleText } from "@/lib/proofread";

/** Attente de l'évènement final après la réponse de la commande. */
const FINAL_EVENT_GRACE_MS = 2000;

export interface ProofreadError {
  code: ProofreadErrorCode;
  /** Message du moteur, dans la langue de l'interface. */
  message: string;
  /** Action qui a échoué (« Try again » la refait). */
  mode: ProofreadMode;
}

export interface ProofreadRun {
  /** Identifiant du travail (proofread_cancel). */
  id: string;
  mode: ProofreadMode;
  /** Texte envoyé : le texte d'avant (Undo). */
  source: string;
  /** Modèle annoncé par le moteur ; null avant `started`. */
  model: string | null;
  /** Découpage du moteur ; null avant `started`. */
  parts: ProofreadPart[] | null;
  /** Un par passage ; null : pas encore terminé. */
  results: (PartResult | null)[];
  /** Fragments reçus du passage en cours. */
  streaming: { index: number; text: string } | null;
  /** Arrêt demandé (Stop), en attente de la fin. */
  stopping: boolean;
  /** Fin par Stop. */
  stopped: boolean;
  /** Correction du raccourci global (action « Show » de sa notification). */
  fromSelection: boolean;
}

export type ProofreadPhase = "edit" | "running" | "review";

/** Revue annulée par Undo, que Redo rétablit. */
export interface UndoneReview {
  run: ProofreadRun;
  error: ProofreadError | null;
}

/** Contenu de la page mis de côté (jamais un travail en cours). */
export interface SavedPage {
  phase: "edit" | "review";
  draft: string;
  run: ProofreadRun | null;
  error: ProofreadError | null;
  undone: UndoneReview | null;
}

export interface ProofreadState {
  phase: ProofreadPhase;
  /** Contenu de la zone de texte ; en revue, le texte d'avant. */
  draft: string;
  /** Travail en cours, ou montré en revue ; null en édition. */
  run: ProofreadRun | null;
  /** Erreur sous la zone (édition ou revue). */
  error: ProofreadError | null;
  /** Revue annulée (Redo), tant que le texte rendu n'a pas changé. */
  undone: UndoneReview | null;
  /** Ce que la page contenait avant de montrer la sélection corrigée. */
  saved: SavedPage | null;
}

const INITIAL: ProofreadState = {
  phase: "edit",
  draft: "",
  run: null,
  error: null,
  undone: null,
  saved: null,
};

let state: ProofreadState = INITIAL;
const listeners = new Set<() => void>();
/** Demandes « Show » déjà servies (clés de navigation). */
const servedRequests = new Set<string>();

function set(next: ProofreadState) {
  state = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** État courant, hors rendu (gestionnaires d'évènements). */
export function getProofreadState(): ProofreadState {
  return state;
}

/** État de la page ; le composant est rendu de nouveau à chaque changement. */
export function useProofread(): ProofreadState {
  return useSyncExternalStore(subscribe, getProofreadState, getProofreadState);
}

/** Texte final : celui de la revue, sinon celui de la zone de texte. */
export function currentText(current: ProofreadState = state): string {
  const run = current.run;
  if (current.phase === "review" && run?.parts) {
    return assembleText(run.parts, run.results);
  }
  return current.draft;
}

/**
 * Frappe dans la zone de texte (édition seulement) ; l'erreur s'efface, et
 * Redo n'est plus possible si le texte a changé.
 */
export function setDraft(text: string) {
  if (state.phase !== "edit") return;
  if (text === state.draft && state.error === null) return;
  set({
    ...state,
    draft: text,
    error: null,
    undone: text === state.draft ? state.undone : null,
  });
}

/**
 * Passe de la revue à l'édition du texte final (frappe, clic, Undo natif) ;
 * `draft` reste le texte d'avant tant que la page ne l'a pas remplacé dans
 * la zone de texte (insertText, pour le Ctrl+Z natif), sauf `text` donné.
 */
export function editReview(text?: string) {
  if (state.phase !== "review") return;
  set({
    ...state,
    phase: "edit",
    draft: text ?? state.draft,
    run: null,
    error: null,
    undone: null,
  });
}

/** Undo de la revue : retour à l'édition du texte d'avant (Redo possible). */
export function undoReview() {
  if (state.phase !== "review" || !state.run) return;
  set({
    ...state,
    phase: "edit",
    draft: state.run.source,
    run: null,
    error: null,
    undone: { run: state.run, error: state.error },
  });
}

/** Redo possible : revue annulée, texte d'avant inchangé. */
export function canRedo(current: ProofreadState = state): boolean {
  return (
    current.phase === "edit" &&
    current.undone !== null &&
    current.draft === current.undone.run.source
  );
}

/** Redo : la revue annulée revient. */
export function redoReview() {
  if (!canRedo() || !state.undone) return;
  const { run, error } = state.undone;
  set({
    ...state,
    phase: "review",
    draft: run.source,
    run,
    error,
    undone: null,
  });
}

/** Correct ou Rephrase sur le texte courant ; sans effet s'il est vide. */
export function startProofread(mode: ProofreadMode) {
  if (state.phase === "running") return;
  const text = currentText();
  if (!text.trim()) return;
  const run: ProofreadRun = {
    id: crypto.randomUUID(),
    mode,
    source: text,
    model: null,
    parts: null,
    results: [],
    streaming: null,
    stopping: false,
    stopped: false,
    fromSelection: false,
  };
  set({
    ...state,
    phase: "running",
    draft: text,
    run,
    error: null,
    undone: null,
  });

  let finalEvent: () => void = () => {};
  const ended = new Promise<void>((resolve) => (finalEvent = resolve));
  const channel = new Channel<ProofreadEvent>();
  channel.onmessage = (event) => {
    if (event.event === "done" || event.event === "error") finalEvent();
    apply(run.id, event);
  };
  proofreadRun(run.id, text, mode, channel).then(
    async () => {
      // Tauri n'ordonne pas la réponse et les derniers messages du canal.
      await Promise.race([
        ended,
        new Promise((resolve) => setTimeout(resolve, FINAL_EVENT_GRACE_MS)),
      ]);
      // Fini sans évènement final : terminé tel quel.
      if (isRunning(run.id)) finish(null, false);
    },
    (cause: unknown) => {
      if (!isRunning(run.id)) return;
      finish({ code: "failed", message: errorMessage(cause), mode }, false);
    },
  );
}

/** Stop : le moteur répond par `done { stopped: true }`. */
export function stopProofread() {
  const run = state.run;
  if (state.phase !== "running" || !run || run.stopping) return;
  set({ ...state, run: { ...run, stopping: true } });
  proofreadCancel(run.id).catch((cause: unknown) =>
    console.error("Failed to stop the correction:", cause),
  );
}

/**
 * Contenu de la page à mettre de côté avant de montrer la sélection : un
 * brouillon non vide, une revue, ou le travail en cours, arrêté (passages
 * terminés gardés en revue, sinon son texte en édition) ; null s'il n'y a
 * rien à garder.
 */
function pageToSave(current: ProofreadState): SavedPage | null {
  const { phase, draft, run, error, undone } = current;
  if (phase === "running" && run) {
    proofreadCancel(run.id).catch((cause: unknown) =>
      console.error("Failed to stop the correction:", cause),
    );
    if (run.parts && run.results.some((result) => result !== null)) {
      return {
        phase: "review",
        draft: run.source,
        run: { ...run, streaming: null, stopping: false, stopped: true },
        error: null,
        undone: null,
      };
    }
    return {
      phase: "edit",
      draft: run.source,
      run: null,
      error: null,
      undone: null,
    };
  }
  if (phase === "review" && run) {
    return { phase, draft, run, error, undone: null };
  }
  if (phase === "edit" && draft.trim()) {
    return { phase, draft, run: null, error, undone };
  }
  return null;
}

/**
 * Montre la correction du raccourci global en revue (texte d'origine →
 * corrigé). Ce que la page contenait est mis de côté (« Back to your
 * text ») ; une correction de la page en cours est arrêtée. La revue d'une
 * sélection précédente, elle, est remplacée : le texte mis de côté avant
 * elle reste celui qu'on rétablit.
 */
export function showSelection(selection: SelectionResult) {
  const previousSelection =
    state.phase === "review" && state.run?.fromSelection === true;
  const saved = previousSelection
    ? state.saved
    : (pageToSave(state) ?? state.saved);
  const source = assembleText(selection.parts, []);
  const results = selection.parts.map(
    (_, index) => selection.results[index] ?? null,
  );
  if (!results.some((result) => result !== null)) {
    set({
      phase: "edit",
      draft: source,
      run: null,
      error: null,
      undone: null,
      saved,
    });
    return;
  }
  set({
    phase: "review",
    draft: source,
    run: {
      id: crypto.randomUUID(),
      mode: "correct",
      source,
      model: selection.model,
      parts: selection.parts,
      results,
      streaming: null,
      stopping: false,
      stopped: false,
      fromSelection: true,
    },
    error: null,
    undone: null,
    saved,
  });
}

/** « Back to your text » : la page mise de côté revient (pas pendant un travail). */
export function restoreSaved() {
  if (!state.saved || state.phase === "running") return;
  set({ ...state.saved, saved: null });
}

function isRunning(id: string): boolean {
  return state.phase === "running" && state.run?.id === id;
}

/** Applique un évènement du travail `id` (ignoré s'il n'est plus le travail en cours). */
function apply(id: string, event: ProofreadEvent) {
  const run = state.run;
  if (!run || !isRunning(id)) return;
  switch (event.event) {
    case "started":
      set({
        ...state,
        run: {
          ...run,
          model: event.model,
          parts: event.parts,
          results: event.parts.map(() => null),
        },
      });
      break;
    case "delta": {
      if (!run.parts) return;
      const previous =
        run.streaming?.index === event.index ? run.streaming.text : "";
      set({
        ...state,
        run: {
          ...run,
          streaming: { index: event.index, text: previous + event.text },
        },
      });
      break;
    }
    case "partDone": {
      if (!run.parts || event.index >= run.parts.length) return;
      const results = [...run.results];
      results[event.index] = event.result;
      set({
        ...state,
        run: {
          ...run,
          results,
          streaming:
            run.streaming?.index === event.index ? null : run.streaming,
        },
      });
      break;
    }
    case "done":
      finish(null, event.stopped);
      break;
    case "error":
      finish(
        { code: event.code, message: event.message, mode: run.mode },
        false,
      );
      break;
  }
}

/**
 * Fin du travail en cours : revue s'il a au moins un passage terminé, sinon
 * retour à l'édition du texte d'avant.
 */
function finish(error: ProofreadError | null, stopped: boolean) {
  const run = state.run;
  if (!run) return;
  if (!run.parts || !run.results.some((result) => result !== null)) {
    set({ ...state, phase: "edit", draft: run.source, run: null, error });
    return;
  }
  set({
    ...state,
    phase: "review",
    draft: run.source,
    run: { ...run, streaming: null, stopping: false, stopped },
    error,
  });
}

/**
 * Demande de montrer la sélection corrigée (`key` : une par navigation) :
 * vrai la première fois seulement.
 */
export function claimSelectionRequest(key: string): boolean {
  if (servedRequests.has(key)) return false;
  servedRequests.add(key);
  return true;
}

/** Essais : état initial (le magasin vit aussi longtemps que le module). */
export function resetProofreadForTests() {
  servedRequests.clear();
  set(INITIAL);
}
