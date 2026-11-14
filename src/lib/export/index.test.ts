import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { translator } from "@/i18n";

vi.setConfig({ testTimeout: 30_000 });

// Modules de rendu remplacés : on observe quand ils sont chargés.
const loaded = vi.hoisted(() => ({ docx: 0, pdf: 0 }));
vi.mock("./docx", () => {
  loaded.docx += 1;
  return {
    renderDocx: vi.fn(async () => new Uint8Array([1])),
  };
});
vi.mock("./pdf", () => {
  loaded.pdf += 1;
  return {
    // Manque simulé : tout caractère cyrillique.
    missingCharacters: vi.fn(
      (head: { title: string | null }, blocks: unknown) => [
        ...new Set(
          `${head.title ?? ""}${JSON.stringify(blocks)}`.match(
            /[\u0400-\u04FF]/g,
          ),
        ),
      ],
    ),
    renderPdf: vi.fn(async () => new Uint8Array([2])),
  };
});
const saveExport = vi.hoisted(() => vi.fn(async () => "/tmp/x"));
vi.mock("@/lib/ipc", () => ({ saveExport }));

const t = translator("fr");

describe("exportDocument", () => {
  beforeEach(() => {
    saveExport.mockClear();
  });

  it("loads docx and pdfmake only at the first export of their format", async () => {
    const { exportDocument } = await import("./index");
    expect(loaded).toEqual({ docx: 0, pdf: 0 });
    await exportDocument("# T", "md", t);
    expect(loaded).toEqual({ docx: 0, pdf: 0 });
    await exportDocument("# T", "docx", t);
    expect(loaded).toEqual({ docx: 1, pdf: 0 });
    await exportDocument("# T", "pdf", t);
    await exportDocument("# T", "pdf", t);
    expect(loaded).toEqual({ docx: 1, pdf: 1 });
  });

  it("never imports the rendering libraries statically", () => {
    const source = readFileSync(
      path.join(import.meta.dirname, "index.ts"),
      "utf8",
    );
    const statics = [...source.matchAll(/^import[^;]*from\s+"([^"]+)"/gm)].map(
      (m) => m[1],
    );
    expect(statics).not.toEqual(
      expect.arrayContaining([expect.stringMatching(/docx|pdf/)]),
    );
  });

  it("returns the Markdown unchanged in UTF-8 for .md", async () => {
    const { exportDocument } = await import("./index");
    const bytes = await exportDocument("# Été\n\n« œuvre »", "md", t);
    expect(new TextDecoder().decode(bytes)).toBe("# Été\n\n« œuvre »");
  });

  it("gives the renderers the title, the date in the interface language and the footer", async () => {
    const { exportDocument } = await import("./index");
    const { renderPdf } = await import("./pdf");
    await exportDocument("# Réunion\n\nTexte", "pdf", t, {
      date: new Date(2026, 10, 14),
    });
    expect(vi.mocked(renderPdf).mock.lastCall).toEqual([
      {
        title: "Réunion",
        date: "14 novembre 2026",
        footer: "ChatonLM",
        lang: "fr",
      },
      [{ type: "paragraph", content: [{ text: "Texte" }] }],
    ]);
    const { renderDocx } = await import("./docx");
    await exportDocument("Texte", "docx", translator("es"), {
      title: "Acta",
      date: new Date(2026, 10, 14),
    });
    expect(vi.mocked(renderDocx).mock.lastCall?.[0]).toEqual({
      title: "Acta",
      date: "14 de noviembre de 2026",
      footer: "ChatonLM",
      lang: "es",
    });
  });

  it("saves through the backend with the proposed name", async () => {
    const { saveDocument, suggestedName } = await import("./index");
    expect(await saveDocument("# Réunion\n\nTexte", "pdf", t)).toBe("/tmp/x");
    expect(saveExport).toHaveBeenLastCalledWith(
      new Uint8Array([2]),
      "pdf",
      "Réunion",
    );
    expect(suggestedName("Texte", t)).toBe("Document");
    expect(suggestedName("Texte", translator("es"))).toBe("Documento");
    expect(suggestedName("# H", t, { title: "Titre", name: " Nom " })).toBe(
      "Nom",
    );
    expect(suggestedName("# H", t, { title: "Titre" })).toBe("Titre");
  });

  it("refuses a PDF whose characters the fonts lack, before producing or saving anything", async () => {
    const { exportDocument, ExportError, saveDocument } =
      await import("./index");
    const { renderPdf } = await import("./pdf");
    vi.mocked(renderPdf).mockClear();
    const error = await exportDocument(
      "# Отчёт\n\nТекст",
      "pdf",
      translator("en"),
    ).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ExportError);
    expect(error).toMatchObject({
      code: "unsupportedCharacters",
      characters: ["О", "т", "ч", "ё", "Т", "е", "к", "с"],
      message:
        "This document uses characters the PDF font can't show (О т ч ё Т …). Export it as .docx instead.",
    });
    expect(renderPdf).not.toHaveBeenCalled();
    await expect(saveDocument("Привет", "pdf", t)).rejects.toThrow(
      "Ce document contient des caractères que la police du PDF ne peut pas afficher (П р и в е …). Exportez-le plutôt en .docx.",
    );
    expect(saveExport).not.toHaveBeenCalled();
    // Le .docx, lui, est produit.
    await expect(exportDocument("Привет", "docx", t)).resolves.toBeInstanceOf(
      Uint8Array,
    );
  });
});
