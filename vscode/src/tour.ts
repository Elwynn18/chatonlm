// Visite guidée parlée d'un fichier, d'une sélection ou des modifications d'un
// fichier : le modèle découpe l'explication en segments liés à des lignes
// (`tour.plan`), l'app les lit à voix haute (`speech.read`) ; à chaque segment
// commencé, l'éditeur surligne ses lignes et les centre. Pause, reprise (le
// segment en cours est relu), précédent, suivant, arrêt. Le code part filtré
// comme les actions (hors de cette machine : secrets retirés), avec son reçu.
// Contrat : .tools/research/89-lot2-voice-contract.md (X, « Visite »).
import * as vscode from "vscode";
import type {
  HostMessage,
  Locale,
  Receipt,
  TourKind,
  TourSegment,
  TourView,
} from "../shared/protocol";
import {
  ActionError,
  fenceLanguage,
  MAX_FILE_CHARS,
  relativePath,
} from "./actions";
import { gitApi, type GitRepository } from "./git";
import { type Notification, RpcError } from "./bridge/rpc";
import type { LinkLike } from "./controller";
import { truncateDiff } from "./prompts";
import {
  buildReceipt,
  type Destination,
  filterPieces,
  type Piece,
  splitDiff,
} from "./receipt";

/** Codes d'erreur du lot 2 (contrat E). */
export const VoiceCodes = {
  noVoice: -32030,
  noTranscription: -32031,
  busy: -32032,
  noMicrophone: -32033,
} as const;

/** Plan reçu de l'app, sans confiance : seuls les segments valides restent. */
export function validSegments(
  raw: unknown,
  first: number,
  last: number,
): TourSegment[] {
  const segments = (raw as { segments?: unknown } | null)?.segments;
  if (!Array.isArray(segments)) return [];
  const valid: TourSegment[] = [];
  for (const segment of segments) {
    const { lines, text } = (segment ?? {}) as {
      lines?: unknown;
      text?: unknown;
    };
    if (!Array.isArray(lines) || lines.length !== 2) continue;
    const [a, b] = lines as unknown[];
    if (
      typeof a !== "number" ||
      typeof b !== "number" ||
      !Number.isInteger(a) ||
      !Number.isInteger(b) ||
      a < first ||
      b < a ||
      b > last ||
      typeof text !== "string" ||
      text.trim() === ""
    ) {
      continue;
    }
    valid.push({ lines: [a, b], text: text.trim() });
  }
  return valid;
}

/**
 * Ligne du fichier modifié (à partir de 1) de chaque ligne d'un diff d'un
 * seul fichier : null pour les en-têtes ; une ligne retirée prend la place
 * où elle était (la ligne qui la suit dans le fichier).
 */
export function diffLineMap(diff: string): (number | null)[] {
  const map: (number | null)[] = [];
  let next: number | null = null;
  for (const line of diff.split("\n")) {
    const hunk = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      next = Math.max(1, Number(hunk[1]));
      map.push(null);
      continue;
    }
    if (next === null || line.startsWith("\\")) {
      map.push(null);
      continue;
    }
    if (line.startsWith("-")) {
      map.push(next);
    } else if (line.startsWith("+") || line.startsWith(" ") || line === "") {
      map.push(next);
      next++;
    } else {
      // Fin des sections de changements (autre en-tête).
      map.push(null);
      next = null;
    }
  }
  return map;
}

/** Lignes du fichier couvertes par les lignes `a`…`b` (à partir de 1) du diff. */
export function diffToFile(
  map: (number | null)[],
  [a, b]: [number, number],
): [number, number] | null {
  const lines = map
    .slice(a - 1, b)
    .filter((line): line is number => line !== null);
  if (lines.length > 0) return [Math.min(...lines), Math.max(...lines)];
  const after = map.slice(b).find((line) => line !== null);
  if (after != null) return [after, after];
  const before = map
    .slice(0, a - 1)
    .reverse()
    .find((line) => line !== null);
  return before != null ? [before, before] : null;
}

