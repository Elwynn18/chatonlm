// @vitest-environment node
// Correction des textes (lot 4a, 4) : commentaires extraits par langage
// (chaînes respectées), marques gardées ; jamais le code (passage écarté s'il
// changerait autre chose) ; Markdown sans ses blocs de code, liens et code en
// ligne protégés ; vue de différences, puis rien d'écrit sans « Apply ».
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  codeLines,
  commentRegions,
  keepsMarkdown,
  onlyComments,
  plainCode,
  proseSegments,
  rebuildRegion,
  regionBody,
  regionText,
  scanComments,
  skipRegion,
  syntaxFor,
  type Syntax,
} from "./proofread";
import { until } from "./test/fakeBridge";
import { cleanUp, openEditor, setupController } from "./test/setup";
import { resetTestState, testState, Uri } from "./test/vscode";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const TS = syntaxFor("typescript") as Syntax;

const bodies = (text: string, syntax: Syntax) =>
  commentRegions(text, syntax).map(regionBody);

const applyAll = (
  text: string,
  edit: { start: number; end: number; text: string },
) => text.slice(0, edit.start) + edit.text + text.slice(edit.end);

describe("comments", () => {
  it("skips strings, addresses and regular expressions of text", () => {
    const text = [
      'const url = "http://example.com/*not*/"; // real one',
      "const s = 'it // is not';",
      "const u = `multi",
      "// still a string`;",
      "/* block */ const x = 1;",
      "fetch(http://x); // after",
    ].join("\n");
    expect(
      scanComments(text, TS).map((c) => text.slice(c.start, c.end)),
    ).toEqual(["// real one", "/* block */", "// after"]);
  });

  it("skips regular expression literals, even with comment marks", () => {
    // Rapport 94 (S4, M2) : un `/*` ou un `//` dans une expression régulière
    // n'ouvre pas de commentaire, et le code qui suit ne part jamais.
    const trim = [
      'const trim = (p) => p.replace(/\\/*$/, "");',
      "",
      "/**",
      " * Removes teh trailing slashes.",
      " */",
    ].join("\n");
    expect(bodies(trim, TS)).toEqual(["Removes teh trailing slashes."]);
    const glob = [
      "const glob = /src\\/*.ts/",
      "const limit = maxRetries * 2",
      "await sendReport(apiHost, limit)",
      "/* Retry the upload a few times */",
    ].join("\n");
    expect(bodies(glob, TS)).toEqual(["Retry the upload a few times"]);
    expect(bodies("const re = /[/*]/g; // marks", TS)).toEqual(["marks"]);
    expect(bodies("if (x) return /\\/\\//.test(y); // two", TS)).toEqual([
      "two",
    ]);
    // Une division reste une division.
    expect(bodies("const half = total / 2; // half of it", TS)).toEqual([
      "half of it",
    ]);
  });

  it("skips template literals with nested expressions", () => {
    const text = [
      "const s = `a ${cond ? `inner /* not */` : '// no'} b /* no */`;",
      "// yes",
    ].join("\n");
    expect(bodies(text, TS)).toEqual(["yes"]);
  });

  it("skips Rust raw strings, C# verbatim strings and Lua long brackets", () => {
    const rust = syntaxFor("rust") as Syntax;
    expect(
      bodies(
        'let s = r#"x" // not a comment"#; // real\nlet b = br"/*";',
        rust,
      ),
    ).toEqual(["real"]);
    const cs = syntaxFor("csharp") as Syntax;
    expect(bodies('var p = @"C:\\d\\"; // the folder', cs)).toEqual([
      "the folder",
    ]);
    const lua = syntaxFor("lua") as Syntax;
    expect(bodies("--[==[ long ]] still ]==] x = 1 -- y", lua)).toEqual([
      "long ]] still",
      "y",
    ]);
    expect(bodies("local s = [[ -- not ]] -- yes", lua)).toEqual(["yes"]);
  });

  it("does not send a passage it is not sure about", () => {
    // Imbriqué, non fermé, ou marque collée à du code : rien ne part.
    expect(bodies("/* outer /* inner */ rest */\nlet a = 1;", TS)).toEqual([]);
    expect(bodies("let a = 1;\n/* never closed\nlet b = 2;", TS)).toEqual([]);
    expect(bodies("call(/* inline note */);", TS)).toEqual([]);
    expect(bodies("x = y\\/* odd */", TS)).toEqual([]);
    // Commentaire HTML après une balise : sûr.
    const html = syntaxFor("html") as Syntax;
    expect(bodies("<div><!-- a note --></div>", html)).toEqual(["a note"]);
  });

  it("checks a correction without trusting the comment scanner", () => {
    // Analyseur sans les expressions régulières (comme avant le rapport 94) :
    // il prend `$/, "");` pour un commentaire. La lecture indépendante voit
    // que le code change.
    const blind: Syntax = { ...TS, js: false };
    const text = [
      'const trim = (p) => p.replace(/\\/*$/, "");',
      "/** Removes teh slashes. */",
    ].join("\n");
    const start = text.indexOf("/*$");
    const end = text.length;
    const evil = {
      start,
      end,
      text: text.slice(start, end).replace('""', "''").replace("teh", "the"),
    };
    expect(codeLines(text, blind)).toEqual(
      codeLines(applyAll(text, evil), blind),
    );
    expect(onlyComments(text, evil, blind)).toBe(false);
    // Une vraie correction de commentaire passe.
    const doc = text.indexOf("/**");
    const fair = {
      start: doc,
      end,
      text: "/** Removes the slashes. */",
    };
    expect(onlyComments(text, fair, TS)).toBe(true);
    expect(plainCode(text, TS)).toEqual([
      'const trim = (p) => p.replace(/\\/*$/, "");',
    ]);
  });

  it("knows other languages", () => {
    const py = syntaxFor("python") as Syntax;
    expect(bodies('x = "# no"  # yes\n"""doc # no"""\n# top', py)).toEqual([
      "yes",
      "top",
    ]);
    const sh = syntaxFor("shellscript") as Syntax;
    expect(bodies("echo ${#arr} $# # count\n#!x", sh)).toEqual(["count", "!x"]);
    const rust = syntaxFor("rust") as Syntax;
    expect(bodies("fn f<'a>(c: char) { let q = '\"'; } // note", rust)).toEqual(
      ["note"],
    );
    expect(syntaxFor("brainfuck")).toBeNull();
  });

  it("groups consecutive line comments and rebuilds them exactly", () => {
    const text = [
      "function f() {",
      "  // First line of",
      "  // the comment.",
      "  //",
      "  // Second paragraph.",
      "  return 1; // trailing",
      "}",
    ].join("\n");
    const regions = commentRegions(text, TS);
    expect(regions.map(regionBody)).toEqual([
      "First line of\nthe comment.\n\nSecond paragraph.",
      "trailing",
    ]);
    for (const region of regions) {
      expect(text.slice(region.start, region.end)).toBe(regionText(region));
      expect(rebuildRegion(region, regionBody(region))).toBe(
        regionText(region),
      );
    }
    // Une ligne de plus garde l'indentation et la marque.
    expect(rebuildRegion(regions[0], "One.\nTwo.\n\nThree.\nFour.")).toBe(
      "// One.\n  // Two.\n  //\n  // Three.\n  // Four.",
    );
    // En fin de ligne de code, jamais de ligne ajoutée.
    expect(rebuildRegion(regions[1], "trailing\nmore")).toBe(
      "// trailing more",
    );
  });

  it("rebuilds block and JSDoc comments with their marks", () => {
    const text = "/**\n * Adds two numbrs.\n * @param a first\n */\n/* one */";
    const [doc, one] = commentRegions(text, TS);
    expect(regionBody(doc)).toBe("Adds two numbrs.\n@param a first");
    expect(rebuildRegion(doc, regionBody(doc))).toBe(regionText(doc));
    expect(
      rebuildRegion(doc, "Adds two numbers.\nSee below.\n@param a first"),
    ).toBe("/**\n * Adds two numbers.\n * See below.\n * @param a first\n */");
    expect(rebuildRegion(one, "One.")).toBe("/* One. */");
  });

  it("leaves directives, commented-out code and secrets alone", () => {
    const skipped = (text: string) =>
      commentRegions(text, TS).map((r) => skipRegion(r));
    expect(skipped("// eslint-disable-next-line no-console")).toEqual([true]);
    expect(skipped("// const x = compute(a, b);\n// return x;")).toEqual([
      true,
    ]);
    expect(
      skipped('// token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789"'),
    ).toEqual([true]);
    expect(skipped("// ----")).toEqual([true]);
    // Même filtre que le reçu (rapport 94, S3) : clé privée dans un bloc,
    // valeur YAML au milieu d'un commentaire de plusieurs lignes.
    expect(
      skipped(
        [
          "/*",
          " * Test key:",
          " * -----BEGIN RSA PRIVATE KEY-----",
          " * MIIEowIBAAKCAQEAuvW3",
          " * -----END RSA PRIVATE KEY-----",
          " */",
        ].join("\n"),
      ),
    ).toEqual([true]);
    expect(
      skipped("// Set up the database:\n// password: hunter2xyz\n// then run."),
    ).toEqual([true]);
    expect(skipped("// Explain why we retry.")).toEqual([false]);
  });

  it("sees code changes, not comment changes", () => {
    const text = "let a = 1; // one\n/* two */ let b = 2;";
    expect(codeLines(text, TS)).toEqual(["let a = 1;", " let b = 2;"]);
  });
});

