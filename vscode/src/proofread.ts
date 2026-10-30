// Correction des textes (lot 4a) : commentaires d'un fichier de code
// (« Proofread comments ») ou document Markdown/texte (« Proofread this
// document »), par la correction de l'app (`proofread.run`, passage par
// passage). Jamais le code : les commentaires sont extraits par langage
// (chaînes, expressions régulières, gabarits et chaînes brutes respectés), un
// commentaire douteux ne part pas, leurs marques restent, et chaque passage
// corrigé est écarté s'il changerait autre chose qu'un commentaire (code relu
// après coup par l'analyseur ET par une lecture indépendante) ; en Markdown, les blocs de code ne partent pas
// et un passage qui toucherait au code en ligne, aux liens ou à la structure
// garde son texte d'origine. Rien n'est écrit sans accord : vue de
// différences de VS Code, puis « Apply ». Un passage qui porte un secret
// (même filtre que le reçu) ne part pas. Contrat : .tools/research/93-lot4a-contract.md (4).
import * as vscode from "vscode";
import { type Notification, RpcError } from "./bridge/rpc";
import type { LinkLike } from "./controller";
import { holdsSecret, isSecretFile } from "./receipt";

export type Syntax = {
  line: string[];
  block: [string, string][];
  /** Guillemets qui ouvrent une chaîne. */
  strings: string[];
  /** Chaînes `"""…"""` et `'''…'''` (Python). */
  triple?: boolean;
  /** `'` n'ouvre qu'un caractère `'x'` (Rust : sinon durée de vie). */
  charQuote?: boolean;
  /** `#` ne commente qu'en début de ligne ou après une espace (shell, YAML). */
  hashAfterSpace?: boolean;
  /** JavaScript, TypeScript : littéraux d'expression régulière `/…/` et
   * gabarits `` `…${…}…` `` (rapport 94, S4). */
  js?: boolean;
  /** Rust : chaînes brutes `r#"…"#`, `br"…"`. */
  rawStrings?: boolean;
  /** C# : chaînes `@"…"` (barre oblique inverse littérale, `""` pour `"`). */
  verbatim?: boolean;
  /** Go : `` `…` `` sans échappement. */
  rawBacktick?: boolean;
  /** Lua : commentaires `--[==[ … ]==]` et chaînes `[[…]]` à niveaux. */
  longBrackets?: boolean;
};

const C_LIKE: Syntax = {
  line: ["//"],
  block: [["/*", "*/"]],
  strings: ['"', "'", "`"],
};
const HASH: Syntax = { line: ["#"], block: [], strings: ['"', "'"] };

/** Syntaxe des commentaires d'un langage, ou null (inconnu). */
export function syntaxFor(languageId: string): Syntax | null {
  switch (languageId) {
    case "typescript":
    case "typescriptreact":
    case "javascript":
    case "javascriptreact":
      return { ...C_LIKE, js: true };
    case "go":
      return { ...C_LIKE, rawBacktick: true };
    case "csharp":
      return { ...C_LIKE, verbatim: true };
    case "java":
    case "c":
    case "cpp":
    case "swift":
    case "kotlin":
    case "scala":
    case "dart":
    case "groovy":
    case "objective-c":
    case "jsonc":
    case "json5":
      return C_LIKE;
    case "rust":
      return { ...C_LIKE, strings: ['"'], charQuote: true, rawStrings: true };
    case "php":
      return { ...C_LIKE, line: ["//", "#"] };
    case "css":
      return { line: [], block: [["/*", "*/"]], strings: ['"', "'"] };
    case "scss":
    case "less":
      return { line: ["//"], block: [["/*", "*/"]], strings: ['"', "'"] };
    case "python":
      return { ...HASH, triple: true };
    case "ruby":
    case "perl":
    case "r":
    case "elixir":
    case "julia":
    case "coffeescript":
      return HASH;
    case "shellscript":
    case "yaml":
    case "toml":
    case "dockerfile":
    case "makefile":
    case "properties":
      return { ...HASH, hashAfterSpace: true };
    case "powershell":
      return { ...HASH, block: [["<#", "#>"]], hashAfterSpace: true };
    case "ini":
      return { line: [";", "#"], block: [], strings: [] };
    case "sql":
      return { line: ["--"], block: [["/*", "*/"]], strings: ["'", '"'] };
    case "lua":
      return {
        line: ["--"],
        block: [],
        strings: ["'", '"'],
        longBrackets: true,
      };
    case "haskell":
      return { line: ["--"], block: [["{-", "-}"]], strings: ['"'] };
    case "html":
    case "xml":
    case "vue":
    case "svelte":
      return { line: [], block: [["<!--", "-->"]], strings: [] };
    case "clojure":
    case "lisp":
    case "scheme":
      return { line: [";"], block: [], strings: ['"'] };
    default:
      return null;
  }
}

