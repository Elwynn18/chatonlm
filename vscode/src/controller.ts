// Cœur de l'extension : état du panneau, relais panneau ↔ pont, envois (avec
// leur reçu) et actions ; visite guidée parlée et séance vocale (lot 2).
// Contrats : .tools/research/87-lot1b-contract.md (B et « Panneau ↔ hôte »),
// 88-fix-contract.md (corrections de la relecture), 89-lot2-voice-contract.md (X).
import * as vscode from "vscode";
import type { Chat } from "../../src/bindings/Chat";
import type { ChatEvent } from "../../src/bindings/ChatEvent";
import type { ModelInfo } from "../../src/bindings/ModelInfo";
import type {
  HostMessage,
  Locale,
  Notice,
  PanelMessage,
  PanelState,
  QuickAction,
  Receipt,
  Theme,
} from "../shared/protocol";
import {
  ActionError,
  headingOf,
  prepareAction,
  type ActionRequest,
} from "./actions";
import type { LinkProblem, LinkState } from "./bridge/link";
import {
  ClosedError,
  type Call,
  type Notification,
  RpcError,
  TooLargeError,
} from "./bridge/rpc";
import { commitMessageFrom, compose } from "./prompts";
import {
  buildReceipt,
  type Destination,
  filterPieces,
  type History,
  historyOf,
  looksRemote,
  maskSecrets,
  type Piece,
} from "./receipt";
import type { TerminalFailure } from "./terminal";
import { Tour } from "./tour";
import {
  buildContext,
  CONTEXT_DELAY_MS,
  editorPiece,
  terminalPiece,
  Voice,
  type VoiceContext,
  type VoiceProfile,
} from "./voice";

/** Ce que le contrôleur attend de la liaison (Link, ou un double d'essai). */
export type LinkLike = {
  readonly state: LinkState;
  readonly connected: boolean;
  onState(listener: (state: LinkState) => void): () => void;
  onConnected(listener: () => void): () => void;
  onNotification(listener: (n: Notification) => void): () => void;
  call<T>(method: string, params?: unknown): Call<T>;
  request<T>(method: string, params?: unknown): Promise<T>;
  cancel(id: number): void;
  retryNow(): Promise<void>;
};

export type ControllerDeps = {
  link: LinkLike;
  memento: {
    get<T>(key: string): T | undefined;
    update(key: string, value: unknown): Thenable<void>;
  };
  /** Vers le panneau (rien si aucun panneau n'est prêt). */
  post(message: HostMessage): void;
  /** Montre le panneau. */
  reveal(): Promise<void>;
  /** Lance l'app ; `background` : sans montrer sa fenêtre. */
  launchApp(background: boolean): Promise<void>;
  /** Commande d'appairage (demande le code). */
  pair(): Promise<void>;
  locale(): Locale;
  theme(): Theme;
  /** Journal de l'extension (sortie « ChatonLM »). */
  log?(message: string): void;
  /** Dernière commande échouée du terminal (intégration du shell). */
  terminal?: {
    readonly last: TerminalFailure | null;
    onChange(listener: () => void): () => void;
  };
};

const MODEL_KEY = "chatonlm.model";
/** Délai laissé à l'effacement d'une conversation éphémère quand
 * l'extension s'arrête. */
const FORGET_ON_EXIT_MS = 1500;
/** Reçus gardés entre deux sessions (par message de l'utilisateur). */
const RECEIPTS_KEY = "chatonlm.receipts";
const MAX_RECEIPTS = 200;
/** Relecture d'une réponse en cours après une reconnexion. */
const RESYNC_POLL_MS = 1000;
const RESYNC_POLL_MAX = 600;

type SendResult = {
  chatId: string | null;
  outcome: "done" | "error" | "cancelled";
};

/** Envoi en cours, du clic jusqu'à la réponse de `chat.send`. */
type Sending = {
  /** Numéro de la requête `chat.send`, null pendant la préparation. */
  id: number | null;
  /** Stop demandé pendant la préparation : `chat.send` ne part pas. */
  stopped: boolean;
  /** Génération de la conversation au départ (voir `#generation`). */
  generation: number;
  /** Un événement de fin (done, error, cancelled) a été relayé. */
  ended: boolean;
  /** Conversation créée par cet envoi. */
  created: string | null;
};

/** Refus de l'app pour une réponse plus grosse que la trame (8 Mio). */
const TOO_LARGE = "too large";

const ENDS = new Set<ChatEvent["type"]>(["done", "error", "cancelled"]);

/** Nom affiché d'un modèle (sans `:latest`, comme l'app). */
const shortName = (model: string) => model.replace(/:latest$/, "");