describe("Markdown", () => {
  it("keeps code blocks, front matter and indented code out", () => {
    const text = [
      "---",
      "title: x",
      "---",
      "Some txt.",
      "",
      "```js",
      "const a = 1;",
      "```",
      "",
      "    indented code",
      "",
      "- item with `code`",
    ].join("\n");
    expect(proseSegments(text).map((s) => text.slice(s.start, s.end))).toEqual([
      "Some txt.\n",
      "\n- item with `code`",
    ]);
  });

  it("refuses a correction that changes inline code, links or structure", () => {
    const before = "See `npm run bild` and [the docs](https://a.b/x).\n# Title";
    expect(
      keepsMarkdown(
        before,
        "See `npm run bild` and [the docs](https://a.b/x).\n# Title",
      ),
    ).toBe(true);
    expect(
      keepsMarkdown(
        before,
        "See `npm run build` and [the docs](https://a.b/x).\n# Title",
      ),
    ).toBe(false);
    expect(
      keepsMarkdown(
        before,
        "See `npm run bild` and [the docs](https://a.b/y).\n# Title",
      ),
    ).toBe(false);
    expect(
      keepsMarkdown(
        before,
        "See `npm run bild` and [the docs](https://a.b/x).\nTitle",
      ),
    ).toBe(false);
  });
});

