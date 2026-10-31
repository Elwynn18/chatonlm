// Serveur MCP minimal écrit à la main (pas de SDK) : JSON-RPC 2.0 sur
// l'entrée et la sortie standard, un message JSON par ligne (transport stdio
// de MCP). Méthodes : `initialize`, `ping`, `tools/list`, `tools/call`,
// notification `notifications/cancelled` ; tout le reste est refusé. Chaque
// appel d'outil est noté (journal d'activité lu par le panneau de VS Code) et
// peut être arrêté par l'agent, par le panneau ou par la coupure du réglage.
// Sans dépendance à VS Code (dans `dist/mcp.cjs`). Contrat :
// .tools/research/95-lot4b-contract.md (2).
import { randomBytes } from "node:crypto";
import {
  type ActivityRecord,
  isToolName,
  oneLine,
  type ToolName,
} from "./files";
import {
  type BridgeLike,
  bridgeErrorText,
  detailOf,
  InputError,
  runTool,
  StoppedError,
  type ToolResult,
  toolList,
  validate,
} from "./tools";

/** Versions du protocole MCP connues (la plus récente d'abord). */
export const PROTOCOL_VERSIONS = [
  "2025-11-25",
  "2025-06-18",
  "2025-03-26",
  "2024-11-05",
] as const;

/** Une ligne reçue plus longue est refusée (1 Mio). */
export const MAX_LINE = 1024 * 1024;
/** Appels d'outils en cours à la fois. */
export const MAX_RUNNING = 4;

export type ServerDeps = {
  version: string;
  /** Pont relié, ou un refus (texte pour l'agent) : serveur coupé, app
   * fermée, éditeur pas appairé… */
  bridge(): Promise<BridgeLike | string>;
  /** Appel noté (panneau de VS Code). */
  record(record: ActivityRecord): void;
  /** Message vers l'agent (une ligne JSON). */
  write(line: string): void;
};

type Id = string | number;
type Message = {
  jsonrpc?: unknown;
  id?: unknown;
  method?: unknown;
  params?: unknown;
};

const validId = (id: unknown): id is Id =>
  (typeof id === "string" && id.length <= 200) ||
  (typeof id === "number" && Number.isFinite(id));

export class McpServer {
  readonly #deps: ServerDeps;
  #agent: string | null = null;
  /** Appels en cours, par identifiant de requête de l'agent. */
  readonly #running = new Map<
    string,
    { callId: string; controller: AbortController }
  >();

  /** Requêtes en cours de traitement. */
  readonly #pending = new Set<Promise<void>>();

  constructor(deps: ServerDeps) {
    this.#deps = deps;
  }

  get agent(): string | null {
    return this.#agent;
  }

  /** Identifiants (pour le panneau) des appels en cours. */
  get running(): string[] {
    return [...this.#running.values()].map((r) => r.callId);
  }

  /** Arrête un appel (panneau de VS Code). */
  stop(callId: string): void {
    for (const run of this.#running.values()) {
      if (run.callId === callId) run.controller.abort();
    }
  }

  /** Arrête tout (réglage coupé, désappairage, fin de l'entrée). */
  stopAll(): void {
    for (const run of this.#running.values()) run.controller.abort();
  }

  #send(message: unknown): void {
    this.#deps.write(JSON.stringify(message));
  }

  #reply(id: Id, result: unknown): void {
    this.#send({ jsonrpc: "2.0", id, result });
  }