export function problemText(problem: LinkProblem | undefined): string {
  switch (problem) {
    case "noRuntimeDir":
      return vscode.l10n.t(
        "This system gives ChatonLM no private folder for the link with VS Code.",
      );
    case "badServerProof":
      return vscode.l10n.t(
        "The app that answered is not the paired ChatonLM. Nothing was sent to it.",
      );
    default:
      return vscode.l10n.t("VS Code could not talk to ChatonLM.");
  }
}

export class Controller {
  readonly #deps: ControllerDeps;
  #models: ModelInfo[] = [];
  #appModel: string | null = null;
  #model: string | null;
  #chatId: string | null = null;
  /** Mode éphémère : celui de la conversation ouverte, ou de la prochaine. */
  #ephemeral = false;
  /** Un envoi de la conversation ouverte est resté sur cette machine. */
  #chatLocal = false;
  /** Historique de la conversation ouverte, à sa dernière lecture. */
  #history: { chatId: string; history: History } | null = null;
  #notice: Notice | null = null;
  /**
   * Génération de la conversation : augmentée à chaque nouvelle conversation.
   * Un envoi parti dans une génération précédente n'impose plus sa
   * conversation, et ses événements ne vont plus au panneau.
   */
  #generation = 0;
  #sending: Sending | null = null;
  #replies = new Map<number, string>();
  /** Reçu de chaque envoi en cours, jusqu'à son message enregistré. */
  #pendingReceipts = new Map<number, Receipt>();
  #resync = 0;
  #launching = false;
  readonly #disposers: (() => void)[] = [];
  readonly #tour: Tour;
  readonly #voice: Voice;
  /** Passage d'une visite joint au prochain message tapé. */
  #attachment: Piece | null = null;
  /** Séance vocale : destination du modèle ; pour chaque tour de parole pas
   * encore fini, numéro d'envoi donné au panneau (négatif : jamais celui
   * d'une requête) et génération de la conversation à son début. Les tours
   * sont numérotés par l'app sur toute la connexion : `#voiceLink` augmente
   * à chaque liaison perdue. Un tour continue après la fin de la séance (la
   * réponse s'écrit et se lit encore dans l'app). */
  #voiceLink = 0;
  #voiceDestination: Destination = "unknown";
  readonly #voiceTurns = new Map<
    string,
    { requestId: number; generation: number }
  >();
  #voiceRequests = 0;
  #contextTimer: ReturnType<typeof setTimeout> | null = null;
  #watching: (() => void)[] = [];