/** Contrôleur relié ; la correction de l'app est donnée par `fix`. */
async function proofreading(fix: (text: string) => string) {
  const env = await setupController();
  env.fake.handlers["proofread.run"] = (params, { id, notify }) => {
    const text = params.text as string;
    notify("proofread.event", {
      requestId: id,
      event: { event: "started", model: "m", parts: [{}] },
    });
    return { text: fix(text) };
  };
  return env;
}

const diffOpened = () =>
  testState.executed.filter((e) => e.command === "vscode.diff");

/** Texte proposé dans la vue de différences, lu au moment de la question
 * (« Apply » / « Discard »), puis réponse `choice`. */
async function answering(choice: string | undefined, run: () => Promise<void>) {
  const window = (await import("./test/vscode")).window;
  const original = window.showInformationMessage;
  let proposed: string | null = null;
  window.showInformationMessage = async (text: string, ...items: unknown[]) => {
    const uri = (diffOpened().at(-1)?.args ?? [])[1] as Uri | undefined;
    const provider = testState.contentProviders.get("chatonlm-proofread");
    if (uri && provider) proposed = provider.provideTextDocumentContent(uri);
    testState.choice = choice;
    return original(text, ...items);
  };
  try {
    await run();
  } finally {
    window.showInformationMessage = original;
  }
  return proposed;
}

