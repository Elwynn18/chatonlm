// Export .pdf d'un document (blocs de src/lib/export/blocks.ts), par
// pdfmake (MIT), dans sa version pour navigateur. Chargé par import() au
// premier export (src/lib/export/index.ts) : pdfmake et les polices restent
// hors du paquet initial (scripts/check-bundle.mjs le vérifie).
//
// Polices intégrées au PDF, sous OFL-1.1 (THIRD_PARTY_LICENSES partie 3) :
// Figtree en quatre styles (celle du site, direction visuelle de ChatonLM)
// et JetBrains Mono pour le code, chacune en deux sous-ensembles de
// Fontsource : « latin » (anglais, français, espagnol, Europe de l'Ouest) et
// « latin-ext » (polonais, tchèque, roumain, hongrois, turc, croate...),
// déclarés comme deux familles ; chaque morceau de texte est découpé selon la
// famille qui a ses caractères (`segments`, table de src/lib/export/coverage.ts).
// Fichiers WOFF (fontkit ne sait pas découper un WOFF2), mis dans le morceau
// en base64 (`?inline`) : la CSP de la page (connect-src) interdit de les
// relire par fetch(). Quelques caractères absents des deux (espaces fines,
// traits d'union insécables, flèches, comparaisons, puces, coches...) sont
// remplacés par leur plus proche voisin et les emoji retirés (`fontSafe`) ;
// pour les autres (cyrillique, grec, CJC...),
// `missingCharacters` permet de refuser l'export avant de rien produire.
import pdfMake from "pdfmake/build/pdfmake.js";
import type {
  Content,
  ContentText,
  CustomTableLayout,
  TDocumentDefinitions,
} from "pdfmake/interfaces";
import figtreeBold from "@fontsource/figtree/files/figtree-latin-700-normal.woff?inline";
import figtreeBoldItalic from "@fontsource/figtree/files/figtree-latin-700-italic.woff?inline";
import figtreeItalic from "@fontsource/figtree/files/figtree-latin-400-italic.woff?inline";
import figtreeRegular from "@fontsource/figtree/files/figtree-latin-400-normal.woff?inline";
import figtreeExtBold from "@fontsource/figtree/files/figtree-latin-ext-700-normal.woff?inline";
import figtreeExtBoldItalic from "@fontsource/figtree/files/figtree-latin-ext-700-italic.woff?inline";
import figtreeExtItalic from "@fontsource/figtree/files/figtree-latin-ext-400-italic.woff?inline";
import figtreeExtRegular from "@fontsource/figtree/files/figtree-latin-ext-400-normal.woff?inline";
import monoRegular from "@fontsource/jetbrains-mono/files/jetbrains-mono-latin-400-normal.woff?inline";
import monoExtRegular from "@fontsource/jetbrains-mono/files/jetbrains-mono-latin-ext-400-normal.woff?inline";
import type { Align, Block, DocumentHead, Inline } from "./blocks";
import { COVERAGE, covers } from "./coverage";

/** Couleurs de la direction visuelle (.tools/research/75), thème clair. */
const COLORS = {
  ink: "#2A1A10",
  body: "#382820",
  muted: "#6B5647",
  faint: "#8E7763",
  accent: "#F28A38",
  accentText: "#B84A12",
  sand: "#F6E4D0",
  surface: "#FBF1E4",
  line: "#E8D2BA",
};

/** A4, marges de 2 cm. */
const PAGE = { width: 595.28, margin: 56.7 };
const CONTENT_WIDTH = PAGE.width - 2 * PAGE.margin;

const SANS = "Figtree";
const SANS_EXT = "FigtreeExt";
const MONO = "JetBrainsMono";
const MONO_EXT = "JetBrainsMonoExt";

/** Famille d'un texte : courant, ou code. */
type Family = "sans" | "mono";

/** Polices d'une famille : sous-ensemble latin, puis latin-ext. */
const FONTS: Record<Family, { base: string; ext: string }> = {
  sans: { base: SANS, ext: SANS_EXT },
  mono: { base: MONO, ext: MONO_EXT },
};

