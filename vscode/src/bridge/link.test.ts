// @vitest-environment node
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeBridge, until } from "../test/fakeBridge";
import type { Discovery } from "./discovery";
import { type Credentials, Link, PairError } from "./link";

function setup(
  options: {
    credentials?: Credentials | null;
    discovery?: Discovery | null;
    running?: boolean;
  } = {},
) {
  const fake = new FakeBridge();
  const store = {
    value: options.credentials ?? null,
  } as { value: Credentials | null };
  let discovery: Discovery | null =
    options.discovery === undefined ? fake.discovery : options.discovery;
  let changed: (() => void) | null = null;
  const reads = { count: 0 };
  const link = new Link({
    dir: () => "/run/user/1000/com.chatonlm.chatonlm",
    readDiscovery: async () => {
      reads.count++;
      return discovery;
    },
    connect: fake.connect,
    secrets: {
      get: async () => store.value,
      set: async (c) => {
        store.value = c;
      },
      delete: async () => {
        store.value = null;
      },
    },
    client: { name: "test", version: "0", editor: "VS Code" },
    watch: (_dir, onChange) => {
      changed = onChange;
      return { close: () => (changed = null) };
    },
    appRunning: async () => options.running ?? false,
  });
  links.push(link);
  return {
    fake,
    link,
    store,
    reads,
    setDiscovery: (d: Discovery | null) => (discovery = d),
    fileChanged: () => changed?.(),
  };
}

const links: Link[] = [];
afterEach(() => {
  for (const link of links.splice(0)) link.dispose();
  vi.useRealTimers();
});

