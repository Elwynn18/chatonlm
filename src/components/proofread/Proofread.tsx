// Page Proofread (propre à ChatonLM) : un volet unique. En édition, une
// grande zone de texte ; « Correct » (Ctrl+Entrée) ou « Rephrase »
// (Ctrl+Maj+Entrée) envoie le texte au modèle de la correction (sélecteur en
// tête : réglage `proofreadModel`, vide = « Auto »). Pendant le travail,
// les passages terminés sont montrés corrigés, le passage en cours en flux,
// les suivants atténués ; « Stop » (Échap) remplace les boutons. En revue,
// le texte final : après Correct, mots supprimés et ajoutés marqués (diff de
// mots par passage), nombre de corrections annoncé ; après Rephrase, le
// texte nouveau seul. Undo (Ctrl+Z) revient au texte d'avant, Redo
// (Ctrl+Maj+Z, Ctrl+Y) à la revue tant que ce texte n'a pas changé ; la
// prochaine frappe (touche morte et méthode de saisie comprises, Retour
// arrière et Suppr aussi) ou un clic dans le texte repasse en édition du
// texte final (la touche y arrive, le curseur au point cliqué, la vue au
// même endroit), remplacé par insertText pour que le Ctrl+Z natif de la
// zone retrouve le texte d'avant. Copier une partie de la revue donne son
// texte final, sans les mots supprimés. Une croix, en haut à droite du
// volet tant qu'il a du texte (hors travail en cours), le vide : par
// l'édition native elle aussi, Ctrl+Z retrouve donc le texte effacé.
//
// « Show » (sélection corrigée par le raccourci global) met de côté ce que
// la page contenait : une remarque « Back to your text » le rétablit.
//
// État dans un magasin de module (hooks/useProofread.ts) : une correction
// continue pendant qu'on visite une autre page. Erreurs sous le volet, avec
// une action selon leur code (choisir un modèle, ModelHub, réessayer) ; le
// message vient du moteur, déjà traduit.

import {
  ArrowPathIcon,
  ArrowUturnLeftIcon,
  ArrowUturnRightIcon,
  CheckIcon,
  ExclamationTriangleIcon,
  StopIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { Square2StackIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import {
  type ClipboardEvent,
  type MouseEvent,
  type Ref,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { flushSync } from "react-dom";
import type { ProofreadMode } from "@/bindings/ProofreadMode";
import { ModelSettingSelect } from "@/components/ModelSettingSelect";
import { ProofreadText } from "@/components/proofread/ProofreadText";
import {
  canRedo,
  currentText,
  editReview,
  getProofreadState,
  type ProofreadError,
  redoReview,
  restoreSaved,
  setDraft,
  startProofread,
  stopProofread,
  undoReview,
  useProofread,
} from "@/hooks/useProofread";
import { type TFunction, useT } from "@/i18n";
import { copyText } from "@/lib/clipboard";
import { overlayOpen } from "@/lib/overlay";
import {
  allDone,
  correctionCount,
  finalOffset,
  finalTextOf,
  partViews,
  someKept,
} from "@/lib/proofread";
import {
  isCorrectShortcut,
  isEscapeKey,
  isPasteShortcut,
  isRedoShortcut,
  isRephraseShortcut,
  isUndoShortcut,
  keysText,
  shortcut,
  type ShortcutId,
} from "@/lib/shortcuts";

const BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const PRIMARY_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-700/90 bg-zinc-800 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-zinc-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/10 dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-200";
/**
 * Même police, mêmes marges en édition et en revue : le texte ne bouge pas.
 * Marge droite plus large : la croix qui vide le volet ne couvre aucun mot.
 */
const TEXT =
  "h-full w-full overflow-y-auto p-4 pr-12 text-sm/7 break-words whitespace-pre-wrap text-neutral-900 dark:text-neutral-100";
/** Croix qui vide le volet, dans son coin. */
const CLEAR_BUTTON =
  "absolute top-2.5 right-3 inline-flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-800 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-neutral-100";
const LINK_BUTTON =
  "rounded-md font-medium underline underline-offset-2 outline-none hover:text-red-800 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-red-200";

/** Le champ ou l'élément éditable garde ses touches. */
function isEditable(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target.closest("input, textarea, select, [contenteditable='true']") !==
        null)
  );
}

/** Entrée et Espace activent le bouton ou le lien qui a le focus. */
function activatedByKey(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest("a[href], button, summary, [role='button']") !== null
  );
}