/** Partie base64 d'une adresse `data:` produite par `?inline`. */
function base64(dataUrl: string): string {
  const comma = dataUrl.indexOf(",");
  return comma < 0 ? dataUrl : dataUrl.slice(comma + 1);
}

let fontsReady = false;

/**
 * Polices déclarées à pdfmake, une fois : fichiers du système de fichiers
 * virtuel de pdfmake, nommés par leur style (noms internes, jamais montrés).
 */
function registerFonts() {
  if (fontsReady) return;
  pdfMake.addVirtualFileSystem({
    regular: base64(figtreeRegular),
    bold: base64(figtreeBold),
    italic: base64(figtreeItalic),
    boldItalic: base64(figtreeBoldItalic),
    mono: base64(monoRegular),
    regularExt: base64(figtreeExtRegular),
    boldExt: base64(figtreeExtBold),
    italicExt: base64(figtreeExtItalic),
    boldItalicExt: base64(figtreeExtBoldItalic),
    monoExt: base64(monoExtRegular),
  });
  pdfMake.setFonts({
    [SANS]: {
      normal: "regular",
      bold: "bold",
      italics: "italic",
      bolditalics: "boldItalic",
    },
    // Un seul style : le gras ou l'italique d'un code restent droits.
    [MONO]: {
      normal: "mono",
      bold: "mono",
      italics: "mono",
      bolditalics: "mono",
    },
    [SANS_EXT]: {
      normal: "regularExt",
      bold: "boldExt",
      italics: "italicExt",
      bolditalics: "boldItalicExt",
    },
    [MONO_EXT]: {
      normal: "monoExt",
      bold: "monoExt",
      italics: "monoExt",
      bolditalics: "monoExt",
    },
  });
  fontsReady = true;
}

