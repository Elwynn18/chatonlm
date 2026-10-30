// Séance vocale tenue par VS Code : chaton en caoutchouc (`duck` : on explique
// son bug, le modèle pose des questions) ou question sur un passage de la
// visite (`tour`). L'audio reste dans l'app (micro, transcription, voix) ;
// l'extension démarre, coupe et arrête la séance, et lui donne le contexte
// (passage de l'éditeur, dernière commande échouée), filtré hors machine comme
// le reçu. Contrat : .tools/research/89-lot2-voice-contract.md (X).
import * as vscode from "vscode";
import type { ChatEvent } from "../../src/bindings/ChatEvent";
import type { VoiceModeState } from "../../src/bindings/VoiceModeState";
import type { HostMessage, ReceiptItem, VoiceView } from "../shared/protocol";
import { headingOf } from "./actions";
import type { LinkLike } from "./controller";
import { type Notification, RpcError } from "./bridge/rpc";
import { compose } from "./prompts";
import {
  type Blocked,
  type Destination,
  filterPieces,
  type Piece,
} from "./receipt";
import { VoiceCodes } from "./tour";
import type { TerminalFailure } from "./terminal";

/** Plus long passage de l'éditeur joint au contexte (lignes). */
export const CONTEXT_MAX_LINES = 200;
/** Délai avant d'envoyer un contexte changé (curseur, défilement…). */
export const CONTEXT_DELAY_MS = 800;

export type VoiceProfile = VoiceView["profile"];

/** Contexte prêt à partir : texte pour le modèle, et ce que le panneau en dit. */
export type VoiceContext = {
  text: string;
  shared: ReceiptItem[];
  blocked: Blocked[];
};

/**
 * Contexte d'une séance : morceaux filtrés selon la destination, sous une
 * consigne courte (texte pour le modèle, dans la langue de VS Code).
 */
export function buildContext(
  heading: string,
  pieces: Piece[],
  destination: Destination,
): VoiceContext {
  const filtered = filterPieces(pieces, destination, {
    marker: vscode.l10n.t("[secret removed by ChatonLM]"),
    lines: (label, first, last) =>
      first === last
        ? vscode.l10n.t("{0}, line {1}", label, first)
        : vscode.l10n.t("{0}, lines {1}–{2}", label, first, last),
  });
  const text =
    filtered.pieces.length === 0
      ? ""
      : compose(
          heading,
          filtered.pieces.map((piece) => ({
            piece,
            heading: headingOf(piece),
          })),
        );
  return {
    text,
    shared: filtered.pieces.map((piece) => ({
      kind: piece.kind,
      label: piece.label,
      ...(piece.path !== undefined ? { path: piece.path } : {}),
      ...(piece.lines ? { lines: piece.lines } : {}),
      chars: piece.text.length,
    })),
    blocked: filtered.blocked,
  };
}

/** Morceau de la dernière commande échouée du terminal. */
export function terminalPiece(failure: TerminalFailure): Piece {
  return {
    kind: "terminal",
    label: failure.command,
    text: `$ ${failure.command}\n${failure.output}\n(exit code ${failure.exitCode})`,
    language: "text",
  };
}

/** Passage de l'éditeur actif : la sélection, sinon les lignes visibles. */
export function editorPiece(
  editor: vscode.TextEditor | undefined,
): Piece | null {
  if (!editor) return null;
  const { document, selection } = editor;
  if (document.uri.scheme !== "file" && document.uri.scheme !== "untitled") {
    return null;
  }
  let first: number;
  let last: number;
  if (!selection.isEmpty) {
    first = selection.start.line;
    last = selection.end.line;
    if (selection.end.character === 0 && last > first) last--;
  } else {
    const visible = editor.visibleRanges?.[0];
    first = visible?.start.line ?? 0;
    last = visible?.end.line ?? document.lineCount - 1;
  }
  last = Math.min(last, first + CONTEXT_MAX_LINES - 1, document.lineCount - 1);
  if (last < first) return null;
  const path = vscode.workspace.asRelativePath(document.uri, false);
  const text = document.getText(
    new vscode.Range(first, 0, last, document.lineAt(last).text.length),
  );
  return {
    kind: selection.isEmpty ? "file" : "selection",
    label: path,
    path,
    lines: [first + 1, last + 1],
    text,
    language: document.languageId,
  };
}

