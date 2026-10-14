// Contexte dans les flux (propre à ChatonLM) : événements context,
// compacting et compacted, valeur vivante de la jauge, compactages affichés.

import { afterEach, describe, expect, it } from "vitest";
import type { Compaction } from "@/bindings/Compaction";
import type { Message } from "@/bindings/Message";
import * as streams from "./streams";

afterEach(() => {
  streams.resetStreams();
});

function compaction(id: number, throughMessageId: number): Compaction {
  return {
    id,
    throughMessageId,
    summary: `summary ${id}`,
    automatic: true,
    createdAt: 1,
  };
}

function userMessage(id: number): Message {
  return {
    id,
    role: "user",
    content: "Hi",
    thinking: "",
    model: null,
    createdAt: 1,
    updatedAt: 1,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

function start(streamId = "s", editOf: number | null = null) {
  streams.start({ streamId, chatId: "c", content: "Hi", model: "m", editOf });
}

const stream = (id = "s") => {
  const found = streams.getStream(id);
  if (!found) throw new Error(`no stream ${id}`);
  return found;
};

describe("context events", () => {
  it("context and compacted give the live context, compacted its compaction", () => {
    start();
    expect(stream().context).toBeUndefined();
    streams.apply("s", { type: "context", used: 10, size: 100 });
    expect(stream().context).toEqual({ used: 10, size: 100 });

    streams.apply("s", { type: "compacting" });
    expect(stream().compacting).toBe(true);
    streams.apply("s", {
      type: "compacted",
      compaction: compaction(1, 2),
      context: { used: 3, size: 100 },
    });
    expect(stream().compacting).toBe(false);
    expect(stream().compactions).toEqual([compaction(1, 2)]);
    expect(stream().context).toEqual({ used: 3, size: 100 });

    // Taille devenue inconnue : la jauge se cache.
    streams.apply("s", {
      type: "compacted",
      compaction: compaction(2, 4),
      context: null,
    });
    expect(stream().context).toBeNull();
    expect(stream().compactions.map((c) => c.id)).toEqual([1, 2]);
  });

  it("the compacting line never outlives the next event nor the end", () => {
    start("a");
    streams.apply("a", { type: "compacting" });
    streams.apply("a", {
      type: "replyStarted",
      messageId: 3,
      createdAt: 1,
    });
    expect(stream("a").compacting).toBe(false);

    start("b");
    streams.apply("b", { type: "compacting" });
    streams.apply("b", { type: "error", kind: "model", message: "boom" });
    expect(stream("b").compacting).toBe(false);

    start("d");
    streams.apply("d", { type: "loading" });
    streams.apply("d", { type: "compacting" });
    // Le chargement s'efface au compactage, comme à tout autre événement.
    expect(stream("d").loading).toBe(false);
    expect(stream("d").compacting).toBe(true);
    streams.apply("d", { type: "cancelled" });
    expect(stream("d").compacting).toBe(false);
  });
});

describe("liveContext", () => {
  it("is the saved context until an event, then the last stream's event", () => {
    const saved = { used: 50, size: 100 };
    expect(streams.liveContext(saved, [])).toBe(saved);
    start("a");
    start("b");
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toBe(saved);
    streams.apply("a", { type: "context", used: 60, size: 100 });
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toEqual({
      used: 60,
      size: 100,
    });
    streams.apply("b", { type: "context", used: 70, size: 100 });
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toEqual({
      used: 70,
      size: 100,
    });
    streams.apply("b", {
      type: "compacted",
      compaction: compaction(1, 1),
      context: null,
    });
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toBeNull();
  });
});

describe("liveContext during an edit", () => {
  it("is unknown until the edit's first context event", () => {
    const saved = { used: 50, size: 100 };
    start("s", 1);
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toBeNull();
    streams.apply("s", { type: "context", used: 20, size: 100 });
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toEqual({
      used: 20,
      size: 100,
    });
  });

  it("an edit refused before its message was saved changes nothing", () => {
    const saved = { used: 50, size: 100 };
    start("s", 1);
    streams.apply("s", { type: "error", kind: "model", message: "no" });
    expect(streams.liveContext(saved, streams.streamsFor("c"))).toBe(saved);
  });
});

describe("mergeCompactions", () => {
  it("keeps the saved array when the streams add nothing", () => {
    const saved = [compaction(1, 2)];
    expect(streams.mergeCompactions(saved, [])).toBe(saved);
    start();
    expect(streams.mergeCompactions(saved, streams.streamsFor("c"))).toBe(
      saved,
    );
  });

  it("adds the streams' compactions once, by id", () => {
    const saved = [compaction(1, 2)];
    start();
    streams.apply("s", {
      type: "compacted",
      compaction: compaction(2, 4),
      context: null,
    });
    const merged = streams.mergeCompactions(saved, streams.streamsFor("c"));
    expect(merged.map((c) => c.id)).toEqual([1, 2]);
    // Relue en base pendant que le flux est encore là : pas de doublon.
    expect(
      streams
        .mergeCompactions(
          [compaction(1, 2), compaction(2, 4)],
          streams.streamsFor("c"),
        )
        .map((c) => c.id),
    ).toEqual([1, 2]);
  });

  it("drops the compactions of the messages cut by an edit", () => {
    const saved = [compaction(1, 2), compaction(2, 6)];
    start("s", 5);
    expect(
      streams.mergeCompactions(saved, streams.streamsFor("c")).map((c) => c.id),
    ).toEqual([1]);
    // Modification refusée (finie sans message enregistré) : rien n'a changé.
    streams.apply("s", { type: "error", kind: "model", message: "no" });
    expect(streams.mergeCompactions(saved, streams.streamsFor("c"))).toBe(
      saved,
    );
  });

  it("an edit whose message was saved still cuts after its end", () => {
    const saved = [compaction(1, 2), compaction(2, 6)];
    start("s", 5);
    streams.apply("s", { type: "userMessage", message: userMessage(5) });
    streams.apply("s", { type: "done", messageId: null });
    expect(
      streams.mergeCompactions(saved, streams.streamsFor("c")).map((c) => c.id),
    ).toEqual([1]);
  });
});