/**
 * Touche qui commence une saisie : un caractère, Entrée, une touche morte
 * (accent, « Dead ») ou une touche prise par la méthode de saisie
 * (« Process », composition).
 */
function startsTyping(event: KeyboardEvent) {
  return (
    event.key.length === 1 ||
    event.key === "Enter" ||
    event.key === "Dead" ||
    event.key === "Process" ||
    event.isComposing
  );
}

/** Libellé d'un bouton suivi de ses touches : « Correct (Ctrl+Enter) ». */
function withKeys(t: TFunction, action: string, id: ShortcutId) {
  return t("proofread.actions.withKeys", {
    action,
    keys: keysText(t, shortcut(id).keys),
  });
}

export function Proofread() {
  const t = useT();
  const state = useProofread();
  const { phase, draft, run, error, saved } = state;
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const reviewRef = useRef<HTMLDivElement>(null);
  const stopRef = useRef<HTMLButtonElement>(null);
  const modelRef = useRef<HTMLSelectElement>(null);
  const ids = {
    description: useId(),
    editor: useId(),
    model: useId(),
  };

  // Focus perdu par le changement de phase ou de contenu (bouton ou revue
  // retirés, zone de texte cachée : Ctrl+Entrée depuis la zone) : l'élément
  // principal de la nouvelle phase le prend.
  const runId = run?.id;
  const hasSaved = saved !== null;
  useEffect(() => {
    const active = document.activeElement;
    const lost =
      active === null ||
      active === document.body ||
      (active instanceof HTMLElement && active.closest("[hidden]") !== null);
    if (!lost || overlayOpen()) return;
    if (phase === "edit") textareaRef.current?.focus();
    else if (phase === "running") stopRef.current?.focus();
    else reviewRef.current?.focus({ preventScroll: true });
  }, [phase, runId, hasSaved]);

  /**
   * Revue → édition du texte final, curseur à `offset` (null : à la fin).
   * Appelée pendant la frappe : le focus est déplacé avant que la touche
   * soit traitée, elle arrive donc dans la zone de texte. Après un clic, la
   * zone reprend le défilement de la revue : le texte ne bouge pas et le
   * curseur reste où l'on a cliqué (à la fin, la frappe le montre).
   */
  const editFinalText = useCallback((offset: number | null) => {
    if (getProofreadState().phase !== "review") return null;
    const final = currentText();
    const top = reviewRef.current?.scrollTop ?? 0;
    // Zone montrée tout de suite, encore avec le texte d'avant.
    flushSync(() => editReview());
    const area = textareaRef.current;
    if (!area) return null;
    area.focus({ preventScroll: true });
    area.select();
    try {
      // Remplacement par l'édition native : Ctrl+Z retrouve le texte d'avant.
      document.execCommand("insertText", false, final);
    } catch {
      // Pas d'execCommand : valeur posée ci-dessous.
    }
    flushSync(() => setDraft(final));
    const at = Math.min(offset ?? final.length, area.value.length);
    area.setSelectionRange(at, at);
    if (offset !== null) {
      area.scrollTop = top;
      // WebKit peut encore amener le curseur de l'édition en vue.
      requestAnimationFrame(() => {
        if (document.activeElement === area && area.selectionEnd === at) {
          area.scrollTop = top;
        }
      });
    }
    return area;
  }, []);

  /**
   * Croix : vide le volet. Depuis la revue, le texte final passe d'abord en
   * édition ; l'effacement se fait par l'édition native (Ctrl+Z retrouve le
   * texte), la valeur étant posée à défaut.
   */
  const clearText = useCallback(() => {
    const area =
      getProofreadState().phase === "review"
        ? editFinalText(null)
        : textareaRef.current;
    if (!area) return;
    area.focus({ preventScroll: true });
    area.select();
    try {
      document.execCommand("delete");
    } catch {
      // Pas d'execCommand : valeur posée ci-dessous.
    }
    flushSync(() => setDraft(""));
  }, [editFinalText]);

  // En capture, avant la garde de navigation et l'annuler/rétablir des
  // champs de la route racine (écouteurs de document) : Retour arrière et
  // Suppr dans la revue (la garde les bloquerait hors d'un champ) passent en
  // édition et effacent ; Redo de la revue annulée, depuis la zone de texte
  // aussi (le Ctrl+Maj+Z natif n'y trouverait rien).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || overlayOpen()) return;
      const current = getProofreadState();
      const target = event.target;
      if (isRedoShortcut(event) && canRedo(current)) {
        if (target !== textareaRef.current && isEditable(target)) return;
        event.preventDefault();
        redoReview();
        return;
      }
      const erase = event.key === "Backspace" || event.key === "Delete";
      if (!erase || current.phase !== "review" || isEditable(target)) return;
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      event.preventDefault();
      const area = editFinalText(null);
      if (area) eraseAtCaret(area, event.key === "Backspace");
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [editFinalText]);

  // Raccourcis de la page, depuis tout élément (pas une boîte ni un menu
  // ouvert, ni une touche déjà prise ailleurs).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const current = getProofreadState();
      if (isEscapeKey(event)) {
        if (current.phase !== "running") return;
        event.preventDefault();
        stopProofread();
        return;
      }
      if (overlayOpen()) return;
      if (isCorrectShortcut(event) || isRephraseShortcut(event)) {
        event.preventDefault();
        startProofread(isRephraseShortcut(event) ? "rephrase" : "correct");
        return;
      }
      if (current.phase !== "review" || isEditable(event.target)) return;
      if (isUndoShortcut(event)) {
        event.preventDefault();
        undoReview();
        return;
      }
      if (isPasteShortcut(event)) {
        editFinalText(null);
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      // Pas de preventDefault : la touche (ou la composition commencée par
      // une touche morte) arrive dans la zone de texte.
      if (!startsTyping(event)) return;
      if (
        (event.key === " " || event.key === "Enter") &&
        activatedByKey(event.target)
      ) {
        return;
      }
      editFinalText(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [editFinalText]);

  const onReviewClick = (event: MouseEvent<HTMLDivElement>) => {
    const root = event.currentTarget;
    const selection = window.getSelection();
    // Texte sélectionné (pour le copier) : on reste en revue.
    if (selection && !selection.isCollapsed) return;
    let offset: number | null = null;
    if (
      selection &&
      selection.rangeCount > 0 &&
      selection.anchorNode &&
      root.contains(selection.anchorNode)
    ) {
      offset = finalOffset(root, selection.anchorNode, selection.anchorOffset);
    }
    editFinalText(offset);
  };

  // Copie d'une partie de la revue : le texte final seul, sans les mots
  // supprimés ni les libellés des lecteurs d'écran.
  const onReviewCopy = (event: ClipboardEvent<HTMLDivElement>) => {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) {
      return;
    }
    let text = "";
    for (let index = 0; index < selection.rangeCount; index += 1) {
      text += finalTextOf(selection.getRangeAt(index));
    }
    event.clipboardData.setData("text/plain", text);
    event.preventDefault();
  };

  const views = useMemo(
    () =>
      run?.parts
        ? partViews(run.mode, run.parts, run.results, run.streaming)
        : null,
    [run],
  );
  const text = phase === "edit" ? draft : currentText();
  const empty = !text.trim();

  return (
    <main
      data-testid="proofread"
      className="flex min-h-0 w-full flex-1 flex-col dark:bg-neutral-900"
    >
      <div className="mx-auto flex min-h-0 w-full max-w-4xl flex-1 flex-col gap-3 p-6 pt-2">
        <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
          <p
            id={ids.description}
            className="min-w-0 flex-1 text-sm/6 text-zinc-500 select-none dark:text-zinc-400"
          >
            {t("proofread.description")}
          </p>
          <ModelSelect id={ids.model} selectRef={modelRef} />
        </div>

        <section
          aria-label={t("proofread.title")}
          className="flex min-h-48 flex-1 flex-col overflow-hidden rounded-xl border border-neutral-200 bg-white dark:border-neutral-700/70 dark:bg-neutral-800"
        >
          <div className="relative min-h-0 flex-1">
            {/* Toujours montée : son historique d'édition (Ctrl+Z natif)
                survit à la correction. */}
            <textarea
              ref={textareaRef}
              id={ids.editor}
              aria-label={t("proofread.editor.label")}
              aria-describedby={ids.description}
              placeholder={t("proofread.editor.placeholder")}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              spellCheck
              hidden={phase !== "edit"}
              className={`${TEXT} resize-none bg-transparent outline-none placeholder:text-neutral-400 dark:placeholder:text-neutral-500`}
            />
            {phase === "running" && run && (
              <div
                data-testid="proofread-running"
                aria-busy="true"
                className={TEXT}
              >
                {views ? (
                  <ProofreadText views={views} />
                ) : (
                  <span className="text-neutral-500">{run.source}</span>
                )}
              </div>
            )}
            {phase === "review" && views && (
              <div
                ref={reviewRef}
                data-testid="proofread-review"
                role="region"
                aria-label={t(
                  run?.mode === "rephrase"
                    ? "proofread.review.labelRephrased"
                    : "proofread.review.label",
                )}
                aria-description={t("proofread.review.editHint")}
                tabIndex={0}
                onClick={onReviewClick}
                onCopy={onReviewCopy}
                className={`${TEXT} cursor-text outline-none focus-visible:ring-1 focus-visible:ring-neutral-300 focus-visible:ring-inset dark:focus-visible:ring-neutral-600`}
              >
                <ProofreadText views={views} />
              </div>
            )}
            {phase !== "running" && text !== "" && (
              <button
                type="button"
                className={CLEAR_BUTTON}
                aria-label={t("proofread.actions.clear")}
                title={t("proofread.actions.clear")}
                onClick={clearText}
              >
                <XMarkIcon aria-hidden="true" className="h-4 w-4" />
              </button>
            )}
          </div>

          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-neutral-200 px-4 py-2.5 select-none dark:border-neutral-700/70">
            <Status />
            <div className="flex flex-wrap items-center justify-end gap-2">
              {phase === "running" ? (
                <button
                  ref={stopRef}
                  type="button"
                  className={BUTTON}
                  disabled={run?.stopping}
                  onClick={stopProofread}
                  aria-keyshortcuts={shortcut("stopProofread").aria}
                  title={withKeys(
                    t,
                    t("proofread.actions.stop"),
                    "stopProofread",
                  )}
                >
                  <StopIcon aria-hidden="true" className="h-4 w-4" />
                  {run?.stopping
                    ? t("proofread.running.stopping")
                    : t("proofread.actions.stop")}
                </button>
              ) : (
                <>
                  {phase === "review" && (
                    <>
                      <button
                        type="button"
                        className={BUTTON}
                        onClick={undoReview}
                        aria-keyshortcuts={shortcut("undoCorrection").aria}
                        title={withKeys(
                          t,
                          t("proofread.actions.undo"),
                          "undoCorrection",
                        )}
                      >
                        <ArrowUturnLeftIcon
                          aria-hidden="true"
                          className="h-4 w-4"
                        />
                        {t("proofread.actions.undo")}
                      </button>
                      <CopyTextButton text={text} />
                    </>
                  )}
                  {canRedo(state) && (
                    <button
                      type="button"
                      className={BUTTON}
                      onClick={redoReview}
                      aria-keyshortcuts={shortcut("redoCorrection").aria}
                      title={withKeys(
                        t,
                        t("proofread.actions.redo"),
                        "redoCorrection",
                      )}
                    >
                      <ArrowUturnRightIcon
                        aria-hidden="true"
                        className="h-4 w-4"
                      />
                      {t("proofread.actions.redo")}
                    </button>
                  )}
                  <ModeButton mode="rephrase" disabled={empty} />
                  <ModeButton mode="correct" disabled={empty} />
                </>
              )}
            </div>
          </div>
        </section>

        {error && (
          <ErrorNotice
            error={error}
            onChooseModel={() => modelRef.current?.focus()}
          />
        )}
        {phase === "review" && run && <ReviewNotes />}
        {saved && phase !== "running" && <SavedNote />}
      </div>
    </main>
  );
}