/** Texte d'un refus de séance (codes du lot 2), traduit. */
export function voiceErrorText(error: unknown): string {
  if (error instanceof RpcError) {
    switch (error.code) {
      case VoiceCodes.noVoice:
        return vscode.l10n.t(
          "ChatonLM has no voice for this language yet. Open ChatonLM to download one.",
        );
      case VoiceCodes.noTranscription:
        return vscode.l10n.t(
          "ChatonLM doesn't have its speech recognition model yet. Open ChatonLM to download it.",
        );
      case VoiceCodes.busy:
        return vscode.l10n.t("Voice is already in use in ChatonLM.");
      case VoiceCodes.noMicrophone:
        return (
          error.appMessage ?? vscode.l10n.t("The microphone isn't available.")
        );
    }
    if (error.appMessage) return error.appMessage;
  }
  return vscode.l10n.t("ChatonLM couldn't start the voice session.");
}

/** État de la séance pour la barre d'état de VS Code. */
function statusText(view: VoiceView): string {
  if (view.muted) return vscode.l10n.t("ChatonLM: microphone off");
  switch (view.phase) {
    case "off":
    case "starting":
      return vscode.l10n.t("ChatonLM: getting ready to listen");
    case "listening":
    case "hearing":
      return vscode.l10n.t("ChatonLM is listening");
    case "transcribing":
      return vscode.l10n.t("ChatonLM is transcribing");
    case "responding":
      return vscode.l10n.t("ChatonLM is thinking");
    case "speaking":
      return vscode.l10n.t("ChatonLM is speaking");
    case "paused":
      return vscode.l10n.t("ChatonLM: voice session paused");
  }
}

/** Commande qui montre le panneau (donnée par VS Code pour chaque vue). */
const SHOW_PANEL = "chatonlm.chat.focus";

/** Avis de l'app pendant la séance, en texte (ou null). */
function noticeText(state: VoiceModeState): string | null {
  const notice = state.notice;
  if (!notice) return null;
  switch (notice.kind) {
    case "failed":
      return notice.message;
    case "missingVoice":
      return vscode.l10n.t(
        "ChatonLM has no voice for this language yet. Open ChatonLM to download one.",
      );
    case "unsupportedLanguage":
      return vscode.l10n.t("ChatonLM can't speak this language aloud.");
    case "readFailed":
      return vscode.l10n.t("ChatonLM couldn't read the reply aloud.");
  }
}

export type VoiceDeps = {
  link: LinkLike;
  post(message: HostMessage): void;
  /** Événement d'un tour de parole (relayé à la conversation du panneau). */
  onChat(turn: number, event: ChatEvent): void;
  /** Séance finie (arrêtée, refusée, liaison perdue). */
  onEnded(): void;
  /** L'utilisateur parle (la lecture d'une visite s'arrête). */
  onHearing(): void;
  openApp(): Promise<void>;
};

type StartOptions = {
  profile: VoiceProfile;
  chatId: string | null;
  model: string | null;
  ephemeral: boolean;
  destination: Destination;
  context: VoiceContext;
  noTerminalError: boolean;
};

export class Voice {
  readonly #deps: VoiceDeps;
  #view: VoiceView | null = null;
  #sentContext = "";
  /** Augmente à chaque séance : une réponse d'une séance d'avant est périmée. */
  #session = 0;
  /** Séance dont la réponse de `voice.start` est attendue (ou null). */
  #starting: number | null = null;
  /** Séance finie dans l'app (`off`) avant la réponse de `voice.start` : la
   * réponse décide (un refus est dit avec son code), avec cet avis. */
  #offWhileStarting: { notice: string | null } | null = null;
  /** Dernier contexte donné pendant le démarrage (envoyé après la réponse). */
  #startContext = "";
  /** `voice.stop` déjà renvoyé pour une séance que VS Code ne montre plus. */
  #strayStopped = false;
  /** Barre d'état pendant une séance (micro tenu par VS Code). */
  #status: vscode.StatusBarItem | null = null;
  #talking = false;
  readonly #unsubscribe: () => void;

