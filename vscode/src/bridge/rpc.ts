// Client JSON-RPC 2.0 sur la trame du pont, écrit à la main (pas de
// vscode-jsonrpc) : requêtes numérotées, notifications, annulation.
import type { Duplex } from "node:stream";
import { encode, FrameReader } from "./frame";

/** Codes d'erreur du pont (contrat 1a). */
export const Codes = {
  parse: -32700,
  invalidRequest: -32600,
  methodNotFound: -32601,
  invalidParams: -32602,
  unauthenticated: -32001,
  unknownClient: -32002,
  noPairing: -32003,
  tooManyAttempts: -32004,
  wrongCode: -32005,
  incompatible: -32010,
  refused: -32020,
} as const;

/** Erreur rendue par l'app. */
export class RpcError extends Error {
  readonly code: number;
  readonly data: unknown;
  constructor(code: number, message: string, data?: unknown) {
    super(message);
    this.code = code;
    this.data = data;
  }

  /** Texte de l'app déjà traduit (`data.message` d'un refus), s'il y en a un. */
  get appMessage(): string | null {
    const data = this.data as { message?: unknown } | undefined;
    return typeof data?.message === "string" ? data.message : null;
  }
}

/** Connexion fermée avant la réponse. */
export class ClosedError extends Error {
  constructor() {
    super("connection closed");
  }
}

/** Pas de réponse dans le délai. */
export class TimeoutError extends Error {
  constructor(method: string) {
    super(`${method}: no answer`);
  }
}

export type Notification = { method: string; params: unknown };

type Pending = {
  method: string;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
};

/** Requête envoyée : son numéro (connu avant la réponse) et sa réponse. */
export type Call<T> = { id: number; result: Promise<T> };

/** Message trop gros pour la trame (8 Mio) : seule sa requête échoue. */
export class TooLargeError extends Error {
  constructor(method: string) {
    super(`${method}: message too large`);
  }
}

/**
 * Numéros de requête uniques pour toute la session de l'hôte d'extensions,
 * pas seulement pour une connexion : après une reconnexion, un numéro déjà
 * donné au panneau (reçu, événements d'un envoi) ne désigne jamais un autre
 * envoi.
 */
let nextId = 1;

export class RpcConnection {
  readonly #socket: Duplex;
  readonly #reader = new FrameReader();
  readonly #pending = new Map<number, Pending>();
  readonly #notificationListeners = new Set<(n: Notification) => void>();
  readonly #closeListeners = new Set<() => void>();
  #closed = false;

  constructor(socket: Duplex) {
    this.#socket = socket;
    socket.on("data", (chunk: Buffer) => this.#receive(chunk));
    socket.on("error", () => this.#shutdown());
    socket.on("close", () => this.#shutdown());
    socket.on("end", () => this.#shutdown());
  }

  get closed(): boolean {
    return this.#closed;
  }

  onNotification(listener: (n: Notification) => void): () => void {
    this.#notificationListeners.add(listener);
    return () => this.#notificationListeners.delete(listener);
  }

  onClose(listener: () => void): () => void {
    if (this.#closed) {
      queueMicrotask(listener);
      return () => {};
    }
    this.#closeListeners.add(listener);
    return () => this.#closeListeners.delete(listener);
  }

  /** Envoie une requête ; `timeoutMs` : rejet si l'app ne répond pas à temps. */
  call<T>(method: string, params: unknown = {}, timeoutMs?: number): Call<T> {
    const id = nextId++;
    const result = new Promise<T>((resolve, reject) => {
      if (this.#closed) {
        reject(new ClosedError());
        return;
      }
      let frame: Buffer;
      try {
        frame = encode({ jsonrpc: "2.0", id, method, params });
      } catch {
        reject(new TooLargeError(method));
        return;
      }
      const pending: Pending = {
        method,
        resolve: resolve as (value: unknown) => void,
        reject,
      };
      if (timeoutMs !== undefined) {
        pending.timer = setTimeout(() => {
          this.#pending.delete(id);
          reject(new TimeoutError(method));
        }, timeoutMs);
      }
      this.#pending.set(id, pending);
      this.#writeFrame(frame);
    });
    return { id, result };
  }

  request<T>(method: string, params: unknown = {}, timeoutMs?: number) {
    return this.call<T>(method, params, timeoutMs).result;
  }

  /** Demande l'arrêt d'une requête en cours (`$/cancelRequest`). */
  cancel(id: number): void {
    this.#write({ jsonrpc: "2.0", method: "$/cancelRequest", params: { id } });
  }

  close(): void {
    if (this.#closed) return;
    this.#socket.end();
    this.#socket.destroy();
    this.#shutdown();
  }

  #write(message: unknown): void {
    let frame: Buffer;
    try {
      frame = encode(message);
    } catch {
      return;
    }
    this.#writeFrame(frame);
  }

  #writeFrame(frame: Buffer): void {
    if (this.#closed) return;
    try {
      this.#socket.write(frame);
    } catch {
      this.close();
    }
  }

  #receive(chunk: Buffer): void {
    let messages: unknown[];
    try {
      messages = this.#reader.push(chunk);
    } catch {
      // Trame illisible : on ne peut plus se resynchroniser.
      this.close();
      return;
    }
    for (const message of messages) this.#dispatch(message);
  }

  #dispatch(message: unknown): void {
    if (typeof message !== "object" || message === null) return;
    const m = message as {
      id?: unknown;
      method?: unknown;
      params?: unknown;
      result?: unknown;
      error?: { code?: unknown; message?: unknown; data?: unknown };
    };
    if (typeof m.method === "string") {
      if (m.id !== undefined && m.id !== null) return; // l'app ne demande rien
      for (const listener of this.#notificationListeners) {
        listener({ method: m.method, params: m.params });
      }
      return;
    }
    if (typeof m.id !== "number") return;
    const pending = this.#pending.get(m.id);
    if (!pending) return;
    this.#pending.delete(m.id);
    if (pending.timer) clearTimeout(pending.timer);
    if (m.error) {
      pending.reject(
        new RpcError(
          typeof m.error.code === "number" ? m.error.code : 0,
          typeof m.error.message === "string" ? m.error.message : "error",
          m.error.data,
        ),
      );
    } else {
      pending.resolve(m.result);
    }
  }

  #shutdown(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const pending of this.#pending.values()) {
      if (pending.timer) clearTimeout(pending.timer);
      pending.reject(new ClosedError());
    }
    this.#pending.clear();
    for (const listener of this.#closeListeners) listener();
    this.#closeListeners.clear();
  }
}
