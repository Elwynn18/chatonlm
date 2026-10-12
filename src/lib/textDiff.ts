// Diff de mots de la page Proofread (propre à ChatonLM), sans dépendance : le
// texte d'origine d'un passage face à sa version corrigée, en segments
// « same » (gardé), « del » (supprimé), « ins » (ajouté) et « space »
// (espace remplacée par une autre). Les segments « same », « del » et le
// `before` des « space », dans l'ordre, redonnent le texte d'origine ;
// « same », « ins » et le texte des « space », le texte corrigé.
//
// Jetons : mots (lettres, marques, chiffres), suites de blancs, puis tout
// autre graphème seul (ponctuation, emoji) : « allez » → « allé » est un mot
// remplacé, « , » ajoutée une ponctuation ajoutée. Graphèmes par
// Intl.Segmenter (WebKitGTK, Node) : un emoji avec sa teinte, un drapeau ou
// une séquence liée par ZWJ reste un seul jeton, jamais coupé entre une
// suppression et un ajout. Plus longue sous-suite commune par programmation
// dynamique, après retrait du début et de la fin communs ; les passages du
// moteur (≤ ~1 200 caractères) restent loin de la limite au-delà de laquelle
// le milieu est donné remplacé d'un bloc.
//
// Une espace remplacée par une autre espace de même longueur (U+202F de la
// typographie française → espace ordinaire, insécable → ordinaire) ne se
// voit pas : un segment « space », montré comme le texte gardé et non compté
// comme correction. Le marquer annoncerait une correction que le lecteur ne
// peut pas trouver (deux pastilles vides) ; un signe visible (« ␣ »)
// encombrerait le texte pour un changement sans effet à la lecture. Le texte
// corrigé garde bien la nouvelle espace (Copy, édition).

export type DiffKind = "same" | "del" | "ins" | "space";

export type DiffSegment =
  | { kind: "same" | "del" | "ins"; text: string }
  /** Espace remplacée : `text` dans le texte corrigé, `before` à l'origine. */
  | { kind: "space"; text: string; before: string };

/** Cellules de la table au-delà desquelles le milieu est remplacé d'un bloc. */
const MAX_CELLS = 4_000_000;

/** Graphème de mot (lettres, marques, chiffres) ou de blanc. */
const WORD = /^[\p{L}\p{M}\p{N}_]+$/u;
const BLANK = /^\s+$/u;
/** Espaces seules (séparateurs d'espace, sans saut de ligne). */
const SPACES = /^\p{Zs}+$/u;

let segmenter: Intl.Segmenter | null | undefined;

/** Graphèmes de `text` (points de code sans Intl.Segmenter). */
function graphemes(text: string): string[] {
  if (segmenter === undefined) {
    segmenter =
      typeof Intl.Segmenter === "function"
        ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
        : null;
  }
  if (!segmenter) return Array.from(text);
  return Array.from(segmenter.segment(text), (part) => part.segment);
}

/** Découpe en jetons ; leur concaténation redonne `text`. */
export function tokenize(text: string): string[] {
  const tokens: string[] = [];
  let previous: "word" | "blank" | null = null;
  for (const grapheme of graphemes(text)) {
    const kind = WORD.test(grapheme)
      ? "word"
      : BLANK.test(grapheme)
        ? "blank"
        : null;
    if (kind !== null && kind === previous)
      tokens[tokens.length - 1] += grapheme;
    else tokens.push(grapheme);
    previous = kind;
  }
  return tokens;
}

/** Ajoute un segment, fusionné avec le précédent s'il est de même nature. */
function push(out: DiffSegment[], kind: "same" | "del" | "ins", text: string) {
  if (!text) return;
  const last = out.at(-1);
  if (last?.kind === kind) last.text += text;
  else out.push({ kind, text });
}

/**
 * Groupe de changements : suppression puis ajout, ou segment « space » si
 * seule une espace a été remplacée par une autre de même longueur.
 */
function pushChange(out: DiffSegment[], deleted: string, inserted: string) {
  if (
    SPACES.test(deleted) &&
    SPACES.test(inserted) &&
    [...deleted].length === [...inserted].length
  ) {
    out.push({ kind: "space", text: inserted, before: deleted });
    return;
  }
  push(out, "del", deleted);
  push(out, "ins", inserted);
}

/**
 * Segments du passage `before` corrigé en `after`. Dans un groupe de
 * changements, la suppression vient avant l'ajout.
 */
export function diffWords(before: string, after: string): DiffSegment[] {
  const a = tokenize(before);
  const b = tokenize(after);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) {
    start += 1;
  }
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }

  const out: DiffSegment[] = [];
  push(out, "same", a.slice(0, start).join(""));
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  const n = midA.length;
  const m = midB.length;
  if ((n + 1) * (m + 1) > MAX_CELLS) {
    pushChange(out, midA.join(""), midB.join(""));
  } else {
    middle(out, midA, midB);
  }
  push(out, "same", a.slice(endA).join(""));
  return out;
}

/** Milieu par plus longue sous-suite commune (table des suffixes). */
function middle(out: DiffSegment[], a: string[], b: string[]) {
  const n = a.length;
  const m = b.length;
  const width = m + 1;
  // lcs[i * width + j] : longueur commune de a[i..] et b[j..].
  const lcs = new Uint32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i * width + j] =
        a[i] === b[j]
          ? lcs[(i + 1) * width + j + 1] + 1
          : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }
  // Groupe de changements en cours : suppressions puis ajouts.
  let deleted = "";
  let inserted = "";
  const flush = () => {
    pushChange(out, deleted, inserted);
    deleted = "";
    inserted = "";
  };
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      flush();
      push(out, "same", a[i]);
      i += 1;
      j += 1;
    } else if (
      j >= m ||
      (i < n && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
    ) {
      deleted += a[i];
      i += 1;
    } else {
      inserted += b[j];
      j += 1;
    }
  }
  flush();
}

/**
 * Nombre de corrections : groupes de segments supprimés ou ajoutés (une
 * espace remplacée par une autre ne compte pas).
 */
export function changeCount(segments: readonly DiffSegment[]): number {
  let count = 0;
  let inChange = false;
  for (const segment of segments) {
    const changed = segment.kind === "del" || segment.kind === "ins";
    if (changed && !inChange) count += 1;
    inChange = changed;
  }
  return count;
}

/** Texte d'origine (`del`) ou corrigé (`ins`) des segments. */
export function sideOf(
  segments: readonly DiffSegment[],
  side: "del" | "ins",
): string {
  return segments
    .map((segment) => {
      if (segment.kind === "space") {
        return side === "del" ? segment.before : segment.text;
      }
      return segment.kind === "same" || segment.kind === side
        ? segment.text
        : "";
    })
    .join("");
}