/**
 * Segments après une modification du fichier (lignes à partir de 0 dans la
 * modification, à partir de 1 dans les segments) : ceux d'après sont
 * décalés, un segment qui contient toute la modification s'allonge ou se
 * raccourcit (sa dernière ligne retirée entière, qui finit au début de la
 * ligne suivante, comprise) ; null si une modification déborde d'un segment
 * (les lignes ne peuvent plus le suivre).
 */
export function shiftSegments(
  segments: TourSegment[],
  change: { start: number; end: number; endCharacter?: number; text: string },
): TourSegment[] | null {
  const added = change.text.split("\n").length - 1;
  const delta = added - (change.end - change.start);
  const shifted: TourSegment[] = [];
  for (const segment of segments) {
    const a = segment.lines[0] - 1;
    const b = segment.lines[1] - 1;
    // Une modification qui finit au tout début de la ligne `a` (lignes
    // ajoutées ou retirées juste avant) ne touche pas le segment : il glisse.
    const before =
      change.end < a || (change.end === a && change.endCharacter === 0);
    if (before) {
      shifted.push({ ...segment, lines: [a + 1 + delta, b + 1 + delta] });
    } else if (change.start > b) {
      shifted.push(segment);
    } else if (
      change.start >= a &&
      (change.end <= b || (change.end === b + 1 && change.endCharacter === 0))
    ) {
      if (b + delta < a) return null;
      shifted.push({ ...segment, lines: [a + 1, b + 1 + delta] });
    } else {
      return null;
    }
  }
  return shifted;
}

export type TourDeps = {
  link: LinkLike;
  post(message: HostMessage): void;
  reveal(): Promise<void>;
  /** Modèle des envois (celui du panneau, sinon celui de l'app). */
  model(): string | null;
  destination(
    model: string | null,
  ): Promise<{ destination: Destination; contextLength: number | null }>;
  locale(): Locale;
  /** Ouvre l'app (bouton d'un message d'erreur). */
  openApp(): Promise<void>;
  /** Segment commencé, ou visite finie (séance vocale `tour` : contexte). */
  onSegment?(): void;
  onEnded?(): void;
};

/** Ce qui part au modèle pour préparer la visite. */
type Source = {
  kind: TourKind;
  uri: vscode.Uri;
  document: vscode.TextDocument;
  path: string;
  language: string;
  piece: Piece;
  /** Lignes du texte envoyé → lignes du fichier. */
  toFile(lines: [number, number]): [number, number] | null;
};

type SpeechEvent = {
  readId: string;
  type: "started" | "finished" | "stopped" | "error";
  segmentId?: string;
  message?: string;
};

/** Événements arrivés avant la réponse de `speech.read` (gardés au plus). */
const MAX_EARLY_EVENTS = 64;

/** Modification du fichier, en lignes (voir `shiftSegments`). */
type Edit = { start: number; end: number; endCharacter?: number; text: string };

/** Segments après des modifications, dans l'ordre (null : impossible). */
function applyEdits(
  segments: TourSegment[],
  edits: Edit[],
): TourSegment[] | null {
  let shifted: TourSegment[] | null = segments;
  for (const edit of edits) {
    if (!shifted) return null;
    shifted = shiftSegments(shifted, edit);
  }
  return shifted;
}

/** Modifications d'un évènement de VS Code, du bas vers le haut (chacune
 * garde ainsi les lignes des suivantes). */
function editsOf(event: vscode.TextDocumentChangeEvent): Edit[] {
  return [...event.contentChanges]
    .sort((x, y) => y.range.start.line - x.range.start.line)
    .map((change) => ({
      start: change.range.start.line,
      end: change.range.end.line,
      endCharacter: change.range.end.character,
      text: change.text,
    }));
}

/** Le diff de travail d'un fichier, d'après son dépôt. */
async function fileDiff(
  repository: GitRepository,
  uri: vscode.Uri,
): Promise<string> {
  if (typeof repository.diffWithHEAD === "function") {
    return repository.diffWithHEAD(uri.fsPath);
  }
  const path = vscode.workspace.asRelativePath(uri, false);
  const all = await repository.diff(false);
  return (
    splitDiff(all).find((section) => path.endsWith(section.file))?.text ?? ""
  );
}

