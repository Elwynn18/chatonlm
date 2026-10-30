// Faux pont en mémoire pour les essais : même trame, mêmes preuves et mêmes
// erreurs que l'app (contrat 1a), sur des flux jumelés (`duplexPair`) ; formes
// des corrections de la relecture (88-fix-contract.md : `app.state.local`,
// `models.details.contextLength`, `chat.forget`, `chat.keep`) ; lot 2
// (89-lot2-voice-contract.md, E) : arrêts de lecture et de séance, reprise
// d'une séance en pause (relecture 90) acceptés,
// le reste (`tour.plan`, `speech.read`, `voice.start`…) donné par chaque essai,
// notifications envoyées par `notify`.
import { createHmac, randomBytes } from "node:crypto";
import { type Duplex, duplexPair } from "node:stream";
import type { Discovery } from "../bridge/discovery";
import { encode, FrameReader } from "../bridge/frame";

type Session = {
  socket: Duplex;
  nonces: { server: Buffer; client: Buffer } | null;
  client: string | null;
  closed: boolean;
};

export type Handler = (
  params: Record<string, unknown>,
  context: { id: number; notify(method: string, params: unknown): void },
) => unknown;

export class RpcFailure {
  constructor(
    readonly code: number,
    readonly message: string,
    readonly data?: unknown,
  ) {}
}

const hmac = (secret: Buffer, ...parts: Buffer[]) => {
  const mac = createHmac("sha256", secret);
  for (const part of parts) mac.update(part);
  return mac.digest();
};

export class FakeBridge {
  readonly clients = new Map<string, Buffer>();
  pairingCode: string | null = null;
  protocol: [number, number] = [1, 1];
  /** Rend une preuve fausse (une autre app que celle appairée). */
  wrongServerProof = false;
  /** Refuse toute connexion (fichier resté d'un arrêt brutal). */
  refuse = false;
  /** Réponse à `auth` retardée (écriture lente de l'app dans sa base). */
  authDelayMs = 0;
  readonly sessions: Session[] = [];
  readonly requests: { method: string; params: unknown }[] = [];
  readonly cancelled: number[] = [];
  /** Le serveur Ollama visé est sur cette machine (`app.state.local`). */
  local = true;
  /** Conversations effacées par `chat.forget`, gardées par `chat.keep`. */
  readonly forgotten: string[] = [];
  readonly kept: string[] = [];
  readonly handlers: Record<string, Handler> = {
    ping: () => ({}),
    "app.state": () => ({
      version: "0.9.4",
      platform: "linux",
      selectedModel: "qwen3:0.6b",
      language: "en",
      local: this.local,
      contextLength: 0,
    }),
    "models.list": () => [
      { model: "qwen3:0.6b", contextLength: 40960 },
      { model: "gpt-oss:120b-cloud", contextLength: 131072 },
    ],
    "models.details": (params) => {
      const remote = String(params.model).endsWith("-cloud");
      return {
        model: params.model,
        remote,
        // Contexte effectif du moteur (réglage borné par le modèle).
        contextLength: remote ? null : 8192,
      };
    },
    "chat.forget": (params) => {
      this.forgotten.push(params.chatId as string);
      return { forgotten: true };
    },
    "chat.keep": (params) => {
      this.kept.push(params.chatId as string);
      return {};
    },
    "speech.stop": () => ({}),
    "voice.stop": () => ({}),
    "voice.resume": () => ({}),
    "voice.mute": () => ({}),
    "voice.context": () => ({}),
  };

  readonly discovery: Discovery = {
    protocol: [1, 1],
    endpoint: "/fake/bridge.sock",
    pid: 1,
    app: "0.9.4",
    instance: "test",
    startedAt: 0,
  };

  connect = async (endpoint: string): Promise<Duplex> => {
    if (this.refuse || endpoint !== this.discovery.endpoint) {
      throw Object.assign(new Error("connect ECONNREFUSED"), {
        code: "ECONNREFUSED",
      });
    }
    const [client, server] = duplexPair();
    const session: Session = {
      socket: server,
      nonces: null,
      client: null,
      closed: false,
    };
    this.sessions.push(session);
    const reader = new FrameReader();
    server.on("data", (chunk: Buffer) => {
      for (const message of reader.push(chunk)) {
        void this.#handle(session, message as Record<string, unknown>);
      }
    });
    server.on("error", () => {});
    server.on("end", () => {
      session.closed = true;
      server.destroy();
    });
    server.on("close", () => (session.closed = true));
    client.on("error", () => {});
    return client;
  };

