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
  /** Fin de ligne du document (CRLF sous Windows) : hors de `lineAt().text`,
   * comme dans VS Code. */
  readonly #eol: number;
  constructor(
    file: string,
    readonly text: string,
    readonly languageId = "typescript",
  ) {
    this.uri = file.includes("://") ? Uri.parse(file) : Uri.file(file);
    this.#lines = text.split(/\r?\n/);
    this.#eol = text.includes("\r\n") ? 2 : 1;
  }
  get lineCount(): number {
    return this.#lines.length;
  }
  lineAt(line: number): { text: string } {
    return { text: this.#lines[line] ?? "" };
  }
  #offset(p: Position): number {
    let offset = 0;
    for (let i = 0; i < p.line; i++) {
      offset += this.#lines[i].length + this.#eol;
    }
    return offset + p.character;
  }
  getText(range?: Range): string {
    if (!range) return this.text;
    return this.text.slice(this.#offset(range.start), this.#offset(range.end));
  }
}

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

export class ThemeColor {
  constructor(readonly id: string) {}
}

export enum TextEditorRevealType {
  Default = 0,
  InCenter = 1,
  InCenterIfOutsideViewport = 2,
  AtTop = 3,
}

/** Motif relatif à un dossier (`files.readonlyInclude`…). */
export class RelativePattern {
  readonly baseUri: Uri;
  constructor(
    base: { uri: Uri } | Uri | string,
    readonly pattern: string,
  ) {
    this.baseUri =
      typeof base === "string"
        ? Uri.file(base)
        : base instanceof Uri
          ? base
          : base.uri;
  }
}