export type Comment = {
  kind: "line" | "block";
  start: number;
  end: number;
  open: string;
  close: string;
  /** Bloc fermé par sa marque (faux : il court jusqu'à la fin du texte). */
  closed: boolean;
};

/** Mots après lesquels `/` ouvre une expression régulière (JS). */
const REGEX_KEYWORDS = new Set([
  "return",
  "typeof",
  "case",
  "in",
  "of",
  "delete",
  "void",
  "throw",
  "new",
  "instanceof",
  "yield",
  "await",
  "do",
  "else",
]);

/** Un `/` à `i` peut-il ouvrir une expression régulière (et non diviser) ? */
function regexAllowed(text: string, i: number): boolean {
  let j = i - 1;
  while (j >= 0 && /\s/.test(text[j])) j--;
  if (j < 0) return true;
  const c = text[j];
  if ("(,=:[!&|?{};+-*%<>~^".includes(c)) return true;
  if (!/[\w$]/.test(c)) return false;
  let k = j;
  while (k >= 0 && /[\w$]/.test(text[k])) k--;
  return REGEX_KEYWORDS.has(text.slice(k + 1, j + 1));
}

/** Fin d'une expression régulière ouverte à `i` (drapeaux compris), ou
 * null si ce n'en est pas une (pas de `/` final sur la ligne). */
function regexEnd(text: string, i: number): number | null {
  let j = i + 1;
  let inClass = false;
  while (j < text.length) {
    const c = text[j];
    if (c === "\n" || c === "\r") return null;
    if (c === "\\") {
      j += 2;
      continue;
    }
    if (inClass) {
      if (c === "]") inClass = false;
    } else if (c === "[") {
      inClass = true;
    } else if (c === "/") {
      j++;
      while (j < text.length && /[A-Za-z]/.test(text[j])) j++;
      return j;
    }
    j++;
  }
  return null;
}

/** Commentaires du texte, dans l'ordre (chaînes, expressions régulières et
 * gabarits sautés ; les commentaires d'une expression `${…}` de gabarit ne
 * sont pas gardés). */