  /** Ajoute un client connu (comme un appairage déjà fait). */
  addClient(id = "client-1"): { clientId: string; secret: string } {
    const secret = randomBytes(32);
    this.clients.set(id, secret);
    return { clientId: id, secret: secret.toString("hex") };
  }

  /** Coupe toutes les connexions (app fermée ou relancée). */
  dropAll(): void {
    for (const session of this.sessions) session.socket.destroy();
  }

  /** Notification de l'app à chaque éditeur authentifié. */
  notify(method: string, params: unknown): void {
    for (const session of this.authenticated) {
      this.#send(session, { jsonrpc: "2.0", method, params });
    }
  }

  /** Requêtes reçues pour une méthode (paramètres). */
  calls(method: string): Record<string, unknown>[] {
    return this.requests
      .filter((r) => r.method === method)
      .map((r) => r.params as Record<string, unknown>);
  }

  get authenticated(): Session[] {
    return this.sessions.filter((s) => s.client !== null && !s.closed);
  }

  #send(session: Session, message: unknown) {
    if (!session.socket.destroyed) session.socket.write(encode(message));
  }

  async #handle(session: Session, message: Record<string, unknown>) {
    const method = message.method as string;
    const id = message.id as number | undefined;
    const params = (message.params ?? {}) as Record<string, unknown>;
    this.requests.push({ method, params });
    const reply = (result: unknown) =>
      this.#send(session, { jsonrpc: "2.0", id, result });
    const fail = (code: number, text: string, data?: unknown) =>
      this.#send(session, {
        jsonrpc: "2.0",
        id,
        error: { code, message: text, ...(data ? { data } : {}) },
      });
    if (method === "$/cancelRequest") {
      this.cancelled.push(params.id as number);
      return;
    }
    if (id === undefined) return;
    if (method === "initialize") {
      const [min, max] = params.protocol as [number, number];
      if (max < this.protocol[0] || min > this.protocol[1]) {
        return fail(-32010, "incompatible protocol", { app: this.protocol });
      }
      const server = randomBytes(32);
      session.nonces = {
        server,
        client: Buffer.from(params.clientNonce as string, "hex"),
      };
      return reply({
        protocol: 1,
        app: { version: "0.9.4", platform: "linux" },
        serverNonce: server.toString("hex"),
      });
    }
    if (method === "pair") {
      if (!session.nonces) return fail(-32600, "initialize first");
      if (!this.pairingCode) return fail(-32003, "no pairing code");
      if (params.code !== this.pairingCode) return fail(-32005, "wrong code");
      this.pairingCode = null;
      const { clientId, secret } = this.addClient(
        `client-${this.clients.size + 1}`,
      );
      return reply({ clientId, secret });
    }
    if (method === "auth") {
      if (!session.nonces) return fail(-32600, "initialize first");
      const secret = this.clients.get(params.clientId as string);
      if (!secret) return fail(-32002, "unknown client");
      const expected = hmac(
        secret,
        Buffer.from("chatonlm-bridge/1 client"),
        session.nonces.server,
        session.nonces.client,
      ).toString("hex");
      if (params.proof !== expected) return fail(-32001, "bad proof");
      if (this.authDelayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, this.authDelayMs));
      }
      session.client = params.clientId as string;
      const proof = hmac(
        this.wrongServerProof ? randomBytes(32) : secret,
        Buffer.from("chatonlm-bridge/1 server"),
        session.nonces.client,
        session.nonces.server,
      );
      return reply({ serverProof: proof.toString("hex") });
    }
    if (!session.client) return fail(-32001, "not authenticated");
    const handler = this.handlers[method];
    if (!handler) return fail(-32601, "method not found");
    try {
      const result = await handler(params, {
        id,
        notify: (m, p) =>
          this.#send(session, { jsonrpc: "2.0", method: m, params: p }),
      });
      reply(result);
    } catch (error) {
      if (error instanceof RpcFailure)
        fail(error.code, error.message, error.data);
      else throw error;
    }
  }
}

/** Promesse résolue par l'essai. */
export function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

/** Attend qu'une condition devienne vraie (essais asynchrones). */
export async function until(check: () => boolean, ms = 2000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("condition not met in time");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}
