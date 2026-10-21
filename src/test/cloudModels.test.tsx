// Modèles cloud de bout en bout côté frontend : l'app face au simulateur, qui
// rejoue les séquences cloud-*.json enregistrées par les tests du moteur Rust
// contre le faux serveur (kimi-k2.6:cloud, chatonlm-limit:cloud).

import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { AccountStatus } from "@/bindings/AccountStatus";
import * as streams from "@/lib/streams";
import { fakeBackend } from "@/test/backend";
import { renderApp, texts } from "@/test/renderApp";
import {
  installSimulator,
  SEQUENCE_TIME,
  type Sequence,
} from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  streams.resetStreams();
});

const MODEL = "kimi-k2.6:cloud";
const HELLO = "Say hello in one word.";

const signedIn = (plan: string): AccountStatus => ({
  state: "signedIn",
  name: "ada",
  email: "ada@example.com",
  plan,
  avatar: null,
});

function setup({
  model = MODEL,
  account = { state: "signedOut" } as AccountStatus,
} = {}) {
  const simulator = installSimulator({ latencyMs: 20 });
  const backend = fakeBackend(simulator, {
    selectedModel: model,
    recommended: [{ model: MODEL, requiredPlan: "pro" }, "gemma4"],
  });
  backend.settings.cloudModels = true;
  backend.account = account;
  const app = renderApp("/c/new");
  return { simulator, backend, app };
}

/** Tape et envoie, une fois le modèle cloud choisi affiché. */
async function send(text: string) {
  const textarea = await screen.findByLabelText("Message");
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toContain(MODEL),
  );
  await userEvent.type(textarea, `${text}{Enter}`);
}

/** Réponse finie : flux retiré, bouton d'envoi revenu. */
async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    expect(streams.pendingNewStream()).toBeNull();
    screen.getByLabelText("Send message");
  });
}

const errorBlock = () => screen.getByRole("alert");

/**
 * Relance du message 1 de « chat-1 », telle que le moteur l'enregistrerait :
 * message réécrit (même id), puis une réponse.
 */
function retrySequence(): Sequence {
  return {
    description: "Retry of the message (built by the test).",
    request: {
      chatId: "chat-1",
      streamId: "stream-1",
      content: HELLO,
      model: MODEL,
      editMessageId: 1,
    },
    events: [
      {
        type: "userMessage",
        message: {
          id: 1,
          role: "user",
          content: HELLO,
          thinking: "",
          model: null,
          createdAt: SEQUENCE_TIME,
          updatedAt: SEQUENCE_TIME,
          thinkingStartedAt: null,
          thinkingEndedAt: null,
          attachments: [],
          toolCalls: [],
        },
      },
      { type: "replyStarted", messageId: 2, createdAt: SEQUENCE_TIME },
      { type: "content", text: "Hello!" },
      { type: "done", messageId: 2 },
    ],
    error: null,
  };
}

describe("cloud model without an account", () => {
  it("stops with the error and Sign In, which opens the account card", async () => {
    const { simulator, app } = setup();
    simulator.queue("cloud-signed-out");
    await send(HELLO);
    await replyEnded();
    expect(
      within(errorBlock()).getByText("Cloud models require an Ollama account"),
    ).toBeTruthy();
    expect(texts("assistant")).toEqual([]);
    expect(texts("user")).toEqual([HELLO]);
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: "Sign In" }),
    );
    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(app.router.state.location.search).toEqual({ section: "account" });
    // Rien ne s'ouvre tout seul dans le navigateur.
    expect(simulator.callsTo("account_sign_in")).toEqual([]);
  });

  it("signed in meanwhile: Retry sends the message again", async () => {
    const { simulator } = setup({ account: signedIn("free") });
    simulator.queue("cloud-signed-out");
    await send(HELLO);
    await replyEnded();
    simulator.queue(retrySequence());
    await userEvent.click(
      await within(errorBlock()).findByRole("button", { name: "Retry" }),
    );
    await replyEnded();
    await waitFor(() => expect(texts("assistant")).toEqual(["Hello!"]));
    expect(texts("user")).toEqual([HELLO]);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("cloud model whose usage limit is reached", () => {
  it.each([
    ["free", "https://ollama.com/upgrade"],
    ["pro", "https://ollama.com/settings/billing"],
  ])("Upgrade opens ollama.com for a %s account", async (plan, url) => {
    const { simulator } = setup({
      model: "chatonlm-limit:cloud",
      account: signedIn(plan),
    });
    simulator.handle("open_external", () => null);
    simulator.queue("cloud-limit");
    await screen.findByLabelText("Message");
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toContain(
        "chatonlm-limit:cloud",
      ),
    );
    await userEvent.type(screen.getByLabelText("Message"), `${HELLO}{Enter}`);
    await replyEnded();
    expect(
      within(errorBlock()).getByText(
        "You've reached your usage limit, please upgrade to continue",
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: "Upgrade" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([{ url }]),
    );
  });
});

describe("cloud models turned off", () => {
  it("the error leads to Settings › Models", async () => {
    const { simulator, app } = setup();
    simulator.queue("cloud-off");
    await send(HELLO);
    await replyEnded();
    expect(
      within(errorBlock()).getByText(
        "Cloud models are turned off: kimi-k2.6:cloud runs on ollama.com. Turn them on in Settings › Models to use it.",
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: "Open settings" }),
    );
    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(app.router.state.location.search).toEqual({ section: "models" });
  });
});

describe("opening a chat held with a cloud model", () => {
  function withChat(cloudModels: boolean) {
    const simulator = installSimulator({ latencyMs: 20 });
    const backend = fakeBackend(simulator, {
      selectedModel: "qwen3.5:0.8b",
      models: ["qwen3:0.6b", "qwen3.5:0.8b"],
    });
    backend.settings.cloudModels = cloudModels;
    const message = (id: number, role: "user" | "assistant") => ({
      id,
      role,
      content: role === "user" ? HELLO : "Hello!",
      thinking: "",
      model: role === "assistant" ? MODEL : null,
      createdAt: SEQUENCE_TIME,
      updatedAt: SEQUENCE_TIME,
      thinkingStartedAt: null,
      thinkingEndedAt: null,
      attachments: [],
      toolCalls: [],
    });
    backend.chats.set("chat-1", {
      id: "chat-1",
      title: HELLO,
      createdAt: SEQUENCE_TIME,
      updatedAt: SEQUENCE_TIME,
      messages: [message(1, "user"), message(2, "assistant")],
      context: null,
      compactions: [],
    });
    renderApp("/c/chat-1");
    return { simulator, backend };
  }

  it("brings it back once cloud models are turned on", async () => {
    const { backend } = withChat(true);
    await waitFor(() => expect(backend.settings.selectedModel).toBe(MODEL));
  });

  it("keeps the chosen model while they are turned off", async () => {
    const { simulator, backend } = withChat(false);
    await screen.findByText("Hello!");
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(backend.settings.selectedModel).toBe("qwen3.5:0.8b");
    expect(
      simulator
        .callsTo("update_settings")
        .filter((args) => "selectedModel" in (args.patch as object)),
    ).toEqual([]);
  });
});
