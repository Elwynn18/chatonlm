// Réparation du Markdown incomplet d'une réponse en cours de flux : un gras,
// un code en ligne, un lien... ouverts mais pas encore fermés s'affichent
// comme s'ils l'étaient (« Du **gras » en gras), au lieu de montrer leurs
// marques le temps que la fin arrive.
//
// Streamdown sait le faire (parseIncompleteMarkdown), mais il répare chacun
// de ses blocs, y compris ceux déjà finis et l'intérieur d'un bloc de code
// pas encore fermé (« echo $$ » affiché « echo $$$$ », copié tel quel), et
// ses réparations restent affichées à la fin du flux. Ici, seul le dernier
// bloc est réparé, jamais un bloc de code, et seule la fin du texte change :
// les blocs précédents gardent leur texte, donc Streamdown, qui mémoïse ses
// blocs sur leur texte, ne les rend de nouveau ni pendant le flux ni à sa fin.
//
// Les règles sont celles de Streamdown (lien ou image incomplets, ***, **,
// __, *, _, `, ~~, $$), avec ses exceptions, appliquées au dernier paragraphe
// (ou titre, ou cellule de tableau) du dernier bloc. Elles suivent en plus
// les règles de CommonMark sur les délimiteurs, ce qui évite d'ajouter des
// marques que le Markdown ne lirait pas : une marque qui ne peut pas ouvrir
// (« 2 * 3 », puces « * », « Du ** ») ou qui ne fait que fermer
// (« Note* importante ») n'est pas fermée ; le code (blocs, code en ligne),
// les maths et les adresses des liens ne comptent pas ; les fermetures se
// font de la plus récente à la plus ancienne (« **gras `code » donne
// « **gras `code`** »).
import { Lexer, type Token, type Tokens } from "marked";

/**
 * Adresse d'un lien dont l'adresse n'est pas encore arrivée, celle de
 * Streamdown : react-markdown la vide, et ExternalLink rend le lien inerte.
 */
export const INCOMPLETE_LINK = "streamdown:incomplete-link";

/**
 * `text` réparé pour l'affichage pendant le flux, ou `text` lui-même s'il
 * n'y a rien à réparer. Seule la fin change : un suffixe est ajouté (les
 * espaces finaux en moins), ou un lien ou une image incomplets sont coupés.
 */
export function repairIncompleteMarkdown(text: string): string {
  const leaf = lastLeaf(text);
  if (!leaf) return text;
  const repair = planRepair(text.slice(leaf.start, leaf.end));
  return repair ? text.slice(0, leaf.start + repair.cut) + repair.suffix : text;
}

/** Réparation d'un passage : le couper à `cut`, puis ajouter `suffix`. */
interface Repair {
  cut: number;
  suffix: string;
}

// ---------------------------------------------------------------------------
// Dernier bloc : découpage du lexer de marked, comme Streamdown.

/** Bloc de premier niveau. */
interface Block {
  token: Token;
  /** Début du bloc. */
  start: number;
  /**
   * Position de token.raw dans le texte : `start`, sauf pour un bloc relu
   * depuis un point de reprise dans le bloc, dont le raw commence par
   * l'en-tête de la reprise (avant `at`).
   */
  rawStart: number;
}

/**
 * Point de reprise du découpage : le lexer relit `head` suivi du texte à
 * partir de `at`, et le premier bloc relu commence en `start`. Au début d'un
 * bloc, `head` est vide et `start` vaut `at`. Dans une liste, `at` est le
 * début de son dernier élément et `head` une puce comme la sienne suivie
 * d'un mot : marked lit cet élément comme dans la liste entière, en élément
 * qui en suit un autre (`type` : le premier bloc relu doit être une liste
 * d'au moins deux éléments, dont `head`). Dans un tableau, `at` est le début
 * de sa dernière ligne et `head` son en-tête (ses deux premières lignes) :
 * marked lit ses lignes une à une.
 */
interface Resume {
  at: number;
  head: string;
  start: number;
  type?: "list" | "table";
}

/**
 * Dernier découpage : le texte, et où reprendre. Pendant un flux, le texte
 * suivant prolonge le précédent ; seuls ses deux derniers blocs peuvent
 * changer (« 1. a\n\n2 » puis « 2. b » rejoint la liste du bloc d'avant),
 * le reste n'est pas relu à chaque fragment. Une longue liste ou un long
 * tableau ne forment qu'un bloc : ils ne sont relus qu'à partir de leur
 * dernier élément ou de leur dernière ligne quand c'est sûr (premier point
 * de reprise), sinon depuis le début de l'avant-dernier bloc (le second).
 */
