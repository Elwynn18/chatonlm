// Actions de l'éditeur : ce qu'elles prennent (sélection, fonction sous le
// curseur, problème signalé, diff) et leur consigne. Le texte final et le
// reçu sont faits par le contrôleur, après le choix local ou cloud.
import * as vscode from "vscode";
import type { QuickAction } from "../shared/protocol";
import { changes, gitApi, pickRepository, type GitRepository } from "./git";
import { truncateDiff } from "./prompts";
import type { Piece } from "./receipt";

/** Plus grand fichier envoyé entier (sans sélection ni fonction trouvée). */
export const MAX_FILE_CHARS = 100_000;
/** Lignes de contexte autour d'un problème. */
export const PROBLEM_CONTEXT_LINES = 10;

export type ActionRequest = {
  /** `ask` : message tapé avec le passage d'une visite joint. */
  action: QuickAction | "ask";
  instruction: string;
  pieces: Piece[];
  /** Dépôt dont la boîte de saisie reçoit le message (commitMessage). */
  repository?: GitRepository;
};

/** Échec expliqué à l'utilisateur (texte déjà traduit). */
export class ActionError extends Error {}

const FENCE_LANGUAGES: Record<string, string> = {
  typescriptreact: "tsx",
  javascriptreact: "jsx",
  shellscript: "sh",
  plaintext: "",
};

export function fenceLanguage(languageId: string): string {
  return FENCE_LANGUAGES[languageId] ?? languageId;
}

export function relativePath(uri: vscode.Uri): string {
  return vscode.workspace.asRelativePath(uri, false);
}

function editorOrFail(): vscode.TextEditor {
  const editor = vscode.window.activeTextEditor;
  if (!editor) throw new ActionError(vscode.l10n.t("Open a file first."));
  return editor;
}

function linesPiece(
  kind: Piece["kind"],
  document: vscode.TextDocument,
  first: number,
  last: number,
): Piece {
  const range = new vscode.Range(
    first,
    0,
    last,
    document.lineAt(last).text.length,
  );
  const path = relativePath(document.uri);
  return {
    kind,
    label: path,
    path,
    lines: [first + 1, last + 1],
    text: document.getText(range),
    language: fenceLanguage(document.languageId),
  };
}

const FUNCTION_KINDS = new Set([
  vscode.SymbolKind.Function,
  vscode.SymbolKind.Method,
  vscode.SymbolKind.Constructor,
]);

/** Plus petite fonction ou méthode qui contient la position. */
export function innermostFunction(
  symbols: readonly vscode.DocumentSymbol[],
  position: vscode.Position,
): vscode.DocumentSymbol | null {
  let found: vscode.DocumentSymbol | null = null;
  for (const symbol of symbols) {
    if (!symbol.range.contains(position)) continue;
    if (FUNCTION_KINDS.has(symbol.kind)) found = symbol;
    const inner = innermostFunction(symbol.children ?? [], position);
    if (inner) found = inner;
  }
  return found;
}

/** Sélection, sinon fonction sous le curseur, sinon le fichier entier. */
async function codeUnderCursor(editor: vscode.TextEditor): Promise<Piece> {
  const { document, selection } = editor;
  if (!selection.isEmpty) {
    let last = selection.end.line;
    // Sélection de lignes entières : la ligne suivante n'en fait pas partie.
    if (selection.end.character === 0 && last > selection.start.line) last--;
    const path = relativePath(document.uri);
    return {
      kind: "selection",
      label: path,
      path,
      lines: [selection.start.line + 1, last + 1],
      text: document.getText(selection),
      language: fenceLanguage(document.languageId),
    };
  }
  let symbols: unknown;
  try {
    symbols = await vscode.commands.executeCommand(
      "vscode.executeDocumentSymbolProvider",
      document.uri,
    );
  } catch {
    symbols = undefined;
  }
  // SymbolInformation (sans `children`) : pas de hiérarchie, on s'en passe.
  const tree = Array.isArray(symbols)
    ? (symbols as vscode.DocumentSymbol[]).filter((s) => "children" in s)
    : [];
  const symbol = innermostFunction(tree, selection.active);
  if (symbol) {
    return linesPiece(
      "selection",
      document,
      symbol.range.start.line,
      symbol.range.end.line,
    );
  }
  const text = document.getText();
  if (text.length > MAX_FILE_CHARS) {
    throw new ActionError(
      vscode.l10n.t(
        "This file is too long to send whole. Select the part to send.",
      ),
    );
  }
  const path = relativePath(document.uri);
  return {
    kind: "file",
    label: path,
    path,
    lines: [1, Math.max(1, document.lineCount)],
    text,
    language: fenceLanguage(document.languageId),
  };
}

