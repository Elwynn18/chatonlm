import { afterEach, describe, expect, it, vi } from "vitest";
import type { ImageProgress } from "@/bindings/ImageProgress";
import type { Message } from "@/bindings/Message";
import * as streams from "./streams";

afterEach(() => {
  streams.resetStreams();
  vi.useRealTimers();
});

function message(
  id: number,
  role: "user" | "assistant",
  content: string,
  createdAt = 1,
) {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? "qwen3:0.6b" : null,
    createdAt,
    updatedAt: createdAt,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  } satisfies Message;
}

/** Seul flux d'une conversation. */
function only(chatId: string) {
  const list = streams.streamsFor(chatId);
  expect(list.length).toBeLessThanOrEqual(1);
  return list[0] ?? null;
}

describe("live streams", () => {
  it("shows the attached files with the message before it is saved", () => {
    streams.start({
      streamId: "s",
      chatId: "c",
      content: "Hi",
      model: "m",
      attachments: [
        {
          id: "u1",
          filename: "a.txt",
          kind: "text",
          mime: "text/plain",
          size: 3,
        },
        {
          id: "u2",
          filename: "b.png",
          kind: "image",
          mime: "image/png",
          size: 4,
        },
      ],
    });
    // Identifiants négatifs : pas encore en base (aucune image à demander).
    expect(only("c")?.pending.attachments).toEqual([
      { id: -1, filename: "a.txt", kind: "text", mime: "text/plain", size: 3 },
      { id: -2, filename: "b.png", kind: "image", mime: "image/png", size: 4 },
    ]);
  });

  it("follows a new chat from creation to the end of the reply", () => {
    streams.start({
      streamId: "s",
      chatId: null,
      content: "Hi",
      model: "qwen3:0.6b",
    });
    expect(streams.pendingNewStream()?.streamId).toBe("s");

    streams.apply("s", { type: "chatCreated", chatId: "c" });
    expect(streams.pendingNewStream()).toBeNull();
    streams.apply("s", {
      type: "userMessage",
      message: message(1, "user", "Hi"),
    });
    streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 5 });
    streams.apply("s", { type: "thinking", text: "Hmm" });
    streams.apply("s", { type: "content", text: "Hel" });
    streams.apply("s", { type: "content", text: "lo" });

    const live = only("c");
    expect(live?.replies.at(-1)).toMatchObject({
      id: 2,
      content: "Hello",
      thinking: "Hmm",
      model: "qwen3:0.6b",
    });
    expect(live?.replies.at(-1)?.thinkingStartedAt).not.toBeNull();
    expect(live?.replies.at(-1)?.thinkingEndedAt).not.toBeNull();
    expect(live?.finished).toBe(false);
    expect(streams.activeStream(streams.streamsFor("c"))?.streamId).toBe("s");

    streams.apply("s", { type: "done", messageId: 2 });
    expect(only("c")?.finished).toBe(true);
    expect(streams.activeStream(streams.streamsFor("c"))).toBeNull();
    streams.remove("s");
    expect(streams.streamsFor("c")).toEqual([]);
  });

  it("closes a thinking left open when the reply ends (stop, error)", () => {
    vi.useFakeTimers({ now: 1000 });
    for (const end of ["cancelled", "error", "done"] as const) {
      streams.start({ streamId: end, chatId: end, content: "x", model: "m" });
      streams.apply(end, { type: "replyStarted", messageId: 2, createdAt: 1 });
      streams.apply(end, { type: "thinking", text: "Hmm" });
      vi.advanceTimersByTime(2500);
      if (end === "error") {
        streams.apply(end, { type: "error", kind: "model", message: "boom" });
      } else if (end === "done") {
        streams.apply(end, { type: "done", messageId: 2 });
      } else {
        streams.apply(end, { type: "cancelled" });
      }
      const reply = only(end)?.replies.at(-1);
      expect(only(end)?.finished).toBe(true);
      expect(reply?.thinkingEndedAt).not.toBeNull();
      expect(reply!.thinkingEndedAt! - reply!.thinkingStartedAt!).toBe(2500);
    }

    // Raisonnement déjà clos par le contenu, ou absent : réponse inchangée.
    streams.start({ streamId: "s", chatId: "s", content: "x", model: "m" });
    streams.apply("s", { type: "replyStarted", messageId: 3, createdAt: 1 });
    streams.apply("s", { type: "thinking", text: "Hmm" });
    vi.advanceTimersByTime(500);
    streams.apply("s", { type: "content", text: "Hi" });
    const closed = only("s")?.replies.at(-1);
    vi.advanceTimersByTime(500);
    streams.apply("s", { type: "done", messageId: 3 });
    expect(only("s")?.replies.at(-1)).toBe(closed);
    expect(closed!.thinkingEndedAt! - closed!.thinkingStartedAt!).toBe(500);

    streams.start({ streamId: "t", chatId: "t", content: "x", model: "m" });
    streams.apply("t", { type: "replyStarted", messageId: 4, createdAt: 1 });
    streams.apply("t", { type: "content", text: "Hi" });
    const plain = only("t")?.replies.at(-1);
    streams.apply("t", { type: "cancelled" });
    expect(only("t")?.replies.at(-1)).toBe(plain);
    expect(plain?.thinkingEndedAt).toBeNull();
  });

  it("follows the download of a missing model, before the reply", () => {
    streams.start({ streamId: "s", chatId: "c", content: "x", model: "m" });
    expect(only("c")?.download).toBeNull();
    streams.apply("s", {
      type: "userMessage",
      message: message(1, "user", "x"),
    });
    streams.apply("s", {
      type: "download",
      total: 5000,
      completed: 1000,
      done: false,
    });
    expect(only("c")?.download).toEqual({
      total: 5000,
      completed: 1000,
      done: false,
    });
    expect(only("c")?.finished).toBe(false);
    expect(only("c")?.replies).toEqual([]);
    streams.apply("s", {
      type: "download",
      total: 5000,
      completed: 5000,
      done: true,
    });
    expect(only("c")?.download).toEqual({
      total: 5000,
      completed: 5000,
      done: true,
    });

    // La réponse suit ; le téléchargement fini reste noté jusqu'à la fin.
    streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 1 });
    streams.apply("s", { type: "content", text: "Hi" });
    expect(only("c")?.replies.at(-1)?.content).toBe("Hi");
    expect(only("c")?.download?.done).toBe(true);
    streams.apply("s", { type: "done", messageId: 2 });
    expect(only("c")?.download).toBeNull();
  });

  it("forgets the download when the stream ends (stop, error)", () => {
    for (const end of ["cancelled", "error"] as const) {
      streams.start({ streamId: end, chatId: end, content: "x", model: "m" });
      streams.apply(end, {
        type: "download",
        total: 5000,
        completed: 10,
        done: false,
      });
      streams.apply(
        end,
        end === "error"
          ? { type: "error", kind: "download", message: "offline" }
          : { type: "cancelled" },
      );
      expect(only(end)?.finished).toBe(true);
      expect(only(end)?.download).toBeNull();
    }
    expect(streams.errorFor("error")).toEqual({
      kind: "download",
      message: "offline",
    });
  });

  it("follows the loading of the model, before the reply", () => {
    streams.start({ streamId: "s", chatId: "c", content: "x", model: "m" });
    expect(only("c")?.loading).toBe(false);
    streams.apply("s", {
      type: "userMessage",
      message: message(1, "user", "x"),
    });
    streams.apply("s", { type: "loading" });
    expect(only("c")?.loading).toBe(true);
    // Rien d'autre ne change : pas de réponse, pas de fin.
    expect(only("c")?.replies).toEqual([]);
    expect(only("c")?.finished).toBe(false);
    streams.apply("s", { type: "loaded" });
    expect(only("c")?.loading).toBe(false);
    streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 1 });
    expect(only("c")?.loading).toBe(false);
  });

  it("ends the loading at any following event, even without loaded", () => {
    const ends = [
      { type: "replyStarted", messageId: 2, createdAt: 1 },
      { type: "content", text: "Hi" },
      { type: "error", kind: "model", message: "out of memory" },
      { type: "cancelled" },
      { type: "done", messageId: null },
    ] as const;
    for (const [index, event] of ends.entries()) {
      const id = `s${index}`;
      streams.start({ streamId: id, chatId: id, content: "x", model: "m" });
      streams.apply(id, { type: "loading" });
      expect(only(id)?.loading).toBe(true);
      streams.apply(id, event);
      expect(only(id)?.loading).toBe(false);
    }
    // Refusé par la commande (aucun événement final) : fini, plus de chargement.
    streams.start({ streamId: "f", chatId: "f", content: "x", model: "m" });
    streams.apply("f", { type: "loading" });
    streams.fail("f", { kind: "internal", message: "refused" });
    expect(only("f")?.loading).toBe(false);
  });

  it("keeps an error until the next message of that chat", () => {
    streams.start({ streamId: "a", chatId: "c", content: "x", model: "m" });
    streams.apply("a", {
      type: "error",
      kind: "model",
      message: "model runner has unexpectedly stopped",
    });
    streams.remove("a");
    expect(streams.errorFor("c")?.message).toBe(
      "model runner has unexpectedly stopped",
    );
    streams.start({ streamId: "b", chatId: "c", content: "y", model: "m" });
    expect(streams.errorFor("c")).toBeNull();
  });

  it("keeps a new chat refused before its creation on the new chat page", () => {
    streams.start({ streamId: "a", chatId: null, content: "x", model: "m" });
    streams.fail("a", { kind: "internal", message: "database is locked" });
    streams.remove("a");
    expect(streams.errorFor(streams.NEW_CHAT)?.message).toBe(
      "database is locked",
    );

    // Le prochain envoi d'une nouvelle conversation l'efface...
    streams.start({ streamId: "b", chatId: null, content: "y", model: "m" });
    expect(streams.errorFor(streams.NEW_CHAT)).toBeNull();
    streams.fail("b", { kind: "internal", message: "database is locked" });
    // ...tout comme un envoi dans une autre conversation (erreur périmée).
    streams.start({ streamId: "c", chatId: "c", content: "z", model: "m" });
    expect(streams.errorFor(streams.NEW_CHAT)).toBeNull();
  });

  it("clears the error of a page that is left", () => {
    streams.start({ streamId: "a", chatId: null, content: "x", model: "m" });
    streams.fail("a", { kind: "internal", message: "database is locked" });
    const listener = vi.fn();
    const unsubscribe = streams.subscribe(listener);
    streams.clearError(streams.NEW_CHAT);
    expect(streams.errorFor(streams.NEW_CHAT)).toBeNull();
    expect(listener).toHaveBeenCalled();
    unsubscribe();
  });

  it("an error after the creation goes to the created chat", () => {
    streams.start({ streamId: "a", chatId: null, content: "x", model: "m" });
    streams.apply("a", { type: "chatCreated", chatId: "c" });
    streams.apply("a", { type: "error", kind: "unreachable", message: "down" });
    expect(streams.errorFor("c")?.message).toBe("down");
    expect(streams.errorFor(streams.NEW_CHAT)).toBeNull();
  });

  it("notifies at once, then at most every 16 ms", () => {
    vi.useFakeTimers();
    const listener = vi.fn();
    const unsubscribe = streams.subscribe(listener);
    streams.start({ streamId: "s", chatId: "c", content: "x", model: "m" });
    expect(listener).toHaveBeenCalledTimes(1);
    streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 1 });
    for (let i = 0; i < 50; i++) {
      streams.apply("s", { type: "content", text: "a" });
    }
    expect(listener).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(16);
    expect(listener).toHaveBeenCalledTimes(2);
    expect(only("c")?.replies.at(-1)?.content).toBe("a".repeat(50));
    unsubscribe();
  });

  it("lists every stream of a chat, finished ones included, in order", () => {
    streams.start({ streamId: "a", chatId: "c", content: "A", model: "m" });
    streams.apply("a", { type: "done", messageId: null });
    streams.start({ streamId: "b", chatId: "c", content: "B", model: "m" });
    streams.start({ streamId: "x", chatId: "other", content: "X", model: "m" });

    const list = streams.streamsFor("c");
    expect(list.map((s) => s.streamId)).toEqual(["a", "b"]);
    expect(streams.activeStream(list)?.streamId).toBe("b");
  });

  it("returns the same snapshot until the chat's streams change", () => {
    streams.start({ streamId: "a", chatId: "c", content: "A", model: "m" });
    const first = streams.streamsFor("c");
    expect(streams.streamsFor("c")).toBe(first);

    // Une autre conversation change : même instantané.
    streams.start({ streamId: "x", chatId: "other", content: "X", model: "m" });
    expect(streams.streamsFor("c")).toBe(first);

    streams.apply("a", { type: "replyStarted", messageId: 2, createdAt: 1 });
    const second = streams.streamsFor("c");
    expect(second).not.toBe(first);
    expect(streams.streamsFor("c")).toBe(second);

    expect(streams.streamsFor("none")).toBe(streams.streamsFor("none"));
    streams.remove("a");
    expect(streams.streamsFor("c")).toEqual([]);
    expect(streams.streamsFor("c")).toBe(streams.streamsFor("c"));
  });
});