let previous: { text: string; resumes: Resume[] } | null = null;

/** Dernier bloc de premier niveau (hors lignes vides) et son début. */
function lastBlock(text: string): Block | null {
  const resumes =
    previous && text.startsWith(previous.text)
      ? previous.resumes
      : [{ at: 0, head: "", start: 0 }];
  for (const resume of resumes) {
    const blocks = blocksFrom(text, resume);
    if (!blocks) continue;
    const last = blocks.at(-1);
    const beforeLast = blocks.at(-2);
    const from = beforeLast ?? last;
    // Une liste d'au moins deux éléments ou un tableau en dernier bloc ne
    // rejoignent plus le bloc d'avant : leur première ligne est finie.
    const inside =
      (last && resumeInside(text, last)) ??
      (beforeLast && resumeInside(text, beforeLast));
    previous = {
      text,
      resumes: from
        ? [
            ...(inside ? [inside] : []),
            { at: from.start, head: "", start: from.start },
          ]
        : [resume],
    };
    return last ?? null;
  }
  previous = null;
  return null;
}

/**
 * Blocs (hors lignes vides) relus à partir de `resume`, ou null : la reprise
 * dans un bloc ne tient plus (sa ligne n'est plus un élément de la liste :
 * « - » devenu « -- » ou « - - - »), ou le lexer n'a pas gardé tout le texte
 * (les positions seraient fausses).
 */
function blocksFrom(text: string, resume: Resume): Block[] | null {
  const tokens = new Lexer({ gfm: true }).blockTokens(
    resume.head + text.slice(resume.at),
  );
  const first = tokens[0];
  if (resume.type) {
    if (first?.type !== resume.type) return null;
    if (first.type === "list") {
      const items = (first as Tokens.List).items;
      if (items.length < 2 || items[0].raw !== resume.head) return null;
    }
  }
  const blocks: Block[] = [];
  let offset = resume.at - resume.head.length;
  for (const token of tokens) {
    if (token.type !== "space") {
      const start = token === first ? resume.start : offset;
      blocks.push({ token, start, rawStart: offset });
    }
    offset += token.raw.length;
  }
  return offset === text.length ? blocks : null;
}

/** Puce d'un élément de liste (retrait compris), au début de son raw. */
const BULLET = /^ {0,3}(?:[*+-]|\d{1,9}[.)])/;

/**
 * Point de reprise dans `block` : début du dernier élément d'une liste d'au
 * moins deux éléments, ou de la dernière ligne d'un tableau ; null sinon.
 */
function resumeInside(text: string, block: Block): Resume | null {
  const { token, start, rawStart } = block;
  if (token.type === "list") {
    const items = (token as Tokens.List).items;
    if (items.length < 2) return null;
    const bullet = BULLET.exec(items[items.length - 1].raw);
    if (!bullet) return null;
    let at = rawStart;
    for (const item of items.slice(0, -1)) at += item.raw.length;
    return { at, head: `${bullet[0]} x\n`, start, type: "list" };
  }
  if (token.type === "table") {
    const header = text.indexOf("\n", start);
    const delimiter = header < 0 ? -1 : text.indexOf("\n", header + 1);
    if (delimiter < 0) return null;
    const end = rawStart + token.raw.trimEnd().length;
    const at = text.lastIndexOf("\n", end - 1) + 1;
    if (at <= delimiter) return null;
    return {
      at,
      head: text.slice(start, delimiter + 1),
      start,
      type: "table",
    };
  }
  return null;
}

/** Dernier élément d'une liste de blocs, hors lignes vides. */
function lastNonSpace(tokens: Token[] | undefined): Token | undefined {
  return tokens?.findLast((token) => token.type !== "space");
}

/**
 * Passage à réparer : le dernier paragraphe (ou titre, ou cellule de tableau)
 * du dernier bloc, jusqu'au dernier caractère visible. Rien pour un bloc de
 * code, ouvert ou non, même dans une liste ou une citation, ni pour le HTML.
 */
