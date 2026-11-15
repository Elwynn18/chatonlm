import { unzlibSync } from "fflate";
import { describe, expect, it, vi } from "vitest";
import { parseMarkdown, splitTitle } from "./blocks";
import { fontSafe, missingCharacters, renderPdf, segments } from "./pdf";

vi.setConfig({ testTimeout: 30_000 });

/** Objets d'un PDF : dictionnaire et flux (décompressé). */
function objects(
  bytes: Uint8Array,
): Map<number, { dict: string; stream?: Uint8Array }> {
  const latin1 = new TextDecoder("latin1").decode(bytes);
  const out = new Map<number, { dict: string; stream?: Uint8Array }>();
  for (const match of latin1.matchAll(/(\d+) 0 obj\s*([\s\S]*?)endobj/g)) {
    const body = match[2];
    const at = body.indexOf("stream");
    if (at < 0) {
      out.set(Number(match[1]), { dict: body });
      continue;
    }
    const start =
      (match.index ?? 0) + match[0].indexOf(body) + at + "stream".length;
    const begin = latin1[start] === "\r" ? start + 2 : start + 1;
    const end = latin1.indexOf("endstream", begin);
    let stream = bytes.subarray(begin, end);
    if (body.slice(0, at).includes("/FlateDecode")) {
      // Fin de ligne avant endstream comprise : unzlib s'arrête à la fin des données.
      stream = unzlibSync(stream);
    }
    out.set(Number(match[1]), { dict: body.slice(0, at), stream });
  }
  return out;
}

/** Table glyphe → texte d'un CMap ToUnicode (bfchar et bfrange). */
function toUnicode(cmap: string): Map<number, string> {
  const map = new Map<number, string>();
  const utf16 = (hex: string) =>
    String.fromCharCode(
      ...(hex.match(/.{4}/g) ?? []).map((h) => parseInt(h, 16)),
    );
  for (const [, body] of cmap.matchAll(/beginbfchar([\s\S]*?)endbfchar/g)) {
    for (const [, gid, uni] of body.matchAll(
      /<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>/g,
    )) {
      map.set(parseInt(gid, 16), utf16(uni));
    }
  }
  for (const [, body] of cmap.matchAll(/beginbfrange([\s\S]*?)endbfrange/g)) {
    for (const [, from, , list] of body.matchAll(
      /<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>\s*\[([^\]]*)\]/g,
    )) {
      [...list.matchAll(/<([0-9a-fA-F]+)>/g)].forEach(([, uni], i) =>
        map.set(parseInt(from, 16) + i, utf16(uni)),
      );
    }
  }
  return map;
}

/**
 * Texte d'un PDF de pdfkit, comme un lecteur le retrouve : chaque police
 * Type0 décodée par son CMap ToUnicode, chaque TJ des pages lu avec la police
 * du dernier Tf, les TJ séparés par une espace, les blancs réduits à une
 * espace (un mot suivi d'une ponctuation dans un autre style en garde une).
 */
function pdfText(bytes: Uint8Array): string {
  const objs = objects(bytes);
  const text = (id: number) =>
    new TextDecoder("latin1").decode(objs.get(id)?.stream);
  const cmaps = new Map<number, Map<number, string>>();
  for (const [id, obj] of objs) {
    const ref = /\/ToUnicode (\d+) 0 R/.exec(obj.dict);
    if (ref) cmaps.set(id, toUnicode(text(Number(ref[1]))));
  }
  const parts: string[] = [];
  for (const obj of objs.values()) {
    if (!/\/Type \/Page\b/.test(obj.dict)) continue;
    const fonts = new Map<string, number>();
    const resources = /\/Resources (\d+) 0 R/.exec(obj.dict);
    const resourceDict = resources
      ? (objs.get(Number(resources[1]))?.dict ?? "")
      : obj.dict;
    const fontDict = /\/Font <<([\s\S]*?)>>/.exec(resourceDict)?.[1] ?? "";
    for (const [, name, id] of fontDict.matchAll(/\/(\w+) (\d+) 0 R/g)) {
      fonts.set(name, Number(id));
    }
    const contents = /\/Contents (\d+) 0 R/.exec(obj.dict);
    if (!contents) continue;
    let cmap = new Map<number, string>();
    for (const op of text(Number(contents[1])).matchAll(
      /\/(\w+) [\d.]+ Tf|\[([^\]]*)\] TJ/g,
    )) {
      if (op[1]) {
        cmap = cmaps.get(fonts.get(op[1]) ?? -1) ?? new Map();
        continue;
      }
      const glyphs = [...op[2].matchAll(/<([0-9a-fA-F]+)>/g)].flatMap(
        ([, hex]) =>
          (hex.match(/.{4}/g) ?? []).map(
            (h) => cmap.get(parseInt(h, 16)) ?? "�",
          ),
      );
      parts.push(glyphs.join(""));
    }
  }
  return parts.join(" ").replace(/\s+/g, " ");
}