/** Remplacements des caractères absents des polices intégrées. */
const SUBSTITUTES: Record<string, string> = {
  "\u2007": "\u00A0", // espace tabulaire, insécable
  "\u202F": "\u00A0", // espace fine insécable (typographie française)
  "\u2010": "-",
  "\u2011": "-",
  "\u2012": "\u2013",
  "\u2015": "\u2014",
  "\u201B": "\u2018",
  "\u201F": "\u201C",
  "\u2024": ".",
  "\u2025": "..",
  "\u2027": "\u00B7",
  "\u2028": "\n",
  "\u2029": "\n",
  "\u2032": "'",
  "\u2033": '"',
  "\u2034": "'''",
  "\u203C": "!!",
  "\u2043": "-",
  "\u2047": "??",
  "\u2048": "?!",
  "\u2049": "!?",
  "\u2E3A": "\u2014",
  "\u2E3B": "\u2014",
  "\u22EF": "\u2026",
  // t cédille (anciens encodages du roumain) : t virgule souscrite, la forme
  // correcte, que Figtree a.
  Ţ: "\u021A",
  ţ: "\u021B",
  // Signes fréquents dans les documents d'un modèle (synthèse) : flèches,
  // comparaisons, puces, coches, guillemets ornés, en équivalents simples.
  "\u2190": "<-",
  "\u2192": "->",
  "\u2194": "<->",
  "\u2195": "\u2191\u2193",
  "\u2196": "\u2191",
  "\u2197": "\u2191",
  "\u2198": "\u2193",
  "\u2199": "\u2193",
  "\u21A6": "->",
  "\u21A9": "<-",
  "\u21AA": "->",
  "\u21D0": "<=",
  "\u21D2": "=>",
  "\u21D4": "<=>",
  "\u27F5": "<-",
  "\u27F6": "->",
  "\u27F7": "<->",
  "\u27F8": "<=",
  "\u27F9": "=>",
  "\u27FA": "<=>",
  "\u2794": "->",
  "\u279C": "->",
  "\u279D": "->",
  "\u279E": "->",
  "\u27A1": "->",
  "\u2B05": "<-",
  "\u2B06": "\u2191",
  "\u2B07": "\u2193",
  "\u2264": "<=",
  "\u2265": ">=",
  "\u2A7D": "<=",
  "\u2A7E": ">=",
  "\u2260": "!=",
  "\u2248": "~",
  "\u2243": "~",
  "\u2245": "~",
  "\u223C": "~",
  "\u2261": "=",
  "\u226A": "<<",
  "\u226B": ">>",
  "\u2217": "*",
  "\u2219": "\u00B7",
  "\u22C5": "\u00B7",
  "\u2023": "\u2022",
  "\u25AA": "\u2022",
  "\u25AB": "\u2022",
  "\u25B6": "\u2022",
  "\u25B7": "\u2022",
  "\u25B8": "\u2022",
  "\u25B9": "\u2022",
  "\u25BA": "\u2022",
  "\u25BB": "\u2022",
  "\u25E6": "\u2022",
  "\u25CF": "\u2022",
  "\u25CB": "\u2022",
  "\u25C6": "\u2022",
  "\u25C7": "\u2022",
  "\u25A0": "\u2022",
  "\u25A1": "\u2022",
  "\u25FB": "\u2022",
  "\u25FC": "\u2022",
  "\u2981": "\u2022",
  "\u2713": "[x]",
  "\u2714": "[x]",
  "\u2705": "[x]",
  "\u2611": "[x]",
  "\u2610": "[ ]",
  "\u2717": "\u00D7",
  "\u2718": "\u00D7",
  "\u274C": "\u00D7",
  "\u274E": "\u00D7",
  "\u2612": "\u00D7",
  "\u2605": "*",
  "\u2606": "*",
  "\u2B50": "*",
  "\u26A0": "(!)",
  ℹ: "(i)",
  "\u2757": "!",
  "\u2755": "!",
  "\u2753": "?",
  "\u2754": "?",
  "\u275B": "\u2018",
  "\u275C": "\u2019",
  "\u275D": "\u201C",
  "\u275E": "\u201D",
  "\u27E8": "<",
  "\u27E9": ">",
  "\u2153": "1/3",
  "\u2154": "2/3",
  "\u2155": "1/5",
  "\u2159": "1/6",
  "\u215B": "1/8",
  "\u2103": "\u00B0C",
  "\u2109": "\u00B0F",
  "\u2116": "No",
};

/** Un des caractères remplacés. */
const SUBSTITUTED = new RegExp(
  `[${Object.keys(SUBSTITUTES)
    .map((c) => `\\u{${(c.codePointAt(0) ?? 0).toString(16)}}`)
    .join("")}]`,
  "gu",
);

/**
 * Suite d'emoji : pictogrammes (sauf © ® ™, que les polices ont), sélecteurs
 * de variante, touche « keycap », teintes de peau, drapeaux, étiquettes ;
 * avec l'espace qui la précède et celle qui la suit.
 */
const EMOJI =
  /( ?)(?:(?![\u00A9\u00AE\u2122])\p{Extended_Pictographic}|\p{Emoji_Modifier}|\p{Regional_Indicator}|[\uFE00-\uFE0F]|\u20E3|[\u{E0020}-\u{E007F}])+( ?)/gu;

/**
 * Texte écrit avec les glyphes des polices intégrées : espaces Unicode
 * ramenées à l'espace (ou à l'insécable), caractères invisibles (césure
 * conditionnelle, chasse nulle, marques de sens) retirés, signes courants
 * remplacés par leur équivalent (flèches, comparaisons, puces, coches,
 * guillemets ornés...), emoji retirés (une seule espace gardée entre deux
 * mots), tabulation en quatre espaces. Le reste est inchangé
 * (`missingCharacters` le vérifie).
 */
