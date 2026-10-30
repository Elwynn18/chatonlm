// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HostMessage } from "../shared/protocol";
import { type Credentials, Link } from "./bridge/link";
import { Controller } from "./controller";
import { deferred, FakeBridge, RpcFailure, until } from "./test/fakeBridge";
import {
  Diagnostic,
  Position,
  Range,
  resetTestState,
  Selection,
  TextDocument,
  testState,
  Uri,
  window,
} from "./test/vscode";

type Sent = {
  content: string;
  model?: string;
  chatId?: string;
  ephemeral?: boolean;
};

function chatOf(id: string, extra: Record<string, unknown> = {}) {
  return {
    id,
    title: "",
    createdAt: 0,
    updatedAt: 0,
    messages: [],
    context: null,
    compactions: [],
    replyRunning: null,
    ephemeral: false,
    ...extra,
  };
}

async function setup(options: { paired?: boolean } = {}) {
  const fake = new FakeBridge();
  let credentials: Credentials | null =
    options.paired === false ? null : fake.addClient();
  const sent: Sent[] = [];
  let chats = 0;
  const ephemeralChats = new Set<string>();
  fake.handlers["chat.send"] = (params, { id, notify }) => {
    const p = params as Sent;
    sent.push(p);
    const chatId = p.chatId ?? `chat-${++chats}`;
    if (p.ephemeral) ephemeralChats.add(chatId);
    if (!p.chatId)
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId },
      });
    notify("chat.event", {
      requestId: id,
      event: { type: "replyStarted", messageId: 2, createdAt: 0 },
    });
    notify("chat.event", {
      requestId: id,
      event: { type: "content", text: "fix: a\n\n- b" },
    });
    notify("chat.event", {
      requestId: id,
      event: { type: "done", messageId: 2 },
    });
    return { chatId, outcome: "done" };
  };
  fake.handlers["chat.get"] = (params) => {
    if (params.chatId === "gone")
      throw new RpcFailure(-32020, "refused", { message: "No." });
    const chatId = params.chatId as string;
    return chatOf(chatId, { ephemeral: ephemeralChats.has(chatId) });
  };
  const link = new Link({
    dir: () => "/run",
    readDiscovery: async () => fake.discovery,
    connect: fake.connect,
    secrets: {
      get: async () => credentials,
      set: async (c) => void (credentials = c),
      delete: async () => void (credentials = null),
    },
    client: { name: "t", version: "0", editor: "VS Code" },
    watch: () => null,
    appRunning: async () => false,
  });
  const posted: HostMessage[] = [];
  const memory = new Map<string, unknown>();
  const deps = {
    link,
    memento: {
      get: <T>(key: string) => memory.get(key) as T | undefined,
      update: async (key: string, value: unknown) =>
        void memory.set(key, value),
    },
    post: (m: HostMessage) => posted.push(m),
    reveal: vi.fn(async () => {}),
    launchApp: vi.fn(async () => {}),
    pair: vi.fn(async () => {}),
    locale: () => "fr" as const,
    theme: () => "dark" as const,
  };
  const controller = new Controller(deps);
  cleanups.push(() => {
    controller.dispose();
    link.dispose();
  });
  await link.start();
  if (link.connected) await until(() => controller.state.models.length > 0);
  return { fake, link, controller, posted, sent, deps, memory };
}

const cleanups: (() => void)[] = [];
beforeEach(() => resetTestState());
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
});

const ofType = <T extends HostMessage["type"]>(
  posted: HostMessage[],
  type: T,
) =>
  posted.filter((m): m is Extract<HostMessage, { type: T }> => m.type === type);

function editor(
  file: string,
  text: string,
  from: [number, number],
  to: [number, number],
  languageId = "typescript",
) {
  const document = new TextDocument(file, text, languageId);
  testState.documents.set(file, document);
  window.activeTextEditor = {
    document,
    selection: new Selection(new Position(...from), new Position(...to)),
  };
  return document;
}