export function scanComments(text: string, syntax: Syntax): Comment[] {
  const comments: Comment[] = [];
  const at = (i: number, s: string) => text.startsWith(s, i);
  /** Gabarit ouvert juste avant `from` : rend la fin (après le `` ` ``). */
  const template = (from: number): number => {
    let j = from;
    while (j < text.length) {
      const c = text[j];
      if (c === "\\") {
        j += 2;
        continue;
      }
      if (c === "`") return j + 1;
      if (c === "$" && text[j + 1] === "{") {
        j = code(j + 2, true);
        continue;
      }
      j++;
    }
    return text.length;
  };
  /** Code à partir de `from`, jusqu'à la fin, ou jusqu'à l'accolade qui
   * ferme une expression de gabarit (`nested`) : rend où il s'arrête. */
  const code = (from: number, nested: boolean): number => {
    let i = from;
    let depth = 0;
    outer: while (i < text.length) {
      const c = text[i];
      if (nested) {
        if (c === "{") depth++;
        else if (c === "}") {
          if (depth === 0) return i + 1;
          depth--;
        }
      }
      if (syntax.triple && (at(i, '"""') || at(i, "'''"))) {
        const quote = text.slice(i, i + 3);
        const close = text.indexOf(quote, i + 3);
        i = close === -1 ? text.length : close + 3;
        continue;
      }
      if (syntax.longBrackets) {
        // Lua : --[==[ commentaire ]==], [==[ chaîne ]==].
        const long = /^(--)?\[(=*)\[/.exec(text.slice(i, i + 64));
        if (long) {
          const close = `]${long[2]}]`;
          const found = text.indexOf(close, i + long[0].length);
          const end = found === -1 ? text.length : found + close.length;
          if (long[1] && !nested) {
            comments.push({
              kind: "block",
              start: i,
              end,
              open: long[0],
              close,
              closed: found !== -1,
            });
          }
          i = end;
          continue;
        }
      }
      for (const [open, close] of syntax.block) {
        if (at(i, open)) {
          const found = text.indexOf(close, i + open.length);
          const end = found === -1 ? text.length : found + close.length;
          if (!nested) {
            comments.push({
              kind: "block",
              start: i,
              end,
              open,
              close,
              closed: found !== -1,
            });
          }
          i = end;
          continue outer;
        }
      }
      for (const marker of syntax.line) {
        if (!at(i, marker)) continue;
        if (
          marker === "#" &&
          syntax.hashAfterSpace &&
          i > 0 &&
          !/\s/.test(text[i - 1])
        ) {
          continue;
        }
        // « http:// » d'une adresse n'est pas un commentaire.
        if (marker === "//" && text[i - 1] === ":") continue;
        let end = text.indexOf("\n", i);
        if (end === -1) end = text.length;
        if (text[end - 1] === "\r") end--;
        if (nested) {
          i = end;
          continue outer;
        }
        comments.push({
          kind: "line",
          start: i,
          end,
          open: marker,
          close: "",
          closed: true,
        });
        i = end;
        continue outer;
      }
      if (syntax.rawStrings && (c === "r" || c === "b")) {
        // Rust : r"…", r#"…"#, br"…" (pas au milieu d'un nom).
        const raw = /^b?r(#*)"/.exec(text.slice(i, i + 260));
        if (raw && (i === 0 || !/[\w$]/.test(text[i - 1]))) {
          const close = `"${raw[1]}`;
          const found = text.indexOf(close, i + raw[0].length);
          i = found === -1 ? text.length : found + close.length;
          continue;
        }
      }
      if (syntax.verbatim && c === "@" && text[i + 1] === '"') {
        // C# : @"C:\dir\" (« "" » pour un guillemet).
        let j = i + 2;
        while (j < text.length) {
          if (text[j] === '"') {
            if (text[j + 1] === '"') {
              j += 2;
              continue;
            }
            break;
          }
          j++;
        }
        i = j + 1;
        continue;
      }
      if (syntax.charQuote && c === "'") {
        const char = /^'(?:\\.|[^\\'\n])'/.exec(text.slice(i, i + 12));
        i += char ? char[0].length : 1;
        continue;
      }
      if (syntax.js && c === "`") {
        i = template(i + 1);
        continue;
      }
      if (syntax.js && c === "/" && regexAllowed(text, i)) {
        const end = regexEnd(text, i);
        if (end !== null) {
          i = end;
          continue;
        }
      }
      if (syntax.rawBacktick && c === "`") {
        const close = text.indexOf("`", i + 1);
        i = close === -1 ? text.length : close + 1;
        continue;
      }
      if (syntax.strings.includes(c)) {
        let j = i + 1;
        while (j < text.length && text[j] !== c) {
          if (text[j] === "\\") j++;
          // Une chaîne simple ne passe pas la ligne (apostrophe d'un texte).
          else if (text[j] === "\n" && c !== "`") break;
          j++;
        }
        i = j + 1;
        continue;
      }
      i++;
    }
    return i;
  };
  code(0, false);
  return comments;
}

/**
 * Commentaire assez sûr pour partir (rapport 94, S4) : bloc fermé, marque au
 * début de sa ligne ou après une espace (après `>` pour `<!--`), et pas de
 * marque d'ouverture du même bloc à l'intérieur (imbrication, littéral mal
 * reconnu). En cas de doute, le passage ne part pas.
 */
export function trustedComment(text: string, comment: Comment): boolean {
  if (!comment.closed) return false;
  const before = comment.start > 0 ? text[comment.start - 1] : "\n";
  if (!/\s/.test(before) && !(before === ">" && comment.open.startsWith("<"))) {
    return false;
  }
  if (comment.kind === "block") {
    const inner = text.slice(
      comment.start + comment.open.length,
      comment.end - comment.close.length,
    );
    if (inner.includes(comment.open)) return false;
  }
  return true;
}

/** Lignes de code (commentaires retirés, lignes vides ôtées, fins de ligne
 * sans espaces) : ce qu'une correction ne doit jamais changer. */
export function codeLines(text: string, syntax: Syntax): string[] {
  let code = "";
  let last = 0;
  for (const comment of scanComments(text, syntax)) {
    code += text.slice(last, comment.start);
    last = comment.end;
  }
  code += text.slice(last);
  return code
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.length > 0);
}

/** Ligne d'un passage : marque (avec l'indentation), texte, fin (espaces,
 * `\r`). */
type Line = { prefix: string; body: string; trail: string };

/** Passage de commentaire à corriger : zone du texte et ses lignes. */
export type Region = {
  start: number;
  end: number;
  lines: Line[];
  /** Lignes à corriger (les autres restent : `/**` seul, ` *\/`). */
  first: number;
  last: number;
  /** Marque d'une ligne ajoutée ; null : pas de ligne ajoutée possible
   * (commentaire en fin de ligne de code, ou d'une seule ligne). */
  continuation: string | null;
};