  #fail(id: Id | null, code: number, message: string): void {
    this.#send({ jsonrpc: "2.0", id, error: { code, message } });
  }

  /** Une ligne reçue de l'agent. */
  receive(line: string): void {
    if (!line.trim()) return;
    if (line === TOO_LONG || line.length > MAX_LINE) {
      this.#fail(null, -32600, "Message too large");
      return;
    }
    let message: unknown;
    try {
      message = JSON.parse(line);
    } catch {
      this.#fail(null, -32700, "Parse error");
      return;
    }
    if (typeof message !== "object" || message === null) {
      this.#fail(null, -32600, "Invalid request");
      return;
    }
    if (Array.isArray(message)) {
      // Lots retirés de MCP (2025-06-18).
      this.#fail(null, -32600, "Batches are not supported");
      return;
    }
    const pending = this.#dispatch(message as Message);
    this.#pending.add(pending);
    void pending.finally(() => this.#pending.delete(pending));
  }

  /** Toutes les requêtes reçues ont eu leur réponse. */
  async settled(): Promise<void> {
    while (this.#pending.size > 0) {
      await Promise.allSettled([...this.#pending]);
    }
  }

  async #dispatch(message: Message): Promise<void> {
    const { id, method, params } = message;
    if (typeof method !== "string") return; // réponse à rien : ignorée
    const isRequest = id !== undefined && id !== null;
    if (!isRequest) {
      if (method === "notifications/cancelled") {
        const requestId = (params as { requestId?: unknown } | null)?.requestId;
        if (validId(requestId)) {
          this.#running.get(String(requestId))?.controller.abort();
        }
      }
      return; // autres notifications (initialized…) : rien à faire
    }
    if (!validId(id)) {
      this.#fail(null, -32600, "Invalid request id");
      return;
    }
    switch (method) {
      case "initialize":
        this.#initialize(id, params);
        return;
      case "ping":
        this.#reply(id, {});
        return;
      case "tools/list":
        this.#reply(id, { tools: toolList() });
        return;
      case "tools/call":
        await this.#call(id, params);
        return;
      default:
        this.#fail(id, -32601, "Method not found");
    }
  }

  #initialize(id: Id, params: unknown): void {
    const p = (params ?? {}) as {
      protocolVersion?: unknown;
      clientInfo?: { name?: unknown; title?: unknown };
    };
    const asked = p.protocolVersion;
    const version = (PROTOCOL_VERSIONS as readonly unknown[]).includes(asked)
      ? (asked as string)
      : PROTOCOL_VERSIONS[0];
    this.#agent =
      oneLine(p.clientInfo?.title, 60) ?? oneLine(p.clientInfo?.name, 60);
    this.#reply(id, {
      protocolVersion: version,
      capabilities: { tools: { listChanged: false } },
      serverInfo: { name: "chatonlm", version: this.#deps.version },
      instructions:
        "ChatonLM runs language models, image generation, speech and proofreading on this computer. Every call is shown to the user in VS Code, who can stop it.",
    });
  }

  async #call(id: Id, params: unknown): Promise<void> {
    const p = (params ?? {}) as { name?: unknown; arguments?: unknown };
    // Liste blanche : un autre nom est une erreur du protocole.
    if (!isToolName(p.name)) {
      this.#fail(id, -32602, `Unknown tool: ${oneLine(p.name, 60) ?? ""}`);
      return;
    }
    const name: ToolName = p.name;
    let args: Record<string, string | number>;
    try {
      args = validate(name, p.arguments);
    } catch (error) {
      this.#reply(id, {
        content: [
          {
            type: "text",
            text:
              error instanceof InputError ? error.message : "Invalid input.",
          },
        ],
        isError: true,
      });
      return;
    }
    const key = String(id);
    if (this.#running.size >= MAX_RUNNING || this.#running.has(key)) {
      this.#reply(id, {
        content: [
          { type: "text", text: "Too many ChatonLM calls at once. Try again." },
        ],
        isError: true,
      });
      return;
    }
    const callId = randomBytes(8).toString("hex");
    const controller = new AbortController();
    this.#running.set(key, { callId, controller });
    const record = (state: ActivityRecord["state"]) =>
      this.#deps.record({
        id: callId,
        tool: name,
        agent: this.#agent,
        state,
        at: Date.now(),
        detail: detailOf(name, args),
      });
    record("running");
    let result: ToolResult;
    let state: ActivityRecord["state"];
    try {
      const bridge = await this.#deps.bridge();
      if (typeof bridge === "string") {
        result = { content: [{ type: "text", text: bridge }], isError: true };
      } else {
        if (controller.signal.aborted) throw new StoppedError();
        result = await runTool(name, args, bridge, controller.signal);
      }
      state = result.isError ? "failed" : "done";
    } catch (error) {
      if (error instanceof StoppedError || controller.signal.aborted) {
        result = {
          content: [{ type: "text", text: "Stopped by the user in ChatonLM." }],
          isError: true,
        };
        state = "stopped";
      } else {
        result = {
          content: [
            {
              type: "text",
              text:
                error instanceof InputError
                  ? error.message
                  : bridgeErrorText(error),
            },
          ],
          isError: true,
        };
        state = "failed";
      }
    } finally {
      this.#running.delete(key);
    }
    record(state);
    this.#reply(id, result);
  }
}

/** Ligne trop longue, jetée (rendue à la place de son contenu). */
export const TOO_LONG = "\u0000too long";

/** Découpe un flux en lignes (UTF-8, `\n` ou `\r\n`). Une ligne de plus de
 * MAX_LINE caractères est jetée jusqu'à sa fin et rendue comme TOO_LONG. */
export class LineReader {
  #buffer = "";
  #skipping = false;
  readonly #decoder = new TextDecoder();

  push(chunk: Uint8Array | string): string[] {
    this.#buffer +=
      typeof chunk === "string"
        ? chunk
        : this.#decoder.decode(chunk, { stream: true });
    const parts = this.#buffer.split("\n");
    this.#buffer = parts.pop() ?? "";
    const lines: string[] = [];
    for (const part of parts) {
      if (this.#skipping) {
        this.#skipping = false;
        lines.push(TOO_LONG);
      } else {
        lines.push(part.length > MAX_LINE ? TOO_LONG : part.replace(/\r$/, ""));
      }
    }
    if (this.#buffer.length > MAX_LINE) {
      this.#buffer = "";
      this.#skipping = true;
    }
    return lines;
  }
}