describe("Controller", () => {
  it("answers ready with the locale, theme and state", async () => {
    const { controller, posted } = await setup();
    await controller.handle({ type: "ready" });
    expect(ofType(posted, "init").at(-1)).toEqual({
      type: "init",
      locale: "fr",
      theme: "dark",
      state: {
        connection: "connected",
        models: ["qwen3:0.6b", "gpt-oss:120b-cloud"],
        cloudModels: ["gpt-oss:120b-cloud"],
        model: "qwen3:0.6b",
        ephemeral: false,
        chatId: null,
        busy: false,
        generation: 0,
        notice: null,
      },
    });
  });

  it("relays a send: receipt first, then events under the same id, then the chat", async () => {
    const { controller, posted, sent } = await setup();
    posted.length = 0;
    await controller.handle({ type: "send", text: "Hello" });
    expect(sent).toEqual([{ content: "Hello", model: "qwen3:0.6b" }]);
    const kinds = posted
      .filter((m) => m.type !== "state")
      .map((m) => (m.type === "event" ? `event:${m.event.type}` : m.type));
    expect(kinds).toEqual([
      "receipt",
      "event:chatCreated",
      "event:replyStarted",
      "event:content",
      "event:done",
      // Reçus redonnés avec chaque relecture de la conversation.
      "receipts",
      "chat",
    ]);
    const receipt = ofType(posted, "receipt")[0];
    const ids = new Set(ofType(posted, "event").map((m) => m.requestId));
    expect(ids).toEqual(new Set([receipt.requestId]));
    expect(receipt.receipt).toEqual({
      items: [],
      chars: 5,
      tokens: 2,
      // Contexte que le moteur emploie (models.details), pas le maximum.
      contextLength: 8192,
      destination: "local",
      blocked: [],
    });
    expect(controller.state.chatId).toBe("chat-1");
    expect(controller.state.busy).toBe(false);
    // La suite va dans la même conversation.
    await controller.handle({ type: "send", text: "More" });
    expect(sent[1]).toEqual({
      content: "More",
      model: "qwen3:0.6b",
      chatId: "chat-1",
    });
  });

  it("keeps each receipt under its saved message and gives them back when the panel opens", async () => {
    const { fake, controller, posted, memory, deps } = await setup();
    fake.handlers["chat.send"] = (params, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId: "chat-9" },
      });
      notify("chat.event", {
        requestId: id,
        event: {
          type: "userMessage",
          message: { id: 41, role: "user", content: (params as Sent).content },
        },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "done", messageId: null },
      });
      return { chatId: "chat-9", outcome: "done" };
    };
    // Envoi fait panneau fermé : le reçu en direct est perdu pour lui.
    await controller.handle({ type: "send", text: "Hello" });
    const receipt = ofType(posted, "receipt")[0].receipt;
    posted.length = 0;
    await controller.handle({ type: "ready" });
    const given = ofType(posted, "receipts");
    expect(given.length).toBeGreaterThan(0);
    for (const message of given) {
      expect(message).toEqual({ type: "receipts", receipts: { 41: receipt } });
    }
    // Gardé d'une session de l'éditeur à l'autre.
    expect(memory.get("chatonlm.receipts")).toEqual({ 41: receipt });
    const again: HostMessage[] = [];
    const next = new Controller({ ...deps, post: (m) => again.push(m) });
    cleanups.push(() => next.dispose());
    await next.handle({ type: "ready" });
    expect(ofType(again, "receipts")[0].receipts).toEqual({ 41: receipt });
  });

  it("writes a panel error to the log", async () => {
    const { deps } = await setup();
    const lines: string[] = [];
    const controller = new Controller({ ...deps, log: (m) => lines.push(m) });
    cleanups.push(() => controller.dispose());
    await controller.handle({ type: "panelError", message: "boom" });
    expect(lines).toEqual(["Panel error: boom"]);
  });

  it("is busy while a reply is written, and stops it by request id", async () => {
    const { fake, controller, posted } = await setup();
    const release = deferred();
    fake.handlers["chat.send"] = async (_p, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId: "c" },
      });
      await release.promise;
      return { chatId: "c", outcome: "cancelled" };
    };
    const sending = controller.handle({ type: "send", text: "x" });
    await until(() => controller.state.chatId === "c");
    expect(controller.state.busy).toBe(true);
    expect(ofType(posted, "state").at(-1)?.state.busy).toBe(true);
    await controller.handle({ type: "stop" });
    const id = ofType(posted, "receipt")[0].requestId;
    await until(() => fake.cancelled.includes(id));
    release.resolve();
    await sending;
    expect(controller.state.busy).toBe(false);
  });

  it("starts an ephemeral chat when asked", async () => {
    const { controller, sent } = await setup();
    await controller.handle({ type: "send", text: "a" });
    await controller.handle({ type: "newChat", ephemeral: true });
    expect(controller.state).toMatchObject({ chatId: null, ephemeral: true });
    await controller.handle({ type: "send", text: "b" });
    expect(sent[1]).toEqual({
      content: "b",
      model: "qwen3:0.6b",
      ephemeral: true,
    });
  });

  it("remembers the chosen model and says when it runs in the cloud", async () => {
    const { controller, posted, sent, memory } = await setup();
    await controller.handle({
      type: "selectModel",
      model: "gpt-oss:120b-cloud",
    });
    expect(memory.get("chatonlm.model")).toBe("gpt-oss:120b-cloud");
    await controller.handle({ type: "send", text: "hi" });
    expect(sent[0].model).toBe("gpt-oss:120b-cloud");
    expect(ofType(posted, "receipt")[0].receipt).toMatchObject({
      destination: "cloud",
      contextLength: null,
    });
  });

  it("explains the selection with its path and lines", async () => {
    const { controller, sent, posted, deps } = await setup();
    editor(
      "/work/src/a.ts",
      "const a = 1;\nfunction f() {\n  return a;\n}\n",
      [1, 0],
      [4, 0],
    );
    await controller.runAction("explain");
    expect(deps.reveal).toHaveBeenCalled();
    expect(sent[0].content).toBe(
      "Explain what this code does and how it works.\n\n`src/a.ts`, lines 2–4:\n```typescript\nfunction f() {\n  return a;\n}\n```",
    );
    expect(ofType(posted, "receipt")[0].receipt.items).toEqual([
      {
        kind: "selection",
        label: "src/a.ts",
        path: "src/a.ts",
        lines: [2, 4],
        chars: 29,
      },
    ]);
  });

  it("keeps a secret file away from a cloud model, but not from a local one", async () => {
    const { controller, sent } = await setup();
    editor("/work/.env", "TOKEN=abcd1234\n", [0, 0], [0, 14], "dotenv");
    await controller.selectModel("gpt-oss:120b-cloud");
    await controller.runAction("explain");
    expect(sent).toEqual([]);
    expect(testState.shown.at(-1)?.text).toMatch(/^Nothing was sent/);
    await controller.selectModel("qwen3:0.6b");
    await controller.runAction("explain");
    expect(sent[0].content).toContain("TOKEN=abcd1234");
  });

  it("masks secret lines for a cloud model and lists them in the receipt", async () => {
    const { controller, sent, posted } = await setup();
    editor(
      "/work/src/db.ts",
      'const user = "a";\nconst password = "hunter22";\n',
      [0, 0],
      [2, 0],
    );
    await controller.selectModel("gpt-oss:120b-cloud");
    await controller.runAction("tests");
    expect(sent[0].content).not.toContain("hunter22");
    expect(sent[0].content).toContain(
      'const user = "a";\n[secret removed by ChatonLM]\n',
    );
    expect(ofType(posted, "receipt")[0].receipt.blocked).toEqual([
      { label: "src/db.ts, line 2", reason: "secretValue" },
    ]);
  });

  it("fixes the problem given by the quick fix", async () => {
    const { controller, sent } = await setup();
    const lines = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join(
      "\n",
    );
    editor("/work/src/b.ts", lines, [0, 0], [0, 0]);
    const diagnostic = new Diagnostic(
      new Range(14, 0, 14, 4),
      "Cannot find name 'x'.",
    );
    diagnostic.source = "ts";
    diagnostic.code = 2304;
    await controller.runAction("fix", [Uri.file("/work/src/b.ts"), diagnostic]);
    const content = sent[0].content;
    expect(content).toMatch(/^Fix this problem\./);
    expect(content).toContain(
      "Problem in `src/b.ts`, line 15:\n```\nerror (ts 2304): Cannot find name 'x'.\n```",
    );
    expect(content).toContain(
      "`src/b.ts`, lines 5–25:\n```typescript\nline 5\n",
    );
  });

  it("finds the problem under the cursor, or says there is none", async () => {
    const { controller, sent } = await setup();
    editor("/work/src/c.ts", "a\nb\nc", [1, 0], [1, 0]);
    await controller.runAction("fix");
    expect(sent).toEqual([]);
    expect(testState.shown.at(-1)?.text).toBe(
      "No problem is reported under the cursor.",
    );
    testState.diagnostics = [new Diagnostic(new Range(1, 0, 1, 1), "Oops", 1)];
    await controller.runAction("fix");
    expect(sent[0].content).toContain("warning: Oops");
  });

  it("writes the commit message into the repository's box", async () => {
    const { controller, sent, posted } = await setup();
    const repository = {
      rootUri: Uri.file("/work"),
      inputBox: { value: "" },
      diff: vi.fn(async (cached?: boolean) =>
        cached ? "" : "diff --git a/x b/x\n+y\n",
      ),
    };
    testState.extensions.set("vscode.git", {
      enabled: true,
      getAPI: () => ({
        repositories: [repository],
        getRepository: () => repository,
      }),
    });
    await controller.runAction("commitMessage", [
      { rootUri: Uri.file("/work") },
    ]);
    expect(repository.diff).toHaveBeenCalledWith(true);
    expect(repository.diff).toHaveBeenCalledWith(false);
    expect(sent[0].content).toContain(
      "Uncommitted changes:\n```diff\ndiff --git a/x b/x\n+y\n```",
    );
    expect(ofType(posted, "receipt")[0].receipt.items[0]).toMatchObject({
      kind: "diff",
      label: "Uncommitted changes",
    });
    expect(repository.inputBox.value).toBe("fix: a\n\n- b");
  });

  it("asks to connect instead of sending when the app is not linked", async () => {
    const { controller, deps, sent } = await setup({ paired: false });
    expect(controller.state.connection).toBe("notPaired");
    editor("/work/src/a.ts", "x", [0, 0], [0, 1]);
    await controller.runAction("explain");
    expect(sent).toEqual([]);
    expect(deps.reveal).toHaveBeenCalled();
    expect(testState.shown.at(-1)?.level).toBe("warning");
    await controller.handle({ type: "pair" });
    expect(deps.pair).toHaveBeenCalled();
  });

  it("reads the chat again after a reconnection", async () => {
    const { fake, controller, posted, link } = await setup();
    await controller.handle({ type: "send", text: "a" });
    posted.length = 0;
    fake.dropAll();
    await until(() => !link.connected);
    await until(() => link.connected, 3000);
    await until(() => ofType(posted, "chat").length > 0);
    expect(ofType(posted, "chat")[0].chat.id).toBe("chat-1");
  });

  it("forgets a chat the app no longer has", async () => {
    const { fake, controller } = await setup();
    fake.handlers["chat.send"] = () => ({ chatId: "gone", outcome: "done" });
    await controller.handle({ type: "send", text: "a" });
    expect(controller.state.chatId).toBeNull();
  });

  it("shows the connection as starting while the app launches", async () => {
    const { controller, deps, posted } = await setup({ paired: false });
    const release = deferred();
    deps.launchApp.mockImplementation(() => release.promise);
    // App fermée pendant le lancement : « connecting », pas « appClosed ».
    const link = deps.link as Link;
    vi.spyOn(link, "state", "get").mockReturnValue({ connection: "appClosed" });
    const launching = controller.handle({ type: "launchApp" });
    expect(ofType(posted, "state").at(-1)?.state.connection).toBe("connecting");
    expect(deps.launchApp).toHaveBeenCalledWith(true);
    release.resolve();
    await launching;
    expect(ofType(posted, "state").at(-1)?.state.connection).toBe("appClosed");
  });

  it("opens only web links and copies text", async () => {
    const { controller } = await setup();
    await controller.handle({ type: "openLink", url: "https://chatonlm.com" });
    await controller.handle({ type: "openLink", url: "file:///etc/passwd" });
    await controller.handle({
      type: "openLink",
      url: "command:workbench.action.quit",
    });
    await controller.handle({
      type: "openLink",
      url: "mailto:a@b.c?body=secret",
    });
    expect(testState.opened).toEqual(["https://chatonlm.com"]);
    await controller.handle({ type: "copy", text: "abc" });
    expect(testState.clipboard).toBe("abc");
  });
});