describe("tool calls (web search)", () => {
  const search = (query: string) => ({
    name: "web_search",
    arguments: JSON.stringify({ query }),
    result: null,
    error: null,
  });

  /** Envoi « c » : message 1, première réponse 2 qui a raisonné. */
  function startTurn() {
    streams.start({ streamId: "s", chatId: "c", content: "Q", model: "m" });
    streams.apply("s", {
      type: "userMessage",
      message: message(1, "user", "Q"),
    });
    streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 1 });
    streams.apply("s", { type: "thinking", text: "Let me search" });
  }

  it("one reply per turn: calls, results, then the next reply", () => {
    vi.useFakeTimers({ now: 1000 });
    startTurn();
    vi.advanceTimersByTime(300);
    streams.apply("s", {
      type: "toolCalls",
      messageId: 2,
      toolCalls: [search("a"), search("b")],
    });
    // Fin du tour : raisonnement clos, appels sans résultat.
    const turn = only("c")!.replies[0];
    expect(turn.toolCalls.map((call) => call.arguments)).toEqual([
      '{"query":"a"}',
      '{"query":"b"}',
    ]);
    expect(turn.thinkingEndedAt! - turn.thinkingStartedAt!).toBe(300);

    streams.apply("s", {
      type: "toolResult",
      messageId: 2,
      index: 1,
      result: null,
      error: "unauthorized",
    });
    streams.apply("s", {
      type: "toolResult",
      messageId: 2,
      index: 0,
      result: '{"results":[]}',
      error: null,
    });
    expect(only("c")!.replies[0].toolCalls).toEqual([
      { ...search("a"), result: '{"results":[]}' },
      { ...search("b"), error: "unauthorized" },
    ]);

    // Tour suivant : nouvelle réponse, qui seule reçoit le texte.
    streams.apply("s", { type: "replyStarted", messageId: 3, createdAt: 2 });
    streams.apply("s", { type: "content", text: "Found" });
    streams.apply("s", { type: "done", messageId: 3 });
    const live = only("c")!;
    expect(live.replies.map((r) => [r.id, r.content])).toEqual([
      [2, ""],
      [3, "Found"],
    ]);
    expect(live.replies[0].thinking).toBe("Let me search");
    expect(live.replies[1].thinkingStartedAt).toBeNull();
    expect(live.finished).toBe(true);
  });

  it("ignores calls and results for an unknown reply", () => {
    startTurn();
    const before = only("c");
    streams.apply("s", {
      type: "toolCalls",
      messageId: 9,
      toolCalls: [search("a")],
    });
    streams.apply("s", {
      type: "toolResult",
      messageId: 9,
      index: 0,
      result: "{}",
      error: null,
    });
    expect(only("c")!.replies).toEqual(before!.replies);
  });

  it("a stop during a tool keeps the call without result", () => {
    startTurn();
    streams.apply("s", {
      type: "toolCalls",
      messageId: 2,
      toolCalls: [search("slow")],
    });
    streams.apply("s", { type: "cancelled" });
    const live = only("c")!;
    expect(live.finished).toBe(true);
    expect(live.replies[0].toolCalls).toEqual([search("slow")]);
  });

  it("places every reply of a send, the live ones over the saved ones", () => {
    startTurn();
    streams.apply("s", {
      type: "toolCalls",
      messageId: 2,
      toolCalls: [search("a")],
    });
    streams.apply("s", {
      type: "toolResult",
      messageId: 2,
      index: 0,
      result: "{}",
      error: null,
    });
    streams.apply("s", { type: "replyStarted", messageId: 3, createdAt: 2 });
    streams.apply("s", { type: "content", text: "Done" });
    // get_chat relu pendant le second tour : premier tour sans son résultat.
    const behind = [
      message(1, "user", "Q"),
      { ...message(2, "assistant", ""), toolCalls: [search("a")] },
    ];
    const merged = streams.mergeMessages(behind, streams.streamsFor("c"));
    expect(merged.map((m) => [m.id, m.content])).toEqual([
      [1, "Q"],
      [2, ""],
      [3, "Done"],
    ]);
    expect(merged[1].toolCalls[0].result).toBe("{}");
  });

  it("an edit cuts a finished stream by any of its replies", () => {
    const history = [message(1, "user", "Hi"), message(2, "assistant", "Yo")];
    streams.start({ streamId: "a", chatId: "c", content: "A", model: "m" });
    streams.apply("a", { type: "replyStarted", messageId: 4, createdAt: 1 });
    streams.apply("a", { type: "replyStarted", messageId: 5, createdAt: 1 });
    streams.apply("a", { type: "done", messageId: 5 });
    // Message envoyé jamais relu : seule une réponse du flux le situe après
    // le message modifié.
    streams.start({
      streamId: "b",
      chatId: "c",
      content: "Hi again",
      model: "m",
      editOf: 5,
    });
    const merged = streams.mergeMessages(history, streams.streamsFor("c"));
    expect(merged.map((m) => m.content)).toEqual(["Hi", "Yo", "Hi again"]);
  });
});

