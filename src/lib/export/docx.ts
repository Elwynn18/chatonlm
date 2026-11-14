// Export .docx d'un document (blocs de src/lib/export/blocks.ts), par le
// paquet docx (MIT). Chargé par import() au premier export
// (src/lib/export/index.ts), hors du paquet initial.
//
// Polices : celles qu'ont Word et LibreOffice (Calibri, remplacée par
// Carlito sous Linux ; Consolas pour le code), rien d'intégré. Couleurs de
// la direction visuelle de ChatonLM, sobres : texte brun foncé, titres de
// niveau 1 et liens en roux foncé, filets et fonds sable.
import {
  AlignmentType,
  BorderStyle,
  Document,
  ExternalHyperlink,
  Footer,
  HeadingLevel,
  LevelFormat,
  Packer,
  PageNumber,
  Paragraph,
  ShadingType,
  Table,
  TableCell,
  TableRow,
  TabStopType,
  TextRun,
  UnderlineType,
  WidthType,
  type ILevelsOptions,
  type IParagraphOptions,
  type ParagraphChild,
} from "docx";
import type { Align, Block, DocumentHead, Inline } from "./blocks";

/** Couleurs de la direction visuelle (.tools/research/75), sans « # ». */
const COLORS = {
  ink: "2A1A10",
  body: "382820",
  muted: "6B5647",
  faint: "8E7763",
  accent: "F28A38",
  accentText: "B84A12",
  sand: "F6E4D0",
  surface: "FBF1E4",
  line: "E8D2BA",
};

const SANS = "Calibri";
const MONO = "Consolas";

/** A4 et marges de 2 cm, en vingtièmes de point. */
const PAGE = { width: 11906, height: 16838, margin: 1134 };
const CONTENT_WIDTH = PAGE.width - 2 * PAGE.margin;

/** Retrait d'un niveau de liste ou de citation. */
const INDENT = 360;

const BULLETS = ["\u2022", "\u25E6", "\u25AA"];

/**
 * Niveaux d'une liste (9, le maximum de Word) ; `start` : premier numéro du
 * niveau `first`, celui de la liste (une sous-liste a sa propre définition).
 */
function levels(ordered: boolean, start: number, first = 0): ILevelsOptions[] {
  return Array.from({ length: 9 }, (_, level) => ({
    level,
    format: ordered ? LevelFormat.DECIMAL : LevelFormat.BULLET,
    text: ordered ? `%${level + 1}.` : BULLETS[level % BULLETS.length],
    alignment: AlignmentType.LEFT,
    start: ordered && level === first ? start : 1,
    style: {
      paragraph: {
        indent: { left: INDENT * (level + 1), hanging: INDENT },
      },
      run: { color: COLORS.accentText },
    },
  }));
}

/** Contexte d'un bloc : profondeur de liste et de citation. */
interface Context {
  /** Profondeur de liste (0 : hors liste). */
  list: number;
  /** Profondeur de citation. */
  quote: number;
}

const TOP: Context = { list: 0, quote: 0 };

/** Conversion d'un document : les listes numérotées rencontrées. */
class Writer {
  /** Définitions des listes, une par liste numérotée (sa propre suite). */
  readonly numbering: { reference: string; levels: ILevelsOptions[] }[] = [
    { reference: "bullets", levels: levels(false, 1) },
  ];

  /** Morceaux de texte d'un paragraphe. */
  runs(content: Inline[], base: { color?: string } = {}): ParagraphChild[] {
    return content
      .map((piece) => {
        const lines = piece.text.split("\n");
        const options = {
          bold: piece.bold,
          italics: piece.italic,
          strike: piece.strike,
          color: piece.link ? COLORS.accentText : base.color,
          ...(piece.link && { underline: { type: UnderlineType.SINGLE } }),
          ...(piece.code && {
            font: MONO,
            size: 19,
            shading: {
              type: ShadingType.CLEAR,
              fill: COLORS.sand,
              color: "auto",
            },
          }),
        };
        const children = lines.map(
          (line, i) =>
            new TextRun({ ...options, text: line, break: i > 0 ? 1 : 0 }),
        );
        if (piece.link)
          return new ExternalHyperlink({ link: piece.link, children });
        return children;
      })
      .flat();
  }

  /** Retrait gauche d'un bloc qui n'est pas un élément de liste. */
  private indent(context: Context): number {
    return INDENT * (context.list + context.quote);
  }

