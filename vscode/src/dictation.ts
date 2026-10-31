// Dictée de ChatonLM dans l'éditeur : la séance (micro, transcription
// locale, polissage selon les réglages de la dictée) reste dans l'app ;
// l'extension la démarre avec les noms du projet (glossaire : symboles du
// fichier actif, identifiants fréquents, noms des fichiers ouverts), puis
// insère le texte au curseur (`editor.edit`, jamais par le presse-papiers ;
// sélection remplacée). Barre d'état pendant la dictée ; Échap ou la
// commande l'arrêtent. Contrat : .tools/research/95-lot4b-contract.md (1) ;
// formes du pont : src-tauri/src/bridge/dictation.rs (`dictation.error
// { message }` : échec après l'ouverture du micro ; -32800 : arrêtée avant).
import path from "node:path";
import * as vscode from "vscode";
import type { LinkLike } from "./controller";
import { type Notification, RpcError } from "./bridge/rpc";
import { isSecretFile } from "./receipt";
import { VoiceCodes } from "./tour";

/** Glossaire d'une séance : 200 mots au plus, 64 caractères chacun. */
export const GLOSSARY_MAX = 200;
export const WORD_MAX = 64;
/** Texte du fichier actif parcouru pour les identifiants. */
const SCAN_MAX = 200_000;
/** Délai laissé au fournisseur de symboles. */
export const SYMBOLS_TIMEOUT_MS = 1500;
/** Texte encore attendu après la fin de la séance. */
export const LATE_TEXT_MS = 5000;
/** Requête arrêtée (`RequestCancelled` de LSP). */
const CANCELLED = -32800;

export type DictationPhase =
  "idle" | "recording" | "transcribing" | "polishing";

/** Un nom acceptable pour le glossaire (identifiant ou mot du projet). */
const NAME = /^[\p{L}_$][\p{L}\p{N}_$.-]*$/u;
/** Identifiants qui valent d'être appris : camelCase, PascalCase composé,
 * snake_case, SCREAMING_CASE. */
const IDENTIFIER =
  /\b(?:[a-z][a-z0-9]*[A-Z][A-Za-z0-9]*|[A-Z][a-z0-9]+[A-Z][A-Za-z0-9]*|[A-Za-z][A-Za-z0-9]*_[A-Za-z0-9_]*[A-Za-z0-9])\b/g;

/** Nom propre à garder : identifiant de 2 à 64 caractères, pas une valeur
 * qui ressemble à un secret (longue suite de chiffres et de lettres). */
export function cleanWord(raw: string): string | null {
  // eslint-disable-next-line no-control-regex
  const word = raw.replace(/[\u0000-\u001f\u007f]/g, "").trim();
  if (word.length < 2 || word.length > WORD_MAX) return null;
  if (!NAME.test(word)) return null;
  if ((word.match(/\d/g)?.length ?? 0) >= 6) return null;
  return word;
}

