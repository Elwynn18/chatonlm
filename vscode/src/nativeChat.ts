// Modèles de ChatonLM dans le chat de VS Code (bonus) :
// `lm.registerLanguageModelChatProvider` (VS Code ≥ 1.104 ; absent sous
// Cursor, sans effet). Réglage `chatonlm.nativeChat.enabled` COUPÉ par défaut
// (pour ne pas encombrer le sélecteur), lu seulement dans les réglages de
// l'utilisateur. Seulement les modèles de cette machine (jamais un modèle
// distant, ni, avec une app 0.9.x, ceux d'un serveur réglé sur une autre
// machine : `app.state.local`) ;
// chaque demande part dans une conversation éphémère de l'app,
// effacée aussitôt après (rien dans l'historique) ; réponse relayée en flux ;
// texte seulement (pas d'outils, pas d'images). Contrat :
// .tools/research/95-lot4b-contract.md (3).
import * as vscode from "vscode";
import { type ListedModel, listedModels } from "../shared/models";
import type { LinkLike } from "./controller";
import { RpcError, TooLargeError } from "./bridge/rpc";
import { settingOn } from "./agents";
import { looksRemote } from "./receipt";

export const VENDOR = "chatonlm";
/** Réponse la plus longue annoncée (jetons). */
export const MAX_OUTPUT_TOKENS = 4096;
/** Contexte annoncé quand l'app ne le dit pas. */
export const DEFAULT_CONTEXT = 4096;

export const nativeChatEnabled = () => settingOn("nativeChat.enabled");

/**
 * Famille d'un modèle pour le sélecteur de VS Code : l'identifiant sans sa
 * quantification ni son étiquette (`Qwen/Qwen3-0.6B-GGUF:Q8_0` →
 * `Qwen/Qwen3-0.6B-GGUF`, `qwen3:0.6b` → `qwen3`).
 */
export function family(id: string): string {
  const colon = id.lastIndexOf(":");
  return colon > id.lastIndexOf("/") && colon > 0 ? id.slice(0, colon) : id;
}

type SendResult = {
  chatId: string | null;
  outcome: "done" | "error" | "cancelled";
};

type Details = { remote?: unknown; contextLength?: unknown };

export type NativeChatDeps = {
  link: LinkLike;
  /** Envoi de ce fournisseur : ses évènements ne vont pas au panneau. */
  own(requestId: number): void;
  release(requestId: number): void;
};

/** Texte d'un message du chat (parties texte seulement). */
function textOf(message: vscode.LanguageModelChatRequestMessage): string {
  return message.content
    .map((part) =>
      part instanceof vscode.LanguageModelTextPart ? part.value : "",
    )
    .join("")
    .trim();
}

/**
 * Messages du chat en un seul texte pour `chat.send` (une conversation
 * éphémère neuve à chaque demande) : le dernier message de l'utilisateur,
 * précédé des messages d'avant s'il y en a.
 */
export function flatten(
  messages: readonly vscode.LanguageModelChatRequestMessage[],
): string {
  const parts = messages
    .map((message) => ({ role: message.role, text: textOf(message) }))
    .filter((part) => part.text);
  if (parts.length === 0) return "";
  const last = parts[parts.length - 1];
  if (parts.length === 1) return last.text;
  const label = (role: vscode.LanguageModelChatMessageRole) =>
    role === vscode.LanguageModelChatMessageRole.User
      ? "User"
      : role === vscode.LanguageModelChatMessageRole.Assistant
        ? "Assistant"
        : "System";
  const before = parts
    .slice(0, -1)
    .map((part) => `${label(part.role)}: ${part.text}`)
    .join("\n\n");
  return `Earlier in this conversation:\n\n${before}\n\n---\n\n${last.text}`;
}

export class NativeChat implements vscode.LanguageModelChatProvider {
  readonly #deps: NativeChatDeps;
  readonly #changed = new vscode.EventEmitter<void>();
  readonly #disposables: { dispose(): void }[] = [];
  readonly onDidChangeLanguageModelChatInformation = this.#changed.event;

