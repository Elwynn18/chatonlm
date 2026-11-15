// Modèle intermédiaire des exports (.docx, .pdf) : un document Markdown lu
// par le lexer de marked (déjà livré pour le rendu des réponses), réduit aux
// blocs qu'un traitement de texte sait montrer. Le même modèle nourrit
// src/lib/export/docx.ts et src/lib/export/pdf.ts ; il ne dépend d'aucune
// des deux bibliothèques, chargées à la demande.
//
// Gardé : titres (niveaux 4 à 6 rendus au niveau 3), paragraphes, gras,
// italique, barré, code en ligne, liens (http, https et mailto seulement :
// un lien file:, de partage réseau ou d'un autre schéma garde son texte et
// perd son adresse, comme dans l'app), retours à la ligne forcés, blocs de
// code, listes à puces et numérotées imbriquées (cases à cocher en « [x] »),
// citations (imbriquées), règles horizontales, tableaux. Le HTML brut est
// gardé comme texte, une image devient son texte de remplacement.
import { Lexer, type Token, type Tokens } from "marked";

/**
 * Adresse gardée dans un export : http(s) ou mailto, telle quelle ; null pour
 * tout autre schéma (file:, partage réseau \\serveur, javascript:...), une
 * adresse relative ou illisible. Le document vient d'un modèle qui a lu du
 * contenu du web : un traitement de texte suivrait ces liens.
 */
export function exportLink(href: string | undefined): string | null {
  if (!href) return null;
  try {
    const url = new URL(href);
    return url.protocol === "http:" ||
      url.protocol === "https:" ||
      url.protocol === "mailto:"
      ? href
      : null;
  } catch {
    return null;
  }
}

/** Morceau de texte d'un paragraphe, avec sa mise en forme. */
export interface Inline {
  text: string;
  bold?: boolean;
  italic?: boolean;
  strike?: boolean;
  code?: boolean;
  /** Adresse d'un lien. */
  link?: string;
}

/** Élément d'une liste : ses blocs (paragraphe, sous-liste...). */
export interface ListItem {
  blocks: Block[];
  /** Case à cocher d'une liste de tâches ; absent sinon. */
  checked?: boolean;
}

/** Alignement d'une colonne de tableau ; null : celui du texte. */
export type Align = "left" | "center" | "right" | null;

export type Block =
  | { type: "heading"; level: 1 | 2 | 3; content: Inline[] }
  | { type: "paragraph"; content: Inline[] }
  | { type: "code"; text: string; lang?: string }
  | { type: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { type: "quote"; blocks: Block[] }
  | { type: "rule" }
  | {
      type: "table";
      align: Align[];
      header: Inline[][];
      rows: Inline[][][];
    };

/** Mise en forme héritée des marques englobantes. */
type Marks = Omit<Inline, "text">;

const ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: "\u00A0",
};

/**
 * Entités HTML d'un texte (le lexer les laisse telles quelles) : nommées
 * courantes et numériques ; une entité inconnue reste écrite.
 */