/** Correct ou Rephrase (titre : le raccourci). */
function ModeButton({
  mode,
  disabled,
}: {
  mode: ProofreadMode;
  disabled: boolean;
}) {
  const t = useT();
  const id: ShortcutId = mode === "correct" ? "correct" : "rephrase";
  const label =
    mode === "correct"
      ? t("proofread.actions.correct")
      : t("proofread.actions.rephrase");
  return (
    <button
      type="button"
      className={mode === "correct" ? PRIMARY_BUTTON : BUTTON}
      disabled={disabled}
      onClick={() => startProofread(mode)}
      aria-keyshortcuts={shortcut(id).aria}
      title={withKeys(t, label, id)}
    >
      {label}
    </button>
  );
}

/** Copy du texte final ; « Copied » ou « Copy failed » un instant. */
function CopyTextButton({ text }: { text: string }) {
  const t = useT();
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const timer = setTimeout(() => setState("idle"), 2000);
    return () => clearTimeout(timer);
  }, [state]);
  return (
    <button
      type="button"
      className={BUTTON}
      onClick={() =>
        // Dans le clic : WebKitGTK refuse l'écriture loin du geste.
        copyText(text).then(
          () => setState("copied"),
          () => setState("failed"),
        )
      }
    >
      {state === "copied" ? (
        <CheckIcon aria-hidden="true" className="h-4 w-4" />
      ) : state === "failed" ? (
        <XMarkIcon aria-hidden="true" className="h-4 w-4" />
      ) : (
        <Square2StackIcon aria-hidden="true" className="h-4 w-4" />
      )}
      <span aria-live="polite">
        {state === "copied"
          ? t("common.actions.copied")
          : state === "failed"
            ? t("proofread.actions.copyFailed")
            : t("common.actions.copy")}
      </span>
    </button>
  );
}