const HEAD = {
  title: "Réunion du comité",
  date: "14 de noviembre de 2026",
  footer: "ChatonLM",
  lang: "es",
};

async function render(markdown: string) {
  return renderPdf(HEAD, splitTitle(parseMarkdown(markdown)).body);
}

describe("renderPdf", () => {
  it("writes a PDF with its four styles and the code font embedded, text readable", async () => {
    const bytes = await render(
      "## Œuvres et señales\n\nTexte **gras**, _italique_, ***les deux*** : « élève », l’été, ¿Qué tal?, ¡Sí!, ñandú, Ça.\n\n- puce\n  1. numéro\n\n`code`\n\n> cité\n\n| A | B |\n|---|---|\n| é | ñ |\n",
    );
    const latin1 = new TextDecoder("latin1").decode(bytes);
    expect(latin1.startsWith("%PDF-")).toBe(true);
    expect(latin1.trimEnd().endsWith("%%EOF")).toBe(true);
    // Polices intégrées (sous-ensembles), aucune police standard.
    const embedded = [
      ...latin1.matchAll(/\/FontName \/[A-Z]{6}\+([\w-]+)/g),
    ].map((m) => m[1]);
    expect(new Set(embedded)).toEqual(
      new Set([
        "Figtree-Regular",
        "Figtree-Bold",
        "Figtree-Italic",
        "Figtree-BoldItalic",
        "JetBrainsMono-Regular",
      ]),
    );
    expect(latin1.match(/\/FontFile2 \d+ 0 R/g)).toHaveLength(5);
    expect(latin1).not.toMatch(/\/BaseFont \/(Helvetica|Times|Courier)/);
    const text = pdfText(bytes);
    for (const words of [
      "Réunion",
      "comité",
      "14 de noviembre de 2026",
      "Œuvres",
      "señales",
      "gras",
      "italique",
      "«",
      "élève",
      "l’été",
      "¿Qué",
      "¡Sí!",
      "ñandú",
      "Ça.",
      "numéro",
      "code",
      "cité",
      "ChatonLM",
      "1 / 1",
    ]) {
      expect(text).toContain(words);
    }
    expect(text).not.toContain("�");
  });

  it("numbers the pages of a long document and keeps the metadata", async () => {
    const long = Array.from(
      { length: 120 },
      (_, i) => `Paragraphe ${i + 1}, assez long pour remplir.`,
    ).join("\n\n");
    const bytes = await render(long);
    const text = pdfText(bytes);
    const pages = Number(/(\d+) \/ (\d+)/.exec(text)?.[2]);
    expect(pages).toBeGreaterThan(1);
    expect(text).toContain(`${pages} / ${pages}`);
    expect(text).toContain("Paragraphe 120,");
    // Métadonnées : créateur ChatonLM, titre du document.
    const objs = objects(bytes);
    const info = (key: string) => {
      const ref = new RegExp(`/${key} (\\d+) 0 R`).exec(
        new TextDecoder("latin1").decode(bytes),
      );
      return objs.get(Number(ref?.[1]))?.dict.trim();
    };
    expect(info("Creator")).toBe("(ChatonLM)");
    expect(info("Title")).toMatch(/^\(.+\)$/);
  });
});

describe("fontSafe", () => {
  it("replaces the spaces and signs missing from the embedded fonts", () => {
    expect(
      fontSafe("Bonjour\u202F! a\u2009b c\u2011d e\u00ADf\u200Bg\tŸ"),
    ).toBe("Bonjour\u00A0! a b c-d efg    Ÿ");
    expect(fontSafe("« élève » — ñ")).toBe("« élève » — ñ");
  });

  it("writes the usual signs of a model's document with simple equivalents", () => {
    expect(fontSafe("Entrée → sortie ← ⇒ ↔ ➜ ↗")).toBe(
      "Entrée -> sortie <- => <-> -> \u2191",
    );
    expect(fontSafe("≥ 10 € ≤ 5 ≠ 3 ≈ 2")).toBe(">= 10 € <= 5 != 3 ~ 2");
    expect(fontSafe("▪ un ◦ deux ► trois ✓ fait ✗ non ☐ à faire")).toBe(
      "• un • deux • trois [x] fait × non [ ] à faire",
    );
    expect(fontSafe("❝ cité ❞ ★ ⅓ 20 ℃")).toBe("“ cité ” * 1/3 20 °C");
  });

  it("drops emoji, keeping one space between words, and keeps © ® ™", () => {
    expect(fontSafe("Done ✅ now")).toBe("Done [x] now");
    expect(fontSafe("🎉 Fête finie 🐱")).toBe("Fête finie");
    expect(fontSafe("Bravo 👍🏽 et 🇫🇷 France, chat🐱noir.")).toBe(
      "Bravo et France, chatnoir.",
    );
    expect(fontSafe("👨‍👩‍👧 famille")).toBe("famille");
    expect(fontSafe("1️⃣ un ⚠️ attention")).toBe("1 un (!) attention");
    expect(fontSafe("© ChatonLM® ™")).toBe("© ChatonLM® ™");
  });
});