  constructor(deps: ControllerDeps) {
    this.#deps = deps;
    this.#model = deps.memento.get<string>(MODEL_KEY) ?? null;
    const openApp = () => this.launch(false);
    this.#tour = new Tour({
      link: deps.link,
      post: (message) => deps.post(message),
      reveal: () => deps.reveal(),
      model: () => this.#model ?? this.#appModel,
      destination: (model) => this.#destination(model),
      locale: () => deps.locale(),
      openApp,
      onSegment: () => {
        if (this.#voice.profile === "tour") this.#scheduleContext(0);
      },
      onEnded: () => {
        // La question sur un passage finit avec la visite.
        if (this.#voice.profile === "tour") void this.#voice.stop();
      },
    });
    this.#voice = new Voice({
      link: deps.link,
      post: (message) => deps.post(message),
      onChat: (turn, event) => this.#voiceEvent(turn, event),
      onEnded: () => this.#voiceEnded(),
      // Une parole de l'utilisateur arrête la lecture de la visite.
      onHearing: () => this.#tour.pause(),
      openApp,
    });
    this.#disposers.push(
      deps.link.onState((state) => {
        if (state.connection !== "connected") {
          this.#tour.disconnected();
          this.#voice.disconnected();
          this.#voiceTurnsLost();
        }
        this.#postState();
      }),
      deps.link.onConnected(() => void this.#onConnected()),
      deps.link.onNotification((n) => this.#onNotification(n)),
    );
  }

  dispose(): void {
    for (const dispose of this.#disposers) dispose();
    this.#resync++;
    this.#unwatchEditor();
    this.#tour.dispose();
    this.#voice.dispose();
  }

  /** Visite guidée (commandes de l'éditeur). */
  get tour(): Tour {
    return this.#tour;
  }

  /** Séance vocale tenue par VS Code. */
  get voice(): Voice {
    return this.#voice;
  }

  get busy(): boolean {
    return this.#sending !== null;
  }

  get state(): PanelState {
    const link = this.#deps.link.state;
    // Pendant un lancement, l'app n'a pas encore ouvert son pont.
    const starting =
      this.#launching &&
      (link.connection === "appClosed" || link.connection === "bridgeOff");
    const models = this.#models.map((m) => shortName(m.model));
    const state: PanelState = {
      connection: starting ? "connecting" : link.connection,
      models,
      cloudModels: models.filter(looksRemote),
      model: this.#model ?? this.#appModel,
      ephemeral: this.#ephemeral,
      chatId: this.#chatId,
      busy: this.busy,
      generation: this.#generation,
      notice: this.#notice,
      attachment: this.#attachment
        ? {
            path: this.#attachment.path ?? this.#attachment.label,
            lines: this.#attachment.lines ?? [1, 1],
          }
        : null,
    };
    if (link.connection === "error") state.error = problemText(link.problem);
    return state;
  }

  #postState(): void {
    this.#deps.post({ type: "state", state: this.state });
  }

  /** Reçus gardés, relus à chaque fois : une autre fenêtre a pu en ajouter. */
  #receipts(): Record<number, Receipt> {
    return this.#deps.memento.get<Record<number, Receipt>>(RECEIPTS_KEY) ?? {};
  }

  /** Message du panneau. */
  async handle(message: PanelMessage): Promise<void> {
    switch (message.type) {
      case "ready":
        this.#deps.post({
          type: "init",
          locale: this.#deps.locale(),
          theme: this.#deps.theme(),
          state: this.state,
        });
        this.#deps.post({ type: "receipts", receipts: this.#receipts() });
        // Visite et séance vocale en cours (panneau ouvert ou rechargé après).
        this.#deps.post({ type: "tour", tour: this.#tour.view });
        this.#deps.post({ type: "voice", voice: this.#voice.view });
        // Modèles relus à l'ouverture du panneau (installés depuis dans l'app).
        if (this.#deps.link.connected) await this.refresh();
        if (this.#chatId) await this.#syncChat();
        return;
      case "send": {
        if (!message.text.trim()) return;
        const attachment = this.#attachment;
        if (attachment) {
          // Question sur un passage de la visite : il part avec, filtré.
          await this.send(null, {
            action: "ask",
            instruction: message.text,
            pieces: [attachment],
          });
        } else {
          await this.send(message.text, null);
        }
        return;
      }
      case "tour":
        await this.tourCommand(message.command);
        return;
      case "tourGoto":
        await this.#tour.goto(message.index);
        return;
      case "tourAsk":
        await this.askAboutTour(message.voice);
        return;
      case "clearAttachment":
        this.#attachment = null;
        this.#postState();
        return;
      case "voice":
        switch (message.command) {
          case "start":
            await this.startVoice("duck");
            return;
          case "stop":
            await this.#voice.stop();
            return;
          case "resume":
            await this.#voice.resume();
            return;
          case "mute":
          case "unmute":
            await this.#voice.mute(message.command === "mute");
            return;
        }
        return;
      case "stop":
        this.stop();
        return;
      case "newChat":
        this.newChat(message.ephemeral);
        return;
      case "selectModel":
        await this.selectModel(message.model);
        return;
      case "keep":
        await this.keep();
        return;
      case "refresh":
        await this.refresh();
        return;
      case "pair":
        await this.#deps.pair();
        return;
      case "launchApp":
        await this.launch(true);
        return;
      case "openApp":
        await this.launch(false);
        return;
      case "openLink":
        // Le panneau n'émet que des liens web ; l'hôte, frontière, s'y tient.
        if (/^https?:\/\//i.test(message.url)) {
          await vscode.env.openExternal(vscode.Uri.parse(message.url));
        }
        return;
      case "copy":
        await vscode.env.clipboard.writeText(message.text);
        return;
      case "action":
        await this.runAction(message.action);
        return;
      case "panelError":
        this.#deps.log?.(`Panel error: ${message.message.slice(0, 4000)}`);
        return;
    }
  }

  /** Lance l'app (et attend son pont) ; le panneau dit « connexion » entre-temps. */
  async launch(background: boolean): Promise<void> {
    if (this.#launching) return;
    this.#launching = true;
    this.#postState();
    try {
      await this.#deps.launchApp(background);
    } finally {
      this.#launching = false;
      this.#postState();
    }
    // App ouverte pour y télécharger un modèle : la liste est relue.
    if (this.#deps.link.connected) await this.refresh();
  }

  /**
   * Nouvelle conversation : la conversation éphémère quittée est effacée
   * (comme dans l'app), un envoi en cours n'impose plus la sienne.
   */
  newChat(ephemeral: boolean, notice: Notice | null = null): void {
    // La séance vocale parle dans la conversation quittée : elle s'arrête.
    if (this.#voice.active) void this.#voice.stop();
    const left = this.#ephemeral ? this.#chatId : null;
    this.#generation++;
    this.#resync++;
    this.#chatId = null;
    this.#ephemeral = ephemeral;
    this.#chatLocal = false;
    this.#history = null;
    this.#notice = notice;
    this.#postState();
    if (left) void this.#forget(left);
  }

  /** Efface une conversation éphémère de VS Code (l'app vérifie qu'elle l'est). */
  async #forget(chatId: string): Promise<void> {
    if (!this.#deps.link.connected) return;
    try {
      await this.#deps.link.request("chat.forget", { chatId });
    } catch {
      // App fermée entre-temps : elle efface ses éphémères à son démarrage.
    }
  }

  /** Efface la conversation éphémère ouverte (retrait, arrêt de l'extension). */
  async forgetEphemeral(timeoutMs = FORGET_ON_EXIT_MS): Promise<void> {
    const chatId = this.#ephemeral ? this.#chatId : null;
    if (!chatId) return;
    this.#chatId = null;
    this.#history = null;
    this.#postState();
    await Promise.race([
      this.#forget(chatId),
      new Promise((resolve) => setTimeout(resolve, timeoutMs)),
    ]);
  }

  /** Garde la conversation éphémère ouverte (comme « Keep » de l'app). */
  async keep(): Promise<void> {
    const chatId = this.#chatId;
    if (!chatId || !this.#ephemeral) return;
    try {
      await this.#deps.link.request("chat.keep", { chatId });
    } catch (error) {
      void vscode.window.showErrorMessage(
        vscode.l10n.t(
          "Couldn't keep this chat: {0}",
          error instanceof RpcError
            ? (error.appMessage ?? error.message)
            : vscode.l10n.t("ChatonLM is not connected."),
        ),
      );
      return;
    }
    if (this.#chatId === chatId) {
      this.#ephemeral = false;
      this.#postState();
    }
  }

  async selectModel(model: string): Promise<void> {
    this.#model = model;
    await this.#deps.memento.update(MODEL_KEY, model);
    this.#postState();
    // Conversation commencée sur cette machine : son historique ne part pas
    // avec un modèle d'ailleurs, une nouvelle conversation s'ouvre.
    if (this.#chatId && this.#chatLocal && !this.busy) {
      const { destination } = await this.#destination(model);
      if (destination !== "local" && this.#model === model && this.#chatId) {
        this.newChat(this.#ephemeral, "offMachineNewChat");
      }
    }
  }

  stop(): void {
    const sending = this.#sending;
    if (!sending) return;
    if (sending.id !== null) this.#deps.link.cancel(sending.id);
    else sending.stopped = true;
  }

  /** Modèles et modèle choisi dans l'app (connexion, panneau ouvert, après
   * une réponse, retour dans la fenêtre). */
  async refresh(): Promise<void> {
    try {
      const [models, app] = await Promise.all([
        this.#deps.link.request<ModelInfo[]>("models.list"),
        this.#deps.link.request<{ selectedModel?: string }>("app.state"),
      ]);
      this.#models = Array.isArray(models) ? models : [];
      this.#appModel = app?.selectedModel ? shortName(app.selectedModel) : null;
      const names = this.#models.map((m) => shortName(m.model));
      if (this.#model && !names.includes(this.#model)) this.#model = null;
    } catch (error) {
      // Ollama injoignable (refus de l'app) : liste vide. Liaison perdue
      // entre-temps : la liste reste (l'écran de connexion s'affiche).
      if (error instanceof RpcError) this.#models = [];
    }
    this.#postState();
  }

  async #onConnected(): Promise<void> {
    await this.refresh();
    if (this.#chatId) await this.#syncChat();
  }

  /** Conversation relue (après un envoi, une reconnexion, un panneau rouvert). */
  async #syncChat(): Promise<void> {
    const chatId = this.#chatId;
    if (!chatId || !this.#deps.link.connected) return;
    const round = ++this.#resync;
    for (let i = 0; i < RESYNC_POLL_MAX; i++) {
      let chat: Chat;
      try {
        chat = await this.#deps.link.request<Chat>("chat.get", { chatId });
      } catch (error) {
        if (
          error instanceof RpcError &&
          error.message !== TOO_LARGE &&
          this.#chatId === chatId
        ) {
          // Conversation effacée (éphémère quittée, supprimée dans l'app) ;
          // trop grosse pour la trame, elle reste ouverte (sans relecture).
          this.#chatId = null;
          this.#history = null;
          this.#postState();
        }
        return;
      }
      if (round !== this.#resync || this.#chatId !== chatId) return;
      this.#history = { chatId, history: historyOf(chat.messages) };
      if (this.#ephemeral && !chat.ephemeral) {
        // Gardée depuis l'app.
        this.#ephemeral = false;
        this.#postState();
      }
      this.#deps.post({ type: "receipts", receipts: this.#receipts() });
      this.#deps.post({ type: "chat", chat });
      // Réponse écrite sans nous (envoi perdu à la déconnexion) : relue jusqu'à la fin.
      if (chat.replyRunning === null || this.#sending !== null) return;
      await new Promise((resolve) => setTimeout(resolve, RESYNC_POLL_MS));
      if (round !== this.#resync) return;
    }
  }

  #onNotification({ method, params }: Notification): void {
    if (method !== "chat.event") return;
    const { requestId, event } = (params ?? {}) as {
      requestId?: unknown;
      event?: ChatEvent;
    };
    if (typeof requestId !== "number" || !event) return;
    const sending = this.#sending?.id === requestId ? this.#sending : undefined;
    if (event.type === "chatCreated" && sending) {
      sending.created = event.chatId;
      if (sending.generation === this.#generation) {
        this.#chatId = event.chatId;
        this.#postState();
      }
    }
    if (event.type === "content" && this.#replies.has(requestId)) {
      this.#replies.set(requestId, this.#replies.get(requestId) + event.text);
    }
    if (event.type === "userMessage") {
      const receipt = this.#pendingReceipts.get(requestId);
      if (receipt) this.#keepReceipt(event.message.id, receipt);
    }
    if (ENDS.has(event.type) && sending) sending.ended = true;
    // Envoi d'une conversation quittée : le panneau ne le montre plus.
    if (sending && sending.generation !== this.#generation) return;
    this.#deps.post({ type: "event", requestId, event });
  }

  /** Garde le reçu d'un message enregistré (les plus récents seulement). */
  #keepReceipt(messageId: number, receipt: Receipt): void {
    // Relus juste avant : une autre fenêtre a pu en garder d'autres.
    const kept = Object.entries({ ...this.#receipts(), [messageId]: receipt })
      .sort(([a], [b]) => Number(b) - Number(a))
      .slice(0, MAX_RECEIPTS);
    const receipts = Object.fromEntries(kept);
    void this.#deps.memento.update(RECEIPTS_KEY, receipts);
    // Panneau ouvert pendant l'envoi mais pas encore prêt quand le reçu en
    // direct est parti : il l'a ainsi quand même.
    this.#deps.post({ type: "receipts", receipts });
  }

  /**
   * Où part un envoi (modèle cloud : `models.details.remote` ; Ollama d'une
   * autre machine : `app.state.local`), et le contexte que le moteur emploie
   * pour ce modèle. Ce que l'app ne dit pas est traité comme hors machine.
   */
  async #destination(
    model: string | null,
  ): Promise<{ destination: Destination; contextLength: number | null }> {
    if (!model) return { destination: "unknown", contextLength: null };
    const [details, app] = await Promise.allSettled([
      this.#deps.link.request<{ remote?: unknown; contextLength?: unknown }>(
        "models.details",
        { model },
      ),
      this.#deps.link.request<{ local?: unknown }>("app.state"),
    ]);
    const info = details.status === "fulfilled" ? details.value : undefined;
    const contextLength =
      typeof info?.contextLength === "number" && info.contextLength > 0
        ? info.contextLength
        : null;
    const remote =
      typeof info?.remote === "boolean" ? info.remote : looksRemote(model);
    if (remote) return { destination: "cloud", contextLength };
    const local = app.status === "fulfilled" ? app.value?.local : undefined;
    if (local === false) return { destination: "remote", contextLength };
    if (local === true && typeof info?.remote === "boolean") {
      return { destination: "local", contextLength };
    }
    return { destination: "unknown", contextLength };
  }

  /** Historique qui part avec un envoi dans la conversation `chatId`. */
  async #historyOf(chatId: string): Promise<History | null> {
    if (this.#history?.chatId === chatId) return this.#history.history;
    try {
      const chat = await this.#deps.link.request<Chat>("chat.get", { chatId });
      return historyOf(chat.messages);
    } catch {
      return null;
    }
  }

  /** Peut-on envoyer maintenant ? Sinon, dit pourquoi et montre le panneau. */
  async #ready(): Promise<boolean> {
    if (!this.#deps.link.connected) {
      await this.#deps.reveal();
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return false;
    }
    if (this.busy) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("A reply is being written. Wait for it, or stop it."),
      );
      return false;
    }
    // Les phrases de la séance vocale partent dans la conversation ouverte.
    if (this.#voice.active) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("A voice session is on. Stop it first."),
      );
      return false;
    }
    if (this.#voiceReplying()) return false;
    return true;
  }

  /** Une réponse à la voix s'écrit encore dans la conversation ouverte
   * (séance finie pendant la réponse) : l'app refuserait un autre envoi. */
  #voiceReplying(): boolean {
    for (const turn of this.#voiceTurns.values()) {
      if (turn.generation === this.#generation) {
        void vscode.window.showWarningMessage(
          vscode.l10n.t("A spoken reply is still being written. Wait for it."),
        );
        return true;
      }
    }
    return false;
  }

  /** Commande de la visite (panneau, palette, raccourcis). */
  async tourCommand(
    command: "previous" | "next" | "pause" | "resume" | "stop" | "toggle",
  ): Promise<void> {
    switch (command) {
      case "previous":
        return this.#tour.previous();
      case "next":
        return this.#tour.next();
      case "pause":
        return this.#tour.pause();
      case "resume":
        return this.#tour.resume();
      case "toggle":
        return this.#tour.togglePause();
      case "stop":
        return this.#tour.stop();
    }
  }

  /**
   * « Ask about this part » : la lecture s'arrête ; tapée, le passage est
   * joint au prochain message ; à la voix, une séance `tour` s'ouvre avec le
   * passage en contexte.
   */
  async askAboutTour(voice: boolean): Promise<void> {
    const passage = this.#tour.passage();
    if (!passage || !this.#tour.active) return;
    this.#tour.pause();
    if (voice) {
      await this.startVoice("tour");
      return;
    }
    this.#attachment = passage.piece;
    this.#postState();
    await this.#deps.reveal();
  }

  /** Démarre une séance vocale dans la conversation ouverte (ou une nouvelle). */
  async startVoice(profile: VoiceProfile): Promise<void> {
    if (!this.#deps.link.connected) {
      await this.#deps.reveal();
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    if (this.busy) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("A reply is being written. Wait for it, or stop it."),
      );
      return;
    }
    // Sa première phrase serait refusée (conversation occupée).
    if (this.#voiceReplying()) return;
    const model = this.#model ?? this.#appModel;
    const { destination } = await this.#destination(model);
    // Conversation commencée sur cette machine : son historique ne part pas
    // avec un modèle d'ailleurs (comme un envoi tapé).
    if (this.#chatId && this.#chatLocal && destination !== "local") {
      this.newChat(this.#ephemeral, "offMachineNewChat");
    }
    this.#voiceDestination = destination;
    const generation = this.#generation;
    const started = await this.#voice.start({
      profile,
      chatId: this.#chatId,
      model,
      ephemeral: this.#ephemeral,
      destination,
      context: this.#voiceContext(profile),
      noTerminalError: profile === "duck" && !this.#deps.terminal?.last,
    });
    if (!started || generation !== this.#generation) return;
    if (started.chatId) {
      this.#chatId = started.chatId;
      if (destination === "local") this.#chatLocal = true;
      this.#postState();
    }
    if (profile === "duck") this.#watchEditor();
  }

  /** Contexte de la séance : passage visible (ou sélection) et dernière
   * commande échouée en `duck` ; passage de la visite en `tour`. */
  #voiceContext(profile: VoiceProfile): VoiceContext {
    const destination = this.#voiceDestination;
    if (profile === "tour") {
      const passage = this.#tour.passage();
      if (!passage) return { text: "", shared: [], blocked: [] };
      // Texte du segment (écrit par le modèle de la visite, peut-être un
      // autre) : filtré comme le passage hors de cette machine.
      const said =
        destination === "local"
          ? passage.text
          : maskSecrets(
              passage.text,
              vscode.l10n.t("[secret removed by ChatonLM]"),
            ).text;
      return buildContext(
        vscode.l10n.t(
          "The user is asking about this part of the guided tour. The tour said: {0}",
          said,
        ),
        [passage.piece],
        destination,
      );
    }
    const pieces: Piece[] = [];
    const editor = editorPiece(vscode.window.activeTextEditor);
    if (editor) pieces.push(editor);
    const failure = this.#deps.terminal?.last;
    if (failure) pieces.push(terminalPiece(failure));
    return buildContext(
      vscode.l10n.t("What the user has open in VS Code:"),
      pieces,
      destination,
    );
  }

  /** Contexte renvoyé après un délai (curseur, défilement, terminal). */
  #scheduleContext(delay = CONTEXT_DELAY_MS): void {
    if (this.#contextTimer) clearTimeout(this.#contextTimer);
    this.#contextTimer = setTimeout(() => {
      this.#contextTimer = null;
      const profile = this.#voice.profile;
      if (!profile) return;
      void this.#voice.setContext(
        this.#voiceContext(profile),
        profile === "duck" && !this.#deps.terminal?.last,
      );
    }, delay);
  }

  #watchEditor(): void {
    this.#unwatchEditor();
    const schedule = () => this.#scheduleContext();
    const { window } = vscode;
    const subscriptions = [
      window.onDidChangeActiveTextEditor(schedule),
      window.onDidChangeTextEditorSelection(schedule),
      window.onDidChangeTextEditorVisibleRanges(schedule),
    ];
    this.#watching = [
      ...subscriptions.map((s) => () => s.dispose()),
      ...(this.#deps.terminal ? [this.#deps.terminal.onChange(schedule)] : []),
    ];
  }

  #unwatchEditor(): void {
    for (const dispose of this.#watching) dispose();
    this.#watching = [];
    if (this.#contextTimer) clearTimeout(this.#contextTimer);
    this.#contextTimer = null;
  }

  /** Tour de parole : relayé au panneau comme un envoi (pas s'il a commencé
   * dans une conversation quittée depuis). */
  #voiceEvent(turn: number, event: ChatEvent): void {
    const key = `${this.#voiceLink}:${turn}`;
    let entry = this.#voiceTurns.get(key);
    if (!entry) {
      // Un nouveau tour n'appartient qu'à une séance en cours.
      if (!this.#voice.active) return;
      entry = {
        requestId: -++this.#voiceRequests,
        generation: this.#generation,
      };
      this.#voiceTurns.set(key, entry);
    }
    const current = entry.generation === this.#generation;
    if (ENDS.has(event.type)) this.#voiceTurns.delete(key);
    if (!current) return;
    if (event.type === "chatCreated") {
      this.#chatId = event.chatId;
      if (this.#voiceDestination === "local") this.#chatLocal = true;
      this.#postState();
    }
    this.#deps.post({ type: "event", requestId: entry.requestId, event });
    if (ENDS.has(event.type)) void this.#syncChat();
  }

  /** Séance finie : un tour en cours continue (sa fin arrivera par
   * `voice.chat`) ; le contexte de l'éditeur n'est plus suivi. */
  #voiceEnded(): void {
    this.#unwatchEditor();
    this.#postState();
  }

  /** Liaison perdue : les tours ouverts ne diront plus rien (la
   * conversation est relue à la reconnexion). */
  #voiceTurnsLost(): void {
    for (const { requestId, generation } of this.#voiceTurns.values()) {
      if (generation !== this.#generation) continue;
      this.#deps.post({
        type: "event",
        requestId,
        event: { type: "cancelled" },
      });
    }
    this.#voiceTurns.clear();
    this.#voiceLink++;
  }

  /** Action de l'éditeur : morceaux pris, filtrés selon la destination, envoyés. */
  async runAction(action: QuickAction, args: unknown[] = []): Promise<void> {
    let request: ActionRequest;
    try {
      request = await prepareAction(action, args);
    } catch (error) {
      if (error instanceof ActionError) {
        void vscode.window.showInformationMessage(error.message);
        return;
      }
      throw error;
    }
    if (!(await this.#ready())) return;
    await this.#deps.reveal();
    await this.send(null, request);
  }

  /**
   * Fin d'un envoi sans événement de fin (refus de l'app, liaison perdue,
   * message trop gros) : le panneau la reçoit quand même, sinon il resterait
   * « occupé ».
   */
  #endWithout(requestId: number, error: unknown, result: SendResult | null) {
    let event: ChatEvent;
    if (result) {
      event =
        result.outcome === "error"
          ? {
              type: "error",
              kind: "internal",
              message: vscode.l10n.t("ChatonLM refused the message."),
            }
          : result.outcome === "cancelled"
            ? { type: "cancelled" }
            : { type: "done", messageId: null };
    } else if (error instanceof RpcError) {
      event = {
        type: "error",
        kind: "internal",
        message:
          error.appMessage ?? vscode.l10n.t("ChatonLM refused the message."),
      };
    } else if (error instanceof TooLargeError) {
      event = {
        type: "error",
        kind: "internal",
        message: vscode.l10n.t(
          "This message is too large to send to ChatonLM. Select less code.",
        ),
      };
    } else {
      event = {
        type: "error",
        kind: "connectionLost",
        message: vscode.l10n.t(
          "The link with ChatonLM was lost. The reply may go on in ChatonLM.",
        ),
      };
    }
    this.#deps.post({ type: "event", requestId, event });
  }

  /**
   * Envoi : texte tapé dans le panneau (`text`), ou action (`request`). Le
   * reçu part au panneau avant la réponse, sous le numéro de la requête.
   */
  async send(
    text: string | null,
    request: ActionRequest | null,
  ): Promise<void> {
    if (!(await this.#ready())) return;
    // Passage d'une visite joint : il part avec ce message seulement.
    if (request?.action === "ask") this.#attachment = null;
    // Pris avant toute attente : Stop et « nouvelle conversation » pendant
    // la préparation sont respectés.
    const sending: Sending = {
      id: null,
      stopped: false,
      generation: this.#generation,
      ended: false,
      created: null,
    };
    this.#sending = sending;
    this.#postState();
    try {
      await this.#send(sending, text, request);
    } finally {
      if (this.#sending === sending) this.#sending = null;
      this.#postState();
    }
    // Rien n'est parti (Stop pendant la préparation, rien à envoyer).
    if (sending.id === null) return;
    // Conversation éphémère créée par un envoi d'une conversation quittée
    // depuis : effacée, comme l'app efface celle qu'on quitte.
    if (
      sending.created &&
      sending.generation !== this.#generation &&
      this.#chatId !== sending.created
    ) {
      void this.#forget(sending.created);
    }
    // Modèles relus après chaque réponse (ajoutés ou retirés dans l'app).
    if (this.#deps.link.connected) await this.refresh();
    await this.#syncChat();
  }

  async #send(
    sending: Sending,
    text: string | null,
    request: ActionRequest | null,
  ): Promise<void> {
    const model = this.#model ?? this.#appModel;
    const { destination, contextLength } = await this.#destination(model);
    if (sending.stopped || sending.generation !== this.#generation) return;
    // L'avis d'une nouvelle conversation dure jusqu'au prochain envoi.
    this.#notice = null;
    // Conversation commencée sur cette machine et modèle d'ailleurs (réglage
    // de l'app changé depuis, par exemple) : l'historique ne le suit pas.
    if (this.#chatId && this.#chatLocal && destination !== "local") {
      this.newChat(this.#ephemeral, "offMachineNewChat");
      sending.generation = this.#generation;
    }
    const chatId = this.#chatId;
    const ephemeral = this.#ephemeral;
    const history = chatId ? await this.#historyOf(chatId) : null;
    if (sending.stopped || sending.generation !== this.#generation) return;
    let content: string;
    let receipt;
    if (request) {
      const { pieces, blocked } = filterPieces(request.pieces, destination, {
        marker: vscode.l10n.t("[secret removed by ChatonLM]"),
        lines: (label, first, last) =>
          first === last
            ? vscode.l10n.t("{0}, line {1}", label, first)
            : vscode.l10n.t("{0}, lines {1}–{2}", label, first, last),
      });
      if (pieces.length === 0) {
        void vscode.window.showWarningMessage(
          vscode.l10n.t(
            "Nothing was sent: the model runs outside this computer and everything selected may hold secrets.",
          ),
        );
        return;
      }
      content = compose(
        request.instruction,
        pieces.map((piece) => ({ piece, heading: headingOf(piece) })),
      );
      receipt = buildReceipt(
        content,
        pieces,
        blocked,
        destination,
        contextLength,
        history,
      );
    } else {
      content = text ?? "";
      receipt = buildReceipt(
        content,
        [],
        [],
        destination,
        contextLength,
        history,
      );
    }
    const params: Record<string, unknown> = { content };
    if (model) params.model = model;
    if (chatId) params.chatId = chatId;
    else if (ephemeral) params.ephemeral = true;
    const call: Call<SendResult> = this.#deps.link.call<SendResult>(
      "chat.send",
      params,
    );
    sending.id = call.id;
    this.#replies.set(call.id, "");
    this.#pendingReceipts.set(call.id, receipt);
    this.#deps.post({ type: "receipt", requestId: call.id, receipt });
    this.#postState();
    let result: SendResult | null = null;
    let failure: unknown = null;
    try {
      result = await call.result;
    } catch (error) {
      if (
        !(error instanceof RpcError) &&
        !(error instanceof ClosedError) &&
        !(error instanceof TooLargeError)
      ) {
        throw error;
      }
      failure = error;
    }
    const reply = this.#replies.get(call.id) ?? "";
    this.#replies.delete(call.id);
    this.#pendingReceipts.delete(call.id);
    const current = sending.generation === this.#generation;
    if (!sending.ended && current) this.#endWithout(call.id, failure, result);
    if (!current) return;
    if (result?.chatId) this.#chatId = result.chatId;
    else if (sending.created) this.#chatId = sending.created;
    if (this.#chatId && destination === "local") this.#chatLocal = true;
    if (request?.repository && result?.outcome === "done") {
      const message = commitMessageFrom(reply);
      if (message) {
        request.repository.inputBox.value = message;
        void vscode.window.showInformationMessage(
          vscode.l10n.t("The commit message is in the Source Control box."),
        );
      }
    }
  }
}