function severityName(severity: vscode.DiagnosticSeverity): string {
  switch (severity) {
    case vscode.DiagnosticSeverity.Error:
      return vscode.l10n.t("error");
    case vscode.DiagnosticSeverity.Warning:
      return vscode.l10n.t("warning");
    default:
      return vscode.l10n.t("note");
  }
}

/** Problème signalé sous le curseur (le plus grave), sinon sur sa ligne. */
export function diagnosticAt(
  diagnostics: readonly vscode.Diagnostic[],
  position: vscode.Position,
): vscode.Diagnostic | null {
  const bySeverity = [...diagnostics].sort((a, b) => a.severity - b.severity);
  return (
    bySeverity.find((d) => d.range.contains(position)) ??
    bySeverity.find(
      (d) =>
        d.range.start.line <= position.line &&
        position.line <= d.range.end.line,
    ) ??
    null
  );
}

function diagnosticCode(diagnostic: vscode.Diagnostic): string {
  const code = diagnostic.code;
  if (code === undefined || code === null) return "";
  return typeof code === "object" ? String(code.value) : String(code);
}

async function fixRequest(args: unknown[]): Promise<ActionRequest> {
  let document: vscode.TextDocument;
  let diagnostic: vscode.Diagnostic | null;
  const [uriArg, diagnosticArg] = args;
  if (
    uriArg instanceof vscode.Uri &&
    diagnosticArg instanceof vscode.Diagnostic
  ) {
    document = await vscode.workspace.openTextDocument(uriArg);
    diagnostic = diagnosticArg;
  } else {
    const editor = editorOrFail();
    document = editor.document;
    diagnostic = diagnosticAt(
      vscode.languages.getDiagnostics(document.uri),
      editor.selection.active,
    );
  }
  if (!diagnostic) {
    throw new ActionError(
      vscode.l10n.t("No problem is reported under the cursor."),
    );
  }
  const first = Math.max(
    0,
    diagnostic.range.start.line - PROBLEM_CONTEXT_LINES,
  );
  const last = Math.min(
    document.lineCount - 1,
    diagnostic.range.end.line + PROBLEM_CONTEXT_LINES,
  );
  const code = linesPiece("file", document, first, last);
  const path = relativePath(document.uri);
  const where = diagnostic.range.start.line + 1;
  const source = [diagnostic.source, diagnosticCode(diagnostic)]
    .filter(Boolean)
    .join(" ");
  const problem: Piece = {
    kind: "diagnostic",
    label: path,
    path,
    lines: [where, diagnostic.range.end.line + 1],
    text: `${severityName(diagnostic.severity)}${source ? ` (${source})` : ""}: ${diagnostic.message}`,
  };
  return {
    action: "fix",
    instruction: vscode.l10n.t(
      "Fix this problem. Show the corrected code, then explain the change in a few words.",
    ),
    pieces: [problem, code],
  };
}

