// Client du lien VS Code pour les essais : ce que fera l'extension, en Node
// seul (JSON-RPC 2.0 sur la trame de LSP, preuves HMAC-SHA256). Contrat :
// .tools/research/87-lot1a-contract.md.
import { createHmac, randomBytes } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import net from "node:net";
import { join } from "node:path";

/** Dossier du pont et son fichier de découverte, ou null. */
export function findDiscovery() {
  const runtime = process.env.XDG_RUNTIME_DIR;
  if (!runtime || !existsSync(runtime)) return null;
  for (const name of readdirSync(runtime)) {
    const file = join(runtime, name, "bridge.json");
    if (name.startsWith("com.chatonlm") && existsSync(file)) {
      return { dir: join(runtime, name), file };
    }
  }
  return null;
}

export function readDiscovery() {
  const found = findDiscovery();
  return found ? JSON.parse(readFileSync(found.file, "utf8")) : null;
}

const hmac = (secret, ...parts) => {
  const mac = createHmac("sha256", secret);
  for (const part of parts) mac.update(part);
  return mac.digest();
};

/** Connexion ouverte au point d'accès. */
export class BridgeClient {
  static async connect(endpoint) {
    const socket = net.connect({ path: endpoint });
    await new Promise((resolve, reject) => {
      socket.once("connect", resolve);
      socket.once("error", reject);
    });
    return new BridgeClient(socket);
  }

  constructor(socket) {
    this.socket = socket;
    this.buffer = Buffer.alloc(0);
    this.next = 1;
    this.pending = new Map();
    this.notifications = [];
    this.closed = false;
    this.nonce = randomBytes(32);
    this.serverNonce = null;
    socket.on("data", (chunk) => this.#receive(chunk));
    this.whenClosed = new Promise((resolve) =>
      socket.on("close", () => {
        this.closed = true;
        for (const { reject } of this.pending.values()) {
          reject(new Error("connection closed"));
        }
        this.pending.clear();
        resolve();
      }),
    );
    socket.on("error", () => {});
  }

  #receive(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const end = this.buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      const headers = this.buffer.subarray(0, end).toString("ascii");
      const length = Number(/content-length:\s*(\d+)/i.exec(headers)?.[1]);
      if (this.buffer.length < end + 4 + length) return;
      const body = this.buffer.subarray(end + 4, end + 4 + length);
      this.buffer = this.buffer.subarray(end + 4 + length);
      const message = JSON.parse(body.toString("utf8"));
      const waiting = message.id != null && this.pending.get(message.id);
      if (waiting && !message.method) {
        this.pending.delete(message.id);
        waiting.resolve(message);
      } else {
        this.notifications.push(message);
      }
    }
  }

  send(message) {
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", ...message }));
    this.socket.write(
      Buffer.concat([
        Buffer.from(`Content-Length: ${body.length}\r\n\r\n`),
        body,
      ]),
    );
  }

  /** Réponse entière (`result` ou `error`). */
  request(method, params = {}) {
    const id = this.next++;
    const answer = new Promise((resolve, reject) =>
      this.pending.set(id, { resolve, reject }),
    );
    this.send({ id, method, params });
    return answer;
  }

  async initialize() {
    const answer = await this.request("initialize", {
      protocol: [1, 1],
      client: { name: "e2e", version: "0", editor: "VS Code" },
      clientNonce: this.nonce.toString("hex"),
    });
    this.serverNonce = Buffer.from(answer.result.serverNonce, "hex");
    return answer.result;
  }

  /** Appairage : `{ clientId, secret }` (secret en Buffer). */
  async pair(code, clientName) {
    const answer = await this.request("pair", { code, clientName });
    if (answer.error) return answer;
    return {
      clientId: answer.result.clientId,
      secret: Buffer.from(answer.result.secret, "hex"),
    };
  }

  /** Preuve de l'éditeur, puis vérification de celle de l'app. */
  async auth(clientId, secret) {
    const proof = hmac(
      secret,
      Buffer.from("chatonlm-bridge/1 client"),
      this.serverNonce,
      this.nonce,
    );
    const answer = await this.request("auth", {
      clientId,
      proof: proof.toString("hex"),
    });
    if (answer.error) return answer;
    const expected = hmac(
      secret,
      Buffer.from("chatonlm-bridge/1 server"),
      this.nonce,
      this.serverNonce,
    ).toString("hex");
    if (answer.result.serverProof !== expected) {
      throw new Error("the app did not prove itself");
    }
    return answer;
  }

  close() {
    this.socket.end();
    return this.whenClosed;
  }
}
