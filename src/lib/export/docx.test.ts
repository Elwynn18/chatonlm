import { strFromU8, unzipSync } from "fflate";
import { describe, expect, it, vi } from "vitest";
import { parseMarkdown, splitTitle } from "./blocks";
import { renderDocx } from "./docx";

vi.setConfig({ testTimeout: 30_000 });

const MARKDOWN = `# Compte rendu

## Première partie

Un **passage en gras** et _en italique_, « guillemets » et l’apostrophe : œuvre, ñandú, ¿qué?

- puce
  - sous-puce
    1. numéro

3. trois
4. quatre

> Citation

| A | B |
|---|---|
| 1 | 2 |

\`\`\`
let x = 1;
\`\`\`
`;

const HEAD = {
  title: "Compte rendu",
  date: "14 novembre 2026",
  footer: "ChatonLM",
  lang: "fr",
};

async function unzip(markdown: string) {
  const bytes = await renderDocx(
    HEAD,
    splitTitle(parseMarkdown(markdown)).body,
  );
  const files = unzipSync(bytes);
  return { bytes, files, text: (name: string) => strFromU8(files[name]) };
}

describe("renderDocx", () => {
  it("writes a Word document whose XML has the headings, lists, bold and accents", async () => {
    const { bytes, files, text } = await unzip(MARKDOWN);
    expect(strFromU8(bytes.subarray(0, 2))).toBe("PK");
    expect(Object.keys(files)).toEqual(
      expect.arrayContaining([
        "[Content_Types].xml",
        "word/document.xml",
        "word/numbering.xml",
        "word/styles.xml",
      ]),
    );
    const xml = text("word/document.xml");
    // Titre et date en tête, puis le corps (titre de niveau 1 retiré).
    expect(xml).toMatch(/<w:pStyle w:val="Title"\/>[\s\S]*?Compte rendu/);
    expect(xml.match(/Compte rendu/g)).toHaveLength(1);
    expect(xml).toContain("14 novembre 2026");
    expect(xml).toMatch(/<w:pStyle w:val="Heading2"\/>[\s\S]*?Première partie/);
    // Gras : un run marqué <w:b/> qui porte le texte.
    expect(xml).toMatch(/<w:b\/>(?:(?!<\/w:r>)[\s\S])*passage en gras/);
    expect(xml).toMatch(/<w:i\/>(?:(?!<\/w:r>)[\s\S])*en italique/);
    for (const word of [
      "« guillemets »",
      "l’apostrophe",
      "œuvre",
      "ñandú",
      "¿qué?",
    ]) {
      expect(xml).toContain(word);
    }
    // Listes : trois niveaux, la liste numérotée a sa propre définition.
    const levels = [...xml.matchAll(/<w:ilvl w:val="(\d)"\/>/g)].map(
      (m) => m[1],
    );
    expect(levels).toEqual(["0", "1", "2", "0", "0"]);
    const numbering = text("word/numbering.xml");
    expect(numbering).toContain('w:val="decimal"');
    expect(numbering).toContain('<w:start w:val="3"/>');
    expect(xml).toContain("<w:tbl>");
    // Pied de page : ChatonLM et numéros de page.
    const footer = Object.keys(files).find((name) =>
      /^word\/footer\d*\.xml$/.test(name),
    );
    expect(footer).toBeDefined();
    expect(text(footer as string)).toContain("ChatonLM");
    expect(text(footer as string)).toContain("NUMPAGES");
  });

  it("starts each numbered list at its own first number", async () => {
    const { text } = await unzip("1. a\n2. b\n\nTexte\n\n5. c\n");
    const numbering = text("word/numbering.xml");
    expect(numbering).toContain('<w:start w:val="1"/>');
    expect(numbering).toContain('<w:start w:val="5"/>');
    const ids = [
      ...text("word/document.xml").matchAll(/<w:numId w:val="(\d+)"\/>/g),
    ].map((m) => m[1]);
    expect(new Set(ids).size).toBe(2);
  });
});
