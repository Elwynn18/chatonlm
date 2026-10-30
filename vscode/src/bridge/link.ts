// Liaison avec l'app : découverte, connexion, appairage, authentification
// (preuve de l'app vérifiée avant tout), reconnexion (0,25 s doublée jusqu'à
// 5 s ; app fermée ou pont coupé : jusqu'à 30 s, et dès que `bridge.json`
// change). Sans dépendance à VS Code : tout ce
// qui touche le système est passé par `LinkDeps`. Contrats :
// .tools/research/87-lot1a-contract.md, 87-lot1b-contract.md (B).
import type { Duplex } from "node:stream";
import type { Connection } from "../../shared/protocol";
import { compatible, type Discovery, PROTOCOL } from "./discovery";
import { fromHex, newNonce, clientProof, verifyServerProof } from "./proof";
import {
  type Call,
  ClosedError,
  Codes,
  type Notification,
  RpcConnection,
  RpcError,
} from "./rpc";

/** Ce que l'app a donné à l'appairage, gardé dans le stockage secret. */
export type Credentials = { clientId: string; secret: string };

export type ClientInfo = { name: string; version: string; editor: string };

export type AppInfo = { version: string; platform: string };

/** Réponse à `initialize` : 5 s (l'app ferme au bout de 5 s une connexion
 * qui ne s'authentifie pas). */
export const HANDSHAKE_TIMEOUT_MS = 5000;
/** Réponse à `pair` et `auth`, qui attendent une écriture de l'app dans sa
 * base (lente sous une forte charge disque) : 15 s. */
export const PAIR_AUTH_TIMEOUT_MS = 15_000;
export const FIRST_RETRY_MS = 250;
export const MAX_RETRY_MS = 5000;
/** App fermée ou pont coupé : sondage espacé (le dossier surveillé, un
 * lancement ou le retour dans la fenêtre relancent l'essai aussitôt). */
export const IDLE_RETRY_MS = 30_000;

export type LinkDeps = {
  /** Dossier de `bridge.json`, ou null (pas de dossier privé). */
  dir(): string | null;
  readDiscovery(dir: string): Promise<Discovery | null>;
  connect(endpoint: string): Promise<Duplex>;
  secrets: {
    get(): Promise<Credentials | null>;
    set(credentials: Credentials): Promise<void>;
    delete(): Promise<void>;
  };
  client: ClientInfo;
  /** Surveille le dossier ; rend de quoi arrêter, ou null s'il n'existe pas. */
  watch(dir: string, changed: () => void): { close(): void } | null;
  /** L'app tourne-t-elle (pour distinguer pont coupé et app fermée) ? */
  appRunning(): Promise<boolean | null>;
};

export type LinkState = {
  connection: Connection;
  /** Code d'une erreur à traduire (état `error`). */
  problem?: LinkProblem;
  app?: AppInfo;
};

export type LinkProblem =
  /** Linux sans $XDG_RUNTIME_DIR. */
  | "noRuntimeDir"
  /** L'app n'a pas prouvé qu'elle connaît le secret. */
  | "badServerProof"
  /** Autre échec de la poignée de main. */
  | "handshake";

/** Échec de l'appairage, avec le code de l'app. */
export class PairError extends Error {
  readonly reason:
    | "appClosed"
    | "bridgeOff"
    | "incompatible"
    | "noPairing"
    | "wrongCode"
    | "tooManyAttempts"
    | "failed";
  constructor(reason: PairError["reason"], message: string = reason) {
    super(message);
    this.reason = reason;
  }
}

type Handshake = { rpc: RpcConnection; nonce: Buffer; serverNonce: Buffer };

export class Link {
  readonly #deps: LinkDeps;
  #state: LinkState = { connection: "connecting" };
  #rpc: RpcConnection | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #delay = FIRST_RETRY_MS;
  #watcher: { close(): void } | null = null;
  #watchedDir: string | null = null;
  #attempting: Promise<void> | null = null;
  #again = false;
  #stopped = false;
  /** Secret de la connexion en cours. */
  #used: Credentials | null = null;
  readonly #stateListeners = new Set<(state: LinkState) => void>();
  readonly #connectedListeners = new Set<() => void>();
  readonly #notificationListeners = new Set<(n: Notification) => void>();

  constructor(deps: LinkDeps) {
    this.#deps = deps;
  }

  get state(): LinkState {
    return this.#state;
  }

  get connected(): boolean {
    return this.#state.connection === "connected" && this.#rpc !== null;
  }

  onState(listener: (state: LinkState) => void): () => void {
    this.#stateListeners.add(listener);
    return () => this.#stateListeners.delete(listener);
  }

  /** À chaque connexion authentifiée (la première comme les suivantes). */
  onConnected(listener: () => void): () => void {
    this.#connectedListeners.add(listener);
    return () => this.#connectedListeners.delete(listener);
  }

