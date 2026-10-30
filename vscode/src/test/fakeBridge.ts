// Faux pont en mémoire pour les essais : même trame, mêmes preuves et mêmes
// erreurs que l'app (contrat 1a), sur des flux jumelés (`duplexPair`) ; formes
// des corrections de la relecture (88-fix-contract.md : `app.state.local`,
// `models.details.contextLength`, `chat.forget`, `chat.keep`) ; lot 2
// (89-lot2-voice-contract.md, E) : arrêts de lecture et de séance, reprise
// d'une séance en pause (relecture 90) acceptés,
// le reste (`tour.plan`, `speech.read`, `voice.start`…) donné par chaque essai,
// notifications envoyées par `notify` ; lot 3 (91-lot3-gpu-completion-contract.md,
// E ; formes réelles : 87-lot1a-contract.md, lot 3) : chef de la carte
// graphique simulé (`gpu`), `gpu.status { model? }`,
// `completion.suggest|prepare|measure`, `complete.fim` (`timedOut`, -32020 pour
// un modèle sans FIM), `models.offerPull` (`{ installed }`, -32602, -32007) ;
// lot 4a (93-lot4a-contract.md) : `notify.send` (refus -32034/-32035 réglé par
// `notifyRefusal`), `notify.task`, `app.show` (pages relevées dans `shown`) ;
// `study.*`, `image.generate`, `proofread.run` donnés par chaque essai.
import { createHmac, randomBytes } from "node:crypto";
import { type Duplex, duplexPair } from "node:stream";
import type { GpuPlan, GpuSlot } from "../../shared/protocol";
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
  /** Chef de la carte graphique simulé : mode du plan, modèle
   * d'autocomplétion en mémoire, réponse de conversation en cours. */
  readonly gpu = {
    mode: "together" as GpuPlan["mode"],
    completion: null as string | null,
    chatBusy: false,
    /** Modèle de la conversation en mémoire. */
    chatLoaded: true,
    /** Modèle passé à `gpu.status`. */
    watched: null as string | null,
    installed: new Set(["qwen2.5-coder:1.5b-base", "qwen2.5-coder:0.5b-base"]),
  };
  /** Texte rendu par `complete.fim`. */
  fimText = "return a + b;";
  /** Premier jeton trop lent : `complete.fim` rend `timedOut`. */
  fimTimedOut = false;
  /** Modèles sans FIM (refus -32020 du moteur). */
  readonly noFim = new Set(["chatonlm-nofim"]);
  /** Modèles dont l'app a montré la proposition de téléchargement. */
  readonly offered: string[] = [];
  /** Refus de `notify.send` (-32034 : Easy Link non relié ; -32035 :
   * réglage de l'app coupé), ou null : envoyé. */
  notifyRefusal: number | null = null;
  /** Pages ouvertes par `app.show`. */
  readonly shown: string[] = [];

  /** Plan du faux chef. */
  plan(): GpuPlan {
    const slot = (model: string, size: number, loaded = true): GpuSlot => ({
      model,
      loaded,
      sizeBytes: size,
      vramBytes: size,
      estimatedBytes: null,
      partialGpu: false,
    });
    return {
      mode: this.gpu.mode,
      chat: slot("qwen3:0.6b", 1_000_000_000, this.gpu.chatLoaded),
      completion: this.gpu.completion
        ? slot(this.gpu.completion, 1_200_000_000)
        : null,
      memory: { total: 8_000_000_000, free: 5_800_000_000, gpu: true },
      chatBusy: this.gpu.chatBusy,
      ...(this.#planReason() ? { reason: this.#planReason() } : {}),
    };
  }

  /** Raison du plan, dans l'ordre de l'app (absente : prête). */
  #planReason(): GpuPlan["reason"] {
    const model = this.gpu.watched ?? this.gpu.completion;
    if (!this.local || (model && /[:-]cloud$/.test(model))) return "notLocal";
    if (this.gpu.chatBusy) return "chatBusy";
    if (model && this.gpu.completion === model) return undefined;
    if (this.gpu.mode === "doesNotFit") return "doesNotFit";
    if (this.gpu.mode === "exclusive" && this.gpu.chatLoaded) {
      return "exclusive";
    }
    return "notLoaded";
  }

  /** Refus « en pause » de l'app (-32007). */
  #paused(reason: string): RpcFailure {
    return new RpcFailure(-32007, "paused", { reason });
  }

  /** Raison d'un refus pour ce modèle, ou null : il est prêt. */
  #notReady(model: string): string | null {
    if (!this.local || /[:-]cloud$/.test(model)) return "notLocal";
    if (!this.gpu.installed.has(model)) return "notInstalled";
    if (this.gpu.chatBusy) return "chatBusy";
    if (this.gpu.completion !== model) return "notLoaded";
    return null;
  }
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
    "notify.send": () => {
      if (this.notifyRefusal !== null) {
        throw new RpcFailure(this.notifyRefusal, "refused");
      }
      return { sent: 1 };
    },
    "notify.task": () => ({}),
    "app.show": (params) => {
      this.shown.push(params.route as string);
      return {};
    },
    "speech.stop": () => ({}),
    "voice.stop": () => ({}),
    "voice.resume": () => ({}),
    "voice.mute": () => ({}),
    "voice.context": () => ({}),
    "gpu.status": (params) => {
      if (typeof params.model === "string") this.gpu.watched = params.model;
      return this.plan();
    },
    "completion.suggest": () => ({
      recommended: "qwen2.5-coder:1.5b-base",
      options: [
        ["qwen2.5-coder:0.5b-base", "Apache-2.0", true],
        ["qwen2.5-coder:1.5b-base", "Apache-2.0", true],
        ["qwen2.5-coder:3b-base", "Qwen Research", false],
        ["qwen2.5-coder:7b-base", "Apache-2.0", true],
      ].map(([model, license, commercialUse]) => ({
        model,
        installed: this.gpu.installed.has(model as string),
        license,
        commercialUse,
        fits: model !== "qwen2.5-coder:7b-base",
        estimatedBytes: 1_000_000_000,
      })),
    }),
    // Charge seulement si le plan le permet (jamais pendant une réponse).
    "completion.prepare": (params) => {
      const model = params.model as string;
      if (!this.local) throw this.#paused("notLocal");
      if (!this.gpu.installed.has(model)) throw this.#paused("notInstalled");
      if (this.gpu.mode === "exclusive" && this.gpu.chatLoaded) {
        throw this.#paused("exclusive");
      }
      if (this.gpu.mode === "doesNotFit") throw this.#paused("doesNotFit");
      if (this.gpu.chatBusy) throw this.#paused("chatBusy");
      this.gpu.completion = model;
      return { plan: this.plan() };
    },
    // Jamais de chargement : modèle absent de la mémoire → en pause.
    "complete.fim": (params) => {
      const reason = this.#notReady(params.model as string);
      if (reason) throw this.#paused(reason);
      if (this.noFim.has(params.model as string)) {
        throw new RpcFailure(-32020, "refused", {
          message: "This model can't complete code in the middle of a file.",
        });
      }
      return this.fimTimedOut
        ? {
            text: "",
            firstTokenMs: 0,
            totalMs: 1500,
            cachedTokens: 0,
            timedOut: true,
          }
        : {
            text: this.fimText,
            firstTokenMs: 30,
            totalMs: 80,
            cachedTokens: 0,
            timedOut: false,
          };
    },
    "completion.measure": (params) => {
      const reason = this.#notReady(params.model as string);
      if (reason) throw this.#paused(reason);
      return {
        rating: "smooth",
        firstTokenMs: 120,
        tokensPerSecond: 85,
        partialGpu: false,
      };
    },
    // Seulement un nom proposé ; serveur distant : en pause `notLocal`.
    "models.offerPull": (params) => {
      const model = params.model as string;
      if (!/^qwen2\.5-coder:(0\.5|1\.5|3|7)b-base$/.test(model)) {
        throw new RpcFailure(-32602, "invalid params");
      }
      if (!this.local) throw this.#paused("notLocal");
      this.offered.push(model);
      return { installed: this.gpu.installed.has(model) };
    },
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
