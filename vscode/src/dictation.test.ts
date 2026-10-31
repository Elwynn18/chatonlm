// @vitest-environment node
// Dictée dans l'éditeur (lot 4b, 1) : glossaire tiré du projet (symboles,
// identifiants, fichiers ouverts ; plafonné ; rien d'un fichier sensible)
// envoyé à `dictation.start`, texte inséré au curseur par `editor.edit`
// (jamais par le presse-papiers) dans l'éditeur du départ s'il est encore
// visible, barre d'état, arrêt, refus, liaison perdue.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  connectedSoon,
  buildGlossary,
  cleanWord,
  frequentIdentifiers,
  GLOSSARY_MAX,
  phaseOf,
  symbolNames,
  WORD_MAX,
} from "./dictation";
import { RpcFailure, until } from "./test/fakeBridge";
import { cleanUp, openEditor, setupController } from "./test/setup";
import {
  Position,
  resetTestState,
  Selection,
  testState,
  window,
} from "./test/vscode";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const CODE = [
  "export function parseInvoice(rawInvoice) {",
  "  const invoice_total = sumLines(rawInvoice);",
  "  return invoice_total + parseInvoice.length;",
  "}",
].join("\n");

describe("glossary", () => {
  it("puts the active file's names first, then the open files", () => {
    const glossary = buildGlossary({
      path: "src/invoice.ts",
      symbols: ["parseInvoice", "InvoiceReader"],
      text: CODE,
      files: ["src/billingService.ts", "README.md"],
    });
    expect(glossary.slice(0, 2)).toEqual(["parseInvoice", "InvoiceReader"]);
    expect(glossary).toContain("rawInvoice");
    expect(glossary).toContain("invoice_total");
    expect(glossary).toContain("sumLines");
    expect(glossary.indexOf("billingService")).toBeGreaterThan(
      glossary.indexOf("sumLines"),
    );
    // Une seule fois chacun.
    expect(new Set(glossary).size).toBe(glossary.length);
  });

  it("takes nothing from a file that may hold secrets", () => {
    const glossary = buildGlossary({
      path: ".env",
      symbols: ["API_TOKEN"],
      text: "API_TOKEN=sk_live_abc\nDB_PASSWORD=hunter2",
      files: [".env", "src/app.ts"],
    });
    expect(glossary).toEqual(["app"]);
  });

  it("is capped: 200 names of 64 characters at most", () => {
    const symbols = Array.from({ length: 500 }, (_, i) => `symbolNumber${i}x`);
    const glossary = buildGlossary({
      path: "a.ts",
      symbols: [...symbols, "x".repeat(WORD_MAX + 1)],
      text: "",
      files: [],
    });
    expect(glossary).toHaveLength(GLOSSARY_MAX);
    expect(glossary.every((word) => word.length <= WORD_MAX)).toBe(true);
    expect(cleanWord("a\u0007b")).toBe("ab");
    expect(cleanWord("x")).toBeNull();
    expect(cleanWord("token123456789")).toBeNull();
    expect(cleanWord("not a name")).toBeNull();
  });

  it("reads identifiers by frequency and symbol trees in order", () => {
    expect(frequentIdentifiers("fooBar x fooBar snake_case fooBar")).toEqual([
      "fooBar",
      "snake_case",
    ]);
    expect(
      symbolNames([
        { name: "Outer", children: [{ name: "innerMethod", children: [] }] },
        { name: "other" },
      ]),
    ).toEqual(["Outer", "innerMethod", "other"]);
    expect(phaseOf("recording")).toBe("recording");
    expect(phaseOf({ phase: "polishing" })).toBe("polishing");
    expect(phaseOf("loud")).toBeNull();
  });
});

async function dictating() {
  const env = await setupController();
  const editor = openEditor("/work/src/invoice.ts", CODE);
  editor.selection = new Selection(new Position(1, 2), new Position(1, 2));
  testState.symbols = [
    { name: "parseInvoice", children: [{ name: "rawInvoice", children: [] }] },
  ];
  testState.tabs = ["/work/src/billingService.ts"];
  const dictation = env.controller.dictation;
  return { ...env, editor, dictation };
}