  /** Filet roux d'une citation. */
  private quoteBorder(context: Context): IParagraphOptions["border"] {
    return context.quote > 0
      ? {
          left: {
            style: BorderStyle.SINGLE,
            size: 12,
            color: COLORS.accent,
            space: 8,
          },
        }
      : undefined;
  }

  blocks(blocks: Block[], context: Context): (Paragraph | Table)[] {
    return blocks.flatMap((block) => this.block(block, context));
  }

  private block(block: Block, context: Context): (Paragraph | Table)[] {
    const indent = this.indent(context);
    const quoted = context.quote > 0;
    const color = quoted ? COLORS.muted : undefined;
    switch (block.type) {
      case "heading":
        return [
          new Paragraph({
            heading: [
              HeadingLevel.HEADING_1,
              HeadingLevel.HEADING_2,
              HeadingLevel.HEADING_3,
            ][block.level - 1],
            children: this.runs(block.content),
            ...(indent > 0 && { indent: { left: indent } }),
          }),
        ];
      case "paragraph":
        return [
          new Paragraph({
            children: this.runs(block.content, { color }),
            ...(indent > 0 && { indent: { left: indent } }),
            border: this.quoteBorder(context),
          }),
        ];
      case "code":
        return [
          new Paragraph({
            children: block.text.split("\n").map(
              (line, i) =>
                new TextRun({
                  text: line,
                  font: MONO,
                  size: 18,
                  color: COLORS.ink,
                  break: i > 0 ? 1 : 0,
                }),
            ),
            shading: {
              type: ShadingType.CLEAR,
              fill: COLORS.surface,
              color: "auto",
            },
            border: {
              top: {
                style: BorderStyle.SINGLE,
                size: 4,
                color: COLORS.line,
                space: 4,
              },
              bottom: {
                style: BorderStyle.SINGLE,
                size: 4,
                color: COLORS.line,
                space: 4,
              },
              left: {
                style: BorderStyle.SINGLE,
                size: 4,
                color: COLORS.line,
                space: 4,
              },
              right: {
                style: BorderStyle.SINGLE,
                size: 4,
                color: COLORS.line,
                space: 4,
              },
            },
            spacing: { before: 60, after: 160, line: 260 },
            indent: { left: indent + 80, right: 80 },
          }),
        ];
      case "list":
        return this.list(block, context);
      case "quote":
        return this.blocks(block.blocks, {
          ...context,
          quote: context.quote + 1,
        });
      case "rule":
        return [
          new Paragraph({
            children: [],
            border: {
              bottom: {
                style: BorderStyle.SINGLE,
                size: 6,
                color: COLORS.line,
                space: 1,
              },
            },
            spacing: { before: 120, after: 240 },
          }),
        ];
      case "table":
        return [this.table(block, indent)];
    }
  }

  private list(block: Extract<Block, { type: "list" }>, context: Context) {
    const level = Math.min(context.list, 8);
    // Chaque liste numérotée a sa définition : elle part de son numéro.
    let reference = "bullets";
    if (block.ordered) {
      reference = `ordered-${this.numbering.length}`;
      this.numbering.push({
        reference,
        levels: levels(true, block.start, level),
      });
    }
    const inner: Context = { ...context, list: context.list + 1 };
    return block.items.flatMap((item) => {
      const [first, ...rest] = item.blocks;
      const box =
        item.checked === undefined
          ? []
          : [new TextRun({ text: item.checked ? "[x] " : "[ ] ", font: MONO })];
      const head =
        first?.type === "paragraph" || first?.type === "heading"
          ? new Paragraph({
              numbering: { reference, level },
              children: [...box, ...this.runs(first.content)],
              spacing: { after: 60 },
              border: this.quoteBorder(context),
            })
          : new Paragraph({
              numbering: { reference, level },
              children: box,
              spacing: { after: 60 },
            });
      const others =
        first?.type === "paragraph" || first?.type === "heading"
          ? rest
          : item.blocks;
      return [head, ...this.blocks(others, inner)];
    });
  }

