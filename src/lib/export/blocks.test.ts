import { describe, expect, it } from "vitest";
import {
  type Block,
  decodeEntities,
  parseMarkdown,
  plainText,
  splitTitle,
} from "./blocks";

describe("parseMarkdown", () => {
  it("reads headings 1 to 3, deeper ones becoming level 3", () => {
    const blocks = parseMarkdown("# Un\n\n## Deux\n\n### Trois\n\n#### Quatre");
    expect(
      blocks.map((b) =>
        b.type === "heading" ? [b.level, plainText(b.content)] : b.type,
      ),
    ).toEqual([
      [1, "Un"],
      [2, "Deux"],
      [3, "Trois"],
      [3, "Quatre"],
    ]);
  });

  it("keeps bold, italic, strike, inline code and links with nested marks", () => {
    const [paragraph] = parseMarkdown(
      "Du **gras _et italique_** puis `code` ~~barré~~ et [un **lien**](https://chatonlm.com).",
    );
    expect(paragraph).toEqual({
      type: "paragraph",
      content: [
        { text: "Du " },
        { text: "gras ", bold: true },
        { text: "et italique", bold: true, italic: true },
        { text: " puis " },
        { text: "code", code: true },
        { text: " " },
        { text: "barré", strike: true },
        { text: " et " },
        { text: "un ", link: "https://chatonlm.com" },
        { text: "lien", bold: true, link: "https://chatonlm.com" },
        { text: "." },
      ],
    });
  });

  it("decodes entities and escapes, keeps raw HTML as text and line breaks", () => {
    const [paragraph] = parseMarkdown(
      "a \\*b\\* &amp; &#233;t&eacute; <b>x</b>  \nligne",
    );
    expect(paragraph.type === "paragraph" && plainText(paragraph.content)).toBe(
      "a *b* & ét&eacute; <b>x</b>\nligne",
    );
  });

  it("reads nested bullet and numbered lists, their start and task boxes", () => {
    const blocks = parseMarkdown(
      "- un\n  - deux\n    3. trois\n    4. quatre\n- [x] fait\n- [ ] à faire\n\n7. sept\n8. huit",
    );
    const text = (block: Block) =>
      block.type === "paragraph" ? plainText(block.content) : block.type;
    expect(blocks).toHaveLength(2);
    const [bullets, numbered] = blocks;
    if (bullets.type !== "list" || numbered.type !== "list") throw new Error();
    expect(bullets.ordered).toBe(false);
    expect(bullets.items.map((item) => item.blocks.map(text))).toEqual([
      ["un", "list"],
      ["fait"],
      ["à faire"],
    ]);
    expect(bullets.items.map((item) => item.checked)).toEqual([
      undefined,
      true,
      false,
    ]);
    const inner = bullets.items[0].blocks[1];
    if (inner.type !== "list") throw new Error();
    expect(inner.items[0].blocks.map(text)).toEqual(["deux", "list"]);
    const deepest = inner.items[0].blocks[1];
    expect(deepest).toMatchObject({ type: "list", ordered: true, start: 3 });
    expect(numbered).toMatchObject({ ordered: true, start: 7 });
    expect(numbered.items.map((item) => item.blocks.map(text))).toEqual([
      ["sept"],
      ["huit"],
    ]);
  });

  it("reads code blocks, quotes (nested), rules and tables", () => {
    const blocks = parseMarkdown(
      '```rust\nfn main() {\n    println!("é");\n}\n```\n\n> cité\n>\n> > encore\n\n---\n\n| Nom | Prix |\n|:--|--:|\n| thé | **3 €** |\n| café |',
    );
    expect(blocks.map((b) => b.type)).toEqual([
      "code",
      "quote",
      "rule",
      "table",
    ]);
    expect(blocks[0]).toEqual({
      type: "code",
      lang: "rust",
      text: 'fn main() {\n    println!("é");\n}',
    });
    const quote = blocks[1];
    if (quote.type !== "quote") throw new Error();
    expect(quote.blocks.map((b) => b.type)).toEqual(["paragraph", "quote"]);
    expect(blocks[3]).toEqual({
      type: "table",
      align: ["left", "right"],
      header: [[{ text: "Nom" }], [{ text: "Prix" }]],
      rows: [
        [[{ text: "thé" }], [{ text: "3 €", bold: true }]],
        [[{ text: "café" }], []],
      ],
    });
  });

  it("turns an image into its alternative text and drops control characters", () => {
    const [paragraph] = parseMarkdown(
      "Voir ![le chaton](chat.png)\u0007 ici\r\n",
    );
    expect(paragraph.type === "paragraph" && plainText(paragraph.content)).toBe(
      "Voir le chaton ici",
    );
  });
});

describe("splitTitle", () => {
  it("takes the leading level 1 heading out of the body", () => {
    const { title, body } = splitTitle(parseMarkdown("# Réunion\n\nTexte"));
    expect(title).toBe("Réunion");
    expect(body.map((b) => b.type)).toEqual(["paragraph"]);
  });

  it("keeps the body whole when a title is given or none leads", () => {
    expect(splitTitle(parseMarkdown("# H\n\nT"), "Donné").body).toHaveLength(2);
    expect(splitTitle(parseMarkdown("## H2\n\nT"))).toMatchObject({
      title: null,
    });
  });
});

describe("decodeEntities", () => {
  it("decodes named and numeric entities and leaves unknown ones", () => {
    expect(decodeEntities("&lt;&#x153;&#241;&nbsp;&zz;")).toBe("<œñ &zz;");
  });
});
