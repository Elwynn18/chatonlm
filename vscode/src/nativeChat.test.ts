// @vitest-environment node
// Modèles de ChatonLM dans le chat natif de VS Code (lot 4b, 3) : coupés par
// défaut et jamais activés par un dossier ; seulement les modèles de cette
// machine ; demande dans une conversation éphémère effacée après ; flux
// relayé ; arrêt ; rien au panneau.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { deferred, until } from "./test/fakeBridge";
import { cleanUp, ofType, setupController } from "./test/setup";
import {
  CancellationTokenSource,
  LanguageModelChatMessageRole,
  LanguageModelTextPart,
  resetTestState,
  testState,
} from "./test/vscode";
import { family, flatten, VENDOR } from "./nativeChat";
import type * as vscode from "vscode";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const message = (role: LanguageModelChatMessageRole, text: string) =>
  ({
    role,
    content: [new LanguageModelTextPart(text)],
    name: undefined,
  }) as unknown as vscode.LanguageModelChatRequestMessage;

const token = (source: CancellationTokenSource) =>
  source.token as unknown as vscode.CancellationToken;

const OPTIONS = {
  toolMode: 1,
} as unknown as vscode.ProvideLanguageModelChatResponseOptions;

async function provider() {
  const env = await setupController();
  const chat = env.controller.nativeChat;
  return { ...env, chat };
}

