// Page Proofread (propre à ChatonLM) : fonctions pures sur les passages d'une
// correction (texte final, segments montrés, nombre de corrections), testées
// à part. Le découpage vient du moteur (évènement `started`) : les `before`
// + `text` + `after` de tous les passages redonnent le texte envoyé. Le
// frontend ne compare jamais un texte du moteur à un texte attendu : il se
// fie aux codes et aux champs (`kept`).

import type { PartResult } from "@/bindings/PartResult";
import type { ProofreadMode } from "@/bindings/ProofreadMode";
import type { ProofreadPart } from "@/bindings/ProofreadPart";
import { changeCount, type DiffSegment, diffWords } from "@/lib/textDiff";

/**
 * Texte reconstitué : résultat des passages terminés, texte d'origine des
 * autres, séparateurs d'origine (comme `assemble` du moteur).
 */
export function assembleText(
  parts: readonly ProofreadPart[],
  results: readonly (PartResult | null | undefined)[],
): string {
  return parts
    .map(
      (part, index) =>
        part.before + (results[index]?.text ?? part.text) + part.after,
    )
    .join("");
}

/** Passage tel que la page le montre. */
export type PartView =
  /** Terminé : segments du diff (Correct) ou texte nouveau seul (Rephrase). */
  | {
      state: "done";
      before: string;
      after: string;
      segments: DiffSegment[];
      kept: boolean;
    }
  /** En cours : fragments reçus (rien encore : texte d'origine atténué). */
  | { state: "streaming"; before: string; after: string; text: string }
  /** À venir : texte d'origine atténué. */
  | { state: "pending"; before: string; after: string; text: string };

/**
 * Diff d'un passage terminé, par résultat (objet gardé tel quel d'un
 * évènement à l'autre) : le flux du passage suivant ne recalcule pas ceux
 * des passages déjà terminés.
 */
const diffs = new WeakMap<PartResult, DiffSegment[]>();

function segmentsOf(part: ProofreadPart, result: PartResult): DiffSegment[] {
  let segments = diffs.get(result);
  if (!segments) {
    segments = diffWords(part.text, result.text);
    diffs.set(result, segments);
  }
  return segments;
}

/**
 * Passages à montrer. Correct : mots supprimés et ajoutés marqués ; Rephrase :
 * texte nouveau sans marques (un seul segment « same »).
 */
export function partViews(
  mode: ProofreadMode,
  parts: readonly ProofreadPart[],
  results: readonly (PartResult | null)[],
  streaming: { index: number; text: string } | null,
): PartView[] {
  return parts.map((part, index) => {
    const result = results[index];
    const { before, after } = part;
    if (result) {
      const segments =
        mode === "correct" && !result.kept
          ? segmentsOf(part, result)
          : [{ kind: "same" as const, text: result.text }];
      return { state: "done", before, after, segments, kept: result.kept };
    }
    if (streaming?.index === index && streaming.text) {
      return { state: "streaming", before, after, text: streaming.text };
    }
    return { state: "pending", before, after, text: part.text };
  });
}

/** Corrections des passages terminés (groupes de mots changés). */
export function correctionCount(views: readonly PartView[]): number {
  return views.reduce(
    (count, view) =>
      view.state === "done" ? count + changeCount(view.segments) : count,
    0,
  );
}

/** Un passage terminé a-t-il été gardé tel quel par le garde-fou ? */
export function someKept(results: readonly (PartResult | null)[]): boolean {
  return results.some((result) => result?.kept === true);
}

/** Tous les passages sont-ils terminés ? */
export function allDone(results: readonly (PartResult | null)[]): boolean {
  return results.every((result) => result !== null);
}

/**
 * Texte final d'une plage du conteneur (copie d'une sélection) : sans les
 * éléments `data-skip` (mots supprimés, libellés des lecteurs d'écran). Une
 * plage tout entière dans un mot supprimé ne donne rien.
 */
export function finalTextOf(range: Range): string {
  const common = range.commonAncestorContainer;
  const element = common instanceof Element ? common : common.parentElement;
  if (element?.closest("[data-skip]")) return "";
  const fragment = range.cloneContents();
  for (const skipped of fragment.querySelectorAll("[data-skip]")) {
    skipped.remove();
  }
  return fragment.textContent;
}

/**
 * Position dans le texte final d'un point du conteneur (sélection repliée
 * après un clic) : longueur du texte qui le précède, sans les éléments
 * `data-skip`. Un point dans un mot supprimé donne le début de ce mot.
 */
export function finalOffset(root: Node, node: Node, offset: number): number {
  const range = document.createRange();
  range.setStart(root, 0);
  range.setEnd(node, offset);
  return finalTextOf(range).length;
}
