// Module `vscode` simulé pour les essais vitest de l'hôte (alias dans
// vite.config.ts) : seulement ce que l'extension emploie, avec de quoi
// régler l'éditeur actif et relever les messages montrés.
import path from "node:path";

export class Uri {
  readonly scheme: string;
  readonly path: string;
  private constructor(scheme: string, p: string) {
    this.scheme = scheme;
    this.path = p;
  }
  static file(p: string): Uri {
    return new Uri("file", p);
  }
  static parse(value: string): Uri {
    const match = /^([a-z][\w+.-]*):(.*)$/i.exec(value);
    return match ? new Uri(match[1], match[2]) : new Uri("file", value);
  }
  static joinPath(base: Uri, ...parts: string[]): Uri {
    return new Uri(base.scheme, path.posix.join(base.path, ...parts));
  }
  get fsPath(): string {
    return this.path;
  }
  toString(): string {
    return this.scheme === "file"
      ? `file://${this.path}`
      : `${this.scheme}:${this.path}`;
  }
}

export class Position {
  constructor(
    readonly line: number,
    readonly character: number,
  ) {}
  isBefore(other: Position): boolean {
    return (
      this.line < other.line ||
      (this.line === other.line && this.character < other.character)
    );
  }
  isAfter(other: Position): boolean {
    return other.isBefore(this);
  }
}

export class Range {
  readonly start: Position;
  readonly end: Position;
  constructor(
    a: Position | number,
    b: Position | number,
    c?: number,
    d?: number,
  ) {
    this.start = a instanceof Position ? a : new Position(a, b as number);
    this.end =
      b instanceof Position ? b : new Position(c as number, d as number);
  }
  get isEmpty(): boolean {
    return (
      this.start.line === this.end.line &&
      this.start.character === this.end.character
    );
  }
  contains(p: Position | Range): boolean {
    if (p instanceof Range)
      return this.contains(p.start) && this.contains(p.end);
    return !p.isBefore(this.start) && !p.isAfter(this.end);
  }
}

export class Selection extends Range {
  readonly anchor: Position;
  readonly active: Position;
  constructor(anchor: Position, active: Position) {
    const forward = !active.isBefore(anchor);
    super(forward ? anchor : active, forward ? active : anchor);
    this.anchor = anchor;
    this.active = active;
  }
}

export enum DiagnosticSeverity {
  Error = 0,
  Warning = 1,
  Information = 2,
  Hint = 3,
}

export class Diagnostic {
  source?: string;
  code?: string | number | { value: string | number; target: Uri };
  constructor(
    readonly range: Range,
    readonly message: string,
    readonly severity: DiagnosticSeverity = DiagnosticSeverity.Error,
  ) {}
}

export enum SymbolKind {
  File = 0,
  Module = 1,
  Namespace = 2,
  Package = 3,
  Class = 4,
  Method = 5,
  Property = 6,
  Field = 7,
  Constructor = 8,
  Enum = 9,
  Interface = 10,
  Function = 11,
}

export class DocumentSymbol {
  children: DocumentSymbol[] = [];
  constructor(
    readonly name: string,
    readonly detail: string,
    readonly kind: SymbolKind,
    readonly range: Range,
    readonly selectionRange: Range,
  ) {}
}

export class CodeActionKind {
  static readonly QuickFix = new CodeActionKind("quickfix");
  private constructor(readonly value: string) {}
}

export class CodeAction {
  diagnostics?: Diagnostic[];
  command?: { command: string; title: string; arguments?: unknown[] };
  constructor(
    readonly title: string,
    readonly kind?: CodeActionKind,
  ) {}
}

export enum ColorThemeKind {
  Light = 1,
  Dark = 2,
  HighContrast = 3,
  HighContrastLight = 4,
}