async function commitRequest(args: unknown[]): Promise<ActionRequest> {
  const api = await gitApi();
  if (!api) {
    throw new ActionError(
      vscode.l10n.t("Git is not available in this window."),
    );
  }
  const repository = pickRepository(
    api,
    args[0],
    vscode.window.activeTextEditor?.document.uri,
  );
  if (!repository) {
    throw new ActionError(vscode.l10n.t("Open a Git repository first."));
  }
  const { diff, staged } = await changes(repository);
  if (!diff.trim()) {
    throw new ActionError(vscode.l10n.t("There are no changes to describe."));
  }
  const { text, truncated } = truncateDiff(diff);
  const piece: Piece = {
    kind: "diff",
    language: "diff",
    label: staged
      ? vscode.l10n.t("Staged changes")
      : vscode.l10n.t("Uncommitted changes"),
    text,
  };
  const instruction = [
    vscode.l10n.t(
      "Write a commit message for these changes: a summary line of 72 characters at most, a blank line, then a few short bullet points if they help. Reply with the commit message only.",
    ),
    truncated ? vscode.l10n.t("The diff below is cut short.") : "",
  ]
    .filter(Boolean)
    .join(" ");
  return {
    action: "commitMessage",
    instruction,
    pieces: [piece],
    repository,
  };
}

/** Ce que prend une action et sa consigne ; ActionError si rien à envoyer. */
export async function prepareAction(
  action: QuickAction,
  args: unknown[] = [],
): Promise<ActionRequest> {
  switch (action) {
    case "explain": {
      const editor = editorOrFail();
      const piece = await codeUnderCursor(editor);
      return {
        action,
        instruction: vscode.l10n.t(
          "Explain what this code does and how it works.",
        ),
        pieces: [piece],
      };
    }
    case "tests": {
      const editor = editorOrFail();
      const piece = await codeUnderCursor(editor);
      return {
        action,
        instruction: vscode.l10n.t(
          "Write unit tests for this code. Use the test framework the project already uses if the code shows it.",
        ),
        pieces: [piece],
      };
    }
    case "fix":
      return fixRequest(args);
    case "commitMessage":
      return commitRequest(args);
  }
}

/** Titre d'un morceau dans le texte envoyé. */
export function headingOf(piece: Piece): string {
  const name = piece.path ? `\`${piece.path}\`` : piece.label;
  if (piece.kind === "diagnostic") {
    return vscode.l10n.t(
      "Problem in {0}, line {1}:",
      name,
      piece.lines?.[0] ?? 1,
    );
  }
  if (piece.kind === "diff") return `${piece.label}:`;
  if (piece.kind === "terminal") {
    return vscode.l10n.t("Last command that failed in the terminal:");
  }
  if (!piece.lines) return `${name}:`;
  const [first, last] = piece.lines;
  return first === last
    ? vscode.l10n.t("{0}, line {1}:", name, first)
    : vscode.l10n.t("{0}, lines {1}–{2}:", name, first, last);
}

/**
 * Action rapide « Fix with ChatonLM » sur les problèmes signalés, seulement
 * quand l'éditeur est appairé (sinon elle encombrerait le menu des
 * corrections de qui n'emploie pas ChatonLM).
 */
export class FixProvider implements vscode.CodeActionProvider {
  static readonly kinds = [vscode.CodeActionKind.QuickFix];
  readonly #available: () => boolean;

  constructor(available: () => boolean) {
    this.#available = available;
  }

  provideCodeActions(
    document: vscode.TextDocument,
    _range: vscode.Range | vscode.Selection,
    context: vscode.CodeActionContext,
  ): vscode.CodeAction[] {
    if (!this.#available()) return [];
    const diagnostic = [...context.diagnostics].sort(
      (a, b) => a.severity - b.severity,
    )[0];
    if (!diagnostic) return [];
    const action = new vscode.CodeAction(
      vscode.l10n.t("Fix with ChatonLM"),
      vscode.CodeActionKind.QuickFix,
    );
    action.diagnostics = [diagnostic];
    action.command = {
      command: "chatonlm.fix",
      title: vscode.l10n.t("Fix with ChatonLM"),
      arguments: [document.uri, diagnostic],
    };
    return [action];
  }
}