/** Identifiants du texte, les plus fréquents d'abord. */
export function frequentIdentifiers(text: string): string[] {
  const counts = new Map<string, number>();
  for (const match of text.slice(0, SCAN_MAX).matchAll(IDENTIFIER)) {
    counts.set(match[0], (counts.get(match[0]) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([word]) => word);
}

/**
 * Glossaire de la séance, les noms les plus proches du fichier ouvert
 * d'abord (Whisper n'en garde qu'une partie) : symboles du fichier actif,
 * identifiants fréquents du fichier, puis noms des fichiers ouverts. Rien
 * n'est tiré d'un fichier sensible (`.env`, clés…).
 */
export function buildGlossary(input: {
  path: string | null;
  symbols: string[];
  text: string;
  files: string[];
}): string[] {
  const secret = input.path !== null && isSecretFile(input.path);
  const candidates = [
    ...(secret ? [] : input.symbols),
    ...(secret ? [] : frequentIdentifiers(input.text)),
    ...input.files
      .filter((file) => !isSecretFile(file))
      .map((file) => path.basename(file).replace(/\.[^.]*$/, "")),
  ];
  const seen = new Set<string>();
  const glossary: string[] = [];
  for (const candidate of candidates) {
    const word = cleanWord(candidate);
    if (!word || seen.has(word)) continue;
    seen.add(word);
    glossary.push(word);
    if (glossary.length >= GLOSSARY_MAX) break;
  }
  return glossary;
}

type SymbolLike = { name?: unknown; children?: unknown };

/** Noms des symboles, dans l'ordre du fichier (enfants après parents). */
export function symbolNames(symbols: unknown): string[] {
  const names: string[] = [];
  const walk = (list: unknown) => {
    if (!Array.isArray(list)) return;
    for (const symbol of list as SymbolLike[]) {
      if (typeof symbol?.name === "string") names.push(symbol.name);
      walk(symbol?.children);
    }
  };
  walk(symbols);
  return names;
}

/** Symboles du document (fournisseurs de langue), sans les attendre trop. */
async function documentSymbols(uri: vscode.Uri): Promise<string[]> {
  try {
    const symbols = await Promise.race([
      vscode.commands.executeCommand<unknown>(
        "vscode.executeDocumentSymbolProvider",
        uri,
      ),
      new Promise<undefined>((resolve) =>
        setTimeout(() => resolve(undefined), SYMBOLS_TIMEOUT_MS),
      ),
    ]);
    return symbolNames(symbols);
  } catch {
    return [];
  }
}

/** Fichiers ouverts (onglets et documents), relatifs au dossier. */
function openFiles(): string[] {
  const uris: vscode.Uri[] = [];
  for (const group of vscode.window.tabGroups?.all ?? []) {
    for (const tab of group.tabs) {
      const input = tab.input as { uri?: unknown } | undefined;
      if (input?.uri instanceof vscode.Uri) uris.push(input.uri);
    }
  }
  for (const document of vscode.workspace.textDocuments ?? []) {
    uris.push(document.uri);
  }
  return uris
    .filter((uri) => uri.scheme === "file" || uri.scheme === "untitled")
    .map((uri) => vscode.workspace.asRelativePath(uri, false));
}

/** Glossaire tiré de l'éditeur actif et des fichiers ouverts. */
export async function glossaryFor(
  editor: vscode.TextEditor | undefined,
): Promise<string[]> {
  const document = editor?.document;
  const local =
    document &&
    (document.uri.scheme === "file" || document.uri.scheme === "untitled");
  return buildGlossary({
    path: local ? vscode.workspace.asRelativePath(document.uri, false) : null,
    symbols: local ? await documentSymbols(document.uri) : [],
    text: local ? document.getText() : "",
    files: openFiles(),
  });
}

/** Phase de la dictée, depuis `dictation.state` (texte ou `{ phase }`). */
export function phaseOf(state: unknown): DictationPhase | null {
  const value =
    typeof state === "string"
      ? state
      : (state as { phase?: unknown } | null)?.phase;
  return value === "idle" ||
    value === "recording" ||
    value === "transcribing" ||
    value === "polishing"
    ? value
    : null;
}

/** Éditeur où l'on peut écrire. */
function writable(editor: vscode.TextEditor | undefined): boolean {
  if (!editor) return false;
  const { scheme } = editor.document.uri;
  if (scheme === "output" || scheme === "vscode-settings") return false;
  return vscode.workspace.fs.isWritableFileSystem(scheme) !== false;
}

/**
 * Texte inséré au curseur de chaque sélection (une sélection est
 * remplacée). Une espace est ajoutée entre un mot déjà là et un mot dicté.
 */
export async function insertAtCursor(
  editor: vscode.TextEditor,
  text: string,
): Promise<boolean> {
  const selections = editor.selections?.length
    ? editor.selections
    : [editor.selection];
  return editor.edit((builder) => {
    for (const selection of selections) {
      const start = selection.start;
      const before =
        start.character > 0
          ? editor.document.lineAt(start.line).text.charAt(start.character - 1)
          : "";
      const spaced =
        /[\p{L}\p{N}.,;:!?)]/u.test(before) && /^[\p{L}\p{N}]/u.test(text)
          ? ` ${text}`
          : text;
      builder.replace(selection, spaced);
    }
  });
}

export type DictationDeps = {
  link: LinkLike;
  openApp(): Promise<void>;
};

/** Texte d'un refus de la dictée, traduit. */
export function dictationErrorText(error: unknown): string {
  if (error instanceof RpcError) {
    switch (error.code) {
      case VoiceCodes.noTranscription:
        return vscode.l10n.t(
          "ChatonLM doesn't have its speech recognition model yet. Open ChatonLM to download it.",
        );
      case VoiceCodes.busy:
        return (
          error.appMessage ??
          vscode.l10n.t(
            "The microphone is already in use in ChatonLM (dictation or voice mode).",
          )
        );
      case VoiceCodes.noMicrophone:
        return (
          error.appMessage ?? vscode.l10n.t("The microphone isn't available.")
        );
      case -32601:
        return vscode.l10n.t(
          "This version of ChatonLM can't dictate into VS Code. Update ChatonLM.",
        );
    }
    if (error.appMessage) return error.appMessage;
  }
  return vscode.l10n.t("ChatonLM couldn't start the dictation.");
}

export class Dictation {
  readonly #deps: DictationDeps;
  /** Phase de la séance, null : aucune. */
  #phase: DictationPhase | "starting" | null = null;
  /** Augmente à chaque séance. */
  #session = 0;
  /** Éditeur actif au départ (repli si l'éditeur actif n'accepte rien). */
  #editor: vscode.TextEditor | null = null;
  /** Texte encore attendu après la fin (horodatage), ou 0. */
  #lateUntil = 0;
  /** Arrêt de l'écoute demandé : un état « recording » encore en route
   * n'est plus montré. */
  #stopAsked = false;
  #status: vscode.StatusBarItem | null = null;
  #dictating = false;
  readonly #unsubscribe: () => void;

  constructor(deps: DictationDeps) {
    this.#deps = deps;
    this.#unsubscribe = deps.link.onNotification((n) =>
      this.#onNotification(n),
    );
  }

  get active(): boolean {
    return this.#phase !== null;
  }

  get phase(): DictationPhase | "starting" | null {
    return this.#phase;
  }

  dispose(): void {
    this.#unsubscribe();
    if (this.#phase && this.#deps.link.connected) {
      void this.#deps.link.request("dictation.stop", {}).catch(() => {});
    }
    this.#phase = null;
    this.#setDictating(false);
    this.#status?.dispose();
    this.#status = null;
  }

  /** Commande « Dictate » : démarre, ou arrête la séance en cours. */
  async toggle(): Promise<void> {
    if (this.#phase) {
      await this.stop();
      return;
    }
    await this.start();
  }

  async start(): Promise<void> {
    if (this.#phase) return;
    // Liaison en cours d'établissement (ouverture de VS Code, appairage
    // tout juste fait) : attendue un moment plutôt que refusée.
    if (!this.#deps.link.connected) await connectedSoon(this.#deps.link);
    if (!this.#deps.link.connected) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    const editor = vscode.window.activeTextEditor;
    if (!writable(editor)) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("Open a file you can edit, then dictate."),
      );
      return;
    }
    const session = ++this.#session;
    this.#editor = editor ?? null;
    this.#lateUntil = 0;
    this.#stopAsked = false;
    this.#set("starting");
    const glossary = await glossaryFor(editor);
    if (session !== this.#session) return;
    try {
      await this.#deps.link.request("dictation.start", { glossary });
    } catch (error) {
      if (session !== this.#session) return;
      this.#session++;
      this.#set(null);
      // Arrêtée avant l'ouverture du micro (bandeau de l'app) : rien à dire.
      if (error instanceof RpcError && error.code === CANCELLED) return;
      await this.#refused(error);
      return;
    }
    // Pas encore d'état de l'app : elle écoute dès sa réponse.
    if (session === this.#session && this.#phase === "starting") {
      this.#set("recording");
    }
  }

  async #refused(error: unknown): Promise<void> {
    const text = dictationErrorText(error);
    if (
      error instanceof RpcError &&
      error.code === VoiceCodes.noTranscription
    ) {
      const open = vscode.l10n.t("Open ChatonLM");
      const choice = await vscode.window.showErrorMessage(text, open);
      if (choice === open) await this.#deps.openApp();
      return;
    }
    void vscode.window.showErrorMessage(text);
  }

  /** Arrêt de l'écoute : l'app transcrit ce qui a été dit. */
  async stop(): Promise<void> {
    if (!this.#phase) return;
    if (this.#phase === "starting") {
      // Pas encore d'écoute : rien à transcrire.
      this.#session++;
      this.#set(null);
      if (this.#deps.link.connected) {
        void this.#deps.link.request("dictation.stop", {}).catch(() => {});
      }
      return;
    }
    this.#stopAsked = true;
    if (this.#phase === "recording") this.#set("transcribing");
    try {
      await this.#deps.link.request("dictation.stop", {});
    } catch {
      // Séance déjà finie dans l'app.
    }
  }

  /** Liaison perdue : l'app arrête le micro avec la connexion. */
  disconnected(): void {
    if (!this.#phase) return;
    this.#session++;
    this.#lateUntil = 0;
    this.#set(null);
  }

  #onNotification({ method, params }: Notification): void {
    const p = (params ?? {}) as {
      state?: unknown;
      text?: unknown;
      message?: unknown;
    };
    if (method === "dictation.state") {
      const phase = phaseOf(p.state);
      if (!phase || !this.#phase) return;
      if (phase === "idle") {
        // Fin de la séance : le texte peut encore suivre de peu.
        if (this.#phase !== "starting") {
          this.#lateUntil = Date.now() + LATE_TEXT_MS;
          this.#set(null);
        }
        return;
      }
      if (phase === "recording" && this.#stopAsked) return;
      this.#set(phase);
      return;
    }
    if (method === "dictation.error") {
      // Échec après l'ouverture du micro (rien entendu, transcription).
      const expected =
        (this.#phase !== null && this.#phase !== "starting") ||
        Date.now() < this.#lateUntil;
      if (!expected) return;
      this.#lateUntil = 0;
      this.#session++;
      this.#set(null);
      void vscode.window.showErrorMessage(
        typeof p.message === "string" && p.message
          ? p.message
          : vscode.l10n.t("ChatonLM couldn't transcribe the dictation."),
      );
      return;
    }
    if (method === "dictation.text") {
      if (typeof p.text !== "string") return;
      const expected =
        (this.#phase !== null && this.#phase !== "starting") ||
        Date.now() < this.#lateUntil;
      if (!expected) return;
      this.#lateUntil = 0;
      void this.#insert(p.text);
    }
  }

  async #insert(raw: string): Promise<void> {
    const text = raw.trim();
    if (!text) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("ChatonLM heard nothing."),
      );
      return;
    }
    // L'éditeur du départ (celui d'où vient le glossaire) s'il est encore
    // visible et modifiable : un fichier ou un message de commit ouvert
    // pendant la transcription ne reçoit pas le texte. Sinon l'éditeur actif.
    const start =
      this.#editor && vscode.window.visibleTextEditors.includes(this.#editor)
        ? this.#editor
        : undefined;
    const active = vscode.window.activeTextEditor;
    const editor = writable(start)
      ? start
      : writable(active)
        ? active
        : undefined;
    if (editor && (await insertAtCursor(editor, text))) return;
    // Nulle part où écrire : proposé à copier (geste de l'utilisateur).
    const copy = vscode.l10n.t("Copy");
    const choice = await vscode.window.showWarningMessage(
      vscode.l10n.t("No editor to type into. ChatonLM heard: {0}", text),
      copy,
    );
    if (choice === copy) await vscode.env.clipboard.writeText(text);
  }

  #set(phase: DictationPhase | "starting" | null): void {
    this.#phase = phase === "idle" ? null : phase;
    this.#setDictating(
      this.#phase === "starting" || this.#phase === "recording",
    );
    this.#statusBar();
  }

  #statusBar(): void {
    const phase = this.#phase;
    if (!phase) {
      this.#status?.hide();
      return;
    }
    this.#status ??= vscode.window.createStatusBarItem(
      "chatonlm.dictation",
      vscode.StatusBarAlignment.Left,
      99,
    );
    this.#status.name = vscode.l10n.t("ChatonLM dictation");
    this.#status.text =
      phase === "starting"
        ? `$(mic) ${vscode.l10n.t("ChatonLM: getting ready to listen")}`
        : phase === "recording"
          ? `$(mic-filled) ${vscode.l10n.t("ChatonLM is listening (Escape to stop)")}`
          : `$(loading~spin) ${vscode.l10n.t("ChatonLM is transcribing")}`;
    this.#status.tooltip = vscode.l10n.t(
      "Dictation into the editor through ChatonLM. Click to stop listening.",
    );
    this.#status.command = "chatonlm.dictateStop";
    this.#status.show();
  }

  #setDictating(dictating: boolean): void {
    if (dictating === this.#dictating) return;
    this.#dictating = dictating;
    void vscode.commands.executeCommand(
      "setContext",
      "chatonlm.dictating",
      dictating,
    );
  }
}

/** Attente de la liaison quand elle s'établit (au plus `ms`), rien sinon. */
export function connectedSoon(link: LinkLike, ms = 5000): Promise<void> {
  if (link.connected || link.state.connection !== "connecting") {
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    let stop = () => {};
    const timer = setTimeout(() => {
      stop();
      resolve();
    }, ms);
    stop = link.onState((state) => {
      if (state.connection !== "connecting") {
        clearTimeout(timer);
        stop();
        resolve();
      }
    });
  });
}
