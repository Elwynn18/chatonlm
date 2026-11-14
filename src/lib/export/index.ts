// Exports d'un document Markdown en .md, .docx et .pdf (propre à ChatonLM ;
// outil Transcription, puis toute page qui produit du Markdown : fiche d'un
// cours de l'onglet Study...). Point d'entrée léger : marked est déjà dans
// le paquet initial, docx et pdfmake (et les polices du PDF) ne sont chargés
// par import() qu'au premier export de leur format.
//
//   const bytes = await exportDocument(markdown, "pdf", t, { title });
//   const path = await saveDocument(markdown, "pdf", t, { title });
//
// Enregistrement : commande Rust `save_document` (src/lib/ipc.ts,
// `saveExport`), boîte « Enregistrer sous » native, un seul chemin pour les
// trois formats, sous Linux comme sous Windows.
import type { TFunction } from "@/i18n";
import { localized } from "@/i18n";
import { saveExport, type ExportKind } from "@/lib/ipc";
import { type DocumentHead, parseMarkdown, splitTitle } from "./blocks";

export type { ExportKind };

/** Raisons d'un export refusé. */
export type ExportErrorCode = "unsupportedCharacters";

/**
 * Export refusé avant d'avoir rien produit, avec un message déjà traduit que
 * la page peut montrer tel quel. `unsupportedCharacters` : le document a des
 * caractères que les polices du PDF n'ont pas (`characters`) ; le .docx, qui
 * prend les polices du système, les garde.
 */
export class ExportError extends Error {
  constructor(
    readonly code: ExportErrorCode,
    message: string,
    readonly characters: readonly string[] = [],
  ) {
    super(message);
    this.name = "ExportError";
  }
}

/** Caractères cités au plus dans le message d'un document refusé. */
const SHOWN_CHARACTERS = 5;

/** Formats proposés, dans l'ordre d'un menu. */
export const EXPORT_KINDS: readonly ExportKind[] = ["md", "docx", "pdf"];

export interface ExportOptions {
  /**
   * Titre en tête (.docx, .pdf). Absent : le titre de niveau 1 qui ouvre le
   * document, retiré du corps ; sinon pas de titre.
   */
  title?: string;
  /** Date en tête ; maintenant par défaut. */
  date?: Date;
}

/** Date en tête d'un document : « 14 novembre 2026 ». */
export function exportDate(t: TFunction, date: Date): string {
  return localized(
    new Intl.DateTimeFormat(t.lang, { dateStyle: "long" }).format(date),
  );
}

/**
 * Octets du document dans le format demandé. `.md` : le Markdown tel quel,
 * en UTF-8 ; `.docx` et `.pdf` : titre et date en tête, pied de page
 * « ChatonLM » et numéros de page. Un .pdf dont un caractère manque aux
 * polices intégrées est refusé par une `ExportError` (rien n'est produit).
 */
export async function exportDocument(
  markdown: string,
  kind: ExportKind,
  t: TFunction,
  options: ExportOptions = {},
): Promise<Uint8Array> {
  if (kind === "md") return new TextEncoder().encode(markdown);
  const { title, body } = splitTitle(parseMarkdown(markdown), options.title);
  const head: DocumentHead = {
    title,
    date: exportDate(t, options.date ?? new Date()),
    footer: t("common.export.footer"),
    lang: t.lang,
  };
  if (kind === "docx") {
    const { renderDocx } = await import("./docx");
    return renderDocx(head, body);
  }
  const { missingCharacters, renderPdf } = await import("./pdf");
  const missing = missingCharacters(head, body);
  if (missing.length > 0) {
    const shown = missing.slice(0, SHOWN_CHARACTERS).join(" ");
    throw new ExportError(
      "unsupportedCharacters",
      t("common.export.unsupportedCharacters", {
        characters: missing.length > SHOWN_CHARACTERS ? `${shown} …` : shown,
      }),
      missing,
    );
  }
  return renderPdf(head, body);
}

/**
 * Nom proposé dans la boîte « Enregistrer sous », sans extension : `name`,
 * sinon le titre (donné, ou le titre de niveau 1 du document), sinon
 * « Document » dans la langue de l'interface. Le moteur retire les caractères
 * qu'un nom de fichier ne peut pas porter et ajoute l'extension.
 */
export function suggestedName(
  markdown: string,
  t: TFunction,
  options: ExportOptions & { name?: string } = {},
): string {
  const given = options.name?.trim() || options.title?.trim();
  if (given) return given;
  const { title } = splitTitle(parseMarkdown(markdown.slice(0, 2000)));
  return title ?? t("common.export.defaultName");
}

/**
 * Exporte puis enregistre le document là où l'utilisateur le choisit ;
 * chemin écrit, ou null s'il annule la boîte.
 */
export async function saveDocument(
  markdown: string,
  kind: ExportKind,
  t: TFunction,
  options: ExportOptions & { name?: string } = {},
): Promise<string | null> {
  const bytes = await exportDocument(markdown, kind, t, options);
  return saveExport(bytes, kind, suggestedName(markdown, t, options));
}