describe("dictation into the editor", () => {
  it("starts the app's dictation with the project's names", async () => {
    const { dictation, fake } = await dictating();
    await dictation.toggle();
    const [params] = fake.calls("dictation.start");
    const glossary = params.glossary as string[];
    expect(glossary.slice(0, 2)).toEqual(["parseInvoice", "rawInvoice"]);
    expect(glossary).toContain("billingService");
    expect(testState.contexts["chatonlm.dictating"]).toBe(true);
    const status = testState.statusItems.find(
      (item) => item.id === "chatonlm.dictation",
    );
    expect(status?.visible).toBe(true);
    expect(status?.text).toMatch(/listening/);
  });

  it("inserts the text at the cursor, never through the clipboard", async () => {
    const { dictation, fake, editor } = await dictating();
    await dictation.start();
    fake.notify("dictation.state", { state: "recording" });
    await dictation.stop();
    expect(fake.calls("dictation.stop")).toHaveLength(1);
    expect(testState.contexts["chatonlm.dictating"]).toBe(false);
    fake.notify("dictation.state", { state: "transcribing" });
    fake.notify("dictation.text", { text: "// total of the lines\n  " });
    await until(() => testState.editorEdits.length === 1);
    expect(editor.document.lineAt(1).text).toBe(
      "  // total of the linesconst invoice_total = sumLines(rawInvoice);",
    );
    expect(testState.clipboard).toBe("");
    fake.notify("dictation.state", { state: "idle" });
    await until(() => !dictation.active);
    const status = testState.statusItems.find(
      (item) => item.id === "chatonlm.dictation",
    );
    expect(status?.visible).toBe(false);
  });

  it("replaces the selection, and spaces words apart", async () => {
    const { dictation, fake, editor } = await dictating();
    // « sumLines » sélectionné.
    editor.selection = new Selection(new Position(1, 24), new Position(1, 32));
    await dictation.start();
    fake.notify("dictation.text", { text: "addLines" });
    await until(() => testState.editorEdits.length === 1);
    expect(editor.document.lineAt(1).text).toBe(
      "  const invoice_total = addLines(rawInvoice);",
    );
    // Après un mot : une espace avant le mot dicté.
    editor.selection = new Selection(new Position(0, 6), new Position(0, 6));
    await dictation.start();
    fake.notify("dictation.text", { text: "default" });
    await until(() => testState.editorEdits.length === 2);
    expect(editor.document.lineAt(0).text).toBe(
      "export default function parseInvoice(rawInvoice) {",
    );
  });

  it("writes into the editor it started in, while that one is still visible", async () => {
    const { dictation, fake, editor } = await dictating();
    await dictation.start();
    // Un autre fichier ouvert pendant la transcription, les deux visibles.
    const notes = openEditor(
      "/work/notes.md",
      "Notes\n",
      undefined,
      "markdown",
    );
    window.visibleTextEditors = [editor, notes];
    fake.notify("dictation.state", { state: "recording" });
    fake.notify("dictation.text", { text: "totalAmount" });
    await until(() => testState.editorEdits.length === 1);
    expect(editor.document.lineAt(1).text).toContain("totalAmount");
    expect(notes.document.lineAt(0).text).toBe("Notes");
    fake.notify("dictation.state", { state: "idle" });
    await until(() => !dictation.active);
    // Nouvelle dictée depuis invoice.ts, puis invoice.ts fermé : l'éditeur
    // actif reçoit le texte.
    window.activeTextEditor = editor;
    await dictation.start();
    fake.notify("dictation.state", { state: "recording" });
    window.activeTextEditor = notes;
    window.visibleTextEditors = [notes];
    fake.notify("dictation.text", { text: "later" });
    await until(() => testState.editorEdits.length === 2);
    expect(notes.document.lineAt(0).text).toContain("later");
  });

  it("takes the text that comes just after the end of the session", async () => {
    const { dictation, fake, editor } = await dictating();
    await dictation.start();
    fake.notify("dictation.state", { state: "recording" });
    fake.notify("dictation.state", { state: "idle" });
    await until(() => !dictation.active);
    fake.notify("dictation.text", { text: "late" });
    await until(() => testState.editorEdits.length === 1);
    expect(editor.document.lineAt(1).text).toContain("late");
    // Un texte sans séance (une autre dictée de l'app) n'est jamais écrit.
    fake.notify("dictation.text", { text: "stray" });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(testState.editorEdits).toHaveLength(1);
  });

  it("offers to open ChatonLM when the speech model is missing", async () => {
    const { dictation, fake } = await dictating();
    fake.handlers["dictation.start"] = () => {
      throw new RpcFailure(-32031, "no transcription");
    };
    await dictation.start();
    expect(testState.shown.at(-1)).toEqual({
      level: "error",
      text: "ChatonLM doesn't have its speech recognition model yet. Open ChatonLM to download it.",
    });
    expect(dictation.active).toBe(false);
    expect(testState.contexts["chatonlm.dictating"]).toBe(false);
  });

  it("shows the app's failure after the microphone opened", async () => {
    const { dictation, fake } = await dictating();
    await dictation.start();
    fake.notify("dictation.state", { state: "recording" });
    fake.notify("dictation.error", { message: "Nothing was heard." });
    await until(() => !dictation.active);
    expect(testState.shown.at(-1)).toEqual({
      level: "error",
      text: "Nothing was heard.",
    });
    expect(testState.editorEdits).toEqual([]);
  });

  it("says nothing when the app stops it before the microphone opens", async () => {
    const { dictation, fake } = await dictating();
    fake.handlers["dictation.start"] = () => {
      throw new RpcFailure(-32800, "cancelled");
    };
    await dictation.start();
    expect(dictation.active).toBe(false);
    expect(testState.shown).toEqual([]);
  });

  it("needs an editor and a link to the app", async () => {
    const { dictation, fake, link } = await dictating();
    window.activeTextEditor = undefined;
    await dictation.start();
    expect(fake.calls("dictation.start")).toEqual([]);
    expect(testState.shown.at(-1)?.text).toMatch(/Open a file/);
    link.dispose();
    await dictation.start();
    expect(fake.calls("dictation.start")).toEqual([]);
  });

  it("ends when the link with the app is lost", async () => {
    const { dictation, fake } = await dictating();
    await dictation.start();
    expect(dictation.active).toBe(true);
    fake.dropAll();
    await until(() => !dictation.active);
    expect(testState.contexts["chatonlm.dictating"]).toBe(false);
  });
});

describe("connectedSoon", () => {
  type State = { connection: string };
  function fakeLink(initial: string) {
    let state: State = { connection: initial };
    const listeners = new Set<(s: State) => void>();
    return {
      get state() {
        return state;
      },
      get connected() {
        return state.connection === "connected";
      },
      onState(listener: (s: State) => void) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      set(connection: string) {
        state = { connection };
        for (const listener of [...listeners]) listener(state);
      },
      listeners,
    };
  }

  it("waits for a link that is connecting, then lets the dictation go", async () => {
    const link = fakeLink("connecting");
    const waiting = connectedSoon(link as never, 5000);
    setTimeout(() => link.set("connected"), 20);
    await waiting;
    expect(link.connected).toBe(true);
    expect(link.listeners.size).toBe(0);
  });

  it("does not wait when the app is closed or not paired", async () => {
    const link = fakeLink("appClosed");
    const started = Date.now();
    await connectedSoon(link as never, 5000);
    expect(Date.now() - started).toBeLessThan(100);
  });

  it("gives up after its delay", async () => {
    const link = fakeLink("connecting");
    await connectedSoon(link as never, 30);
    expect(link.connected).toBe(false);
    expect(link.listeners.size).toBe(0);
  });
});