const LINE_PARTS = /^(.*?)(\s*\r?)$/s;

function splitTrail(text: string): { body: string; trail: string } {
  const match = LINE_PARTS.exec(text);
  return { body: match?.[1] ?? text, trail: match?.[2] ?? "" };
}

/** Début de la ligne qui contient `offset`. */
function lineStart(text: string, offset: number): number {
  return text.lastIndexOf("\n", offset - 1) + 1;
}

/**
 * Passages de commentaires : commentaires de ligne consécutifs (même marque,
 * même indentation, seuls sur leur ligne) réunis ; un commentaire en fin de
 * ligne de code reste seul ; un commentaire de bloc est un passage. Limités
 * à `[from, to)` s'ils sont donnés (sélection).
 */
export function commentRegions(
  text: string,
  syntax: Syntax,
  from = 0,
  to = text.length,
): Region[] {
  const comments = scanComments(text, syntax).filter(
    (c) => c.end > from && c.start < to && trustedComment(text, c),
  );
  const regions: Region[] = [];
  let k = 0;
  while (k < comments.length) {
    const comment = comments[k];
    const start = lineStart(text, comment.start);
    const indent = text.slice(start, comment.start);
    const alone = /^[ \t]*$/.test(indent);
    if (comment.kind === "line") {
      const group = [comment];
      while (alone && k + group.length < comments.length) {
        const next = comments[k + group.length];
        const prev = group[group.length - 1];
        const between = text.slice(prev.end, next.start);
        if (
          next.kind !== "line" ||
          next.open !== comment.open ||
          !new RegExp(`^\\r?\\n${indent}$`).test(between)
        ) {
          break;
        }
        group.push(next);
      }
      k += group.length;
      const lines: Line[] = group.map((c, index) => {
        const raw = text.slice(c.start + c.open.length, c.end);
        const space = /^[ \t]?/.exec(raw)?.[0] ?? "";
        const { body, trail } = splitTrail(raw.slice(space.length));
        const end =
          index < group.length - 1
            ? text.slice(c.end, group[index + 1].start)
            : "";
        return {
          prefix: c.open + space,
          body,
          trail: trail + end,
        };
      });
      regions.push({
        start: comment.start,
        end: group[group.length - 1].end,
        lines,
        first: 0,
        last: lines.length - 1,
        continuation: alone ? `${indent}${comment.open} ` : null,
      });
      continue;
    }
    k++;
    // Bloc : marque d'ouverture (et « * » de JSDoc), lignes intérieures,
    // marque de fermeture.
    const inner = text.slice(
      comment.start + comment.open.length,
      comment.end - comment.close.length,
    );
    const parts = inner.split("\n");
    const lines: Line[] = parts.map((part, index) => {
      const eol = index < parts.length - 1 ? "\n" : "";
      // Première ligne : après la marque (« * » de « /** » compris) ;
      // suivantes : indentation et « * » de début de ligne.
      const lead =
        (index === 0
          ? /^\*?[ \t]*/.exec(part)?.[0]
          : /^[ \t]*(\*[ \t]?)?/.exec(part)?.[0]) ?? "";
      const { body, trail } = splitTrail(part.slice(lead.length));
      return {
        prefix: (index === 0 ? comment.open : "") + lead,
        body,
        trail: trail + eol,
      };
    });
    lines[lines.length - 1].trail += comment.close;
    let first = lines.findIndex((l) => l.body.trim() !== "");
    let last = lines.length - 1;
    while (last >= 0 && lines[last].body.trim() === "") last--;
    if (first === -1) {
      first = 0;
      last = -1;
    }
    const middle = lines.slice(1).find((l) => l.body.trim() !== "");
    regions.push({
      start: comment.start,
      end: comment.end,
      lines,
      first,
      last,
      continuation: middle ? middle.prefix : null,
    });
  }
  return regions;
}

/** Texte d'origine d'un passage (exactement la zone). */
export function regionText(region: Region): string {
  return region.lines.map((l) => l.prefix + l.body + l.trail).join("");
}

/** Texte envoyé à la correction : les lignes à corriger, sans marques. */
export function regionBody(region: Region): string {
  return region.lines
    .slice(region.first, region.last + 1)
    .map((l) => l.body)
    .join("\n");
}