/** Motif glob (`**`, `*`, `?`, `{a,b}`) en expression régulière. */
function globToRegExp(glob: string): RegExp {
  let out = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      i++;
      if (glob[i + 1] === "/") {
        i++;
        out += "(?:.*/)?";
      } else out += ".*";
    } else if (c === "*") out += "[^/]*";
    else if (c === "?") out += "[^/]";
    else if (c === "{") out += "(?:";
    else if (c === "}") out += ")";
    else if (c === ",") out += "|";
    else out += c.replace(/[.+^$()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${out}$`);
}

/** Proposition en ligne (autocomplétion). */
export class InlineCompletionItem {
  constructor(
    readonly insertText: string,
    readonly range?: Range,
  ) {}
}

export enum InlineCompletionTriggerKind {
  Invoke = 0,
  Automatic = 1,
}

export enum ConfigurationTarget {
  Global = 1,
  Workspace = 2,
  WorkspaceFolder = 3,
}

/** Jeton d'annulation, comme celui que VS Code passe à un fournisseur. */
export class CancellationTokenSource {
  readonly #emitter = new EventEmitter<void>();
  #cancelled = false;
  readonly token: {
    readonly isCancellationRequested: boolean;
    onCancellationRequested(listener: () => void): { dispose(): void };
  };
  constructor() {
    const state = () => this.#cancelled;
    const emitter = this.#emitter;
    this.token = {
      get isCancellationRequested() {
        return state();
      },
      onCancellationRequested: (listener) => emitter.event(listener),
    };
  }
  cancel(): void {
    if (this.#cancelled) return;
    this.#cancelled = true;
    this.#emitter.fire();
  }
}

export enum StatusBarAlignment {
  Left = 1,
  Right = 2,
}

export enum OverviewRulerLane {
  Left = 1,
  Center = 2,
  Right = 4,
  Full = 7,
}

/** Éditeur visible : décorations posées et plages montrées relevées. */
export class TextEditor {
  readonly decorations = new Map<unknown, Range[]>();
  readonly revealed: { range: Range; type: TextEditorRevealType }[] = [];
  visibleRanges: Range[];
  constructor(
    readonly document: TextDocument,
    public selection: Selection = new Selection(
      new Position(0, 0),
      new Position(0, 0),
    ),
  ) {
    this.visibleRanges = [
      new Range(0, 0, Math.max(0, document.lineCount - 1), 0),
    ];
  }
  setDecorations(type: unknown, ranges: Range[]): void {
    this.decorations.set(type, ranges);
  }
  revealRange(range: Range, type: TextEditorRevealType): void {
    this.revealed.push({ range, type });
  }
}

export type StatusItem = {
  text: string;
  name?: string;
  tooltip?: string;
  accessibilityInformation?: { label: string };
  command?: string;
  visible: boolean;
  show(): void;
  hide(): void;
  dispose(): void;
};

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
  decorationTypes: [] as { options: Record<string, unknown> }[],
  statusItems: [] as StatusItem[],
  contexts: {} as Record<string, unknown>,
  /** Réglages (clé complète, ex. « chatonlm.completion.enabled »). */
  config: {} as Record<string, unknown>,
  /** Schémas en lecture seule (`workspace.fs.isWritableFileSystem`). */
  readOnlySchemes: [] as string[],
  /** Choix dans un `showQuickPick` : rend l'élément à prendre. */
  pick: undefined as
    ((items: { label: string }[]) => { label: string } | undefined) | undefined,
  quickPicks: [] as { label: string }[][],
  inlineProviders: [] as { selector: unknown; provider: unknown }[],
  events: newEvents(),
};

/** Évènements de l'éditeur, déclenchés par les essais. */
function newEvents() {
  return {
    visibleEditors: new EventEmitter<unknown>(),
    activeEditor: new EventEmitter<unknown>(),
    selection: new EventEmitter<unknown>(),
    visibleRanges: new EventEmitter<unknown>(),
    documentChange: new EventEmitter<{
      document: TextDocument;
      contentChanges: { range: Range; text: string }[];
    }>(),
    shellStart: new EventEmitter<{ execution: unknown }>(),
    shellEnd: new EventEmitter<{
      execution: unknown;
      exitCode: number | undefined;
    }>(),
    configuration: new EventEmitter<{
      affectsConfiguration(section: string): boolean;
    }>(),
    windowState: new EventEmitter<{ focused: boolean }>(),
  };
}

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
  testState.decorationTypes = [];
  testState.statusItems = [];
  testState.contexts = {};
  testState.config = {};
  testState.readOnlySchemes = [];
  testState.pick = undefined;
  testState.quickPicks = [];
  testState.inlineProviders = [];
  testState.events = newEvents();
  window.activeTextEditor = undefined;
  window.visibleTextEditors = [];
  window.activeColorTheme = { kind: ColorThemeKind.Light };
  env.language = "en";
  env.remoteName = undefined;
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

export const window = {
  createOutputChannel: () => ({
    error: () => {},
    info: () => {},
    dispose: () => {},
  }),
  activeTextEditor: undefined as
    | { document: TextDocument; selection: Selection; visibleRanges?: Range[] }
    | undefined,
  visibleTextEditors: [] as TextEditor[],
  createTextEditorDecorationType(options: Record<string, unknown>) {
    const type = { options, dispose: () => {} };
    testState.decorationTypes.push(type);
    return type;
  },
  createStatusBarItem(): StatusItem {
    const item: StatusItem = {
      text: "",
      visible: false,
      show: () => void (item.visible = true),
      hide: () => void (item.visible = false),
      dispose: () => void (item.visible = false),
    };
    testState.statusItems.push(item);
    return item;
  },
  async showTextDocument(uri: Uri): Promise<TextEditor> {
    const document = testState.documents.get(uri.path);
    if (!document) throw new Error(`no document ${uri.path}`);
    const editor = new TextEditor(document);
    window.visibleTextEditors.push(editor);
    return editor;
  },
  onDidChangeVisibleTextEditors: (listener: (value: unknown) => void) =>
    testState.events.visibleEditors.event(listener),
  onDidChangeActiveTextEditor: (listener: (value: unknown) => void) =>
    testState.events.activeEditor.event(listener),
  onDidChangeTextEditorSelection: (listener: (value: unknown) => void) =>
    testState.events.selection.event(listener),
  onDidChangeTextEditorVisibleRanges: (listener: (value: unknown) => void) =>
    testState.events.visibleRanges.event(listener),
  onDidStartTerminalShellExecution: (
    listener: (value: { execution: unknown }) => void,
  ) => testState.events.shellStart.event(listener),
  onDidEndTerminalShellExecution: (
    listener: (value: {
      execution: unknown;
      exitCode: number | undefined;
    }) => void,
  ) => testState.events.shellEnd.event(listener),
  activeColorTheme: { kind: ColorThemeKind.Light },
  showInformationMessage: show("info"),
  showWarningMessage: show("warning"),
  showErrorMessage: show("error"),
  showInputBox: async () => testState.inputBox,
  async showQuickPick<T extends { label: string }>(
    items: T[],
  ): Promise<T | undefined> {
    testState.quickPicks.push(items);
    return testState.pick?.(items) as T | undefined;
  },
  onDidChangeWindowState: (listener: (value: { focused: boolean }) => void) =>
    testState.events.windowState.event(listener),
};

export const commands = {
  registerCommand(command: string, callback: (...args: unknown[]) => unknown) {
    testState.commands.set(command, callback);
    return { dispose: () => testState.commands.delete(command) };
  },
  async executeCommand(command: string, ...args: unknown[]) {
    testState.executed.push({ command, args });
    if (command === "setContext") testState.contexts[String(args[0])] = args[1];
    return testState.commands.get(command)?.(...args);
  },
};

export const workspace = {
  get workspaceFolders() {
    return [{ uri: Uri.file(testState.workspaceRoot), name: "work", index: 0 }];
  },
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
  getConfiguration(section?: string) {
    const full = (key: string) => (section ? `${section}.${key}` : key);
    return {
      get<T>(key: string, fallback?: T): T | undefined {
        const value = testState.config[full(key)];
        return value === undefined ? fallback : (value as T);
      },
      inspect(key: string) {
        return { globalValue: testState.config[full(key)] };
      },
      async update(key: string, value: unknown) {
        const name = full(key);
        testState.config[name] = value;
        testState.events.configuration.fire({
          affectsConfiguration: (prefix: string) =>
            name === prefix || name.startsWith(`${prefix}.`),
        });
      },
    };
  },
  onDidChangeConfiguration: (
    listener: (value: {
      affectsConfiguration(section: string): boolean;
    }) => void,
  ) => testState.events.configuration.event(listener),
  fs: {
    isWritableFileSystem: (scheme: string) =>
      testState.readOnlySchemes.includes(scheme) ? false : true,
  },
  onDidChangeTextDocument: (
    listener: (value: {
      document: TextDocument;
      contentChanges: { range: Range; text: string }[];
    }) => void,
  ) => testState.events.documentChange.event(listener),
};

export const languages = {
  getDiagnostics: () => testState.diagnostics,
  /** Filtre `{ pattern }` seulement : 10 si le chemin du document y répond. */
  match(
    selector: { pattern?: string | RelativePattern },
    document: TextDocument,
  ): number {
    const { pattern } = selector;
    if (pattern === undefined) return 0;
    const file = document.uri.path;
    if (typeof pattern === "string") {
      return globToRegExp(pattern).test(file) ? 10 : 0;
    }
    const base = `${pattern.baseUri.path.replace(/\/$/, "")}/`;
    if (!file.startsWith(base)) return 0;
    return globToRegExp(pattern.pattern).test(file.slice(base.length)) ? 10 : 0;
  },
  registerInlineCompletionItemProvider(selector: unknown, provider: unknown) {
    testState.inlineProviders.push({ selector, provider });
    return { dispose: () => {} };
  },
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
  /** Fenêtre distante (« wsl », « ssh-remote »…), sinon undefined. */
  remoteName: undefined as string | undefined,
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