/**
 * Efface le caractère avant (Retour arrière) ou après (Suppr) le curseur de
 * la zone, par l'édition native (pile d'annulation) ; sans execCommand, le
 * point de code voisin est retiré à la main.
 */
function eraseAtCaret(area: HTMLTextAreaElement, backward: boolean) {
  let done = false;
  try {
    done = document.execCommand(backward ? "delete" : "forwardDelete");
  } catch {
    // Pas d'execCommand : repli ci-dessous.
  }
  if (done) return;
  const at = area.selectionStart;
  const value = area.value;
  const head = Array.from(value.slice(0, at));
  const tail = Array.from(value.slice(at));
  if (backward) head.pop();
  else tail.shift();
  const before = head.join("");
  setDraft(before + tail.join(""));
  area.value = before + tail.join("");
  area.setSelectionRange(before.length, before.length);
}

/**
 * Ligne d'état du volet, annoncée discrètement : travail en cours (passage
 * n sur m), puis nombre de corrections, « No mistakes found » (« No
 * corrections made » si des passages ont été gardés) ou « Text rephrased ».
 * Revue partielle (arrêt, erreur, fin sans évènement final) : jamais « No
 * mistakes found » ni « Text rephrased », qui diraient le texte entier relu.
 */
function Status() {
  const t = useT();
  const { phase, run } = useProofread();
  let text: string | null = null;
  let detail: string | null = null;
  if (phase === "running" && run) {
    text =
      run.mode === "correct"
        ? t("proofread.running.correcting")
        : t("proofread.running.rephrasing");
    if (run.parts && run.parts.length > 1) {
      const done = run.results.filter((result) => result !== null).length;
      detail = t("proofread.running.progress", {
        current: Math.min(done + 1, run.parts.length),
        total: run.parts.length,
      });
    }
  } else if (phase === "review" && run?.parts) {
    const partial = !allDone(run.results);
    if (run.mode === "rephrase") {
      text = partial
        ? t("proofread.review.rephrasedPartial")
        : t("proofread.review.rephrased");
    } else {
      const count = correctionCount(
        partViews(run.mode, run.parts, run.results, null),
      );
      // Aucun changement parce que le garde-fou a gardé des passages : ce
      // n'est pas « aucune faute ».
      const kept = run.results.some((result) => result?.kept === true);
      text =
        count > 0
          ? t("proofread.review.corrections", { count })
          : kept
            ? t("proofread.review.noChange")
            : partial
              ? t("proofread.review.noMistakesPartial")
              : t("proofread.review.noMistakes");
    }
    detail = run.model;
  }
  return (
    <p
      role="status"
      data-testid="proofread-status"
      className="flex min-w-0 items-center gap-2 text-sm/6 text-zinc-600 dark:text-zinc-300"
    >
      {phase === "running" && (
        <ArrowPathIcon
          aria-hidden="true"
          className="h-4 w-4 flex-none animate-spin text-zinc-500"
        />
      )}
      {text && <span>{text}</span>}
      {detail && <span className="truncate text-zinc-500">{detail}</span>}
    </p>
  );
}