const DIRECTIVE =
  /^\s*(eslint|prettier-ignore|@ts-|tslint:|istanbul |c8 |jshint|global |noqa|type:\s*ignore|pylint:|rubocop:|#?region\b|#?endregion\b|nolint|NOLINT|clang-format|SPDX-|@license|@preserve|-\*-|vim:|!|\/\s*<reference|pragma\b|biome-ignore|deno-lint|cspell:)/i;
const CODE_LINE =
  /[;{}]\s*$|^\s*(import|export|return|if|for|while|const|let|var|def|fn|func|class|public|private|#include)\b.*[({=:;]/;

/** Passage à laisser : sans lettres, directive d'outil, code commenté,
 * secret. */
export function skipRegion(region: Region): boolean {
  const body = regionBody(region);
  if (!/\p{L}{2}/u.test(body)) return true;
  const lines = body.split("\n").filter((l) => l.trim());
  if (DIRECTIVE.test(lines[0] ?? "")) return true;
  const code = lines.filter((l) => CODE_LINE.test(l)).length;
  if (code * 2 >= lines.length) return true;
  // Même filtre que le reçu : ligne par ligne, clés privées comprises.
  return holdsSecret(body);
}

/**
 * Passage reconstruit avec le texte corrigé : chaque ligne garde sa marque ;
 * des lignes en plus prennent la marque de continuation (ou sont réunies en
 * une seule si aucune n'est possible) ; des lignes en moins sont ôtées.
 */
export function rebuildRegion(region: Region, corrected: string): string {
  let parts = corrected.replace(/\r\n/g, "\n").replace(/\n+$/, "").split("\n");
  const count = region.last - region.first + 1;
  if (parts.length > count && region.continuation === null) {
    parts = [
      ...parts.slice(0, count - 1),
      parts
        .slice(count - 1)
        .join(" ")
        .replace(/\s+/g, " "),
    ];
  }
  const before = region.lines.slice(0, region.first);
  const after = region.lines.slice(region.last + 1);
  const used = region.lines.slice(region.first, region.last + 1);
  const out: string[] = before.map((l) => l.prefix + l.body + l.trail);
  const eol = used[0]?.trail.includes("\r") ? "\r\n" : "\n";
  parts.forEach((body, index) => {
    const isLast = index === parts.length - 1;
    const original = used[Math.min(index, used.length - 1)];
    const prefix =
      index < used.length ? used[index].prefix : (region.continuation ?? "");
    // Fin de ligne d'origine : celle de la dernière ligne pour la dernière
    // (fermeture du bloc), sinon celle de la ligne (retour à la ligne et, dans
    // un groupe de commentaires de ligne, indentation de la suivante) ; une
    // ligne ajoutée finit par un retour à la ligne, la continuation apporte
    // l'indentation et la marque.
    let trail: string;
    if (isLast) trail = used[used.length - 1].trail;
    else if (index < used.length - 1) trail = original.trail;
    else trail = eol;
    // Une ligne vide garde sa marque, sans espace au bout.
    const line =
      body.trim() === "" ? prefix.trimEnd() : prefix + body.trimEnd();
    out.push(line + trail);
  });
  out.push(...after.map((l) => l.prefix + l.body + l.trail));
  return out.join("");
}

export type Edit = { start: number; end: number; text: string };

/** Texte avec les remplacements (dans l'ordre, sans chevauchement). */
export function applyEdits(text: string, edits: Edit[]): string {
  let out = "";
  let last = 0;
  for (const edit of [...edits].sort((a, b) => a.start - b.start)) {
    out += text.slice(last, edit.start) + edit.text;
    last = edit.end;
  }
  return out + text.slice(last);
}

/**
 * Code d'un texte vu SANS l'analyseur des commentaires (rapport 94, S4) :
 * ligne par ligne, une marque n'est reconnue qu'au début d'une ligne ou après
 * une espace (après `>` pour `<!--`), chaînes et littéraux ignorés. Grossier
 * mais indépendant : une ligne de code que `scanComments` prendrait pour un
 * commentaire (littéral qu'il ne connaît pas) reste ici du code, et son
 * changement se voit ; ses erreurs propres sont les mêmes avant et après.
 */
export function plainCode(text: string, syntax: Syntax): string[] {
  const out: string[] = [];
  /** Marque qui ferme le bloc en cours. */
  let inBlock: string | null = null;
  for (const line of text.split(/\r?\n/)) {
    let code = "";
    let i = 0;
    scan: while (i <= line.length) {
      if (inBlock !== null) {
        const close = line.indexOf(inBlock, i);
        if (close === -1) break;
        i = close + inBlock.length;
        inBlock = null;
        continue;
      }
      for (let j = i; j < line.length; j++) {
        const before = j === 0 ? " " : line[j - 1];
        const spaced = /\s/.test(before);
        const long = syntax.longBrackets
          ? /^--\[(=*)\[/.exec(line.slice(j, j + 64))
          : null;
        if (long && spaced) {
          code += line.slice(i, j);
          i = j + long[0].length;
          inBlock = `]${long[1]}]`;
          continue scan;
        }
        for (const [open, close] of syntax.block) {
          if (
            line.startsWith(open, j) &&
            (spaced || (before === ">" && open.startsWith("<")))
          ) {
            code += line.slice(i, j);
            i = j + open.length;
            inBlock = close;
            continue scan;
          }
        }
        for (const marker of syntax.line) {
          if (line.startsWith(marker, j) && spaced) {
            code += line.slice(i, j);
            break scan;
          }
        }
      }
      code += line.slice(i);
      break;
    }
    const kept = code.trimEnd();
    if (kept) out.push(kept);
  }
  return out;
}

const sameLines = (a: string[], b: string[]) =>
  a.length === b.length && a.every((line, i) => line === b[i]);

/**
 * Le remplacement ne change que des commentaires : code identique relu par
 * l'analyseur (une marque de fermeture glissée dans le texte se voit), et
 * code identique vu par une lecture indépendante (`plainCode`).
 */
export function onlyComments(
  text: string,
  edit: Edit,
  syntax: Syntax,
): boolean {
  const changed = applyEdits(text, [edit]);
  return (
    sameLines(codeLines(text, syntax), codeLines(changed, syntax)) &&
    sameLines(plainCode(text, syntax), plainCode(changed, syntax))
  );
}

// --- Markdown et texte -----------------------------------------------------

export type Segment = { start: number; end: number };

/**
 * Passages de prose d'un document Markdown : hors des blocs de code (``` et
 * ~~~), de l'en-tête YAML (--- en tête) et des lignes de code indentées.
 */
export function proseSegments(text: string): Segment[] {
  const segments: Segment[] = [];
  const lines = text.split("\n");
  let offset = 0;
  let fence: string | null = null;
  let frontMatter = lines[0]?.trimEnd() === "---";
  let current: Segment | null = null;
  let previousBlank = true;
  let inIndented = false;
  const close = () => {
    if (current && text.slice(current.start, current.end).trim()) {
      segments.push(current);
    }
    current = null;
  };
  lines.forEach((line, index) => {
    const start = offset;
    const end = offset + line.length;
    offset = end + 1;
    const bare = line.replace(/\r$/, "");
    let code = false;
    if (frontMatter) {
      code = true;
      if (index > 0 && bare.trimEnd() === "---") frontMatter = false;
    } else if (fence) {
      code = true;
      if (
        bare.trim().startsWith(fence) &&
        bare.trim().replace(/[`~]/g, "") === ""
      ) {
        fence = null;
      }
    } else {
      const open = /^\s{0,3}(`{3,}|~{3,})/.exec(bare);
      if (open) {
        fence = open[1];
        code = true;
      } else if (
        /^(\t| {4})/.test(bare) &&
        (previousBlank || inIndented) &&
        !/^\s*([-*+]|\d+[.)])\s/.test(bare)
      ) {
        code = true;
      }
    }
    inIndented = code && !fence && /^(\t| {4})/.test(bare);
    previousBlank = bare.trim() === "";
    if (code) {
      close();
      return;
    }
    if (!current) current = { start, end };
    else current.end = end;
  });
  close();
  return segments;
}

/** Ce qu'une correction de Markdown ne doit pas changer : code en ligne,
 * destinations des liens, balises, adresses, structure des lignes (titres,
 * listes, citations). */
export function protectedTokens(text: string): string[] {
  const tokens: string[] = [];
  for (const m of text.matchAll(/`+[^`]+`+/g)) tokens.push(`code:${m[0]}`);
  for (const m of text.matchAll(/\]\(([^)]*)\)/g)) tokens.push(`link:${m[1]}`);
  for (const m of text.matchAll(/\]\[([^\]]*)\]/g)) tokens.push(`ref:${m[1]}`);
  for (const m of text.matchAll(/<[^>\n]+>/g)) tokens.push(`tag:${m[0]}`);
  for (const m of text.matchAll(/https?:\/\/[^\s)>\]]+/g)) {
    tokens.push(`url:${m[0]}`);
  }
  for (const line of text.split("\n")) {
    const lead = /^\s*((?:#{1,6}|>|[-*+]|\d+[.)])\s+)*/.exec(line)?.[0] ?? "";
    if (lead.trim()) tokens.push(`lead:${lead.replace(/\s+/g, " ").trim()}`);
  }
  return tokens;
}

/** La correction d'un passage de Markdown garde tout ce qui est protégé. */
export function keepsMarkdown(before: string, after: string): boolean {
  const a = protectedTokens(before);
  const b = protectedTokens(after);
  return a.length === b.length && a.every((t, i) => t === b[i]);
}

/** Langages corrigés comme des documents (prose). */
export function isDocument(languageId: string): boolean {
  return languageId === "markdown" || languageId === "plaintext";
}

// --- Commandes ---------------------------------------------------------------

export const SCHEME = "chatonlm-proofread";

export type ProofreadDeps = {
  link: LinkLike;
};

type Job = { text: string; apply(corrected: string): Edit | null };

export class Proofreader implements vscode.TextDocumentContentProvider {
  readonly #deps: ProofreadDeps;
  /** Textes proposés, par adresse de la vue de différences. */
  readonly #proposals = new Map<string, string>();
  readonly #changed = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.#changed.event;
  #ids = 0;
  #running = false;
  /** Passages annoncés par l'app pour le texte en cours. */
  #total = 0;
  /** Passage en cours : numéro de requête et rappel d'avancement. */
  #current: {
    id: number | null;
    part(done: number, total: number): void;
  } | null = null;

  constructor(deps: ProofreadDeps) {
    this.#deps = deps;
  }

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.#proposals.get(uri.toString()) ?? "";
  }

  /** `proofread.event` : passages corrigés du texte en cours. */
  onNotification({ method, params }: Notification): void {
    if (method !== "proofread.event") return;
    const current = this.#current;
    const { requestId, event } = (params ?? {}) as {
      requestId?: unknown;
      event?: { event?: unknown; parts?: unknown; index?: unknown };
    };
    // Numéro pas encore connu : passage annoncé avant le retour de l'envoi.
    if (!current || !event) return;
    if (current.id !== null && requestId !== current.id) return;
    if (event.event === "started" && Array.isArray(event.parts)) {
      this.#total = event.parts.length;
      current.part(0, this.#total);
    } else if (event.event === "partDone" && typeof event.index === "number") {
      current.part(event.index + 1, this.#total);
    }
  }

  /** « Proofread comments » : commentaires de la sélection ou du fichier. */
  async comments(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("Open a file first, then run this command again."),
      );
      return;
    }
    const { document, selection } = editor;
    if (isDocument(document.languageId)) {
      await this.document();
      return;
    }
    const syntax = syntaxFor(document.languageId);
    if (!syntax) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t(
          "ChatonLM does not know how comments are written in this language.",
        ),
      );
      return;
    }
    if (!this.#allowed(document)) return;
    const text = document.getText();
    const from = selection.isEmpty ? 0 : document.offsetAt(selection.start);
    const to = selection.isEmpty
      ? text.length
      : document.offsetAt(selection.end);
    const regions = commentRegions(text, syntax, from, to).filter(
      (r) => !skipRegion(r),
    );
    if (regions.length === 0) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("No comment to proofread here."),
      );
      return;
    }
    const jobs: Job[] = regions.map((region) => ({
      text: regionBody(region),
      apply: (corrected) => {
        const edit = {
          start: region.start,
          end: region.end,
          text: rebuildRegion(region, corrected),
        };
        if (edit.text === regionText(region)) return null;
        // Jamais le code : un passage qui changerait autre chose est écarté.
        return onlyComments(text, edit, syntax) ? edit : null;
      },
    }));
    await this.#run(document, jobs, "comments");
  }

  /** « Proofread this document » : Markdown ou texte (sélection ou tout). */
  async document(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("Open a file first, then run this command again."),
      );
      return;
    }
    const { document, selection } = editor;
    if (!isDocument(document.languageId)) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t(
          "Only Markdown and text documents are proofread whole. For code, use Proofread comments.",
        ),
      );
      return;
    }
    if (!this.#allowed(document)) return;
    const text = document.getText();
    const from = selection.isEmpty ? 0 : document.offsetAt(selection.start);
    const to = selection.isEmpty
      ? text.length
      : document.offsetAt(selection.end);
    const markdown = document.languageId === "markdown";
    const segments = (
      markdown ? proseSegments(text) : [{ start: 0, end: text.length }]
    )
      .map((s) => ({
        start: Math.max(s.start, from),
        end: Math.min(s.end, to),
      }))
      .filter((s) => s.end > s.start && text.slice(s.start, s.end).trim());
    const jobs: Job[] = segments
      .filter((s) => !holdsSecret(text.slice(s.start, s.end)))
      .map((segment) => {
        const original = text.slice(segment.start, segment.end);
        return {
          text: original,
          apply: (corrected) => {
            if (corrected === original || !corrected.trim()) return null;
            if (markdown && !keepsMarkdown(original, corrected)) return null;
            return { start: segment.start, end: segment.end, text: corrected };
          },
        };
      });
    if (jobs.length === 0) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("No text to proofread here."),
      );
      return;
    }
    await this.#run(document, jobs, "document");
  }

  #allowed(document: vscode.TextDocument): boolean {
    if (!this.#deps.link.connected) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return false;
    }
    if (this.#running) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("A proofreading is running. Wait for it, or stop it."),
      );
      return false;
    }
    if (isSecretFile(document.uri.path)) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t(
          "This file may hold secrets: ChatonLM does not proofread it.",
        ),
      );
      return false;
    }
    return true;
  }

  /** Passages corrigés un par un, puis différences montrées. */
  async #run(
    document: vscode.TextDocument,
    jobs: Job[],
    kind: "comments" | "document",
  ): Promise<void> {
    this.#running = true;
    const version = document.version;
    const original = document.getText();
    const edits: Edit[] = [];
    let kept = 0;
    let failure: string | null = null;
    try {
      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: vscode.l10n.t("ChatonLM is proofreading"),
          cancellable: true,
        },
        async (progress, token) => {
          for (let index = 0; index < jobs.length; index++) {
            if (token.isCancellationRequested) return;
            const job = jobs[index];
            progress.report({
              message:
                kind === "comments"
                  ? vscode.l10n.t("Comment {0} of {1}", index + 1, jobs.length)
                  : vscode.l10n.t("Passage {0} of {1}", index + 1, jobs.length),
            });
            const current = {
              id: null as number | null,
              part: (done: number, total: number) => {
                if (kind === "document" && jobs.length === 1 && total > 0) {
                  progress.report({
                    message: vscode.l10n.t("Passage {0} of {1}", done, total),
                  });
                }
              },
            };
            this.#current = current;
            const call = this.#deps.link.call<{ text?: unknown }>(
              "proofread.run",
              { text: job.text, mode: "correct" },
            );
            current.id = call.id;
            const stop = token.onCancellationRequested(() =>
              this.#deps.link.cancel(call.id),
            );
            try {
              const result = await call.result;
              if (token.isCancellationRequested) return;
              if (typeof result?.text !== "string") continue;
              const edit = job.apply(result.text);
              if (edit) edits.push(edit);
              else if (result.text !== job.text) kept++;
            } catch (error) {
              if (token.isCancellationRequested) return;
              failure =
                error instanceof RpcError
                  ? (error.appMessage ??
                    vscode.l10n.t("ChatonLM could not proofread this text."))
                  : vscode.l10n.t("ChatonLM could not proofread this text.");
              return;
            } finally {
              stop.dispose();
              this.#current = null;
            }
          }
        },
      );
    } finally {
      this.#running = false;
    }
    if (failure) {
      void vscode.window.showErrorMessage(failure);
      if (edits.length === 0) return;
    }
    if (edits.length === 0) {
      void vscode.window.showInformationMessage(
        kept > 0
          ? vscode.l10n.t(
              "No correction was kept: the proposed changes touched code, links or formatting.",
            )
          : vscode.l10n.t("Nothing to correct."),
      );
      return;
    }
    const proposed = applyEdits(original, edits);
    const name = vscode.workspace.asRelativePath(document.uri);
    const uri = vscode.Uri.parse(
      `${SCHEME}:/${encodeURIComponent(name)}?${++this.#ids}`,
    );
    this.#proposals.set(uri.toString(), proposed);
    this.#changed.fire(uri);
    await vscode.commands.executeCommand(
      "vscode.diff",
      document.uri,
      uri,
      vscode.l10n.t("{0} ↔ proofread by ChatonLM", name),
    );
    const apply = vscode.l10n.t("Apply");
    const discard = vscode.l10n.t("Discard");
    const choice = await vscode.window.showInformationMessage(
      kept > 0
        ? vscode.l10n.t(
            "Apply the corrections to {0}? Passages left as they were, because their changes touched code, links or formatting: {1}.",
            name,
            kept,
          )
        : vscode.l10n.t("Apply the corrections to {0}?", name),
      apply,
      discard,
    );
    this.#proposals.delete(uri.toString());
    if (choice !== apply) return;
    if (document.version !== version || document.getText() !== original) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t(
          "{0} changed since the proofreading: nothing was applied. Run it again.",
          name,
        ),
      );
      return;
    }
    const workspaceEdit = new vscode.WorkspaceEdit();
    for (const edit of edits) {
      workspaceEdit.replace(
        document.uri,
        new vscode.Range(
          document.positionAt(edit.start),
          document.positionAt(edit.end),
        ),
        edit.text,
      );
    }
    await vscode.workspace.applyEdit(workspaceEdit);
  }
}