describe("native chat models", () => {
  it("registers under ChatonLM's vendor and offers nothing by default", async () => {
    const { chat } = await provider();
    chat.register();
    expect(testState.chatProviders.map((p) => p.vendor)).toEqual([VENDOR]);
    expect(await chat.provideLanguageModelChatInformation()).toEqual([]);
    testState.workspaceConfig["chatonlm.nativeChat.enabled"] = true;
    expect(await chat.provideLanguageModelChatInformation()).toEqual([]);
  });

  it("offers only the models that run on this computer", async () => {
    const { chat, fake } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    // `ChatModel` de l'app 0.10, avec les champs de l'ancien `ModelInfo`.
    fake.handlers["models.list"] = () => [
      {
        id: "Qwen/Qwen3-0.6B-GGUF:Q8_0",
        displayName: "Qwen3 0.6B",
        origin: "hf",
        contextMax: 40960,
        model: "Qwen/Qwen3-0.6B-GGUF:Q8_0",
        digest: "Qwen/Qwen3-0.6B-GGUF:Q8_0",
      },
      {
        id: "@ollama/llama3.2:latest",
        displayName: "llama3.2",
        origin: "ollama",
        contextMax: null,
      },
      // Distant : écarté sans même demander ses détails.
      {
        id: "@remote/openrouter/qwen/qwen3-coder",
        displayName: "Qwen3 Coder",
        origin: "remote",
        contextMax: 262144,
      },
      // L'app dit qu'il tourne ailleurs.
      { id: "elsewhere/x-GGUF:Q4_K_M", displayName: "x", origin: "hf" },
    ];
    const base = fake.handlers["models.details"];
    fake.handlers["models.details"] = (params, context) =>
      params.model === "elsewhere/x-GGUF:Q4_K_M"
        ? { model: params.model, remote: true, contextLength: null }
        : base(params, context);
    const infos = await chat.provideLanguageModelChatInformation();
    expect(infos.map((info) => info.id)).toEqual([
      "Qwen/Qwen3-0.6B-GGUF:Q8_0",
      "@ollama/llama3.2:latest",
    ]);
    expect(infos[0]).toMatchObject({
      name: "Qwen3 0.6B",
      family: "Qwen/Qwen3-0.6B-GGUF",
      version: "Qwen/Qwen3-0.6B-GGUF:Q8_0",
      capabilities: { imageInput: false, toolCalling: false },
    });
    // Repris d'Ollama : l'identifiant garde son étiquette.
    expect(infos[1]).toMatchObject({
      name: "llama3.2",
      family: "@ollama/llama3.2",
      version: "@ollama/llama3.2:latest",
    });
    expect(infos[0].maxInputTokens).toBeGreaterThan(0);
    expect(
      fake.calls("models.details").map((params) => params.model),
    ).not.toContain("@remote/openrouter/qwen/qwen3-coder");
  });

  it("names the family after the id without its quantization or tag", () => {
    expect(family("Qwen/Qwen3-0.6B-GGUF:Q8_0")).toBe("Qwen/Qwen3-0.6B-GGUF");
    expect(family("@ollama/llama3.2:latest")).toBe("@ollama/llama3.2");
    expect(family("qwen3:0.6b")).toBe("qwen3");
    expect(family("qwen3")).toBe("qwen3");
    // Un « : » avant le dernier « / » n'est pas une étiquette.
    expect(family("a:b/c")).toBe("a:b/c");
  });

  it("still reads the model list of an app 0.9.x", async () => {
    const { chat, fake } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    fake.handlers["models.list"] = () => [
      { model: "qwen3:0.6b", contextLength: 40960, digest: "abcdef0123456789" },
      { model: "gpt-oss:120b-cloud", contextLength: 131072 },
    ];
    const infos = await chat.provideLanguageModelChatInformation();
    expect(infos.map((info) => info.id)).toEqual(["qwen3:0.6b"]);
    expect(infos[0]).toMatchObject({
      name: "qwen3:0.6b",
      family: "qwen3",
      version: "qwen3:0.6b",
    });
  });

  it("answers in an ephemeral chat, streams the reply, then forgets the chat", async () => {
    const { chat, fake, posted } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    fake.handlers["chat.send"] = (_params, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId: "chat-7" },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "Hel" },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "lo" },
      });
      return { chatId: "chat-7", outcome: "done" };
    };
    const parts: string[] = [];
    const before = ofType(posted, "event").length;
    await chat.provideLanguageModelChatResponse(
      {
        id: "qwen3:0.6b",
        name: "qwen3:0.6b",
      } as vscode.LanguageModelChatInformation,
      [message(LanguageModelChatMessageRole.User, "Say hello")],
      OPTIONS,
      {
        report: (part) => parts.push((part as LanguageModelTextPart).value),
      },
      token(new CancellationTokenSource()),
    );
    expect(parts.join("")).toBe("Hello");
    expect(fake.calls("chat.send")).toEqual([
      { content: "Say hello", model: "qwen3:0.6b", ephemeral: true },
    ]);
    await until(() => fake.forgotten.includes("chat-7"));
    // Rien de cette demande dans le panneau de l'extension.
    expect(ofType(posted, "event")).toHaveLength(before);
  });

  it("refuses a remote model even if asked directly", async () => {
    const { chat, fake } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    await expect(
      chat.provideLanguageModelChatResponse(
        {
          id: "@remote/openrouter/qwen/qwen3-coder",
          name: "Qwen3 Coder",
        } as vscode.LanguageModelChatInformation,
        [message(LanguageModelChatMessageRole.User, "hi")],
        OPTIONS,
        { report: () => {} },
        token(new CancellationTokenSource()),
      ),
    ).rejects.toThrow(/does not run on this computer/);
    expect(fake.calls("chat.send")).toEqual([]);
  });

  it("offers and answers nothing when the models are on another computer (app 0.9.x)", async () => {
    const { chat, fake } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    fake.local = false;
    fake.handlers["chat.send"] = () => ({ chatId: null, outcome: "done" });
    expect(await chat.provideLanguageModelChatInformation()).toEqual([]);
    await expect(
      chat.provideLanguageModelChatResponse(
        {
          id: "qwen3:0.6b",
          name: "qwen3:0.6b",
        } as vscode.LanguageModelChatInformation,
        [message(LanguageModelChatMessageRole.User, "hi")],
        OPTIONS,
        { report: () => {} },
        token(new CancellationTokenSource()),
      ),
    ).rejects.toThrow(/models run on another computer/);
    expect(fake.calls("chat.send")).toEqual([]);
  });

  it("says a conversation is too long rather than the link was lost", async () => {
    const { chat, link } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    await expect(
      chat.provideLanguageModelChatResponse(
        {
          id: "qwen3:0.6b",
          name: "qwen3:0.6b",
        } as vscode.LanguageModelChatInformation,
        // Plus que la trame du pont (8 Mio).
        [
          message(
            LanguageModelChatMessageRole.User,
            "x".repeat(9 * 1024 * 1024),
          ),
        ],
        OPTIONS,
        { report: () => {} },
        token(new CancellationTokenSource()),
      ),
    ).rejects.toThrow(/too long to send to ChatonLM/);
    expect(link.connected).toBe(true);
  });

  it("stops the reply when VS Code cancels the request", async () => {
    const { chat, fake } = await provider();
    testState.config["chatonlm.nativeChat.enabled"] = true;
    const started = deferred<number>();
    const finish = deferred();
    fake.handlers["chat.send"] = async (_params, { id }) => {
      started.resolve(id);
      await finish.promise;
      return { chatId: null, outcome: "cancelled" };
    };
    const source = new CancellationTokenSource();
    const reply = chat.provideLanguageModelChatResponse(
      {
        id: "qwen3:0.6b",
        name: "qwen3:0.6b",
      } as vscode.LanguageModelChatInformation,
      [message(LanguageModelChatMessageRole.User, "long story")],
      OPTIONS,
      { report: () => {} },
      token(source),
    );
    const id = await started.promise;
    source.cancel();
    await until(() => fake.cancelled.includes(id));
    finish.resolve();
    await reply;
  });

  it("puts the earlier messages before the last one", () => {
    expect(flatten([message(LanguageModelChatMessageRole.User, "only")])).toBe(
      "only",
    );
    const text = flatten([
      message(LanguageModelChatMessageRole.User, "first"),
      message(LanguageModelChatMessageRole.Assistant, "answer"),
      message(LanguageModelChatMessageRole.User, "second"),
    ]);
    expect(text).toContain("User: first");
    expect(text).toContain("Assistant: answer");
    expect(text.endsWith("second")).toBe(true);
  });
});