describe("images (generate_image)", () => {
  const progress = (stage: ImageProgress["stage"], step: number | null) => ({
    stage,
    step,
    total: step === null ? null : 4,
    preview: null,
    cpu: false,
    width: 512,
    height: 512,
  });
  const IMAGE = {
    id: 42,
    filename: "image-7.png",
    kind: "image" as const,
    mime: "image/png",
    size: 2048,
  };

  /** Envoi « c » du bouton Image : réponse 2 qui appelle generate_image. */
  function startImage() {
    streams.start({
      streamId: "s",
      chatId: "c",
      content: "A cat",
      model: "",
      image: true,
    });
    streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 1 });
    streams.apply("s", {
      type: "toolCalls",
      messageId: 2,
      toolCalls: [
        {
          name: "generate_image",
          arguments: '{"prompt":"A cat"}',
          result: null,
          error: null,
        },
      ],
    });
  }

  it("marks a send of the Image button", () => {
    startImage();
    expect(only("c")?.image).toBe(true);
    streams.start({ streamId: "t", chatId: "d", content: "Hi", model: "m" });
    expect(only("d")?.image).toBe(false);
  });

  it("keeps the last progress of each call, the same object until it changes", () => {
    startImage();
    expect(streams.imageProgressFor(2, 0)).toBeNull();
    streams.apply("s", {
      type: "imageProgress",
      messageId: 2,
      index: 0,
      progress: progress("loading", null),
    });
    streams.apply("s", {
      type: "imageProgress",
      messageId: 2,
      index: 0,
      progress: progress("generating", 3),
    });
    const last = streams.imageProgressFor(2, 0);
    expect(last).toEqual(progress("generating", 3));
    expect(streams.imageProgressFor(2, 0)).toBe(last);
    // Un autre appel, une autre réponse : rien.
    expect(streams.imageProgressFor(2, 1)).toBeNull();
    expect(streams.imageProgressFor(3, 0)).toBeNull();
    // Rien en base, rien dans les messages.
    expect(only("c")?.replies[0].toolCalls[0].result).toBeNull();
    streams.remove("s");
    expect(streams.imageProgressFor(2, 0)).toBeNull();
  });

  it("adds the image to the attachments of its reply, before the result", () => {
    startImage();
    streams.apply("s", {
      type: "imageReady",
      messageId: 2,
      index: 0,
      attachment: IMAGE,
    });
    expect(only("c")?.replies[0].attachments).toEqual([IMAGE]);
    // Réponse inconnue : rien.
    streams.apply("s", {
      type: "imageReady",
      messageId: 9,
      index: 0,
      attachment: { ...IMAGE, id: 43 },
    });
    expect(only("c")?.replies.map((r) => r.attachments.length)).toEqual([1]);
  });
});