describe("latin-ext", () => {
  it("splits the text between the latin and latin-ext fonts", () => {
    expect(segments("Łódź, été", "sans")).toEqual([
      { text: "Ł", font: "FigtreeExt" },
      { text: "ód", font: "Figtree" },
      { text: "ź", font: "FigtreeExt" },
      { text: ", été", font: "Figtree" },
    ]);
    expect(segments("x = ş;", "mono")).toEqual([
      { text: "x = ", font: "JetBrainsMono" },
      { text: "ş", font: "JetBrainsMonoExt" },
      { text: ";", font: "JetBrainsMono" },
    ]);
  });

  it("writes Central European text with the latin-ext fonts embedded", async () => {
    const markdown =
      "## Zażółć gęślą jaźń\n\nPříliš žluťoučký kůň, **Árvíztűrő tükörfúrógép**, _Şoseaua ţării_, Đurđevac, Ğğİı.\n\n`kod: łódź`";
    const blocks = splitTitle(parseMarkdown(markdown)).body;
    expect(missingCharacters(HEAD, blocks)).toEqual([]);
    const bytes = await renderPdf(HEAD, blocks);
    const latin1 = new TextDecoder("latin1").decode(bytes);
    const embedded = [
      ...latin1.matchAll(/\/FontName \/[A-Z]{6}\+([\w-]+)/g),
    ].map((m) => m[1]);
    // Les deux sous-ensembles portent le même nom PostScript : deux
    // exemplaires de chaque style employé.
    expect(embedded.filter((name) => name === "Figtree-Bold")).toHaveLength(2);
    expect(
      embedded.filter((name) => name === "JetBrainsMono-Regular"),
    ).toHaveLength(2);
    // Chaque changement de police ouvre un TJ, que pdfText sépare d'une
    // espace (un lecteur les place par leur position) : comparaison sans blancs.
    const text = pdfText(bytes).replace(/\s/g, "");
    for (const words of [
      "Zażółć gęślą jaźń",
      "Příliš",
      "žluťoučký",
      "kůň",
      "Árvíztűrő",
      "tükörfúrógép",
      "Şoseaua",
      "țării",
      "Đurđevac",
      "Ğğİı",
      "łódź",
    ]) {
      expect(text).toContain(words.replace(/\s/g, ""));
    }
    expect(text).not.toContain("\uFFFD");
  });

  it("lists the characters no embedded font has, in order, title and code included", () => {
    const blocks = splitTitle(
      parseMarkdown(
        "Привет, мир — ok\n\n- α\n\n| a |\n|---|\n| 漢 |\n\n```\n🐱\n```",
      ),
    ).body;
    expect(missingCharacters({ ...HEAD, title: "Ω" }, blocks)).toEqual([
      "Ω",
      "П",
      "р",
      "и",
      "в",
      "е",
      "т",
      "м",
      "α",
      "漢",
    ]);
    // Remplacements voisins : pas des manques.
    expect(
      missingCharacters(HEAD, parseMarkdown("a\u202Fb\u2011c\u00ADd")),
    ).toEqual([]);
  });

  it("links only to web addresses, never to a file or a network share", async () => {
    const blocks = parseMarkdown(
      "[voir](file:///C:/Windows/System32/calc.exe) [partage](file://attacker.example/share/x.docx) [web](https://a.example)",
    );
    const latin1 = new TextDecoder("latin1").decode(
      await renderPdf(HEAD, blocks),
    );
    const uris = [...latin1.matchAll(/\/URI \(([^)]*)\)/g)].map((m) => m[1]);
    expect(uris).toEqual(["https://a.example"]);
  });

  it("writes a typical summary (arrows, comparisons, emoji) as a PDF", async () => {
    const blocks = splitTitle(
      parseMarkdown(
        "# Synthèse\n\n## Points clés\n\n- Entrée → sortie\n- Prix ≥ 10 €\n- Fait ✅ 🎉\n\n`a → b`",
      ),
    ).body;
    expect(missingCharacters(HEAD, blocks)).toEqual([]);
    const text = pdfText(await renderPdf(HEAD, blocks)).replace(/\s/g, "");
    for (const words of ["Entrée->sortie", "Prix>=10€", "Fait[x]", "a->b"]) {
      expect(text).toContain(words);
    }
  });
});