  onNotification(listener: (n: Notification) => void): () => void {
    this.#notificationListeners.add(listener);
    return () => this.#notificationListeners.delete(listener);
  }

  start(): Promise<void> {
    this.#stopped = false;
    return this.#attempt();
  }

  dispose(): void {
    this.#stopped = true;
    this.#clearTimer();
    this.#watcher?.close();
    this.#watcher = null;
    this.#watchedDir = null;
    this.#rpc?.close();
    this.#rpc = null;
  }

  /**
   * Le secret gardé a changé (appairage ou retrait dans une autre fenêtre) :
   * nouvel essai, et la connexion en cours est fermée si elle n'emploie plus
   * le secret gardé.
   */
  async secretsChanged(): Promise<void> {
    const stored = await this.#deps.secrets.get();
    const used = this.#used;
    if (
      this.connected &&
      stored &&
      used &&
      stored.clientId === used.clientId &&
      stored.secret === used.secret
    ) {
      return;
    }
    if (this.#rpc) {
      const rpc = this.#rpc;
      this.#rpc = null;
      rpc.close();
    }
    // Sans secret, l'essai s'arrête à « notPaired ».
    await this.retryNow();
  }

  /** Nouvel essai tout de suite (fichier de découverte changé, app lancée…). */
  retryNow(): Promise<void> {
    if (this.#stopped) return Promise.resolve();
    this.#delay = FIRST_RETRY_MS;
    this.#clearTimer();
    return this.#attempt();
  }

  /** Requête authentifiée ; ClosedError si l'app n'est pas reliée. */
  call<T>(method: string, params: unknown = {}): Call<T> {
    if (!this.#rpc || !this.connected) {
      return { id: 0, result: Promise.reject(new ClosedError()) };
    }
    return this.#rpc.call<T>(method, params);
  }

  request<T>(method: string, params: unknown = {}): Promise<T> {
    return this.call<T>(method, params).result;
  }

  cancel(id: number): void {
    this.#rpc?.cancel(id);
  }

  /**
   * Appairage avec le code montré par l'app : nouvelle connexion,
   * `initialize`, `pair`, secret gardé, puis `auth` sur la même connexion.
   */
  async pair(code: string, clientName: string): Promise<void> {
    this.#clearTimer();
    const dir = this.#deps.dir();
    if (!dir) throw new PairError("failed", "no runtime dir");
    const discovery = await this.#deps.readDiscovery(dir);
    if (!discovery) {
      throw new PairError(
        (await this.#deps.appRunning()) ? "bridgeOff" : "appClosed",
      );
    }
    let handshake: Handshake;
    try {
      handshake = await this.#handshake(discovery);
    } catch (error) {
      if (error instanceof RpcError && error.code === Codes.incompatible) {
        this.#set({ connection: "incompatible" });
        throw new PairError("incompatible");
      }
      throw new PairError("appClosed");
    }
    let credentials: Credentials;
    try {
      const answer = await handshake.rpc.request<{
        clientId: string;
        secret: string;
      }>("pair", { code, clientName }, PAIR_AUTH_TIMEOUT_MS);
      credentials = { clientId: answer.clientId, secret: answer.secret };
    } catch (error) {
      handshake.rpc.close();
      if (error instanceof RpcError) {
        if (error.code === Codes.noPairing) throw new PairError("noPairing");
        if (error.code === Codes.wrongCode) throw new PairError("wrongCode");
        if (error.code === Codes.tooManyAttempts) {
          throw new PairError("tooManyAttempts");
        }
      }
      throw new PairError("failed", String(error));
    }
    await this.#deps.secrets.set(credentials);
    const outcome = await this.#authenticate(handshake, credentials);
    if (outcome !== "connected") {
      throw new PairError("failed", outcome);
    }
  }

  /** Oublie le secret de cet éditeur (l'app le garde dans sa liste). */
  async unpair(): Promise<void> {
    await this.#deps.secrets.delete();
    this.#clearTimer();
    this.#rpc?.close();
    this.#rpc = null;
    this.#set({ connection: "notPaired" });
  }

  #set(state: LinkState): void {
    this.#state = state;
    for (const listener of this.#stateListeners) listener(state);
  }

  #clearTimer(): void {
    if (this.#timer) clearTimeout(this.#timer);
    this.#timer = null;
  }

  #schedule(cap = MAX_RETRY_MS): void {
    if (this.#stopped) return;
    this.#clearTimer();
    const delay = Math.min(this.#delay, cap);
    this.#delay = Math.min(this.#delay * 2, cap);
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#attempt();
    }, delay);
  }

  #watch(dir: string): void {
    if (this.#watchedDir === dir && this.#watcher) return;
    this.#watcher?.close();
    this.#watcher = this.#deps.watch(dir, () => {
      if (!this.connected) void this.retryNow();
    });
    this.#watchedDir = this.#watcher ? dir : null;
  }

  /** Un seul essai à la fois ; un essai demandé pendant un autre est rejoué après. */
  #attempt(): Promise<void> {
    if (this.#attempting) {
      this.#again = true;
      return this.#attempting;
    }
    this.#attempting = (async () => {
      try {
        do {
          this.#again = false;
          await this.#attemptOnce();
        } while (this.#again && !this.connected && !this.#stopped);
      } finally {
        this.#attempting = null;
      }
    })();
    return this.#attempting;
  }

  async #attemptOnce(): Promise<void> {
    if (this.#stopped || this.connected) return;
    const dir = this.#deps.dir();
    if (!dir) {
      this.#set({ connection: "error", problem: "noRuntimeDir" });
      return;
    }
    this.#watch(dir);
    const discovery = await this.#deps.readDiscovery(dir);
    if (!discovery) {
      const running = await this.#deps.appRunning();
      this.#set({ connection: running ? "bridgeOff" : "appClosed" });
      this.#schedule(IDLE_RETRY_MS);
      return;
    }
    if (!compatible(discovery)) {
      // Rien à refaire tant que l'app ne change pas (le fichier sera réécrit).
      this.#set({ connection: "incompatible" });
      return;
    }
    const credentials = await this.#deps.secrets.get();
    if (!credentials) {
      // Pas de connexion gardée : l'app la fermerait au bout de 5 s.
      this.#set({ connection: "notPaired" });
      return;
    }
    if (this.#state.connection !== "connecting") {
      this.#set({ connection: "connecting" });
    }
    let handshake: Handshake;
    try {
      handshake = await this.#handshake(discovery);
    } catch (error) {
      if (error instanceof RpcError && error.code === Codes.incompatible) {
        this.#set({ connection: "incompatible" });
        return;
      }
      // Fichier resté d'un arrêt brutal, ou app en train de s'ouvrir.
      this.#set({
        connection: (await this.#deps.appRunning())
          ? "connecting"
          : "appClosed",
      });
      this.#schedule();
      return;
    }
    const outcome = await this.#authenticate(handshake, credentials);
    if (outcome === "retry") this.#schedule();
  }

  async #handshake(discovery: Discovery): Promise<Handshake> {
    const socket = await this.#deps.connect(discovery.endpoint);
    const rpc = new RpcConnection(socket);
    const nonce = newNonce();
    try {
      const answer = await rpc.request<{
        protocol: number;
        app: AppInfo;
        serverNonce: string;
      }>(
        "initialize",
        {
          protocol: PROTOCOL,
          client: this.#deps.client,
          clientNonce: nonce.toString("hex"),
        },
        HANDSHAKE_TIMEOUT_MS,
      );
      const serverNonce = fromHex(answer.serverNonce);
      if (!serverNonce) throw new Error("bad server nonce");
      this.#state = { ...this.#state, app: answer.app };
      return { rpc, nonce, serverNonce };
    } catch (error) {
      rpc.close();
      throw error;
    }
  }

  /** `auth`, puis vérification de la preuve de l'app avant toute autre requête. */
  async #authenticate(
    { rpc, nonce, serverNonce }: Handshake,
    credentials: Credentials,
  ): Promise<"connected" | "notPaired" | "retry" | "badServerProof"> {
    const secret = fromHex(credentials.secret);
    if (!secret) {
      rpc.close();
      await this.#deps.secrets.delete();
      this.#set({ connection: "notPaired" });
      return "notPaired";
    }
    const app = this.#state.app;
    try {
      const answer = await rpc.request<{ serverProof: string }>(
        "auth",
        {
          clientId: credentials.clientId,
          proof: clientProof(secret, serverNonce, nonce).toString("hex"),
        },
        PAIR_AUTH_TIMEOUT_MS,
      );
      if (!verifyServerProof(secret, nonce, serverNonce, answer?.serverProof)) {
        // Ce n'est pas l'app appairée : rien ne lui est envoyé.
        rpc.close();
        this.#set({ connection: "error", problem: "badServerProof" });
        return "badServerProof";
      }
    } catch (error) {
      rpc.close();
      if (
        error instanceof RpcError &&
        (error.code === Codes.unknownClient ||
          error.code === Codes.unauthenticated)
      ) {
        // Éditeur retiré dans l'app, ou secret qui ne correspond plus.
        await this.#deps.secrets.delete();
        this.#set({ connection: "notPaired" });
        return "notPaired";
      }
      this.#set({ connection: "connecting", app });
      return "retry";
    }
    this.#rpc?.close();
    this.#rpc = rpc;
    this.#used = credentials;
    this.#delay = FIRST_RETRY_MS;
    rpc.onNotification((n) => {
      for (const listener of this.#notificationListeners) listener(n);
    });
    rpc.onClose(() => {
      if (this.#rpc !== rpc) return;
      this.#rpc = null;
      if (this.#stopped) return;
      this.#set({ connection: "connecting", app });
      this.#schedule();
    });
    this.#set({ connection: "connected", app });
    for (const listener of this.#connectedListeners) listener();
    return "connected";
  }
}