/** Événements relayés au panneau pour une requête. */
const eventsOf = (posted: HostMessage[], requestId: number) =>
  ofType(posted, "event")
    .filter((m) => m.requestId === requestId)
    .map((m) => m.event);

describe("Controller, after the review", () => {
  it("ends a send the app refused, so the panel is free again (C1)", async () => {
    const { fake, controller, posted } = await setup();
    fake.handlers["chat.send"] = () => {
      throw new RpcFailure(-32020, "refused", { message: "Busy." });
    };
    await controller.handle({ type: "send", text: "x" });
    const id = ofType(posted, "receipt")[0].requestId;
    expect(eventsOf(posted, id)).toEqual([
      { type: "error", kind: "internal", message: "Busy." },
    ]);
    expect(controller.state.busy).toBe(false);
  });

  it("ends a send cut by a lost link, then reads the chat again (C1)", async () => {
    const { fake, controller, posted, link } = await setup();
    fake.handlers["chat.send"] = async (_p, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId: "A" },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "par" },
      });
      setTimeout(() => fake.dropAll(), 5);
      await new Promise(() => {});
    };
    await controller.handle({ type: "send", text: "x" });
    const id = ofType(posted, "receipt")[0].requestId;
    expect(eventsOf(posted, id).at(-1)).toMatchObject({
      type: "error",
      kind: "connectionLost",
    });
    expect(controller.state).toMatchObject({ busy: false, chatId: "A" });
    posted.length = 0;
    await until(() => link.connected, 3000);
    await until(() => ofType(posted, "chat").length > 0);
    expect(ofType(posted, "chat")[0].chat.id).toBe("A");
  });

  it("ends a send whose answer came without an end event (C1)", async () => {
    const { fake, controller, posted } = await setup();
    fake.handlers["chat.send"] = () => ({ chatId: "A", outcome: "cancelled" });
    await controller.handle({ type: "send", text: "x" });
    const id = ofType(posted, "receipt")[0].requestId;
    expect(eventsOf(posted, id)).toEqual([{ type: "cancelled" }]);
  });

  it("keeps a new chat asked for during a reply, ephemeral included (C2)", async () => {
    const { fake, controller, posted } = await setup();
    const release = deferred();
    const sent: Sent[] = [];
    fake.handlers["chat.send"] = async (params, { id, notify }) => {
      const p = params as Sent;
      sent.push(p);
      const chatId = p.chatId ?? (sent.length === 1 ? "A" : "B");
      if (!p.chatId)
        notify("chat.event", {
          requestId: id,
          event: { type: "chatCreated", chatId },
        });
      if (sent.length === 1) {
        await release.promise;
        notify("chat.event", {
          requestId: id,
          event: { type: "content", text: "late" },
        });
      }
      return { chatId, outcome: "done" };
    };
    const sending = controller.handle({ type: "send", text: "first" });
    await until(() => controller.state.chatId === "A");
    await controller.handle({ type: "newChat", ephemeral: true });
    expect(controller.state).toMatchObject({
      chatId: null,
      ephemeral: true,
      generation: 1,
    });
    release.resolve();
    await sending;
    // La réponse finie ne ramène pas l'ancienne conversation…
    expect(controller.state).toMatchObject({ chatId: null, ephemeral: true });
    // … et ses derniers fragments ne vont plus au panneau.
    const first = ofType(posted, "receipt")[0].requestId;
    expect(eventsOf(posted, first).map((e) => e.type)).toEqual(["chatCreated"]);
    await controller.handle({ type: "send", text: "secret" });
    expect(sent[1]).toEqual({
      content: "secret",
      model: "qwen3:0.6b",
      ephemeral: true,
    });
  });

  it("erases the ephemeral chat a reply created after it was left (C2, I1)", async () => {
    const { fake, controller } = await setup();
    const release = deferred();
    fake.handlers["chat.send"] = async (_p, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId: "E" },
      });
      await release.promise;
      return { chatId: "E", outcome: "done" };
    };
    controller.newChat(true);
    const sending = controller.handle({ type: "send", text: "x" });
    await until(() => controller.state.chatId === "E");
    controller.newChat(false);
    await until(() => fake.forgotten.includes("E"));
    release.resolve();
    await sending;
    expect(controller.state.chatId).toBeNull();
  });

  it("stops a send while it is being prepared (C4)", async () => {
    const { fake, controller, posted } = await setup();
    const details = deferred();
    const original = fake.handlers["models.details"];
    fake.handlers["models.details"] = async (params, context) => {
      await details.promise;
      return original(params, context);
    };
    let sends = 0;
    fake.handlers["chat.send"] = () => {
      sends++;
      return { chatId: "A", outcome: "done" };
    };
    const sending = controller.handle({ type: "send", text: "x" });
    await until(() => controller.state.busy);
    await controller.handle({ type: "stop" });
    details.resolve();
    await sending;
    expect(sends).toBe(0);
    expect(ofType(posted, "receipt")).toEqual([]);
    expect(controller.state.busy).toBe(false);
  });

  it("reads the models again when the panel opens, after a reply, and on demand (C5)", async () => {
    const { fake, controller } = await setup();
    let models = [{ model: "qwen3:0.6b", contextLength: 40960 }];
    fake.handlers["models.list"] = () => models;
    models = [...models, { model: "gemma3:270m", contextLength: 32768 }];
    await controller.handle({ type: "ready" });
    expect(controller.state.models).toContain("gemma3:270m");
    models = [{ model: "llama3.2:1b", contextLength: 8192 }];
    await controller.handle({ type: "refresh" });
    expect(controller.state.models).toEqual(["llama3.2:1b"]);
    models = [{ model: "qwen3:0.6b", contextLength: 40960 }];
    await controller.handle({ type: "send", text: "x" });
    expect(controller.state.models).toEqual(["qwen3:0.6b"]);
  });

  it("never reuses a request number after a reconnection (C3)", async () => {
    const { fake, controller, posted, link } = await setup();
    await controller.handle({ type: "send", text: "a" });
    fake.dropAll();
    await until(() => !link.connected);
    await until(() => link.connected, 3000);
    await controller.handle({ type: "send", text: "b" });
    const ids = ofType(posted, "receipt").map((m) => m.requestId);
    expect(ids).toHaveLength(2);
    expect(ids[0]).not.toBe(ids[1]);
  });

  it("treats an Ollama of another computer as off this computer (S2, C9)", async () => {
    const { fake, controller, sent, posted } = await setup();
    fake.local = false;
    editor("/work/src/db.ts", 'const password = "hunter22";\n', [0, 0], [1, 0]);
    await controller.runAction("explain");
    expect(sent[0].content).not.toContain("hunter22");
    expect(ofType(posted, "receipt")[0].receipt).toMatchObject({
      destination: "remote",
      blocked: [{ label: "src/db.ts, line 1", reason: "secretValue" }],
    });
  });

  it("treats an unknown destination as off this computer (S2)", async () => {
    const { fake, controller, sent, posted } = await setup();
    fake.handlers["models.details"] = () => {
      throw new RpcFailure(-32020, "refused", { message: "No." });
    };
    editor("/work/.env", "TOKEN=abcd1234\n", [0, 0], [0, 14], "dotenv");
    await controller.runAction("explain");
    expect(sent).toEqual([]);
    delete fake.handlers["models.details"];
    fake.handlers["app.state"] = () => ({ selectedModel: "qwen3:0.6b" });
    editor("/work/src/a.ts", 'token = "abcd1234"\n', [0, 0], [1, 0]);
    await controller.runAction("explain");
    expect(sent[0].content).not.toContain("abcd1234");
    expect(ofType(posted, "receipt")[0].receipt.destination).toBe("unknown");
  });

  it("opens a new chat when a chat begun here would go off this computer (S3)", async () => {
    const { controller, sent } = await setup();
    editor("/work/.env", "TOKEN=abcd1234\n", [0, 0], [0, 14], "dotenv");
    await controller.runAction("explain");
    expect(controller.state.chatId).toBe("chat-1");
    await controller.selectModel("gpt-oss:120b-cloud");
    expect(controller.state).toMatchObject({
      chatId: null,
      notice: "offMachineNewChat",
      generation: 1,
    });
    await controller.handle({ type: "send", text: "more" });
    expect(sent[1]).toEqual({ content: "more", model: "gpt-oss:120b-cloud" });
    // L'avis dure jusqu'à l'envoi suivant.
    expect(controller.state.notice).toBeNull();
  });

  it("does the same when the app's settings move the model away (S3)", async () => {
    const { fake, controller, sent } = await setup();
    await controller.handle({ type: "send", text: "a" });
    expect(controller.state.chatId).toBe("chat-1");
    fake.local = false;
    await controller.handle({ type: "send", text: "b" });
    expect(sent[1]).toEqual({ content: "b", model: "qwen3:0.6b" });
    expect(controller.state.chatId).toBe("chat-2");
  });

  it("keeps a chat begun off this computer when the model stays off it", async () => {
    const { controller, sent } = await setup();
    await controller.selectModel("gpt-oss:120b-cloud");
    await controller.handle({ type: "send", text: "a" });
    await controller.selectModel("gpt-oss:120b-cloud");
    await controller.handle({ type: "send", text: "b" });
    expect(sent[1].chatId).toBe("chat-1");
  });

  it("counts the earlier messages that go with a send (M1)", async () => {
    const { fake, controller, posted } = await setup();
    const saved = [
      { id: 1, role: "user", content: "abcd", thinking: "", toolCalls: [] },
      {
        id: 2,
        role: "assistant",
        content: "efgh",
        thinking: "",
        toolCalls: [],
      },
    ];
    fake.handlers["chat.get"] = (params) =>
      chatOf(params.chatId as string, { messages: saved });
    await controller.handle({ type: "send", text: "a" });
    expect(ofType(posted, "receipt")[0].receipt).not.toHaveProperty("history");
    await controller.handle({ type: "send", text: "b".repeat(8) });
    expect(ofType(posted, "receipt")[1].receipt).toMatchObject({
      history: { messages: 2, chars: 8 },
      chars: 8,
      tokens: 4,
    });
  });

  it("erases the ephemeral chat left for another one, and only it (I1)", async () => {
    const { fake, controller } = await setup();
    await controller.handle({ type: "send", text: "a" });
    controller.newChat(true);
    expect(fake.forgotten).toEqual([]);
    await controller.handle({ type: "send", text: "b" });
    const ephemeral = controller.state.chatId;
    controller.newChat(false);
    await until(() => fake.forgotten.length > 0);
    expect(fake.forgotten).toEqual([ephemeral]);
  });

  it("erases the ephemeral chat when the extension stops (I1)", async () => {
    const { fake, controller } = await setup();
    controller.newChat(true);
    await controller.handle({ type: "send", text: "a" });
    await controller.forgetEphemeral();
    expect(fake.forgotten).toEqual(["chat-1"]);
    expect(controller.state.chatId).toBeNull();
  });

  it("keeps the ephemeral chat with Keep, like the app", async () => {
    const { fake, controller } = await setup();
    controller.newChat(true);
    await controller.handle({ type: "send", text: "a" });
    await controller.handle({ type: "keep" });
    expect(fake.kept).toEqual(["chat-1"]);
    expect(controller.state).toMatchObject({
      chatId: "chat-1",
      ephemeral: false,
    });
    // Gardée : plus rien à effacer en la quittant.
    controller.newChat(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fake.forgotten).toEqual([]);
  });

  it("says why Keep failed", async () => {
    const { fake, controller } = await setup();
    fake.handlers["chat.keep"] = () => {
      throw new RpcFailure(-32020, "refused", { message: "Gone." });
    };
    controller.newChat(true);
    await controller.handle({ type: "send", text: "a" });
    await controller.handle({ type: "keep" });
    expect(testState.shown.at(-1)).toMatchObject({
      level: "error",
      text: "Couldn't keep this chat: Gone.",
    });
    expect(controller.state.ephemeral).toBe(true);
  });

  it("merges the receipts kept by other windows (C11)", async () => {
    const { fake, deps, memory } = await setup();
    memory.set("chatonlm.model", "qwen3:0.6b");
    fake.handlers["chat.send"] = (params, { id, notify }) => {
      const n = (params as Sent).content === "a" ? 41 : 42;
      notify("chat.event", {
        requestId: id,
        event: {
          type: "userMessage",
          message: { id: n, role: "user", content: "x" },
        },
      });
      return { chatId: "c", outcome: "done" };
    };
    const a = new Controller(deps);
    const b = new Controller(deps);
    cleanups.push(() => (a.dispose(), b.dispose()));
    await a.handle({ type: "send", text: "a" });
    await b.handle({ type: "send", text: "b" });
    expect(Object.keys(memory.get("chatonlm.receipts") as object)).toEqual([
      "41",
      "42",
    ]);
  });
});
