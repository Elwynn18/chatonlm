// Trame du pont : celle de LSP (en-têtes `Content-Length: N`, ligne vide,
// corps JSON). Contrat : .tools/research/87-lot1a-contract.md.

/** En-têtes : 1 Kio au plus (comme l'app). */
export const MAX_HEADERS = 1024;
/** Corps : 8 Mio au plus (comme l'app). */
export const MAX_BODY = 8 * 1024 * 1024;

const SEPARATOR = Buffer.from("\r\n\r\n");

export class FrameError extends Error {}

/** Message JSON-RPC prêt à écrire. */
export function encode(message: unknown): Buffer {
  const body = Buffer.from(JSON.stringify(message), "utf8");
  if (body.length > MAX_BODY) throw new FrameError("message too large");
  return Buffer.concat([
    Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"),
    body,
  ]);
}

/** Découpe un flux d'octets en messages JSON. */
export class FrameReader {
  #buffer: Buffer = Buffer.alloc(0);

  /** Messages complets reçus avec ce morceau ; lève FrameError sur une trame invalide. */
  push(chunk: Buffer): unknown[] {
    this.#buffer =
      this.#buffer.length === 0 ? chunk : Buffer.concat([this.#buffer, chunk]);
    const messages: unknown[] = [];
    for (;;) {
      const end = this.#buffer.indexOf(SEPARATOR);
      if (end < 0) {
        if (this.#buffer.length > MAX_HEADERS) {
          throw new FrameError("headers too long");
        }
        return messages;
      }
      if (end > MAX_HEADERS) throw new FrameError("headers too long");
      const headers = this.#buffer.subarray(0, end).toString("ascii");
      const match = /^content-length:[ \t]*(\d+)[ \t]*$/im.exec(headers);
      if (!match) throw new FrameError("missing Content-Length");
      const length = Number(match[1]);
      if (length > MAX_BODY) throw new FrameError("body too large");
      const start = end + SEPARATOR.length;
      if (this.#buffer.length < start + length) return messages;
      const body = this.#buffer.subarray(start, start + length);
      this.#buffer = this.#buffer.subarray(start + length);
      try {
        messages.push(JSON.parse(body.toString("utf8")));
      } catch {
        throw new FrameError("invalid JSON");
      }
    }
  }
}