/** Uri visé par une commande (menu de l'éditeur, de la gestion de versions). */
function uriArgument(argument: unknown): vscode.Uri | undefined {
  if (argument instanceof vscode.Uri) return argument;
  const resource = (argument as { resourceUri?: unknown } | undefined)
    ?.resourceUri;
  return resource instanceof vscode.Uri ? resource : undefined;
}

/** Fichier ou sélection de l'éditeur actif. */
function editorSource(): Source {
  const editor = vscode.window.activeTextEditor;
  if (!editor) throw new ActionError(vscode.l10n.t("Open a file first."));
  const { document, selection } = editor;
  const path = relativePath(document.uri);
  const language = fenceLanguage(document.languageId);
  if (!selection.isEmpty) {
    let last = selection.end.line;
    // Sélection de lignes entières : la ligne suivante n'en fait pas partie.
    if (selection.end.character === 0 && last > selection.start.line) last--;
    const first = selection.start.line;
    const text = document.getText(
      new vscode.Range(first, 0, last, document.lineAt(last).text.length),
    );
    if (text.length > MAX_FILE_CHARS) {
      throw new ActionError(
        vscode.l10n.t(
          "This selection is too long for a tour. Select less code.",
        ),
      );
    }
    return {
      kind: "selection",
      uri: document.uri,
      document,
      path,
      language,
      piece: {
        kind: "selection",
        label: path,
        path,
        lines: [first + 1, last + 1],
        text,
        language,
      },
      toFile: (lines) => lines,
    };
  }
  const text = document.getText();
  if (text.length > MAX_FILE_CHARS) {
    throw new ActionError(
      vscode.l10n.t(
        "This file is too long to send whole. Select the part to send.",
      ),
    );
  }
  return {
    kind: "file",
    uri: document.uri,
    document,
    path,
    language,
    piece: {
      kind: "file",
      label: path,
      path,
      lines: [1, Math.max(1, document.lineCount)],
      text,
      language,
    },
    toFile: (lines) => lines,
  };
}

/** Modifications de travail d'un fichier (menu de la gestion de versions). */
async function diffSource(args: unknown[]): Promise<Source> {
  const uri =
    uriArgument(args[0]) ?? vscode.window.activeTextEditor?.document.uri;
  if (!uri) throw new ActionError(vscode.l10n.t("Open a file first."));
  const api = await gitApi();
  if (!api) {
    throw new ActionError(
      vscode.l10n.t("Git is not available in this window."),
    );
  }
  const repository = api.getRepository(uri);
  if (!repository) {
    throw new ActionError(vscode.l10n.t("Open a Git repository first."));
  }
  const diff = await fileDiff(repository, uri);
  if (!diff.trim()) {
    throw new ActionError(
      vscode.l10n.t("This file has no changes to walk through."),
    );
  }
  const { text } = truncateDiff(diff);
  const path = relativePath(uri);
  const map = diffLineMap(text);
  const document = await vscode.workspace.openTextDocument(uri);
  const lastLine = Math.max(1, document.lineCount);
  return {
    kind: "diff",
    uri,
    document,
    path,
    language: fenceLanguage(document.languageId),
    piece: {
      kind: "diff",
      label: path,
      path,
      lines: [1, text.split("\n").length],
      text,
      language: "diff",
    },
    toFile: (lines) => {
      const found = diffToFile(map, lines);
      return found
        ? [Math.min(found[0], lastLine), Math.min(found[1], lastLine)]
        : null;
    },
  };
}

export class Tour {
  readonly #deps: TourDeps;
  #view: TourView | null = null;
  #source: Source | null = null;
  /** Numéro de la requête `tour.plan` (annulée par Stop). */
  #planning: number | null = null;
  /** Modifications du fichier pendant la préparation : appliquées au plan
   * reçu (il décrit le texte envoyé). */
  #pendingEdits: Edit[] = [];
  #readId: string | null = null;
  /** Augmente à chaque lecture demandée : une réponse d'avant est périmée. */
  #readSeq = 0;
  /** Une lecture est demandée, sa réponse pas encore reçue. */
  #awaitingRead = false;
  #early: SpeechEvent[] = [];
  #nextId = 1;
  #decoration: vscode.TextEditorDecorationType | null = null;
  #status: vscode.StatusBarItem | null = null;
  readonly #disposables: vscode.Disposable[] = [];
  readonly #unsubscribe: () => void;

