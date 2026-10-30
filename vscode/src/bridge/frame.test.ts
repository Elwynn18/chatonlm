// @vitest-environment node
import { createHmac } from "node:crypto";
import { duplexPair } from "node:stream";
import { describe, expect, it } from "vitest";
import { until } from "../test/fakeBridge";
import {
  encode,
  FrameError,
  FrameReader,
  MAX_BODY,
  MAX_HEADERS,
} from "./frame";
import { clientProof, fromHex, verifyServerProof } from "./proof";
import {
  ClosedError,
  RpcConnection,
  RpcError,
  TimeoutError,
  TooLargeError,
} from "./rpc";

describe("frame", () => {
  it("writes Content-Length in bytes, not characters", () => {
    const frame = encode({ text: "é…" });
    const body = Buffer.from(JSON.stringify({ text: "é…" }));
    expect(frame.toString("ascii", 0, 22)).toBe(
      `Content-Length: ${body.length}\r\n\r\n`.slice(0, 22),
    );
    expect(frame.subarray(frame.length - body.length)).toEqual(body);
  });

  it("reads messages split across chunks and glued together", () => {
    const reader = new FrameReader();
    const bytes = Buffer.concat([encode({ a: 1 }), encode({ b: "ü" })]);
    const seen: unknown[] = [];
    for (let i = 0; i < bytes.length; i += 3) {
      seen.push(...reader.push(bytes.subarray(i, i + 3)));
    }
    expect(seen).toEqual([{ a: 1 }, { b: "ü" }]);
  });

  it("ignores other headers, whatever their case", () => {
    const reader = new FrameReader();
    const body = '{"ok":true}';
    const frame = `Content-Type: application/json\r\ncontent-length: ${body.length}\r\n\r\n${body}`;
    expect(reader.push(Buffer.from(frame))).toEqual([{ ok: true }]);
  });

  it("refuses headers over 1 KiB and bodies over 8 MiB", () => {
    expect(() =>
      new FrameReader().push(Buffer.from("X".repeat(MAX_HEADERS + 1))),
    ).toThrow(FrameError);
    expect(() =>
      new FrameReader().push(
        Buffer.from(`Content-Length: ${MAX_BODY + 1}\r\n\r\n`),
      ),
    ).toThrow(FrameError);
    expect(() => new FrameReader().push(Buffer.from("Foo: 1\r\n\r\n"))).toThrow(
      FrameError,
    );
  });
});

describe("proof", () => {
  // Formules du contrat 1a, recalculées ici sans le module.
  const secret = Buffer.alloc(32, 7);
  const server = Buffer.alloc(32, 1);
  const client = Buffer.alloc(32, 2);
  const mac = (...parts: Buffer[]) => {
    const h = createHmac("sha256", secret);
    for (const p of parts) h.update(p);
    return h.digest();
  };

  it("proves the editor over server nonce then client nonce", () => {
    expect(clientProof(secret, server, client)).toEqual(
      mac(Buffer.from("chatonlm-bridge/1 client"), server, client),
    );
  });

  it("checks the app's proof over client nonce then server nonce", () => {
    const good = mac(Buffer.from("chatonlm-bridge/1 server"), client, server);
    expect(
      verifyServerProof(secret, client, server, good.toString("hex")),
    ).toBe(true);
    const swapped = mac(
      Buffer.from("chatonlm-bridge/1 server"),
      server,
      client,
    );
    expect(
      verifyServerProof(secret, client, server, swapped.toString("hex")),
    ).toBe(false);
    expect(verifyServerProof(secret, client, server, "zz")).toBe(false);
    expect(verifyServerProof(secret, client, server, undefined)).toBe(false);
  });

  it("reads hex only", () => {
    expect(fromHex("0aff")).toEqual(Buffer.from([10, 255]));
    expect(fromHex("0g")).toBeNull();
    expect(fromHex("abc")).toBeNull();
  });
});

describe("RpcConnection", () => {
  function pair() {
    const [a, b] = duplexPair();
    const rpc = new RpcConnection(a);
    const reader = new FrameReader();
    const received: Record<string, unknown>[] = [];
    b.on("data", (chunk: Buffer) =>
      received.push(...(reader.push(chunk) as Record<string, unknown>[])),
    );
    return { rpc, peer: b, received };
  }

  it("numbers requests and matches answers, errors included", async () => {
    const { rpc, peer, received } = pair();
    const one = rpc.call("ping");
    const two = rpc.call("chat.get", { chatId: "x" });
    expect(two.id).toBe(one.id + 1);
    await until(() => received.length === 2);
    expect(received[1]).toEqual({
      jsonrpc: "2.0",
      id: two.id,
      method: "chat.get",
      params: { chatId: "x" },
    });
    peer.write(
      encode({
        jsonrpc: "2.0",
        id: two.id,
        error: { code: -32020, message: "refused", data: { message: "Gone." } },
      }),
    );
    peer.write(encode({ jsonrpc: "2.0", id: one.id, result: {} }));
    await expect(one.result).resolves.toEqual({});
    const error = await two.result.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RpcError);
    expect((error as RpcError).code).toBe(-32020);
    expect((error as RpcError).appMessage).toBe("Gone.");
  });

  it("never gives the same number twice, even on a new connection", () => {
    // Le panneau range reçus et envois par ce numéro : après une
    // reconnexion, un ancien numéro ne doit pas désigner un nouvel envoi.
    const first = pair().rpc;
    const calls = [first.call("ping"), first.call("ping")];
    first.close();
    const second = pair().rpc;
    calls.push(second.call("ping"));
    second.close();
    for (const call of calls) call.result.catch(() => {});
    expect(new Set(calls.map((call) => call.id)).size).toBe(3);
  });

  it("fails only the request too large for the frame", async () => {
    const { rpc, received } = pair();
    const huge = rpc.call("chat.send", {
      content: "x".repeat(9 * 1024 * 1024),
    });
    await expect(huge.result).rejects.toBeInstanceOf(TooLargeError);
    expect(rpc.closed).toBe(false);
    rpc.call("ping");
    await until(() => received.length === 1);
    expect(received[0]).toMatchObject({ method: "ping" });
  });

  it("rejects pending requests when the connection closes", async () => {
    const { rpc, peer } = pair();
    const call = rpc.call("chat.send", { content: "x" });
    peer.destroy();
    await expect(call.result).rejects.toBeInstanceOf(ClosedError);
    expect(rpc.closed).toBe(true);
  });

  it("gives up on a request without answer after its delay", async () => {
    const { rpc } = pair();
    await expect(rpc.request("initialize", {}, 20)).rejects.toBeInstanceOf(
      TimeoutError,
    );
  });

  it("closes on an unreadable frame", async () => {
    const { rpc, peer } = pair();
    peer.write(Buffer.from("Content-Length: 3\r\n\r\n{{{"));
    await until(() => rpc.closed);
  });

  it("sends $/cancelRequest with the request id", async () => {
    const { rpc, received } = pair();
    rpc.cancel(4);
    await until(() => received.length === 1);
    expect(received[0]).toEqual({
      jsonrpc: "2.0",
      method: "$/cancelRequest",
      params: { id: 4 },
    });
  });
});