describe("Proofread comments", () => {
  const FILE = [
    "// Retrys the request twice.",
    'const url = "teh-server";',
    "function f() {",
    "  /* Teh value is",
    "   * cached. */",
    "  return 1;",
    "}",
  ].join("\n");

  it("shows a diff of the comments only, and applies it on Apply", async () => {
    const env = await proofreading((text) =>
      text.replace("Retrys", "Retries").replace("Teh", "The"),
    );
    testState.contentProviders.set(
      "chatonlm-proofread",
      env.controller.proofreader,
    );
    openEditor("/work/src/a.ts", FILE);
    const proposed = await answering("Apply", () =>
      env.controller.proofreader.comments(),
    );
    // Seul le texte des commentaires est parti, jamais le code.
    expect(env.fake.calls("proofread.run")).toEqual([
      { text: "Retrys the request twice.", mode: "correct" },
      { text: "Teh value is\ncached.", mode: "correct" },
    ]);
    expect(diffOpened()).toHaveLength(1);
    const expected = FILE.replace("Retrys", "Retries").replace(
      "Teh value",
      "The value",
    );
    // Le code garde « teh-server » : la chaîne n'est pas un commentaire.
    expect(expected).toContain('"teh-server"');
    expect(proposed).toBe(expected);
    const document = testState.documents.get("/work/src/a.ts");
    expect(document?.text).toBe(expected);
  });

  it("writes nothing without Apply", async () => {
    const env = await proofreading((text) => text.replace("Retrys", "Retries"));
    openEditor("/work/src/a.ts", FILE);
    testState.choice = undefined;
    await env.controller.proofreader.comments();
    expect(diffOpened()).toHaveLength(1);
    expect(testState.edits).toEqual([]);
    expect(testState.documents.get("/work/src/a.ts")?.text).toBe(FILE);
  });

  it("drops a correction that would change the code", async () => {
    // Le modèle referme le commentaire et ajoute du code.
    const env = await proofreading((text) =>
      text.startsWith("Teh") ? "The value is */ evil(); /*\ncached." : text,
    );
    openEditor("/work/src/a.ts", FILE);
    testState.choice = "Apply";
    await env.controller.proofreader.comments();
    expect(diffOpened()).toEqual([]);
    expect(testState.edits).toEqual([]);
    expect(testState.shown.at(-1)?.text).toContain("No correction was kept");
  });

  it("does not apply over a file changed meanwhile", async () => {
    const env = await proofreading((text) => text.replace("Retrys", "Retries"));
    const editor = openEditor("/work/src/a.ts", FILE);
    testState.choice = "Apply";
    // Le fichier change pendant que la question est posée.
    const window = (await import("./test/vscode")).window;
    const original = window.showInformationMessage;
    window.showInformationMessage = async (
      text: string,
      ...items: unknown[]
    ) => {
      editor.document.version++;
      return original(text, ...items);
    };
    try {
      await env.controller.proofreader.comments();
    } finally {
      window.showInformationMessage = original;
    }
    expect(testState.edits).toEqual([]);
    expect(testState.shown.at(-1)?.text).toContain("changed since");
  });

  it("only takes the comments of the selection", async () => {
    const env = await proofreading((text) => text);
    openEditor("/work/src/a.ts", FILE, { from: [3, 0], to: [4, 5] });
    await env.controller.proofreader.comments();
    await until(() => env.fake.calls("proofread.run").length === 1);
    expect(env.fake.calls("proofread.run")[0].text).toBe(
      "Teh value is\ncached.",
    );
  });

  it("refuses a file that may hold secrets", async () => {
    const env = await proofreading((text) => text);
    openEditor("/work/.env", "# a commment\nA=1", undefined, "shellscript");
    await env.controller.proofreader.comments();
    expect(env.fake.calls("proofread.run")).toEqual([]);
  });
});

describe("Proofread this document", () => {
  it("never sends a passage that holds a secret, even in prose", async () => {
    const env = await proofreading((text) => text);
    const NOTES = [
      "Setup",
      "",
      "Configure the database:",
      "password: hunter2xyz",
      "",
      "Then run it.",
    ].join("\n");
    openEditor("/work/notes.txt", NOTES, undefined, "plaintext");
    await env.controller.proofreader.document();
    expect(env.fake.calls("proofread.run")).toEqual([]);
  });

  it("sends the prose, never the code blocks, and keeps links", async () => {
    const env = await proofreading((text) =>
      text
        .replace("Instal", "Install")
        .replace("(https://a.b)", "(https://c.d)"),
    );
    testState.contentProviders.set(
      "chatonlm-proofread",
      env.controller.proofreader,
    );
    const README = [
      "# Projet",
      "",
      "Instal it first.",
      "",
      "```sh",
      "npm instal",
      "```",
      "",
      "See [docs](https://a.b).",
    ].join("\n");
    openEditor("/work/README.md", README, undefined, "markdown");
    const proposed = await answering("Apply", () =>
      env.controller.proofreader.document(),
    );
    const sent = env.fake.calls("proofread.run").map((p) => p.text as string);
    expect(sent.join("")).not.toContain("npm instal");
    expect(diffOpened()).toHaveLength(1);
    // Le lien changé est refusé ; la faute de la prose est corrigée.
    expect(testState.documents.get("/work/README.md")?.text).toBe(
      README.replace("Instal it", "Install it"),
    );
    expect(proposed).toBe(README.replace("Instal it", "Install it"));
  });
});