  constructor(deps: TourDeps) {
    this.#deps = deps;
    this.#unsubscribe = deps.link.onNotification((n) =>
      this.#onNotification(n),
    );
    this.#disposables.push(
      vscode.workspace.onDidChangeTextDocument((event) => this.#onEdit(event)),
      // Fichier rouvert ou montré dans un autre groupe : surlignage remis.
      vscode.window.onDidChangeVisibleTextEditors(() => this.#decorate()),
    );
  }

  get view(): TourView | null {
    return this.#view;
  }

  /** Une visite tourne (préparée, lue ou en pause). */
  get active(): boolean {
    return this.#view !== null && this.#view.status !== "ended";
  }

  /** Passage du segment en cours (pour une question), ou null. */
  passage(): { piece: Piece; text: string } | null {
    const view = this.#view;
    const source = this.#source;
    if (!view || !source || view.current < 0) return null;
    const segment = view.segments[view.current];
    if (!segment) return null;
    const { document } = source;
    const [a, b] = segment.lines;
    const last = Math.min(b, document.lineCount) - 1;
    const text = document.getText(
      new vscode.Range(a - 1, 0, last, document.lineAt(last).text.length),
    );
    return {
      piece: {
        kind: "selection",
        label: source.path,
        path: source.path,
        lines: [a, b],
        text,
        language: source.language,
      },
      text: segment.text,
    };
  }