  private table(
    block: Extract<Block, { type: "table" }>,
    indent: number,
  ): Table {
    const columns = Math.max(
      block.header.length,
      ...block.rows.map((row) => row.length),
    );
    const width = Math.floor((CONTENT_WIDTH - indent) / Math.max(columns, 1));
    const border = { style: BorderStyle.SINGLE, size: 4, color: COLORS.line };
    const cell = (
      pieces: Inline[] | undefined,
      column: number,
      header: boolean,
    ) =>
      new TableCell({
        width: { size: width, type: WidthType.DXA },
        margins: { top: 60, bottom: 60, left: 100, right: 100 },
        ...(header && {
          shading: {
            type: ShadingType.CLEAR,
            fill: COLORS.sand,
            color: "auto",
          },
        }),
        children: [
          new Paragraph({
            alignment: ALIGNMENTS[block.align[column] ?? "left"],
            spacing: { after: 0 },
            children: this.runs(
              (pieces ?? []).map((piece) =>
                header ? { ...piece, bold: true } : piece,
              ),
            ),
          }),
        ],
      });
    return new Table({
      width: { size: width * columns, type: WidthType.DXA },
      columnWidths: Array.from({ length: columns }, () => width),
      indent: { size: indent, type: WidthType.DXA },
      borders: {
        top: border,
        bottom: border,
        left: border,
        right: border,
        insideHorizontal: border,
        insideVertical: border,
      },
      rows: [
        new TableRow({
          tableHeader: true,
          children: Array.from({ length: columns }, (_, i) =>
            cell(block.header[i], i, true),
          ),
        }),
        ...block.rows.map(
          (row) =>
            new TableRow({
              children: Array.from({ length: columns }, (_, i) =>
                cell(row[i], i, false),
              ),
            }),
        ),
      ],
    });
  }
}

const ALIGNMENTS: Record<
  NonNullable<Align>,
  (typeof AlignmentType)[keyof typeof AlignmentType]
> = {
  left: AlignmentType.LEFT,
  center: AlignmentType.CENTER,
  right: AlignmentType.RIGHT,
};

/** Document docx complet : en-tête, corps, pied de page. */
export function docxDocument(head: DocumentHead, blocks: Block[]): Document {
  const writer = new Writer();
  const body = writer.blocks(blocks, TOP);
  const top: Paragraph[] = [];
  if (head.title) {
    top.push(
      new Paragraph({
        heading: HeadingLevel.TITLE,
        children: [new TextRun(head.title)],
      }),
    );
  }
  top.push(
    new Paragraph({
      children: [
        new TextRun({ text: head.date, color: COLORS.muted, size: 19 }),
      ],
      border: {
        bottom: {
          style: BorderStyle.SINGLE,
          size: 12,
          color: COLORS.accent,
          space: 8,
        },
      },
      spacing: { after: 360 },
    }),
  );
  const footer = new Footer({
    children: [
      new Paragraph({
        tabStops: [{ type: TabStopType.RIGHT, position: CONTENT_WIDTH }],
        children: [
          new TextRun({ text: head.footer, color: COLORS.faint, size: 16 }),
          new TextRun({
            children: ["\t", PageNumber.CURRENT, " / ", PageNumber.TOTAL_PAGES],
            color: COLORS.faint,
            size: 16,
          }),
        ],
      }),
    ],
  });
  return new Document({
    creator: head.footer,
    title: head.title ?? head.date,
    styles: {
      default: {
        document: {
          run: {
            font: SANS,
            size: 22,
            color: COLORS.body,
            language: { value: head.lang },
          },
          paragraph: { spacing: { after: 140, line: 288 } },
        },
        title: {
          run: { font: SANS, size: 48, bold: true, color: COLORS.ink },
          paragraph: { spacing: { after: 80 } },
        },
        heading1: {
          run: { font: SANS, size: 34, bold: true, color: COLORS.accentText },
          paragraph: { spacing: { before: 320, after: 120 }, keepNext: true },
        },
        heading2: {
          run: { font: SANS, size: 27, bold: true, color: COLORS.ink },
          paragraph: { spacing: { before: 260, after: 100 }, keepNext: true },
        },
        heading3: {
          run: { font: SANS, size: 23, bold: true, color: COLORS.muted },
          paragraph: { spacing: { before: 220, after: 80 }, keepNext: true },
        },
      },
    },
    numbering: { config: writer.numbering },
    sections: [
      {
        properties: {
          page: {
            size: { width: PAGE.width, height: PAGE.height },
            margin: {
              top: PAGE.margin,
              bottom: PAGE.margin,
              left: PAGE.margin,
              right: PAGE.margin,
            },
          },
        },
        footers: { default: footer },
        children: [...top, ...body],
      },
    ],
  });
}

/** Octets du .docx d'un document. */
export async function renderDocx(
  head: DocumentHead,
  blocks: Block[],
): Promise<Uint8Array> {
  const buffer = await Packer.toArrayBuffer(docxDocument(head, blocks));
  return new Uint8Array(buffer);
}