/** Remarques de la revue : passages gardés, reste laissé tel quel, sélection. */
function ReviewNotes() {
  const t = useT();
  const { run } = useProofread();
  if (!run) return null;
  const notes: { key: string; text: string; warn: boolean }[] = [];
  if (run.fromSelection) {
    notes.push({
      key: "selection",
      text: t("proofread.review.fromSelection"),
      warn: false,
    });
  }
  if (someKept(run.results)) {
    notes.push({
      key: "kept",
      text:
        run.mode === "rephrase"
          ? t("proofread.review.keptRephrase")
          : t("proofread.review.kept"),
      warn: true,
    });
  }
  // Passages sans résultat, quelle que soit la fin (arrêt, erreur, fin sans
  // évènement final, sélection) : le reste n'a pas été relu.
  if (!allDone(run.results)) {
    notes.push({
      key: "partial",
      text: run.stopped
        ? t("proofread.review.stopped")
        : t("proofread.review.partial"),
      warn: false,
    });
  }
  if (notes.length === 0) return null;
  return (
    <ul className="space-y-1 px-1 text-sm/6">
      {notes.map((note) => (
        <li
          key={note.key}
          data-testid={`proofread-note-${note.key}`}
          className={
            note.warn
              ? "flex gap-2 text-amber-700 dark:text-amber-400"
              : "text-zinc-500 dark:text-zinc-400"
          }
        >
          {note.warn && (
            <ExclamationTriangleIcon
              aria-hidden="true"
              className="mt-1 h-4 w-4 flex-none"
            />
          )}
          {note.text}
        </li>
      ))}
    </ul>
  );
}