  dispose(): void {
    this.#end(false);
    this.#unsubscribe();
    for (const disposable of this.#disposables) disposable.dispose();
    this.#decoration?.dispose();
    this.#status?.dispose();
  }

  /** Commande `chatonlm.tour` (`diff` : `chatonlm.tourChanges`). */
  async start(diff: boolean, args: unknown[] = []): Promise<void> {
    let source: Source;
    try {
      source = diff ? await diffSource(args) : editorSource();
    } catch (error) {
      if (error instanceof ActionError) {
        void vscode.window.showInformationMessage(error.message);
        return;
      }
      throw error;
    }
    if (!this.#deps.link.connected) {
      await this.#deps.reveal();
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    // Une seule visite à la fois.
    this.stop();
    const model = this.#deps.model();
    const { destination, contextLength } = await this.#deps.destination(model);
    const { pieces, blocked } = filterPieces([source.piece], destination, {
      marker: vscode.l10n.t("[secret removed by ChatonLM]"),
      lines: (label, first, last) =>
        first === last
          ? vscode.l10n.t("{0}, line {1}", label, first)
          : vscode.l10n.t("{0}, lines {1}–{2}", label, first, last),
    });
    const piece = pieces[0];
    if (!piece) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t(
          "Nothing was sent: the model runs outside this computer and everything selected may hold secrets.",
        ),
      );
      return;
    }
    const receipt: Receipt = buildReceipt(
      piece.text,
      pieces,
      blocked,
      destination,
      contextLength,
    );
    const id = this.#nextId++;
    this.#source = source;
    this.#pendingEdits = [];
    this.#view = {
      id,
      kind: source.kind,
      path: source.path,
      status: "planning",
      segments: [],
      current: -1,
      receipt,
    };
    this.#touring(true);
    this.#publish();
    await this.#deps.reveal();
    const [first] = source.piece.lines ?? [1, 1];
    const lineCount = piece.text.split("\n").length;
    const params: Record<string, unknown> = {
      locale: this.#deps.locale(),
      path: source.path,
      language: source.language,
      code: piece.text,
      firstLine: first,
      kind: source.kind,
    };
    if (model) params.model = model;
    const call = this.#deps.link.call<unknown>("tour.plan", params);
    this.#planning = call.id;
    let plan: unknown;
    try {
      plan = await call.result;
    } catch (error) {
      // Arrêtée (ou remplacée) pendant la préparation : rien à dire.
      if (this.#view?.id !== id || this.#view.status !== "planning") return;
      this.#planning = null;
      this.#end(true);
      this.#deps.post({ type: "tour", tour: null });
      void vscode.window.showErrorMessage(
        error instanceof RpcError && error.appMessage
          ? error.appMessage
          : vscode.l10n.t("ChatonLM couldn't prepare the tour."),
      );
      return;
    }
    this.#planning = null;
    // Arrêtée (ou remplacée) pendant la préparation.
    if (this.#view?.id !== id || this.#view.status !== "planning") return;
    const planned = validSegments(plan, first, first + lineCount - 1)
      .map((segment) => {
        const lines = source.toFile(segment.lines);
        return lines ? { ...segment, lines } : null;
      })
      .filter((segment): segment is TourSegment => segment !== null);
    const edits = this.#pendingEdits;
    this.#pendingEdits = [];
    const segments = applyEdits(planned, edits);
    if (!segments) {
      this.#end(true);
      this.#deps.post({ type: "tour", tour: null });
      this.#tellChanged();
      return;
    }
    if (segments.length === 0) {
      this.#end(true);
      this.#deps.post({ type: "tour", tour: null });
      void vscode.window.showErrorMessage(
        vscode.l10n.t("ChatonLM couldn't prepare the tour."),
      );
      return;
    }
    this.#view = { ...this.#view, segments, status: "paused" };
    this.#publish();
    await this.#read(0);
  }

  pause(): void {
    const view = this.#view;
    if (!view || view.status !== "playing") return;
    this.#view = { ...view, status: "paused" };
    this.#stopReading();
    // Lecture demandée sans réponse encore : périmée (arrêtée à sa réponse).
    this.#readSeq++;
    this.#awaitingRead = false;
    this.#early = [];
    this.#publish();
  }

  /** Reprise : le segment en cours est relu depuis son début. */
  async resume(): Promise<void> {
    const view = this.#view;
    if (!view || view.status !== "paused") return;
    await this.#read(Math.max(0, view.current));
  }

  async togglePause(): Promise<void> {
    if (this.#view?.status === "playing") this.pause();
    else await this.resume();
  }

  async next(): Promise<void> {
    const view = this.#view;
    if (!view || !this.active || view.segments.length === 0) return;
    await this.goto(Math.min(view.segments.length - 1, view.current + 1));
  }

  async previous(): Promise<void> {
    const view = this.#view;
    if (!view || !this.active || view.segments.length === 0) return;
    await this.goto(Math.max(0, view.current - 1));
  }

  /** Lecture reprise au segment `index` (même après la fin de la visite). */
  async goto(index: number): Promise<void> {
    const view = this.#view;
    if (!view || view.status === "planning") return;
    if (!Number.isInteger(index) || index < 0 || index >= view.segments.length)
      return;
    if (view.status === "ended") this.#touring(true);
    await this.#read(index);
  }

  stop(): void {
    if (!this.#view) return;
    if (this.#planning !== null) {
      this.#deps.link.cancel(this.#planning);
      this.#planning = null;
    }
    this.#end(true);
  }

  /** Liaison perdue : la lecture s'est arrêtée avec elle. */
  disconnected(): void {
    if (!this.active) return;
    this.#readId = null;
    this.#planning = null;
    this.#end(true);
  }

  /** Fin : surlignage retiré, lecture arrêtée, segments encore lisibles. */
  #end(publish: boolean): void {
    const view = this.#view;
    if (!view) return;
    this.#stopReading();
    this.#readSeq++;
    if (view.status !== "ended") {
      this.#view = { ...view, status: "ended" };
    }
    this.#touring(false);
    this.#clearDecorations();
    this.#status?.hide();
    if (publish) this.#publish();
    this.#deps.onEnded?.();
  }

  #stopReading(): void {
    const readId = this.#readId;
    this.#readId = null;
    if (readId && this.#deps.link.connected) {
      void this.#deps.link.request("speech.stop", { readId }).catch(() => {});
    }
  }

  async #read(from: number): Promise<void> {
    const view = this.#view;
    if (!view) return;
    this.#stopReading();
    const seq = ++this.#readSeq;
    this.#view = { ...view, status: "playing", current: from };
    this.#awaitingRead = true;
    this.#early = [];
    this.#show(from);
    this.#publish();
    const segments = view.segments.slice(from).map((segment, offset) => ({
      id: `${view.id}:${from + offset}`,
      text: segment.text,
    }));
    let readId: string;
    try {
      const result = await this.#deps.link.request<{ readId?: unknown }>(
        "speech.read",
        { segments, language: this.#deps.locale() },
      );
      if (typeof result?.readId !== "string") throw new Error("no readId");
      readId = result.readId;
    } catch (error) {
      if (seq !== this.#readSeq) return;
      this.#awaitingRead = false;
      if (this.#view) this.#view = { ...this.#view, status: "paused" };
      this.#publish();
      void this.#speechFailed(error);
      return;
    }
    if (seq !== this.#readSeq) {
      // Remplacée (ou arrêtée) pendant la demande : cette lecture s'arrête.
      if (this.#deps.link.connected) {
        void this.#deps.link.request("speech.stop", { readId }).catch(() => {});
      }
      return;
    }
    this.#awaitingRead = false;
    this.#readId = readId;
    const early = this.#early;
    this.#early = [];
    for (const event of early) this.#onSpeech(event);
  }

  async #speechFailed(error: unknown): Promise<void> {
    if (
      error instanceof RpcError &&
      (error.code === VoiceCodes.noVoice ||
        error.code === VoiceCodes.noTranscription)
    ) {
      const open = vscode.l10n.t("Open ChatonLM");
      const choice = await vscode.window.showErrorMessage(
        vscode.l10n.t(
          "ChatonLM has no voice for this language yet. Open ChatonLM to download one.",
        ),
        open,
      );
      if (choice === open) await this.#deps.openApp();
      return;
    }
    void vscode.window.showErrorMessage(
      error instanceof RpcError && error.appMessage
        ? error.appMessage
        : vscode.l10n.t("ChatonLM couldn't read the tour aloud."),
    );
  }

  #onNotification({ method, params }: Notification): void {
    if (method !== "speech.event") return;
    const event = params as SpeechEvent | null;
    if (!event || typeof event.readId !== "string") return;
    if (this.#readId === null) {
      // Avant la réponse de `speech.read` : gardé pour elle.
      if (this.#awaitingRead && this.#early.length < MAX_EARLY_EVENTS) {
        this.#early.push(event);
      }
      return;
    }
    this.#onSpeech(event);
  }