describe("Link", () => {
  it("connects with a stored secret once the app has proved itself", async () => {
    const t = setup();
    t.store.value = t.fake.addClient();
    await t.link.start();
    expect(t.link.state.connection).toBe("connected");
    expect(t.link.state.app).toEqual({ version: "0.9.4", platform: "linux" });
    await expect(t.link.request("ping")).resolves.toEqual({});
    expect(t.fake.requests.map((r) => r.method)).toEqual([
      "initialize",
      "auth",
      "ping",
    ]);
  });

  it("sends nothing after auth when the app's proof is wrong", async () => {
    const t = setup();
    const credentials = t.fake.addClient();
    t.store.value = credentials;
    t.fake.wrongServerProof = true;
    await t.link.start();
    expect(t.link.state).toMatchObject({
      connection: "error",
      problem: "badServerProof",
    });
    expect(t.link.connected).toBe(false);
    await expect(t.link.request("ping")).rejects.toThrow("connection closed");
    expect(t.fake.requests.map((r) => r.method)).toEqual([
      "initialize",
      "auth",
    ]);
    // Le secret reste : c'est l'autre côté qui n'est pas le bon.
    expect(t.store.value).toEqual(credentials);
  });

  it("forgets a secret the app no longer knows (-32002)", async () => {
    const t = setup({
      credentials: { clientId: "gone", secret: "ab".repeat(32) },
    });
    await t.link.start();
    expect(t.link.state.connection).toBe("notPaired");
    expect(t.store.value).toBeNull();
  });

  it("is not paired without a secret, and keeps no connection open", async () => {
    const t = setup();
    await t.link.start();
    expect(t.link.state.connection).toBe("notPaired");
    expect(t.fake.sessions).toHaveLength(0);
  });

  it("pairs with the code, stores the secret and authenticates", async () => {
    const t = setup();
    await t.link.start();
    t.fake.pairingCode = "123456";
    const connected = vi.fn();
    t.link.onConnected(connected);
    await t.link.pair("123456", "VSCodium");
    expect(t.store.value?.clientId).toBe("client-1");
    expect(t.link.state.connection).toBe("connected");
    expect(connected).toHaveBeenCalledOnce();
    expect(t.fake.requests.map((r) => r.method)).toEqual([
      "initialize",
      "pair",
      "auth",
    ]);
    expect(t.fake.requests[1].params).toEqual({
      code: "123456",
      clientName: "VSCodium",
    });
  });

  it("reports why pairing failed", async () => {
    const t = setup();
    await t.link.start();
    await expect(t.link.pair("123456", "x")).rejects.toMatchObject({
      reason: "noPairing",
    });
    t.fake.pairingCode = "654321";
    const error = await t.link.pair("123456", "x").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(PairError);
    expect((error as PairError).reason).toBe("wrongCode");
    expect(t.store.value).toBeNull();
    t.setDiscovery(null);
    await expect(t.link.pair("654321", "x")).rejects.toMatchObject({
      reason: "appClosed",
    });
  });

  it("tells a closed app from a link turned off", async () => {
    const closed = setup({ discovery: null, running: false });
    await closed.link.start();
    expect(closed.link.state.connection).toBe("appClosed");
    const off = setup({ discovery: null, running: true });
    await off.link.start();
    expect(off.link.state.connection).toBe("bridgeOff");
  });

  it("is incompatible when the app speaks another protocol", async () => {
    const t = setup({
      credentials: { clientId: "c", secret: "ab".repeat(32) },
    });
    t.setDiscovery({ ...t.fake.discovery, protocol: [2, 3] });
    await t.link.start();
    expect(t.link.state.connection).toBe("incompatible");
    expect(t.fake.sessions).toHaveLength(0);

    const u = setup({
      credentials: { clientId: "c", secret: "ab".repeat(32) },
    });
    u.fake.protocol = [2, 2];
    await u.link.start();
    expect(u.link.state.connection).toBe("incompatible");
  });

  it("reconnects after the app drops the connection", async () => {
    const t = setup();
    t.store.value = t.fake.addClient();
    const connected = vi.fn();
    t.link.onConnected(connected);
    await t.link.start();
    expect(connected).toHaveBeenCalledTimes(1);
    t.fake.dropAll();
    await until(() => t.link.state.connection === "connecting");
    await until(() => connected.mock.calls.length === 2, 3000);
    expect(t.link.state.connection).toBe("connected");
    expect(t.fake.authenticated).toHaveLength(1);
  });

  it("retries after 0.25 s, doubling up to 30 s while the app is closed", async () => {
    vi.useFakeTimers();
    const t = setup({ discovery: null });
    await t.link.start();
    expect(t.reads.count).toBe(1);
    const at = (ms: number) => vi.advanceTimersByTimeAsync(ms);
    await at(249);
    expect(t.reads.count).toBe(1);
    await at(1); // 250
    expect(t.reads.count).toBe(2);
    await at(500); // 750
    expect(t.reads.count).toBe(3);
    await at(1000); // 1 750
    expect(t.reads.count).toBe(4);
    await at(2000 + 4000); // 3 750, 7 750
    expect(t.reads.count).toBe(6);
    await at(8000 + 16000); // 15 750, 31 750
    expect(t.reads.count).toBe(8);
    await at(29_999); // plafond : 30 s
    expect(t.reads.count).toBe(8);
    await at(1);
    expect(t.reads.count).toBe(9);
  });

  it("retries a refused connection every 5 s at most", async () => {
    vi.useFakeTimers();
    const t = setup({ running: true });
    t.store.value = t.fake.addClient();
    t.fake.refuse = true;
    await t.link.start();
    const at = (ms: number) => vi.advanceTimersByTimeAsync(ms);
    await at(250 + 500 + 1000 + 2000 + 4000);
    const count = t.reads.count;
    await at(5000);
    expect(t.reads.count).toBe(count + 1);
  });

  it("waits 15 s for the app to authenticate (slow disk)", async () => {
    const t = setup();
    t.store.value = t.fake.addClient();
    t.fake.authDelayMs = 6000;
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const started = t.link.start();
    await vi.advanceTimersByTimeAsync(6000);
    vi.useRealTimers();
    await started;
    expect(t.link.state.connection).toBe("connected");
  });

  it("follows a pairing made or removed in another window", async () => {
    const t = setup();
    await t.link.start();
    expect(t.link.state.connection).toBe("notPaired");
    // Appairage fait dans une autre fenêtre : le secret gardé change.
    t.store.value = t.fake.addClient();
    await t.link.secretsChanged();
    expect(t.link.connected).toBe(true);
    // Rien ne change tant que le secret est le même.
    await t.link.secretsChanged();
    expect(t.fake.sessions).toHaveLength(1);
    // Retiré dans l'autre fenêtre : la connexion se ferme.
    t.store.value = null;
    await t.link.secretsChanged();
    expect(t.link.state.connection).toBe("notPaired");
    await until(() => t.fake.authenticated.length === 0);
  });

  it("retries at once when bridge.json changes", async () => {
    vi.useFakeTimers();
    const t = setup({ discovery: null });
    t.store.value = t.fake.addClient();
    await t.link.start();
    expect(t.link.state.connection).toBe("appClosed");
    t.setDiscovery(t.fake.discovery);
    t.fileChanged();
    vi.useRealTimers();
    await until(() => t.link.connected);
  });

  it("relays notifications and cancels by request id", async () => {
    const t = setup();
    t.store.value = t.fake.addClient();
    const seen = vi.fn();
    t.link.onNotification(seen);
    t.fake.handlers["chat.send"] = (_params, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "Hi" },
      });
      return { chatId: "c1", outcome: "done" };
    };
    await t.link.start();
    const call = t.link.call("chat.send", { content: "Hello" });
    await expect(call.result).resolves.toEqual({
      chatId: "c1",
      outcome: "done",
    });
    expect(seen).toHaveBeenCalledWith({
      method: "chat.event",
      params: { requestId: call.id, event: { type: "content", text: "Hi" } },
    });
    t.link.cancel(call.id);
    await until(() => t.fake.cancelled.includes(call.id));
  });

  it("forgets the secret on unpair", async () => {
    const t = setup();
    t.store.value = t.fake.addClient();
    await t.link.start();
    await t.link.unpair();
    expect(t.store.value).toBeNull();
    expect(t.link.state.connection).toBe("notPaired");
    await until(() => t.fake.authenticated.length === 0);
  });
});