export function fontSafe(text: string): string {
  return text
    .replace(/[\u00AD\u200B-\u200F\u202A-\u202E\u2060-\u206F\uFEFF]/g, "")
    .replace(SUBSTITUTED, (c) => SUBSTITUTES[c] ?? c)
    .replace(EMOJI, (_match, before: string, after: string, offset: number) =>
      (before || offset > 0) && after ? " " : "",
    )
    .replace(/[\u2000-\u200A\u205F]/g, " ")
    .replace(/\t/g, "    ");
}

/** Police d'un caractère dans sa famille ; null : aucune ne l'a. */
function fontOf(char: string, family: Family): string | null {
  if (char === "\n") return FONTS[family].base;
  const code = char.codePointAt(0) ?? 0;
  if (covers(COVERAGE[family], code)) return FONTS[family].base;
  if (covers(COVERAGE[`${family}Ext`], code)) return FONTS[family].ext;
  return null;
}

/**
 * Texte (déjà passé par `fontSafe`) découpé en morceaux d'une seule police :
 * le sous-ensemble latin d'abord, latin-ext pour ce qu'il n'a pas. Un
 * caractère qu'aucune n'a reste dans la police de base (`missingCharacters`
 * le signale avant).
 */
export function segments(
  text: string,
  family: Family,
): { text: string; font: string }[] {
  const out: { text: string; font: string }[] = [];
  for (const char of text) {
    const font = fontOf(char, family) ?? FONTS[family].base;
    const last = out.at(-1);
    if (last && last.font === font) last.text += char;
    else out.push({ text: char, font });
  }
  return out;
}

/**
 * Caractères du document qu'aucune police intégrée ne peut montrer, après
 * les remplacements de `fontSafe`, dans l'ordre de première apparition.
 * Vide : le PDF peut être produit.
 */
export function missingCharacters(
  head: DocumentHead,
  blocks: Block[],
): string[] {
  const missing = new Set<string>();
  const check = (text: string, family: Family) => {
    for (const char of fontSafe(text)) {
      if (!fontOf(char, family)) missing.add(char);
    }
  };
  const inline = (content: Inline[]) => {
    for (const piece of content)
      check(piece.text, piece.code ? "mono" : "sans");
  };
  const walk = (list: Block[]) => {
    for (const block of list) {
      switch (block.type) {
        case "heading":
        case "paragraph":
          inline(block.content);
          break;
        case "code":
          check(block.text, "mono");
          break;
        case "list":
          for (const item of block.items) walk(item.blocks);
          break;
        case "quote":
          walk(block.blocks);
          break;
        case "table":
          for (const cell of [...block.header, ...block.rows.flat()])
            inline(cell);
          break;
        case "rule":
          break;
      }
    }
  };
  for (const text of [head.title ?? "", head.date, head.footer]) {
    check(text, "sans");
  }
  walk(blocks);
  return [...missing];
}

/** Morceaux pdfmake d'un texte du corps, police choisie par caractère. */
function textRuns(text: string, family: Family): ContentText[] {
  return segments(fontSafe(text), family).map(({ text: part, font }) =>
    font === SANS ? { text: part } : { text: part, font },
  );
}

/** Morceaux de texte pdfmake d'un morceau du modèle (un par police). */
function run(piece: Inline): ContentText[] {
  return textRuns(piece.text, piece.code ? "mono" : "sans").map((part) =>
    decorate(part, piece),
  );
}

/** Mise en forme d'un morceau du modèle, sur un morceau pdfmake. */
function decorate(out: ContentText, piece: Inline): ContentText {
  if (piece.bold) out.bold = true;
  if (piece.italic) out.italics = true;
  if (piece.strike) out.decoration = "lineThrough";
  if (piece.code) {
    out.fontSize = 9.5;
    out.background = COLORS.sand;
  }
  if (piece.link) {
    out.link = piece.link;
    out.color = COLORS.accentText;
    out.decoration = "underline";
  }
  return out;
}

function runs(content: Inline[]): ContentText[] {
  return content.flatMap(run);
}

const NO_LINES = { hLineWidth: () => 0, vLineWidth: () => 0 };

