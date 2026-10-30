import { describe, expect, it } from "vitest";
import type { Chat } from "@/bindings/Chat";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Message } from "@/bindings/Message";
import type { HostMessage, PanelState, Receipt } from "../shared/protocol";
import {
  initialModel,
  isBusy,
  type PanelAction,
  type PanelModel,
  receiptOf,
  reduce,
  savedMessages,
  visibleTurns,
} from "./model";

const STATE: PanelState = {
  connection: "connected",
  models: ["qwen3:0.6b"],
  model: "qwen3:0.6b",
  ephemeral: false,
  chatId: "c1",
  busy: false,
  cloudModels: [],
  generation: 0,
  notice: null,
};

const RECEIPT: Receipt = {
  items: [],
  chars: 4,
  tokens: 1,
  contextLength: null,
  destination: "local",
  blocked: [],
};

function message(id: number, role: Message["role"], content: string): Message {
  return {
    id,
    role,
    content,
    thinking: "",
    model: "qwen3:0.6b",
    createdAt: 1,
    updatedAt: 1,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

function chat(id: string, messages: Message[]): Chat {
  return {
    id,
    title: "",
    createdAt: 1,
    updatedAt: 1,
    messages,
    context: null,
    compactions: [],
    replyRunning: null,
    ephemeral: false,
  };
}

const host = (message: HostMessage, now = 1000): PanelAction => ({
  type: "host",
  message,
  now,
});
const event = (requestId: number, e: ChatEvent, now = 1000) =>
  host({ type: "event", requestId, event: e }, now);

function run(...actions: PanelAction[]): PanelModel {
  return actions.reduce(
    reduce,
    reduce(
      initialModel(),
      host({ type: "init", locale: "en", theme: "light", state: STATE }),
    ),
  );
}

describe("panel model", () => {
  it("gives a saved message the receipt the host kept for it", () => {
    const model = run(
      host({ type: "init", locale: "en", theme: "light", state: STATE }),
      host({ type: "receipts", receipts: { 7: RECEIPT } }),
      host({ type: "chat", chat: chat("c1", [message(7, "user", "Hi")]) }),
    );
    expect(receiptOf(model, null, 7)).toEqual(RECEIPT);
    expect(receiptOf(model, null, 8)).toBeNull();
  });

  it("applies a streamed reply like the app page", () => {
    const model = run(
      { type: "sent", text: "hi" },
      event(7, { type: "userMessage", message: message(1, "user", "hi") }),
      event(7, { type: "loading" }),
      event(7, { type: "replyStarted", messageId: 2, createdAt: 5 }, 1100),
      event(7, { type: "thinking", text: "hmm" }, 1200),
      event(7, { type: "content", text: "Hel" }, 1300),
      event(7, { type: "content", text: "lo" }, 1400),
    );
    const [turn] = model.turns;
    expect(turn.requestId).toBe(7);
    expect(turn.loading).toBe(false);
    expect(turn.replies[0]).toMatchObject({
      content: "Hello",
      thinking: "hmm",
      thinkingStartedAt: 1200,
      thinkingEndedAt: 1300,
    });
    expect(isBusy(model)).toBe(true);

    const done = reduce(model, event(7, { type: "done", messageId: 2 }));
    expect(done.turns[0].finished).toBe(true);
    expect(isBusy(done)).toBe(false);
  });

  it("closes the thinking of a reply stopped while thinking", () => {
    const model = run(
      event(3, { type: "replyStarted", messageId: 2, createdAt: 5 }),
      event(3, { type: "thinking", text: "hmm" }, 1200),
      event(3, { type: "cancelled" }, 1500),
    );
    expect(model.turns[0].replies[0].thinkingEndedAt).toBe(1500);
    expect(model.turns[0].finished).toBe(true);
  });

  it("binds the first request number to the pending send, others to new turns", () => {
    const model = run(
      { type: "sent", text: "mine" },
      host({ type: "receipt", requestId: 4, receipt: RECEIPT }),
      // Action lancée depuis l'éditeur : un autre envoi.
      event(5, { type: "userMessage", message: message(9, "user", "explain") }),
    );
    expect(model.turns.map((t) => [t.requestId, t.text])).toEqual([
      [4, "mine"],
      [5, null],
    ]);
    expect(receiptOf(model, model.turns[0], null)).toBe(RECEIPT);
    expect(receiptOf(model, model.turns[1], 9)).toBeNull();
  });

  it("keeps the receipt of a sent message once the chat is read again", () => {
    const user = message(1, "user", "hi");
    const reply = message(2, "assistant", "yo");
    const model = run(
      { type: "sent", text: "hi" },
      host({ type: "receipt", requestId: 4, receipt: RECEIPT }),
      event(4, { type: "userMessage", message: user }),
      event(4, { type: "replyStarted", messageId: 2, createdAt: 5 }),
      event(4, { type: "content", text: "yo" }),
      event(4, { type: "done", messageId: 2 }),
      host({ type: "chat", chat: chat("c1", [user, reply]) }),
    );
    expect(model.turns).toEqual([]);
    expect(savedMessages(model, [])).toEqual([user, reply]);
    expect(receiptOf(model, null, 1)).toBe(RECEIPT);
  });

  it("keeps a running reply over the saved one when the chat is read during it", () => {
    const user = message(1, "user", "hi");
    const model = run(
      host({ type: "state", state: { ...STATE, busy: true } }),
      event(4, { type: "userMessage", message: user }),
      event(4, { type: "replyStarted", messageId: 2, createdAt: 5 }),
      event(4, { type: "content", text: "partial and more" }),
      host({
        type: "chat",
        chat: chat("c1", [user, message(2, "assistant", "partial")]),
      }),
    );
    const turns = visibleTurns(model);
    expect(turns).toHaveLength(1);
    expect(savedMessages(model, turns)).toEqual([]);
  });

  it("keeps the error of the last send until the next one", () => {
    const user = message(1, "user", "hi");
    const failed = run(
      event(4, { type: "userMessage", message: user }),
      event(4, { type: "error", kind: "model", message: "boom" }),
      host({ type: "chat", chat: chat("c1", [user]) }),
    );
    expect(failed.turns).toEqual([]);
    expect(failed.lastError?.error.message).toBe("boom");

    const next = reduce(failed, { type: "sent", text: "again" });
    expect(next.lastError).toBeNull();
  });

  it("clears the error of a send not read again at the next one", () => {
    const failed = run(
      event(4, { type: "userMessage", message: message(1, "user", "hi") }),
      event(4, { type: "error", kind: "model", message: "boom" }),
    );
    expect(failed.turns[0].error?.message).toBe("boom");
    const next = reduce(failed, { type: "sent", text: "again" });
    expect(next.turns.map((t) => t.error)).toEqual([null, null]);
    const fromEditor = reduce(
      failed,
      event(5, { type: "userMessage", message: message(3, "user", "x") }),
    );
    expect(fromEditor.turns.map((t) => t.error)).toEqual([null, null]);
  });

  it("forgets a refused send (nothing saved) at the next one", () => {
    const refused = run(
      event(4, { type: "error", kind: "unreachable", message: "down" }),
    );
    expect(refused.turns[0].error?.message).toBe("down");
    const next = reduce(refused, { type: "sent", text: "again" });
    expect(next.turns.map((t) => t.text)).toEqual(["again"]);
  });

  it("drops the turns of the previous chat on a new chat", () => {
    const user = message(1, "user", "hi");
    const model = run(
      event(4, { type: "userMessage", message: user }),
      event(4, { type: "done", messageId: null }),
      host({ type: "chat", chat: chat("c1", []) }),
      host({ type: "state", state: { ...STATE, chatId: null } }),
    );
    expect(model.turns).toEqual([]);
    expect(model.chat).toBeNull();
  });

  it("shows a new chat being created, before and after its id is known", () => {
    const model = run(
      host({ type: "state", state: { ...STATE, chatId: null } }),
      { type: "sent", text: "hi" },
      event(4, { type: "chatCreated", chatId: "c2" }),
    );
    expect(visibleTurns(model)).toHaveLength(1);
    const later = reduce(
      model,
      host({ type: "state", state: { ...STATE, chatId: "c2", busy: true } }),
    );
    expect(visibleTurns(later)).toHaveLength(1);
  });

  it("gives up a send the host never took when the link is lost", () => {
    const model = run(
      { type: "sent", text: "hi" },
      host({ type: "state", state: { ...STATE, connection: "appClosed" } }),
    );
    expect(model.turns).toEqual([]);
  });

  it("stops a send the host has not taken yet", () => {
    const model = run(
      { type: "sent", text: "hi" },
      { type: "stopped", now: 1000 },
    );
    expect(isBusy(model)).toBe(false);
  });

  it("frees a send whose end was lost: Stop, once the host is idle", () => {
    // Le reçu est arrivé (envoi numéroté), puis plus rien : refus perdu.
    const busy = run(
      { type: "sent", text: "hi" },
      host({ type: "receipt", requestId: 7, receipt: RECEIPT }),
      host({ type: "state", state: { ...STATE, busy: true } }),
      { type: "stopped", now: 1000 },
    );
    // L'hôte écrit encore : c'est à lui d'arrêter.
    expect(isBusy(busy)).toBe(true);
    const idle = reduce(reduce(busy, host({ type: "state", state: STATE })), {
      type: "stopped",
      now: 1000,
    });
    expect(isBusy(idle)).toBe(false);
  });

  it("shows the whole saved reply of a send cut by a lost link", () => {
    const user = message(1, "user", "hello");
    const cut = run(
      { type: "sent", text: "hello" },
      host({ type: "receipt", requestId: 5, receipt: RECEIPT }),
      host({ type: "state", state: { ...STATE, busy: true } }),
      event(5, { type: "userMessage", message: user }),
      event(5, { type: "replyStarted", messageId: 2, createdAt: 1 }),
      event(5, { type: "content", text: "par" }),
      // Coupure, reconnexion : l'hôte n'écrit plus, la conversation relue
      // a la réponse complète.
      host({ type: "state", state: STATE }),
      host({
        type: "chat",
        chat: chat("c1", [user, message(2, "assistant", "partial reply done")]),
      }),
    );
    expect(isBusy(cut)).toBe(false);
    const turns = visibleTurns(cut);
    expect(turns).toEqual([]);
    expect(savedMessages(cut, turns).map((m) => m.content)).toEqual([
      "hello",
      "partial reply done",
    ]);
    // Pas pendant que l'hôte écrit encore cette réponse.
    const running = run(
      { type: "sent", text: "hello" },
      host({ type: "receipt", requestId: 5, receipt: RECEIPT }),
      host({ type: "state", state: { ...STATE, busy: true } }),
      event(5, { type: "userMessage", message: user }),
      event(5, { type: "replyStarted", messageId: 2, createdAt: 1 }),
      host({
        type: "chat",
        chat: chat("c1", [user, message(2, "assistant", "")]),
      }),
    );
    expect(visibleTurns(running)).toHaveLength(1);
  });

  it("forgets everything shown on a new chat, the running send included", () => {
    const model = run(
      host({ type: "state", state: { ...STATE, chatId: null } }),
      { type: "sent", text: "first" },
      host({ type: "receipt", requestId: 4, receipt: RECEIPT }),
      event(4, { type: "content", text: "partial" }),
      host({
        type: "state",
        state: { ...STATE, chatId: null, ephemeral: true, generation: 1 },
      }),
    );
    expect(model.turns).toEqual([]);
    expect(model.lastError).toBeNull();
  });

  it("shows the download of a missing model, then forgets it", () => {
    const model = run(
      event(4, { type: "download", total: 200, completed: 50, done: false }),
    );
    expect(model.turns[0].download).toBe(25);
    const done = reduce(
      model,
      event(4, { type: "download", total: 200, completed: 200, done: true }),
    );
    expect(done.turns[0].download).toBeNull();
  });
});