  constructor(deps: NativeChatDeps) {
    this.#deps = deps;
    const changed = () => this.#changed.fire();
    const unsubscribe = [
      deps.link.onConnected(changed),
      deps.link.onState((state) => {
        if (state.connection !== "connected") changed();
      }),
    ];
    this.#disposables.push(
      this.#changed,
      { dispose: () => unsubscribe.forEach((u) => u()) },
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("chatonlm.nativeChat")) changed();
      }),
    );
  }

  /** Enregistré si l'éditeur le permet (VS Code ≥ 1.104). */
  register(): { dispose(): void }[] {
    const lm = vscode.lm as Partial<typeof vscode.lm> | undefined;
    if (typeof lm?.registerLanguageModelChatProvider !== "function") return [];
    return [lm.registerLanguageModelChatProvider(VENDOR, this)];
  }

  /** Modèles relus (après une réponse, un téléchargement dans l'app). */
  refresh(): void {
    this.#changed.fire();
  }

  dispose(): void {
    for (const disposable of this.#disposables) disposable.dispose();
  }

  /** Les modèles de l'app sont-ils sur cette machine ? Toujours, depuis
   * l'app 0.10 ; une app 0.9.x pouvait viser un serveur d'une autre machine.
   * Ce qu'elle ne dit pas : non. */
  async #local(): Promise<boolean> {
    try {
      const app = await this.#deps.link.request<{ local?: unknown }>(
        "app.state",
      );
      return app?.local === true;
    } catch {
      return false;
    }
  }

  /** Tourne-t-il sur cette machine ? Ce que l'app ne dit pas : non.
   * `local` : `app.state.local` déjà lu, sinon demandé. */
  async #details(model: string, local?: boolean): Promise<Details | null> {
    if (looksRemote(model)) return null;
    if (!(local ?? (await this.#local()))) return null;
    try {
      const details = await this.#deps.link.request<Details>("models.details", {
        model,
      });
      return details?.remote === false ? details : null;
    } catch {
      return null;
    }
  }

  async provideLanguageModelChatInformation(): Promise<
    vscode.LanguageModelChatInformation[]
  > {
    if (!nativeChatEnabled() || !this.#deps.link.connected) return [];
    let models: ListedModel[];
    try {
      models = listedModels(
        await this.#deps.link.request<unknown>("models.list"),
      );
    } catch {
      return [];
    }
    // Modèles sur une autre machine (app 0.9.x) : aucun n'est proposé.
    if (!(await this.#local())) return [];
    const infos = await Promise.all(
      models.slice(0, 100).map(async (m) => {
        // Distant : écarté sans le demander à l'app.
        if (m.remote) return null;
        const details = await this.#details(m.id, true);
        if (!details) return null;
        const context =
          typeof details.contextLength === "number" && details.contextLength > 0
            ? details.contextLength
            : (m.contextMax ?? DEFAULT_CONTEXT);
        const name = m.name;
        const info: vscode.LanguageModelChatInformation = {
          id: m.id,
          name,
          family: family(m.id),
          // L'identifiant porte la quantification : un autre fichier est
          // une autre version.
          version: m.id,
          detail: vscode.l10n.t("ChatonLM, on this computer"),
          tooltip: vscode.l10n.t(
            "{0} runs on this computer through ChatonLM. Nothing is kept in ChatonLM's history.",
            name,
          ),
          maxInputTokens: Math.max(1, context - MAX_OUTPUT_TOKENS),
          maxOutputTokens: MAX_OUTPUT_TOKENS,
          capabilities: { imageInput: false, toolCalling: false },
        };
        return info;
      }),
    );
    return infos.filter((info) => info !== null);
  }

  async provideLanguageModelChatResponse(
    model: vscode.LanguageModelChatInformation,
    messages: readonly vscode.LanguageModelChatRequestMessage[],
    _options: vscode.ProvideLanguageModelChatResponseOptions,
    progress: vscode.Progress<vscode.LanguageModelResponsePart>,
    token: vscode.CancellationToken,
  ): Promise<void> {
    const { link } = this.#deps;
    if (!nativeChatEnabled()) {
      throw new Error(
        vscode.l10n.t(
          "ChatonLM models are turned off in VS Code's chat (setting chatonlm.nativeChat.enabled).",
        ),
      );
    }
    if (!link.connected) {
      throw new Error(vscode.l10n.t("ChatonLM is not connected."));
    }
    // Vérifié à chaque demande : jamais un modèle distant, ni des modèles
    // sur une autre machine (app 0.9.x).
    if (!(await this.#local())) {
      throw new Error(
        vscode.l10n.t(
          "ChatonLM's models run on another computer: they are not offered here.",
        ),
      );
    }
    if (!(await this.#details(model.id, true))) {
      throw new Error(
        vscode.l10n.t(
          "{0} does not run on this computer: ChatonLM only offers its local models here.",
          model.name,
        ),
      );
    }
    const content = flatten(messages);
    if (!content) return;
    if (token.isCancellationRequested) return;
    const call = link.call<SendResult>("chat.send", {
      content,
      model: model.id,
      ephemeral: true,
    });
    this.#deps.own(call.id);
    let error: string | null = null;
    let created: string | null = null;
    const off = link.onNotification(({ method, params }) => {
      if (method !== "chat.event") return;
      const p = (params ?? {}) as {
        requestId?: unknown;
        event?: {
          type?: unknown;
          text?: unknown;
          message?: unknown;
          chatId?: unknown;
        };
      };
      if (p.requestId !== call.id || !p.event) return;
      const event = p.event;
      if (event.type === "content" && typeof event.text === "string") {
        if (event.text)
          progress.report(new vscode.LanguageModelTextPart(event.text));
      } else if (event.type === "error" && typeof event.message === "string") {
        error = event.message;
      } else if (
        event.type === "chatCreated" &&
        typeof event.chatId === "string"
      ) {
        created = event.chatId;
      }
    });
    const cancel = token.onCancellationRequested(() => link.cancel(call.id));
    let result: SendResult | null = null;
    try {
      result = await call.result;
    } catch (failure) {
      throw new Error(
        failure instanceof RpcError
          ? (failure.appMessage ??
              vscode.l10n.t("ChatonLM refused the message."))
          : failure instanceof TooLargeError
            ? vscode.l10n.t(
                "This conversation is too long to send to ChatonLM. Start a new one.",
              )
            : vscode.l10n.t(
                "The link with ChatonLM was lost. The reply may go on in ChatonLM.",
              ),
        { cause: failure },
      );
    } finally {
      off();
      cancel.dispose();
      this.#deps.release(call.id);
      // Conversation éphémère : effacée, rien ne reste dans l'historique.
      const chatId = result?.chatId ?? created;
      if (chatId && link.connected) {
        void link.request("chat.forget", { chatId }).catch(() => {});
      }
    }
    if (result?.outcome === "error") {
      throw new Error(error ?? vscode.l10n.t("ChatonLM refused the message."));
    }
  }

  /** Estimation (4 caractères par jeton, comme le reçu). */
  async provideTokenCount(
    _model: vscode.LanguageModelChatInformation,
    text: string | vscode.LanguageModelChatRequestMessage,
  ): Promise<number> {
    const value = typeof text === "string" ? text : textOf(text);
    return Math.ceil(value.length / 4);
  }
}