/** Bloc de code : fond sable clair, sans filets. */
const CODE_LAYOUT: CustomTableLayout = {
  ...NO_LINES,
  fillColor: () => COLORS.surface,
  paddingLeft: () => 9,
  paddingRight: () => 9,
  paddingTop: () => 7,
  paddingBottom: () => 7,
};

/** Citation : filet roux à gauche. */
const QUOTE_LAYOUT: CustomTableLayout = {
  hLineWidth: () => 0,
  vLineWidth: (i) => (i === 0 ? 2 : 0),
  vLineColor: () => COLORS.accent,
  paddingLeft: () => 10,
  paddingRight: () => 0,
  paddingTop: () => 1,
  paddingBottom: () => 1,
};

/** Tableau : filets sable, en-tête sur fond sable. */
const TABLE_LAYOUT: CustomTableLayout = {
  hLineWidth: () => 0.75,
  vLineWidth: () => 0.75,
  hLineColor: () => COLORS.line,
  vLineColor: () => COLORS.line,
  fillColor: (row) => (row === 0 ? COLORS.sand : null),
  paddingLeft: () => 6,
  paddingRight: () => 6,
  paddingTop: () => 4,
  paddingBottom: () => 4,
};

function alignment(align: Align): "left" | "center" | "right" {
  return align ?? "left";
}

/** Contenu pdfmake d'une suite de blocs. */
function contents(blocks: Block[]): Content[] {
  return blocks.map(content);
}

function content(block: Block): Content {
  switch (block.type) {
    case "heading":
      return {
        text: runs(block.content),
        style: `h${block.level}`,
        headlineLevel: block.level,
      };
    case "paragraph":
      return { text: runs(block.content), style: "paragraph" };
    case "code":
      return {
        table: {
          widths: ["*"],
          body: [
            [
              {
                text: textRuns(block.text, "mono"),
                font: MONO,
                fontSize: 9,
                lineHeight: 1.25,
                color: COLORS.ink,
                preserveLeadingSpaces: true,
              },
            ],
          ],
        },
        layout: CODE_LAYOUT,
        margin: [0, 2, 0, 10],
      };
    case "list": {
      const items = block.items.map((item) => {
        const inner = contents(item.blocks);
        const box =
          item.checked === undefined ? null : item.checked ? "[x] " : "[ ] ";
        if (box) inner.unshift({ text: box, font: MONO, fontSize: 9.5 });
        // Paragraphe seul : sans marge propre, pour une liste serrée.
        if (inner.length === 1) return withoutMargin(inner[0]);
        if (box && inner.length === 2 && isText(inner[1])) {
          return { text: [inner[0] as ContentText, ...asRuns(inner[1])] };
        }
        return { stack: inner.map(withoutMargin), margin: [0, 0, 0, 2] };
      });
      const list = block.ordered
        ? { ol: items, start: block.start }
        : { ul: items };
      return {
        ...list,
        markerColor: COLORS.accentText,
        margin: [0, 0, 0, 8],
      } as Content;
    }
    case "quote":
      return {
        table: {
          widths: ["*"],
          body: [[{ stack: contents(block.blocks), color: COLORS.muted }]],
        },
        layout: QUOTE_LAYOUT,
        margin: [0, 2, 0, 10],
      };
    case "rule":
      return {
        canvas: [
          {
            type: "line",
            x1: 0,
            y1: 0,
            x2: CONTENT_WIDTH,
            y2: 0,
            lineWidth: 0.75,
            lineColor: COLORS.line,
          },
        ],
        margin: [0, 6, 0, 14],
      };
    case "table": {
      const columns = Math.max(
        block.header.length,
        ...block.rows.map((row) => row.length),
      );
      const cell = (pieces: Inline[] | undefined, column: number) => ({
        text: runs(pieces ?? []),
        alignment: alignment(block.align[column] ?? null),
      });
      return {
        table: {
          headerRows: 1,
          widths: Array.from({ length: columns }, () => "*"),
          body: [
            Array.from({ length: columns }, (_, i) => ({
              ...cell(block.header[i], i),
              bold: true,
              color: COLORS.ink,
            })),
            ...block.rows.map((row) =>
              Array.from({ length: columns }, (_, i) => cell(row[i], i)),
            ),
          ],
        },
        layout: TABLE_LAYOUT,
        fontSize: 10,
        margin: [0, 2, 0, 10],
      };
    }
  }
}