function lastLeaf(text: string): { start: number; end: number } | null {
  // marked remplace les retours chariot : les positions ne suivraient plus.
  if (text.includes("\r")) return null;
  let end = text.length;
  while (end > 0 && /[ \t\n]/.test(text[end - 1])) end--;
  if (end === 0) return null;

  const block = lastBlock(text);
  if (!block) return null;
  let token: Token | undefined = block.token;
  while (token) {
    if (token.type === "list") {
      token = (token as Tokens.List).items.at(-1);
    } else if (token.type === "list_item" || token.type === "blockquote") {
      token = lastNonSpace(token.tokens);
    } else {
      break;
    }
  }
  if (!token) return null;

  switch (token.type) {
    case "paragraph":
    case "text":
      break;
    case "heading":
      // Titre souligné (« Titre\n--- ») : ajouter après le soulignement le
      // défairait.
      if (!/^ {0,3}#/.test(token.raw)) return null;
      break;
    case "table": {
      // Dernière cellule de la dernière ligne, si elle n'est pas fermée.
      const lineStart = text.lastIndexOf("\n", end - 1) + 1;
      const pipe = lastPipe(text, lineStart, end);
      const start = pipe < 0 ? lineStart : pipe + 1;
      return text.slice(start, end).trim() ? { start, end } : null;
    }
    default:
      return null;
  }
  if (token === block.token) return { start: block.start, end };

  // Paragraphe dans une liste ou une citation : marked le donne sans les
  // marques de ses conteneurs (« - », « > », retrait), mais avec autant de
  // lignes. Sa première ligne est la fin de la ligne d'origine.
  const lines = token.raw.trimEnd().split("\n");
  let lineStart = text.lastIndexOf("\n", end - 1) + 1;
  for (let i = 1; i < lines.length && lineStart > 0; i++) {
    lineStart = text.lastIndexOf("\n", lineStart - 2) + 1;
  }
  lineStart = Math.max(lineStart, block.start);
  const newline = text.indexOf("\n", lineStart);
  const line = text
    .slice(lineStart, newline < 0 || newline > end ? end : newline)
    .trimEnd();
  const first = lines[0].trimEnd();
  const start = line.endsWith(first)
    ? lineStart + line.length - first.length
    : lineStart;
  return { start, end };
}

/** Dernier « | » non échappé de text[from, to), ou -1. */
function lastPipe(text: string, from: number, to: number): number {
  for (let i = to - 1; i >= from; i--) {
    if (text[i] !== "|") continue;
    let slashes = 0;
    while (i - slashes - 1 >= from && text[i - slashes - 1] === "\\") slashes++;
    if (slashes % 2 === 0) return i;
  }
  return -1;
}

// ---------------------------------------------------------------------------
// Réparation d'un passage.

/** Délimiteur d'emphase encore ouvert : « * », « _ » ou « ~ » répété `len` fois. */
interface Opener {
  char: string;
  len: number;
  /** Position du délimiteur. */
  pos: number;
  /** Début de son contenu. */
  end: number;
}

type Tail =
  /** Code en ligne ou maths ouverts : la suite est leur contenu. */
  | { kind: "inline"; pos: number; closer: string }
  /** Lien ou image dont l'adresse n'est pas finie. */
  | { kind: "destination"; open: number; paren: number };

const ASCII_PUNCTUATION = /[!-/:-@[-`{-~]/;
const SPACE = /\s/;
const PUNCTUATION = /[\p{P}\p{S}]/u;
const WORD = /[\p{L}\p{N}]/u;
/** Contenu qui ne mérite pas de fermeture : vide, ou seulement des marques. */
const TRIVIAL = /^[\s_~*`]*$/;

function planRepair(r: string): Repair | null {
  const openers: Opener[] = [];
  /** Crochets ouverts, du plus ancien au plus récent. */
  const brackets: number[] = [];
  let inDollars = false;
  let tail: Tail | null = null;
  let i = 0;

  scan: while (i < r.length) {
    switch (r[i]) {
      case "\\":
        i += ASCII_PUNCTUATION.test(r[i + 1] ?? "") ? 2 : 1;
        continue;
      case "`": {
        const n = runLength(r, i);
        const close = findBackticks(r, n, i + n);
        if (close >= 0) {
          i = close + n;
          continue;
        }
        if (n >= 3 && opensFence(r, i)) return null;
        const closer = codeCloser(r, i, n);
        if (closer) {
          tail = { kind: "inline", pos: i, closer };
          break scan;
        }
        i += n;
        continue;
      }
      case "$": {
        const n = runLength(r, i);
        if (n === 1) {
          // « $ » seul n'ouvre pas de maths ($5 et $10) ; seul « _ » en tient
          // compte, comme Streamdown ($x_1$).
          inDollars = !inDollars;
          i++;
          continue;
        }
        const close = r.indexOf("$$", i + n);
        if (close >= 0) {
          i = close + runLength(r, close);
          continue;
        }
        if (r.slice(i + n).trim()) {
          const closer = r.includes("\n", i + n) ? "\n$$" : "$$";
          tail = { kind: "inline", pos: i, closer };
          break scan;
        }
        i += n;
        continue;
      }
      case "[":
        brackets.push(i);
        i++;
        continue;
      case "]": {
        const open = brackets.pop();
        if (open !== undefined && r[i + 1] === "(") {
          const close = closingParen(r, i + 2);
          if (close < 0) {
            tail = { kind: "destination", open, paren: i + 1 };
            break scan;
          }
          // Lien complet : une emphase ouverte dans son texte ne se ferme
          // plus après lui.
          dropFrom(openers, open);
          i = close + 1;
          continue;
        }
        i++;
        continue;
      }
      case "*":
      case "_":
      case "~": {
        const n = runLength(r, i);
        if (r[i] === "~" && n >= 3 && opensFence(r, i)) return null;
        if (!(r[i] === "_" && inDollars)) delimiterRun(r, i, n, openers);
        i += n;
        continue;
      }
      default:
        i++;
    }
  }

  if (tail?.kind === "destination") {
    if (isImage(r, tail.open)) return cutImage(r, tail.open - 1);
    // Le texte du lien reste ; son adresse devient le marqueur.
    const before = openers.filter((o) => o.pos < tail.open);
    return {
      cut: tail.paren,
      suffix: `(${INCOMPLETE_LINK})${closers(r, before, r.length, true)}`,
    };
  }

  const inner = tail?.closer ?? "";
  const limit = tail?.pos ?? r.length;
  const image = brackets.find((b) => isImage(r, b));
  if (image !== undefined) return cutImage(r, image - 1);
  // Crochet ouvert : c'est le plus récent que fermerait un « ] ».
  const bracket = brackets.at(-1);
  if (bracket !== undefined) {
    const inside = openers.filter((o) => o.pos > bracket);
    const before = openers.filter((o) => o.pos < bracket);
    return {
      cut: r.length,
      suffix:
        inner +
        closers(r, inside, limit, inner !== "") +
        `](${INCOMPLETE_LINK})` +
        closers(r, before, r.length, true),
    };
  }
  const suffix = inner + closers(r, openers, limit, inner !== "");
  return suffix ? { cut: r.length, suffix } : null;
}

/**
 * La suite de « ` » ou de « ~ » (au moins trois) en `i` ouvre-t-elle un bloc
 * de code tapé juste après un paragraphe ? Elle commence la dernière ligne
 * (après des espaces, ou les marques « > » et le retrait des conteneurs),
 * sans autre « ` » derrière elle pour une suite de « ` » : marked n'en fera
 * un bloc de code qu'au saut de ligne suivant, et le paragraphe d'avant
 * deviendra alors un bloc fini, jamais réparé. D'ici là, rien n'est réparé
 * (« Text:\n```python » n'est pas un code en ligne `python`).
 */
function opensFence(r: string, i: number): boolean {
  if (r.includes("\n", i)) return false;
  if (r[i] === "`" && r.includes("`", i + runLength(r, i))) return false;
  const lineStart = r.lastIndexOf("\n", i - 1) + 1;
  return /^[ \t>]*$/.test(r.slice(lineStart, i));
}

/** Longueur de la suite du même caractère qui commence en `i`. */
function runLength(r: string, i: number): number {
  let n = 1;
  while (r[i + n] === r[i]) n++;
  return n;
}

/** Début de la prochaine suite d'exactement `n` « ` » à partir de `from`, ou -1. */
function findBackticks(r: string, n: number, from: number): number {
  for (let j = r.indexOf("`", from); j >= 0;) {
    const m = runLength(r, j);
    if (m === n) return j;
    j = r.indexOf("`", j + m);
  }
  return -1;
}

/**
 * Fermeture d'un code en ligne ouvert par `n` « ` » en `i`, ou "" : rien à
 * fermer (contenu vide ou fait de marques), ou un contenu qui finit déjà par
 * plus de « ` » que l'ouverture. S'il en finit par moins, ils sont complétés.
 */
function codeCloser(r: string, i: number, n: number): string {
  if (TRIVIAL.test(r.slice(i + n))) return "";
  let trailing = 0;
  while (r[r.length - 1 - trailing] === "`") trailing++;
  return trailing < n ? "`".repeat(n - trailing) : "";
}

/** Parenthèse qui ferme l'adresse d'un lien commencée en `from`, ou -1. */
function closingParen(r: string, from: number): number {
  let depth = 1;
  for (let j = from; j < r.length; j++) {
    if (r[j] === "\\") j++;
    else if (r[j] === "(") depth++;
    else if (r[j] === ")" && --depth === 0) return j;
  }
  return -1;
}

/** Retire les délimiteurs ouverts à partir de la position `pos`. */
function dropFrom(openers: Opener[], pos: number) {
  const k = openers.findIndex((o) => o.pos > pos);
  if (k >= 0) openers.length = k;
}

/** Le crochet en `b` ouvre-t-il une image (« ![ », « ! » non échappé) ? */
function isImage(r: string, b: number): boolean {
  return r[b - 1] === "!" && r[b - 2] !== "\\";
}

/** Coupe une image incomplète (et les espaces avant), puis répare le reste. */
function cutImage(r: string, at: number): Repair {
  const kept = r.slice(0, at).trimEnd();
  return planRepair(kept) ?? { cut: kept.length, suffix: "" };
}

/**
 * Suite de délimiteurs d'emphase de longueur `n` en `i` : elle ferme les
 * délimiteurs ouverts du même caractère, puis ouvre avec ce qui lui reste,
 * selon les règles de CommonMark (délimiteurs « flanquants »).
 */
function delimiterRun(r: string, i: number, n: number, openers: Opener[]) {
  const char = r[i];
  // « ~ » seul (« ~5 min ») ne barre pas ici, comme chez Streamdown ; « ~~~ »
  // ne barre jamais.
  if (char === "~" && n !== 2) return;
  const before = i > 0 ? r[i - 1] : " ";
  const after = i + n < r.length ? r[i + n] : " ";
  const left =
    !SPACE.test(after) &&
    (!PUNCTUATION.test(after) ||
      SPACE.test(before) ||
      PUNCTUATION.test(before));
  const right =
    !SPACE.test(before) &&
    (!PUNCTUATION.test(before) || SPACE.test(after) || PUNCTUATION.test(after));
  const inWord = WORD.test(before) && WORD.test(after);
  // « _ » au milieu d'un mot (snake_case, __init__) n'ouvre ni ne ferme ;
  // « * » au milieu d'un mot (« 5*3 ») est plus souvent un calcul qu'une
  // emphase : il ne fait que fermer.
  const canOpen =
    char === "_"
      ? left && (!right || PUNCTUATION.test(before))
      : left && !inWord;
  const canClose =
    char === "_" ? right && (!left || PUNCTUATION.test(after)) : right;

  let rest = n;
  if (canClose) {
    for (
      let k = openers.findLastIndex((o) => o.char === char);
      rest > 0 && k >= 0;
      k = openers.findLastIndex((o) => o.char === char)
    ) {
      const opener = openers[k];
      if (char === "~" && opener.len !== rest) break;
      // Les délimiteurs ouverts entre les deux restent du texte.
      openers.length = k + 1;
      if (rest >= opener.len) {
        rest -= opener.len;
        openers.pop();
      } else {
        opener.len -= rest;
        rest = 0;
      }
    }
  }
  if (rest > 0 && canOpen) {
    openers.push({ char, len: rest, pos: i + n - rest, end: i + n });
  }
}

/**
 * Fermetures des délimiteurs encore ouverts, du plus récent au plus ancien,
 * ou "" quand le plus récent n'a pas encore de contenu (`r` jusqu'à `limit`,
 * plus un code, des maths ou un lien quand `more` est vrai). Un même
 * délimiteur ouvert deux fois (« *.txt et *.md ») est pris pour du texte.
 */
function closers(r: string, openers: Opener[], limit: number, more: boolean) {
  const count = new Map<string, number>();
  for (const o of openers) {
    const key = o.char.repeat(o.len);
    count.set(key, (count.get(key) ?? 0) + 1);
  }
  const open = openers.filter((o) => count.get(o.char.repeat(o.len)) === 1);
  const last = open.at(-1);
  if (!last || (!more && TRIVIAL.test(r.slice(last.end, limit)))) return "";
  return open
    .map((o) => o.char.repeat(o.len))
    .reverse()
    .join("");
}
