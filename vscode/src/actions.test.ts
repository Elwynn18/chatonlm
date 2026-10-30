// @vitest-environment node
import type * as vscode from "vscode";
import { beforeEach, describe, expect, it } from "vitest";
import {
  ActionError,
  diagnosticAt,
  FixProvider,
  headingOf,
  innermostFunction,
  prepareAction,
} from "./actions";
import {
  Diagnostic,
  DiagnosticSeverity,
  DocumentSymbol,
  Position,
  Range,
  resetTestState,
  Selection,
  SymbolKind,
  TextDocument,
  testState,
  window,
} from "./test/vscode";

beforeEach(() => resetTestState());

const source = [
  "class A {", // 0
  "  run() {", // 1
  "    return 1;", // 2
  "  }", // 3
  "}", // 4
  "function g() {}", // 5
].join("\n");

function symbols() {
  const run = new DocumentSymbol(
    "run",
    "",
    SymbolKind.Method,
    new Range(1, 2, 3, 3),
    new Range(1, 2, 1, 5),
  );
  const a = new DocumentSymbol(
    "A",
    "",
    SymbolKind.Class,
    new Range(0, 0, 4, 1),
    new Range(0, 6, 0, 7),
  );
  a.children = [run];
  const g = new DocumentSymbol(
    "g",
    "",
    SymbolKind.Function,
    new Range(5, 0, 5, 15),
    new Range(5, 9, 5, 10),
  );
  return [a, g];
}

function open(cursor: [number, number], text = source) {
  const document = new TextDocument("/work/src/a.ts", text);
  const position = new Position(...cursor);
  window.activeTextEditor = {
    document,
    selection: new Selection(position, position),
  };
}

describe("actions", () => {
  it("takes the function under the cursor when nothing is selected", async () => {
    testState.commands.set("vscode.executeDocumentSymbolProvider", () =>
      symbols(),
    );
    open([2, 4]);
    const request = await prepareAction("tests");
    expect(request.pieces).toEqual([
      {
        kind: "selection",
        label: "src/a.ts",
        path: "src/a.ts",
        lines: [2, 4],
        text: "  run() {\n    return 1;\n  }",
        language: "typescript",
      },
    ]);
  });

  it("takes the whole file outside any function", async () => {
    testState.commands.set("vscode.executeDocumentSymbolProvider", () =>
      symbols(),
    );
    open([0, 1]);
    const [piece] = (await prepareAction("explain")).pieces;
    expect(piece).toMatchObject({ kind: "file", lines: [1, 6], text: source });
  });

  it("refuses without an editor, or with a huge file", async () => {
    await expect(prepareAction("explain")).rejects.toBeInstanceOf(ActionError);
    open([0, 0], "x".repeat(100_001));
    await expect(prepareAction("explain")).rejects.toThrow(/too long/);
  });

  it("finds the innermost function", () => {
    const tree = symbols() as unknown as vscode.DocumentSymbol[];
    expect(
      innermostFunction(tree, new Position(2, 0) as vscode.Position)?.name,
    ).toBe("run");
    expect(
      innermostFunction(tree, new Position(5, 3) as vscode.Position)?.name,
    ).toBe("g");
    expect(
      innermostFunction(tree, new Position(0, 0) as vscode.Position),
    ).toBeNull();
  });

  it("picks the most serious problem under the cursor", () => {
    const warning = new Diagnostic(
      new Range(3, 0, 3, 9),
      "w",
      DiagnosticSeverity.Warning,
    );
    const error = new Diagnostic(
      new Range(3, 2, 3, 4),
      "e",
      DiagnosticSeverity.Error,
    );
    const elsewhere = new Diagnostic(
      new Range(8, 0, 8, 1),
      "x",
      DiagnosticSeverity.Error,
    );
    const all = [warning, error, elsewhere] as unknown as vscode.Diagnostic[];
    expect(
      diagnosticAt(all, new Position(3, 3) as vscode.Position)?.message,
    ).toBe("e");
    expect(
      diagnosticAt(all, new Position(3, 7) as vscode.Position)?.message,
    ).toBe("w");
    expect(diagnosticAt(all, new Position(5, 0) as vscode.Position)).toBeNull();
  });

  it("titles each piece with its path and lines", () => {
    expect(
      headingOf({
        kind: "selection",
        label: "a",
        path: "a.ts",
        lines: [3, 3],
        text: "",
      }),
    ).toBe("`a.ts`, line 3:");
    expect(
      headingOf({
        kind: "file",
        label: "a",
        path: "a.ts",
        lines: [1, 9],
        text: "",
      }),
    ).toBe("`a.ts`, lines 1–9:");
    expect(headingOf({ kind: "diff", label: "Staged changes", text: "" })).toBe(
      "Staged changes:",
    );
  });

  it("offers a quick fix for the most serious problem", () => {
    const document = new TextDocument("/work/src/a.ts", source);
    const warning = new Diagnostic(
      new Range(1, 0, 1, 1),
      "w",
      DiagnosticSeverity.Warning,
    );
    const error = new Diagnostic(
      new Range(1, 0, 1, 1),
      "e",
      DiagnosticSeverity.Error,
    );
    let paired = true;
    const provider = new FixProvider(() => paired);
    const actions = provider.provideCodeActions(
      document as unknown as vscode.TextDocument,
      new Range(1, 0, 1, 0) as unknown as vscode.Range,
      { diagnostics: [warning, error] } as unknown as vscode.CodeActionContext,
    );
    expect(actions).toHaveLength(1);
    expect(actions[0].title).toBe("Fix with ChatonLM");
    expect(actions[0].command?.command).toBe("chatonlm.fix");
    expect(actions[0].command?.arguments).toEqual([document.uri, error]);
    expect(
      provider.provideCodeActions(
        document as unknown as vscode.TextDocument,
        new Range(1, 0, 1, 0) as unknown as vscode.Range,
        { diagnostics: [] } as unknown as vscode.CodeActionContext,
      ),
    ).toEqual([]);
    // Pas appairé : rien dans le menu des corrections.
    paired = false;
    expect(
      provider.provideCodeActions(
        document as unknown as vscode.TextDocument,
        new Range(1, 0, 1, 0) as unknown as vscode.Range,
        { diagnostics: [error] } as unknown as vscode.CodeActionContext,
      ),
    ).toEqual([]);
  });
});
