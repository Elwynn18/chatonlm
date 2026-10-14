// Modèle qui tourne en boucle (propre à ChatonLM) : l'app face au simulateur.
// Répétition mot pour mot : le moteur arrête la réponse (séquence
// loop-stopped, enregistrée contre le faux serveur, modèle chatonlm-repeat) et
// la page propose Retry. Ressassement : bandeau avec Stop au-dessus de la zone
// de saisie, rien d'arrêté sans clic.

import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Message } from "@/bindings/Message";
import * as streams from "@/lib/streams";
import { fakeBackend } from "@/test/backend";
import { renderApp, texts } from "@/test/renderApp";
import {
  installSimulator,
  SEQUENCE_TIME,
  type Sequence,
  sequence,
} from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  streams.resetStreams();
});

const HELLO = "Say hello in one word.";
const LOOP_ERROR =
  "The model kept repeating the same text, so its reply was stopped.";
const RUMINATING =
  "The model has been thinking for a long time and seems to be going in circles.";

function setup(model: string) {
  const simulator = installSimulator({ latencyMs: 20 });
  const backend = fakeBackend(simulator, { selectedModel: model });
  const app = renderApp("/c/new");
  return { simulator, backend, app };
}

async function send(text: string, model: string) {
  const textarea = await screen.findByLabelText("Message");
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe(model),
  );
  await userEvent.type(textarea, `${text}{Enter}`);
}

async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    expect(streams.pendingNewStream()).toBeNull();
    screen.getByLabelText("Send message");
  });
}

const userMessage = (): Message => ({
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
});

/** Relance du message après la boucle (construite par l'essai). */
function retrySequence(model: string): Sequence {
  return {
    description: "Retry of the message after a loop (built by the test).",
    request: {
      chatId: "chat-1",
      streamId: "stream-1",
      content: HELLO,
      model,
      editMessageId: 1,
    },
    events: [
      { type: "userMessage", message: userMessage() },
      { type: "replyStarted", messageId: 3, createdAt: SEQUENCE_TIME },
      { type: "content", text: "Hello!" },
      { type: "done", messageId: 3 },
    ],
    error: null,
  };
}

/** Paragraphes qui ressassent, comme Qwen le 14 oct. 2026. */
function ruminatingParagraphs(count: number): string[] {
  return Array.from(
    { length: count },
    (_, i) =>
      `Need maybe if test${i % 5} after step ${i}: check that the permissions are right once more.\n\n`,
  );
}

/** Raisonnement qui ressasse, puis arrêt par Stop (construite par l'essai). */
function ruminatingSequence(model: string): Sequence {
  // 30 fragments de 10 paragraphes (environ 900 caractères chacun).
  const paragraphs = ruminatingParagraphs(300);
  const thinking: ChatEvent[] = Array.from({ length: 30 }, (_, i) => ({
    type: "thinking",
    text: paragraphs.slice(i * 10, i * 10 + 10).join(""),
  }));
  return {
    description:
      "A thinking that goes in circles, stopped (built by the test).",
    request: { streamId: "stream-1", content: HELLO, model },
    events: [
      { type: "chatCreated", chatId: "chat-1" },
      { type: "userMessage", message: userMessage() },
      { type: "replyStarted", messageId: 2, createdAt: SEQUENCE_TIME },
      ...thinking,
      { type: "cancelled" },
    ],
    error: null,
  };
}

describe("a model stuck in a loop", () => {
  it("a reply stopped for repeating itself offers Retry", async () => {
    const { model } = sequence("loop-stopped").request;
    const { simulator } = setup(model);
    simulator.queue("loop-stopped");
    await send(HELLO, model);
    await replyEnded();
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(LOOP_ERROR)).toBeTruthy();

    simulator.queue(retrySequence(model));
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await replyEnded();
    await waitFor(() => expect(texts("assistant")).toEqual(["Hello!"]));
    expect(texts("user")).toEqual([HELLO]);
    expect(screen.queryByRole("alert")).toBeNull();
    const [request] = simulator.callsTo("send_message").slice(-1);
    expect(request.request).toMatchObject({ editMessageId: 1 });
  });

  it("a thinking going in circles shows a banner whose Stop ends the reply", async () => {
    const model = "chatonlm-ruminate";
    const { simulator } = setup(model);
    const replay = simulator.queue(ruminatingSequence(model), {
      stepwise: true,
    });
    await send(HELLO, model);
    await replay.started;
    // Début du raisonnement (sous 20 000 caractères) : pas de bandeau.
    await replay.step(3 + 15);
    await waitFor(() =>
      expect(streams.streamsFor("chat-1")[0]?.replies).toHaveLength(1),
    );
    expect(screen.queryByText(RUMINATING)).toBeNull();
    // Assez de texte pour juger : le bandeau paraît, la réponse continue.
    await replay.step(15);
    const banner = (await screen.findByText(RUMINATING)).closest<HTMLElement>(
      "[role=status]",
    );
    if (!banner) throw new Error("banner without role=status");
    expect(simulator.callsTo("cancel_message")).toEqual([]);

    await userEvent.click(within(banner).getByRole("button", { name: "Stop" }));
    expect(simulator.callsTo("cancel_message")).toEqual([
      { streamId: replay.streamId },
    ]);
    await replay.finish();
    await replyEnded();
    expect(screen.queryByText(RUMINATING)).toBeNull();
  });
});