export function decodeEntities(text: string): string {
  if (!text.includes("&")) return text;
  return text.replace(
    /&(?:#(\d{1,7})|#[xX]([0-9a-fA-F]{1,6})|([a-zA-Z]+));/g,
    (match, decimal?: string, hex?: string, name?: string) => {
      if (name !== undefined) return ENTITIES[name] ?? match;
      const code = Number.parseInt(decimal ?? hex ?? "", decimal ? 10 : 16);
      return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : match;
    },
  );
}

/** Morceaux d'une suite de jetons en ligne, marques héritées comprises. */
export function inlines(
  tokens: Token[] | undefined,
  marks: Marks = {},
): Inline[] {
  const out: Inline[] = [];
  const push = (text: string, extra: Marks = {}) => {
    if (text === "") return;
    const piece = { text, ...marks, ...extra };
    const last = out.at(-1);
    // Morceaux voisins de même forme : un seul (moins de runs dans le .docx).
    if (last && sameMarks(last, piece)) last.text += text;
    else out.push(piece);
  };
  for (const token of tokens ?? []) {
    switch (token.type) {
      case "strong":
      case "em":
      case "del": {
        const key =
          token.type === "strong"
            ? "bold"
            : token.type === "em"
              ? "italic"
              : "strike";
        for (const piece of inlines(token.tokens, { ...marks, [key]: true })) {
          push(piece.text, piece);
        }
        break;
      }
      case "link": {
        const link = token as Tokens.Link;
        const href = exportLink(link.href);
        for (const piece of inlines(link.tokens, {
          ...marks,
          ...(href ? { link: href } : {}),
        })) {
          push(piece.text, piece);
        }
        break;
      }
      case "image":
        push(decodeEntities((token as Tokens.Image).text));
        break;
      case "codespan":
        push(decodeEntities((token as Tokens.Codespan).text), { code: true });
        break;
      case "br":
        push("\n");
        break;
      case "escape":
      case "html":
        push((token as Tokens.Escape).text);
        break;
      case "text": {
        const text = token as Tokens.Text;
        if (text.tokens && text.tokens.length > 0) {
          for (const piece of inlines(text.tokens, marks))
            push(piece.text, piece);
        } else {
          push(decodeEntities(text.text));
        }
        break;
      }
      default:
        if ("text" in token && typeof token.text === "string") {
          push(decodeEntities(token.text));
        }
    }
  }
  return out;
}

function sameMarks(a: Inline, b: Inline): boolean {
  return (
    !!a.bold === !!b.bold &&
    !!a.italic === !!b.italic &&
    !!a.strike === !!b.strike &&
    !!a.code === !!b.code &&
    a.link === b.link
  );
}

/** Blocs d'une suite de jetons de bloc. */
function blocks(tokens: Token[]): Block[] {
  const out: Block[] = [];
  for (const token of tokens) {
    switch (token.type) {
      case "heading": {
        const heading = token as Tokens.Heading;
        const level = Math.min(Math.max(heading.depth, 1), 3) as 1 | 2 | 3;
        out.push({ type: "heading", level, content: inlines(heading.tokens) });
        break;
      }
      case "paragraph":
        out.push({
          type: "paragraph",
          content: inlines((token as Tokens.Paragraph).tokens),
        });
        break;
      case "text": {
        // Élément d'une liste serrée : son texte, sans paragraphe autour.
        const text = token as Tokens.Text;
        const content = text.tokens
          ? inlines(text.tokens)
          : [{ text: decodeEntities(text.text) }];
        out.push({ type: "paragraph", content });
        break;
      }
      case "code": {
        const code = token as Tokens.Code;
        out.push({
          type: "code",
          text: code.text,
          ...(code.lang ? { lang: code.lang } : {}),
        });
        break;
      }
      case "list": {
        const list = token as Tokens.List;
        const start = typeof list.start === "number" ? list.start : 1;
        out.push({
          type: "list",
          ordered: list.ordered,
          start,
          items: list.items.map((item) => ({
            blocks: blocks(item.tokens),
            ...(item.task ? { checked: !!item.checked } : {}),
          })),
        });
        break;
      }
      case "blockquote":
        out.push({
          type: "quote",
          blocks: blocks((token as Tokens.Blockquote).tokens),
        });
        break;
      case "hr":
        out.push({ type: "rule" });
        break;
      case "table": {
        const table = token as Tokens.Table;
        out.push({
          type: "table",
          align: table.align,
          header: table.header.map((cell) => inlines(cell.tokens)),
          rows: table.rows.map((row) =>
            row.map((cell) => inlines(cell.tokens)),
          ),
        });
        break;
      }
      case "html": {
        const text = (token as Tokens.HTML).text.trim();
        if (text !== "") out.push({ type: "paragraph", content: [{ text }] });
        break;
      }
      default:
        // space, def (définitions de liens) : rien à montrer.
        break;
    }
  }
  return out;
}

/**
 * Blocs d'un document Markdown (GitHub Flavored Markdown). Les caractères de
 * contrôle (hors tabulation et fin de ligne) sont retirés : le XML d'un
 * .docx les refuse.
 */
export function parseMarkdown(markdown: string): Block[] {
  const text = markdown
    .replace(/\r\n?/g, "\n")
    // eslint-disable-next-line no-control-regex -- retrait voulu
    .replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
  return blocks(Lexer.lex(text, { gfm: true }));
}

/** Texte brut d'une suite de morceaux. */
export function plainText(content: Inline[]): string {
  return content.map((piece) => piece.text).join("");
}

/**
 * Titre du document : `title` s'il est donné ; sinon le premier bloc s'il
 * est un titre de niveau 1, alors retiré du corps pour ne pas l'écrire deux
 * fois. `null` : pas de titre.
 */
export function splitTitle(
  body: Block[],
  title?: string,
): { title: string | null; body: Block[] } {
  const given = title?.trim();
  if (given) return { title: given, body };
  const [first, ...rest] = body;
  if (first?.type === "heading" && first.level === 1) {
    const text = plainText(first.content).trim();
    if (text !== "") return { title: text, body: rest };
  }
  return { title: null, body };
}

/** En-tête et pied de page d'un document exporté, textes déjà traduits. */
export interface DocumentHead {
  /** Titre en tête ; null : pas de titre. */
  title: string | null;
  /** Date en tête, déjà mise en forme. */
  date: string;
  /** Texte du pied de page (« ChatonLM »). */
  footer: string;
  /** Langue du document (BCP 47), pour les métadonnées. */
  lang: string;
}