describe("mergeMessages", () => {
  const saved = [message(1, "user", "Hi"), message(2, "assistant", "Hello")];

  it("shows the sent text before it is saved, then the saved message", () => {
    streams.start({ streamId: "s", chatId: "c", content: "Again", model: "m" });
    const pending = streams.mergeMessages(saved, streams.streamsFor("c"));
    expect(pending.map((m) => [m.role, m.content])).toEqual([
      ["user", "Hi"],
      ["assistant", "Hello"],
      ["user", "Again"],
    ]);

    streams.apply("s", {
      type: "userMessage",
      message: message(3, "user", "Again"),
    });
    const withSaved = [...saved, message(3, "user", "Again")];
    expect(
      streams
        .mergeMessages(withSaved, streams.streamsFor("c"))
        .map((m) => m.id),
    ).toEqual([1, 2, 3]);
  });

  it("prefers the live reply to the last saved state", () => {
    streams.start({ streamId: "s", chatId: "c", content: "Again", model: "m" });
    streams.apply("s", {
      type: "userMessage",
      message: message(3, "user", "Again"),
    });
    streams.apply("s", { type: "replyStarted", messageId: 4, createdAt: 1 });
    streams.apply("s", { type: "content", text: "Hello again" });
    const behind = [
      ...saved,
      message(3, "user", "Again"),
      message(4, "assistant", "Hel"),
    ];
    const merged = streams.mergeMessages(behind, streams.streamsFor("c"));
    expect(merged).toHaveLength(4);
    expect(merged[3].content).toBe("Hello again");
  });

  it("does not duplicate a sent message read from the database first", () => {
    vi.useFakeTimers({ now: 10_000 });
    streams.start({ streamId: "s", chatId: "c", content: "Again", model: "m" });
    // get_chat revenu avant l'événement userMessage.
    const read = [...saved, message(3, "user", "Again", 10_002)];
    expect(
      streams.mergeMessages(read, streams.streamsFor("c")).map((m) => m.id),
    ).toEqual([1, 2, 3]);
  });

  it("still shows the sent text when the same text was sent before", () => {
    vi.useFakeTimers({ now: 10_000 });
    const earlier = [
      message(1, "user", "Again", 9_000),
      message(2, "assistant", "Hello", 9_001),
    ];
    streams.start({ streamId: "s", chatId: "c", content: "Again", model: "m" });
    const merged = streams.mergeMessages(earlier, streams.streamsFor("c"));
    expect(merged.map((m) => [m.role, m.content])).toEqual([
      ["user", "Again"],
      ["assistant", "Hello"],
      ["user", "Again"],
    ]);
    expect(merged[2].id).toBeLessThan(0);
  });

  it("a second message does not hide the reply of a finished stream", () => {
    streams.start({ streamId: "a", chatId: "c", content: "A", model: "m" });
    streams.apply("a", {
      type: "userMessage",
      message: message(3, "user", "A"),
    });
    streams.apply("a", { type: "replyStarted", messageId: 4, createdAt: 1 });
    streams.apply("a", { type: "content", text: "Reply A" });
    streams.apply("a", { type: "done", messageId: 4 });
    // Second envoi pendant la relecture finale du premier.
    streams.start({ streamId: "b", chatId: "c", content: "B", model: "m" });

    const merged = streams.mergeMessages(saved, streams.streamsFor("c"));
    expect(merged.map((m) => [m.role, m.content])).toEqual([
      ["user", "Hi"],
      ["assistant", "Hello"],
      ["user", "A"],
      ["assistant", "Reply A"],
      ["user", "B"],
    ]);
    expect(new Set(merged.map((m) => m.id)).size).toBe(merged.length);
  });

  it("an edit also cuts a finished stream not removed yet", () => {
    const history = [
      ...saved,
      message(3, "user", "A"),
      message(4, "assistant", "Reply A"),
    ];
    streams.start({ streamId: "a", chatId: "c", content: "A", model: "m" });
    streams.apply("a", {
      type: "userMessage",
      message: message(3, "user", "A"),
    });
    streams.apply("a", { type: "replyStarted", messageId: 4, createdAt: 1 });
    streams.apply("a", { type: "content", text: "Reply A" });
    streams.apply("a", { type: "done", messageId: 4 });
    // Modification du premier message pendant la relecture finale de « a ».
    streams.start({
      streamId: "b",
      chatId: "c",
      content: "Hi again",
      model: "m",
      editOf: 1,
    });

    const merged = streams.mergeMessages(history, streams.streamsFor("c"));
    expect(merged.map((m) => [m.role, m.content])).toEqual([
      ["user", "Hi again"],
    ]);
  });

  it("two sent messages not saved yet keep distinct identifiers", () => {
    streams.start({ streamId: "a", chatId: "c", content: "A", model: "m" });
    streams.fail("a", { kind: "internal", message: "refused" });
    streams.start({ streamId: "b", chatId: "c", content: "B", model: "m" });
    const merged = streams.mergeMessages([], streams.streamsFor("c"));
    expect(merged.map((m) => m.content)).toEqual(["A", "B"]);
    expect(merged[0].id).not.toBe(merged[1].id);
  });

  it("keeps unchanged messages identical from one merge to the next", () => {
    streams.start({ streamId: "s", chatId: "c", content: "Again", model: "m" });
    const first = streams.mergeMessages(saved, streams.streamsFor("c"));
    streams.apply("s", { type: "replyStarted", messageId: 4, createdAt: 1 });
    streams.apply("s", { type: "content", text: "He" });
    const second = streams.mergeMessages(saved, streams.streamsFor("c"));
    expect(second[0]).toBe(saved[0]);
    expect(second[2]).toBe(first[2]); // message envoyé, pas encore relu
    const third = streams.mergeMessages(saved, streams.streamsFor("c"));
    expect(third[3]).toBe(second[3]); // réponse, sans nouveau fragment
  });
});

describe("whenIdle (voice mode)", () => {
  it("resolves at once without a reply, else when the reply ends", async () => {
    await streams.whenIdle("chat-1");
    streams.start({
      streamId: "s1",
      chatId: "chat-1",
      content: "Hi",
      model: "m",
    });
    let idle = false;
    const waiting = streams.whenIdle("chat-1").then(() => (idle = true));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(idle).toBe(false);
    streams.apply("s1", { type: "cancelled" });
    await waiting;
    expect(idle).toBe(true);
  });

  it("a new chat waits for its pending reply; gives up after the delay", async () => {
    vi.useFakeTimers();
    streams.start({ streamId: "s1", chatId: null, content: "Hi", model: "m" });
    let idle = false;
    void streams.whenIdle(streams.NEW_CHAT, 1000).then(() => (idle = true));
    await vi.advanceTimersByTimeAsync(999);
    expect(idle).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(idle).toBe(true);
  });
});