/**
 * Page mise de côté pour montrer la sélection corrigée : « Back to your
 * text » la rétablit (la sélection est aussi dans le presse-papiers).
 */
function SavedNote() {
  const t = useT();
  return (
    <p
      data-testid="proofread-note-saved"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 px-1 text-sm/6 text-zinc-500 dark:text-zinc-400"
    >
      <span>{t("proofread.saved.note")}</span>
      <button
        type="button"
        className="rounded-md font-medium text-zinc-700 underline underline-offset-2 outline-none hover:text-zinc-950 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-zinc-300 dark:hover:text-white"
        onClick={restoreSaved}
      >
        {t("proofread.saved.restore")}
      </button>
    </p>
  );
}

/** Erreur du moteur sous le volet, avec l'action qui la règle. */
function ErrorNotice({
  error,
  onChooseModel,
}: {
  error: ProofreadError;
  onChooseModel: () => void;
}) {
  const t = useT();
  const modelProblem =
    error.code === "noModel" || error.code === "modelNotFound";
  const retry = error.code === "unreachable" || error.code === "failed";
  return (
    <div
      role="alert"
      className="rounded-lg bg-red-50 px-3 py-2 text-sm/6 break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
    >
      <p>{error.message}</p>
      {(modelProblem || retry) && (
        <p className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
          {modelProblem && (
            <>
              <button
                type="button"
                className={LINK_BUTTON}
                onClick={onChooseModel}
              >
                {t("proofread.error.chooseModel")}
              </button>
              <Link to="/hub" className={LINK_BUTTON}>
                {t("proofread.error.openHub")}
              </Link>
            </>
          )}
          {retry && (
            <button
              type="button"
              className={LINK_BUTTON}
              onClick={() => startProofread(error.mode)}
            >
              {t("proofread.error.tryAgain")}
            </button>
          )}
        </p>
      )}
    </div>
  );
}

/** Modèle de la correction (réglage `proofreadModel`). */
function ModelSelect({
  id,
  selectRef,
}: {
  id: string;
  selectRef: Ref<HTMLSelectElement>;
}) {
  const t = useT();
  return (
    <ModelSettingSelect
      id={id}
      setting="proofreadModel"
      selectRef={selectRef}
      texts={{
        label: t("proofread.model.label"),
        auto: t("proofread.model.auto"),
        notInstalled: (name) => t("proofread.model.notInstalled", { name }),
        saveError: (message) => t("proofread.model.saveError", { message }),
      }}
    />
  );
}