  constructor(deps: VoiceDeps) {
    this.#deps = deps;
    this.#unsubscribe = deps.link.onNotification((n) =>
      this.#onNotification(n),
    );
  }

  get view(): VoiceView | null {
    return this.#view;
  }

  get active(): boolean {
    return this.#view !== null;
  }

  get profile(): VoiceProfile | null {
    return this.#view?.profile ?? null;
  }

  dispose(): void {
    this.#unsubscribe();
    if (this.#view && this.#deps.link.connected) {
      void this.#deps.link.request("voice.stop", {}).catch(() => {});
    }
    this.#view = null;
    this.#setTalking(false);
    this.#status?.dispose();
    this.#status = null;
  }

  /** Démarre une séance ; rend la conversation de l'app, ou null si refusée. */
  async start(
    options: StartOptions,
  ): Promise<{ chatId: string | null } | null> {
    if (this.#view) await this.stop();
    const session = ++this.#session;
    this.#starting = session;
    this.#offWhileStarting = null;
    this.#strayStopped = false;
    this.#startContext = options.context.text;
    this.#view = {
      profile: options.profile,
      phase: "starting",
      muted: false,
      notice: null,
      shared: options.context.shared,
      blocked: options.context.blocked,
      destination: options.destination,
      noTerminalError: options.noTerminalError,
    };
    this.#publish();
    const params: Record<string, unknown> = { profile: options.profile };
    if (options.chatId) params.chatId = options.chatId;
    // Chaton-fantôme sans conversation encore : la séance en ouvre une
    // éphémère (sinon elle irait dans l'historique de l'app).
    else if (options.ephemeral) params.ephemeral = true;
    if (options.model) params.model = options.model;
    if (options.context.text) params.context = options.context.text;
    let result: { chatId?: unknown };
    try {
      result = await this.#deps.link.request<{ chatId?: unknown }>(
        "voice.start",
        params,
      );
    } catch (error) {
      if (this.#starting === session) this.#starting = null;
      // Refus : dit même si l'app a déjà publié la fin de la séance (`off`
      // avant la réponse) ; seul un arrêt (ou une autre séance) le rend périmé.
      if (session !== this.#session) return null;
      this.#offWhileStarting = null;
      this.#session++;
      this.#view = null;
      this.#publish();
      this.#deps.onEnded();
      await this.#refused(error);
      return null;
    }
    if (this.#starting === session) this.#starting = null;
    if (session !== this.#session || !this.#view) {
      // Arrêtée pendant le démarrage : l'app a pu ouvrir le micro après avoir
      // traité l'arrêt ; on le lui redit (sans risque : seule la séance de
      // cette connexion s'arrête). Pas si une autre séance a démarré depuis.
      if (!this.#view && this.#deps.link.connected) {
        void this.#deps.link.request("voice.stop", {}).catch(() => {});
      }
      return null;
    }
    // Lu après l'attente : changé entre-temps par une notification.
    const ended = this.#offWhileStarting as { notice: string | null } | null;
    if (ended) {
      // Finie dans l'app avant même la réponse (micro perdu…).
      this.#offWhileStarting = null;
      this.#end(ended.notice);
      return null;
    }
    this.#sentContext = options.context.text;
    // Contexte changé pendant le démarrage : il part maintenant.
    if (this.#startContext !== this.#sentContext) {
      this.#sentContext = this.#startContext;
      void this.#deps.link
        .request("voice.context", { text: this.#startContext })
        .catch(() => {});
    }
    return {
      chatId: typeof result?.chatId === "string" ? result.chatId : null,
    };
  }

  /** Fin de la séance vue de l'app (`off`), avec son avis. */
  #end(notice: string | null): void {
    this.#session++;
    this.#view = null;
    this.#publish();
    this.#deps.onEnded();
    if (notice) void vscode.window.showErrorMessage(notice);
  }

  async #refused(error: unknown): Promise<void> {
    const text = voiceErrorText(error);
    if (
      error instanceof RpcError &&
      (error.code === VoiceCodes.noVoice ||
        error.code === VoiceCodes.noTranscription)
    ) {
      const open = vscode.l10n.t("Open ChatonLM");
      const choice = await vscode.window.showErrorMessage(text, open);
      if (choice === open) await this.#deps.openApp();
      return;
    }
    void vscode.window.showErrorMessage(text);
  }

  async stop(): Promise<void> {
    if (!this.#view) return;
    this.#session++;
    this.#view = null;
    this.#publish();
    this.#deps.onEnded();
    if (!this.#deps.link.connected) return;
    try {
      await this.#deps.link.request("voice.stop", {});
    } catch {
      // Séance déjà finie dans l'app.
    }
  }

  /** Reprise d'une séance en pause (deux minutes sans parole, micro perdu). */
  async resume(): Promise<void> {
    if (!this.#view || this.#view.phase !== "paused") return;
    try {
      await this.#deps.link.request("voice.resume", {});
    } catch (error) {
      if (error instanceof RpcError && this.#view) {
        void vscode.window.showErrorMessage(voiceErrorText(error));
      }
    }
  }

  async mute(muted: boolean): Promise<void> {
    if (!this.#view) return;
    this.#view = { ...this.#view, muted };
    this.#publish();
    try {
      await this.#deps.link.request("voice.mute", { muted });
    } catch {
      // L'état de l'app (voice.state) corrige l'affichage.
    }
  }

  /** Contexte remplacé (passage changé, nouvelle erreur du terminal). */
  async setContext(
    context: VoiceContext,
    noTerminalError: boolean,
  ): Promise<void> {
    if (!this.#view) return;
    this.#view = {
      ...this.#view,
      shared: context.shared,
      blocked: context.blocked,
      noTerminalError,
    };
    this.#publish();
    // Pas encore de séance dans l'app : envoyé après la réponse.
    if (this.#starting === this.#session) {
      this.#startContext = context.text;
      return;
    }
    if (context.text === this.#sentContext) return;
    this.#sentContext = context.text;
    try {
      await this.#deps.link.request("voice.context", { text: context.text });
    } catch {
      // Séance finie entre-temps.
    }
  }

  /** Liaison perdue : l'app arrête la séance avec la connexion. */
  disconnected(): void {
    this.#strayStopped = false;
    if (!this.#view) return;
    this.#session++;
    this.#view = null;
    this.#publish();
    this.#deps.onEnded();
  }

  #onNotification({ method, params }: Notification): void {
    const p = (params ?? {}) as Record<string, unknown>;
    // Tours de parole : relayés même après la fin de la séance (la réponse
    // en cours continue de s'écrire dans l'app).
    if (method === "voice.chat") {
      const event = p.event as ChatEvent | undefined;
      if (!event || typeof p.turn !== "number") return;
      this.#deps.onChat(p.turn, event);
      return;
    }
    if (!this.#view) {
      // Séance que VS Code ne montre plus (arrêtée pendant son démarrage) :
      // filet, l'app est priée de l'arrêter.
      if (method !== "voice.state") return;
      const phase = (p.state as VoiceModeState | undefined)?.phase;
      if (typeof phase !== "string") return;
      if (phase === "off") {
        this.#strayStopped = false;
      } else if (!this.#strayStopped && this.#deps.link.connected) {
        this.#strayStopped = true;
        void this.#deps.link.request("voice.stop", {}).catch(() => {});
      }
      return;
    }
    switch (method) {
      case "voice.state": {
        const state = p.state as VoiceModeState | undefined;
        if (!state || typeof state.phase !== "string") return;
        if (state.phase === "off") {
          const notice = noticeText(state);
          if (this.#starting === this.#session) {
            // Refus de l'app publié avant sa réponse : la réponse le dira.
            this.#offWhileStarting = { notice };
            return;
          }
          // Séance finie dans l'app (micro perdu, éditeur retiré…).
          this.#end(notice);
          return;
        }
        const hearing =
          state.phase === "hearing" && this.#view.phase !== "hearing";
        this.#view = {
          ...this.#view,
          phase: state.phase,
          muted: state.muted === true,
          notice: noticeText(state),
        };
        this.#publish();
        if (hearing) this.#deps.onHearing();
        return;
      }
      case "voice.level": {
        const raw = p.level as unknown;
        const level =
          typeof raw === "number"
            ? raw
            : typeof (raw as { input?: unknown } | null)?.input === "number"
              ? (raw as { input: number }).input
              : null;
        if (level === null || !Number.isFinite(level)) return;
        this.#deps.post({
          type: "voiceLevel",
          level: Math.min(1, Math.max(0, level)),
        });
        return;
      }
    }
  }

  #publish(): void {
    this.#deps.post({ type: "voice", voice: this.#view });
    this.#statusBar();
  }

  /** Micro tenu par VS Code : visible même panneau replié (barre d'état),
   * et arrêtable depuis la palette (`chatonlm.stopTalking`). */
  #statusBar(): void {
    const view = this.#view;
    this.#setTalking(view !== null);
    if (!view) {
      this.#status?.hide();
      return;
    }
    this.#status ??= vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      99,
    );
    this.#status.text = `${view.muted ? "$(mic)" : "$(mic-filled)"} ${statusText(view)}`;
    this.#status.tooltip = vscode.l10n.t(
      "VS Code is using the microphone through ChatonLM. Click to show the session.",
    );
    this.#status.command = SHOW_PANEL;
    this.#status.show();
  }

  #setTalking(talking: boolean): void {
    if (talking === this.#talking) return;
    this.#talking = talking;
    void vscode.commands.executeCommand(
      "setContext",
      "chatonlm.talking",
      talking,
    );
  }
}