function isText(node: Content): node is ContentText {
  return typeof node === "object" && node !== null && "text" in node;
}

function asRuns(node: ContentText): ContentText[] {
  return Array.isArray(node.text)
    ? (node.text as ContentText[])
    : [{ text: node.text as string }];
}

/**
 * Paragraphe ou sous-liste d'un élément de liste : marge courte au lieu de
 * celle d'un bloc du corps.
 */
function withoutMargin(node: Content): Content {
  if (isText(node) && node.style === "paragraph") {
    return { text: node.text, margin: [0, 0, 0, 2] };
  }
  if (
    typeof node === "object" &&
    node !== null &&
    ("ul" in node || "ol" in node)
  ) {
    return { ...node, margin: [0, 0, 0, 0] } as Content;
  }
  return node;
}

/** Définition pdfmake complète : en-tête, corps, pied de page. */
export function pdfDefinition(
  head: DocumentHead,
  blocks: Block[],
): TDocumentDefinitions {
  const top: Content[] = [];
  if (head.title) {
    top.push({ text: textRuns(head.title, "sans"), style: "title" });
  }
  top.push({ text: textRuns(head.date, "sans"), style: "date" });
  top.push({
    canvas: [
      { type: "rect", x: 0, y: 0, w: 36, h: 3, r: 1.5, color: COLORS.accent },
    ],
    margin: [0, 8, 0, 20],
  });
  return {
    pageSize: "A4",
    pageMargins: [PAGE.margin, PAGE.margin, PAGE.margin, PAGE.margin + 6],
    info: {
      title: head.title ?? head.date,
      creator: head.footer,
      producer: head.footer,
    },
    language: head.lang,
    content: [...top, ...contents(blocks)],
    // Titre jamais seul en bas de page : il passe avec la suite.
    pageBreakBefore: (node, nodes) =>
      node.headlineLevel !== undefined &&
      nodes.getFollowingNodesOnPage().length === 0 &&
      nodes.getNodesOnNextPage().length > 0,
    footer: (page, count) => ({
      columns: [
        { text: textRuns(head.footer, "sans"), color: COLORS.faint },
        { text: `${page} / ${count}`, alignment: "right", color: COLORS.faint },
      ],
      fontSize: 8,
      margin: [PAGE.margin, 18, PAGE.margin, 0],
    }),
    defaultStyle: {
      font: SANS,
      fontSize: 10.5,
      lineHeight: 1.3,
      color: COLORS.body,
    },
    styles: {
      title: { fontSize: 24, bold: true, color: COLORS.ink, lineHeight: 1.15 },
      date: { fontSize: 9.5, color: COLORS.muted, margin: [0, 4, 0, 0] },
      h1: {
        fontSize: 17,
        bold: true,
        color: COLORS.accentText,
        margin: [0, 14, 0, 6],
      },
      h2: {
        fontSize: 13.5,
        bold: true,
        color: COLORS.ink,
        margin: [0, 12, 0, 5],
      },
      h3: {
        fontSize: 11.5,
        bold: true,
        color: COLORS.muted,
        margin: [0, 10, 0, 4],
      },
      paragraph: { margin: [0, 0, 0, 8] },
    },
  };
}

/**
 * Octets du .pdf d'un document. L'appelant vérifie d'abord
 * `missingCharacters` (src/lib/export/index.ts le fait).
 */
export async function renderPdf(
  head: DocumentHead,
  blocks: Block[],
): Promise<Uint8Array> {
  registerFonts();
  const buffer = await pdfMake
    .createPdf(pdfDefinition(head, blocks))
    .getBuffer();
  return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
}