  #onSpeech(event: SpeechEvent): void {
    const view = this.#view;
    if (!view || event.readId !== this.#readId) return;
    const index = this.#indexOf(event.segmentId);
    switch (event.type) {
      case "started":
        if (index === null) return;
        this.#view = { ...view, status: "playing", current: index };
        this.#show(index);
        this.#publish();
        this.#deps.onSegment?.();
        return;
      case "finished":
        if (index === view.segments.length - 1) {
          this.#readId = null;
          this.#end(true);
        }
        return;
      case "stopped":
        // Une autre lecture a pris la voix (dans l'app) : visite en pause.
        this.#readId = null;
        this.#view = { ...view, status: "paused" };
        this.#publish();
        return;
      case "error":
        this.#readId = null;
        this.#view = { ...view, status: "paused" };
        this.#publish();
        void vscode.window.showErrorMessage(
          event.message ??
            vscode.l10n.t("ChatonLM couldn't read the tour aloud."),
        );
        return;
    }
  }

  #indexOf(segmentId: string | undefined): number | null {
    const view = this.#view;
    if (!view || typeof segmentId !== "string") return null;
    const [tour, index] = segmentId.split(":").map(Number);
    if (tour !== view.id || !Number.isInteger(index)) return null;
    return index >= 0 && index < view.segments.length ? index : null;
  }

  /** Segment montré : lignes surlignées et centrées, barre d'état. */
  #show(index: number): void {
    const view = this.#view;
    const source = this.#source;
    const segment = view?.segments[index];
    if (!view || !source || !segment) return;
    void this.#reveal(source.uri, segment.lines);
    this.#decorate();
  }

  async #reveal(uri: vscode.Uri, [a, b]: [number, number]): Promise<void> {
    let editor = vscode.window.visibleTextEditors.find(
      (e) => e.document.uri.toString() === uri.toString(),
    );
    if (!editor) {
      try {
        editor = await vscode.window.showTextDocument(uri, {
          preserveFocus: true,
          preview: false,
        });
      } catch {
        return;
      }
    }
    const last = Math.min(b, editor.document.lineCount) - 1;
    const range = new vscode.Range(
      a - 1,
      0,
      last,
      editor.document.lineAt(last).text.length,
    );
    editor.revealRange(range, vscode.TextEditorRevealType.InCenter);
    this.#decorate();
  }

  #decorationType(): vscode.TextEditorDecorationType {
    // Couleurs du thème de l'éditeur : celles du surlignage d'une plage.
    this.#decoration ??= vscode.window.createTextEditorDecorationType({
      isWholeLine: true,
      backgroundColor: new vscode.ThemeColor("editor.rangeHighlightBackground"),
      borderColor: new vscode.ThemeColor("focusBorder"),
      borderStyle: "solid",
      borderWidth: "0 0 0 2px",
      overviewRulerColor: new vscode.ThemeColor("focusBorder"),
      overviewRulerLane: vscode.OverviewRulerLane.Full,
    });
    return this.#decoration;
  }

  /** Surlignage du segment en cours dans chaque éditeur du fichier. */
  #decorate(): void {
    const view = this.#view;
    const source = this.#source;
    if (!view || !source || view.status === "ended" || view.current < 0) {
      return;
    }
    const segment = view.segments[view.current];
    if (!segment) return;
    const type = this.#decorationType();
    for (const editor of vscode.window.visibleTextEditors) {
      if (editor.document.uri.toString() !== source.uri.toString()) continue;
      const [a, b] = segment.lines;
      const last = Math.min(b, editor.document.lineCount) - 1;
      editor.setDecorations(type, [new vscode.Range(a - 1, 0, last, 0)]);
    }
    this.#statusBar();
  }

  #clearDecorations(): void {
    if (!this.#decoration) return;
    for (const editor of vscode.window.visibleTextEditors) {
      editor.setDecorations(this.#decoration, []);
    }
  }

  #statusBar(): void {
    const view = this.#view;
    if (!view || view.status === "ended" || view.segments.length === 0) {
      this.#status?.hide();
      return;
    }
    this.#status ??= vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      100,
    );
    const step = Math.max(1, view.current + 1);
    const paused = view.status === "paused";
    this.#status.text = `${paused ? "$(debug-pause)" : "$(unmute)"} ${
      paused
        ? vscode.l10n.t(
            "ChatonLM tour {0}/{1}, paused",
            step,
            view.segments.length,
          )
        : vscode.l10n.t("ChatonLM tour {0}/{1}", step, view.segments.length)
    }`;
    this.#status.tooltip = vscode.l10n.t("Pause or resume the tour");
    this.#status.command = "chatonlm.tourTogglePause";
    this.#status.show();
  }

  /** Fichier modifié pendant la visite : les lignes suivent, ou elle s'arrête. */
  #onEdit(event: vscode.TextDocumentChangeEvent): void {
    const view = this.#view;
    const source = this.#source;
    if (!view || !source || view.status === "ended") return;
    if (event.document.uri.toString() !== source.uri.toString()) return;
    if (event.contentChanges.length === 0) return;
    if (view.status === "planning") {
      // Pas encore de segments : gardées pour le plan à venir.
      this.#pendingEdits.push(...editsOf(event));
      return;
    }
    const segments = applyEdits(view.segments, editsOf(event));
    if (!segments) {
      this.stop();
      this.#tellChanged();
      return;
    }
    this.#view = { ...view, segments };
    this.#decorate();
    this.#publish();
  }

  #tellChanged(): void {
    void vscode.window.showInformationMessage(
      vscode.l10n.t(
        "The file changed too much during the tour, so the tour stopped.",
      ),
    );
  }

  #touring(on: boolean): void {
    void vscode.commands.executeCommand("setContext", "chatonlm.touring", on);
  }

  #publish(): void {
    this.#deps.post({ type: "tour", tour: this.#view });
    this.#statusBar();
  }
}