/** Document en mémoire. */
export class TextDocument {
  readonly uri: Uri;
  readonly #lines: string[];
  constructor(
    file: string,
    readonly text: string,
    readonly languageId = "typescript",
  ) {
    this.uri = Uri.file(file);
    this.#lines = text.split("\n");
  }
  get lineCount(): number {
    return this.#lines.length;
  }
  lineAt(line: number): { text: string } {
    return { text: this.#lines[line] ?? "" };
  }
  #offset(p: Position): number {
    let offset = 0;
    for (let i = 0; i < p.line; i++) offset += this.#lines[i].length + 1;
    return offset + p.character;
  }
  getText(range?: Range): string {
    if (!range) return this.text;
    return this.text.slice(this.#offset(range.start), this.#offset(range.end));
  }
}

export type Shown = { level: "info" | "warning" | "error"; text: string };

/** Réglages des essais. */
export const testState = {
  shown: [] as Shown[],
  /** Réponse du prochain showX (bouton choisi). */
  choice: undefined as string | undefined,
  inputBox: undefined as string | undefined,
  commands: new Map<string, (...args: unknown[]) => unknown>(),
  executed: [] as { command: string; args: unknown[] }[],
  diagnostics: [] as Diagnostic[],
  documents: new Map<string, TextDocument>(),
  extensions: new Map<string, unknown>(),
  clipboard: "",
  opened: [] as string[],
  workspaceRoot: "/work",
};

export function resetTestState(): void {
  testState.shown = [];
  testState.choice = undefined;
  testState.inputBox = undefined;
  testState.commands.clear();
  testState.executed = [];
  testState.diagnostics = [];
  testState.documents.clear();
  testState.extensions.clear();
  testState.clipboard = "";
  testState.opened = [];
  window.activeTextEditor = undefined;
  window.activeColorTheme = { kind: ColorThemeKind.Light };
  env.language = "en";
}

export const l10n = {
  t(message: string, ...args: unknown[]): string {
    return message.replace(/\{(\d+)\}/g, (all, n: string) =>
      n in args ? String(args[Number(n)]) : all,
    );
  },
};

const show =
  (level: Shown["level"]) =>
  async (text: string, ...items: unknown[]) => {
    testState.shown.push({ level, text });
    const choice = testState.choice;
    return items.includes(choice) ? choice : undefined;
  };

type Listener<T> = (value: T) => void;

export class EventEmitter<T> {
  #listeners = new Set<Listener<T>>();
  event = (listener: Listener<T>) => {
    this.#listeners.add(listener);
    return { dispose: () => this.#listeners.delete(listener) };
  };
  fire(value: T): void {
    for (const listener of this.#listeners) listener(value);
  }
}

export const window = {
  createOutputChannel: () => ({
    error: () => {},
    info: () => {},
    dispose: () => {},
  }),
  activeTextEditor: undefined as
    { document: TextDocument; selection: Selection } | undefined,
  activeColorTheme: { kind: ColorThemeKind.Light },
  showInformationMessage: show("info"),
  showWarningMessage: show("warning"),
  showErrorMessage: show("error"),
  showInputBox: async () => testState.inputBox,
};

export const commands = {
  registerCommand(command: string, callback: (...args: unknown[]) => unknown) {
    testState.commands.set(command, callback);
    return { dispose: () => testState.commands.delete(command) };
  },
  async executeCommand(command: string, ...args: unknown[]) {
    testState.executed.push({ command, args });
    return testState.commands.get(command)?.(...args);
  },
};

export const workspace = {
  asRelativePath(uri: Uri | string): string {
    const p = typeof uri === "string" ? uri : uri.path;
    const root = `${testState.workspaceRoot}/`;
    return p.startsWith(root) ? p.slice(root.length) : p;
  },
  async openTextDocument(uri: Uri): Promise<TextDocument> {
    const document = testState.documents.get(uri.path);
    if (!document) throw new Error(`no document ${uri.path}`);
    return document;
  },
  getConfiguration() {
    return { get: () => undefined };
  },
};

export const languages = {
  getDiagnostics: () => testState.diagnostics,
};

export const extensions = {
  getExtension(id: string) {
    const exports = testState.extensions.get(id);
    return exports === undefined
      ? undefined
      : { isActive: true, exports, activate: async () => exports };
  },
};

export const env = {
  language: "en",
  appName: "Visual Studio Code",
  clipboard: {
    async writeText(text: string) {
      testState.clipboard = text;
    },
  },
  async openExternal(uri: Uri) {
    testState.opened.push(uri.toString());
    return true;
  },
};

export const version = "1.105.0";
