import { render, screen } from "@testing-library/react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { type Event, listen as listenTo, once } from "@tauri-apps/api/event";
import { type DragDropEvent, getCurrentWebview } from "@tauri-apps/api/webview";
import { createElement, useEffect, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { SendRequest } from "@/bindings/SendRequest";
import { cancelMessage, sendMessage, stageBytes } from "@/lib/ipc";
import {
  type Args,
  installSimulator,
  SEQUENCE_TIME,
  sequence,
  sequenceNames,
  SIMULATOR_MARKER,
} from "@/test/simulator";

const FINAL = ["done", "error", "cancelled"];

/** Requête d'une séquence, avec un identifiant de flux neuf. */
function requestOf(name: string): SendRequest {
  return { ...sequence(name).request, streamId: crypto.randomUUID() };
}

/** Canal qui garde ses événements ; runCallback espionné. */
function listen() {
  const events: ChatEvent[] = [];
  const channel = new Channel<ChatEvent>();
  channel.onmessage = (event) => events.push(event);
  const tauri = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        runCallback: (id: number, data: unknown) => void;
        callbacks: Map<number, unknown>;
      };
    }
  ).__TAURI_INTERNALS__;
  const runCallback = vi.spyOn(tauri, "runCallback");
  const raw = () =>
    runCallback.mock.calls
      .filter(([id]) => id === channel.id)
      .map(([, data]) => data);
  const open = () => tauri.callbacks.has(channel.id);
  return { events, channel, raw, open };
}

/** Problèmes attendus par le test : vérifiés puis retirés. */
function takeProblems(simulator: { problems: string[] }) {
  return simulator.problems.splice(0);
}

describe("simulator", () => {
  it("delivers the events through the channel, closes it, then answers", async () => {
    const simulator = installSimulator();
    simulator.queue("existing-chat");
    const { events, channel, raw, open } = listen();
    const before = Date.now();

    await expect(
      sendMessage(requestOf("existing-chat"), channel),
    ).resolves.toBe(null);

    const expected = sequence("existing-chat").events;
    expect(events.map((e) => e.type)).toEqual(expected.map((e) => e.type));
    expect(raw()).toEqual([
      ...events.map((message, index) => ({ message, index })),
      { end: true, index: expected.length },
    ]);
    // Canal fermé par {end: true}, comme quand le moteur le lâche.
    expect(open()).toBe(false);
    // Horodatages normalisés remplacés par l'heure de la livraison.
    const user = events.find((e) => e.type === "userMessage");
    expect(user?.type === "userMessage" && user.message.createdAt).not.toBe(
      SEQUENCE_TIME,
    );
    expect(
      user?.type === "userMessage" && user.message.createdAt,
    ).toBeGreaterThanOrEqual(before);
    expect(user?.type === "userMessage" && user.message.content).toBe(
      "Say hello in one word.",
    );
  });

  it("a refused sequence closes the channel and rejects with its error", async () => {
    const simulator = installSimulator();
    simulator.queue("refused-empty");
    const { events, channel, raw } = listen();

    await expect(
      sendMessage(requestOf("refused-empty"), channel),
    ).rejects.toEqual({ kind: "invalid", message: "the message is empty" });

    expect(events).toEqual([]);
    expect(raw()).toEqual([{ end: true, index: 0 }]);
  });

  it("an unexpected command is refused and fails the test", async () => {
    const simulator = installSimulator({ handlers: { list_models: () => [] } });

    await expect(invoke("list_models")).resolves.toEqual([]);
    await expect(
      invoke("rename_chat", { id: "c", title: "T" }),
    ).rejects.toThrow(
      `[${SIMULATOR_MARKER}] unexpected command "rename_chat" {"id":"c","title":"T"}: give it a handler`,
    );
    expect(takeProblems(simulator)).toHaveLength(1);
    expect(simulator.callsTo("rename_chat")).toEqual([{ id: "c", title: "T" }]);
  });

  it("a send that does not match the sequence is refused and fails the test", async () => {
    const simulator = installSimulator();
    simulator.queue("new-chat-thinking");
    const { channel } = listen();

    await expect(
      sendMessage(
        { ...requestOf("new-chat-thinking"), content: "Hello" },
        channel,
      ),
    ).rejects.toThrow(
      'send_message for "new-chat-thinking": content is "Hello", the sequence has "Say hello in one word."',
    );
    // Sans séquence en attente non plus.
    await expect(
      sendMessage(requestOf("new-chat-thinking"), listen().channel),
    ).rejects.toThrow("send_message without a queued sequence");
    expect(takeProblems(simulator)).toHaveLength(2);
  });

  it("the think field is compared unless ignored", async () => {
    const simulator = installSimulator();
    const request = requestOf("existing-chat");
    delete request.think;
    simulator.queue("existing-chat");
    await expect(sendMessage(request, listen().channel)).rejects.toThrow(
      "think is absent, the sequence has false",
    );
    takeProblems(simulator);

    simulator.queue("existing-chat", { ignore: ["think"] });
    await expect(
      sendMessage(
        { ...request, streamId: crypto.randomUUID() },
        listen().channel,
      ),
    ).resolves.toBe(null);
  });

  it("a stepwise replay delivers nothing until asked", async () => {
    const simulator = installSimulator();
    const replay = simulator.queue("model-error", { stepwise: true });
    const { events, channel, open } = listen();
    let answered = false;
    const sending = sendMessage(requestOf("model-error"), channel).then(() => {
      answered = true;
    });

    await replay.started;
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(events).toEqual([]);

    await replay.step();
    expect(events.map((e) => e.type)).toEqual(["chatCreated"]);
    expect(replay.chatId).toBe("chat-1");
    await replay.until("content");
    // Chargement du modèle éventuel (loading, loaded) avant la réponse.
    expect(
      events
        .map((e) => e.type)
        .filter((type) => type !== "loading" && type !== "loaded"),
    ).toEqual(["chatCreated", "userMessage", "replyStarted", "content"]);
    expect(replay.remaining).toBe(2);
    expect(answered).toBe(false);
    expect(open()).toBe(true);

    await replay.finish();
    await sending;
    expect(events.at(-1)).toEqual({
      type: "error",
      kind: "model",
      message: "model runner has unexpectedly stopped",
    });
    expect(answered).toBe(true);
    expect(open()).toBe(false);
  });

  it("cancelled only comes after cancel_message", async () => {
    const simulator = installSimulator();
    const replay = simulator.queue("cancelled");
    const { events, channel } = listen();
    const request = requestOf("cancelled");
    let answered = false;
    const sending = sendMessage(request, channel).then(() => {
      answered = true;
    });

    // Tout, sauf « cancelled ».
    const before = sequence("cancelled").events.length - 1;
    await vi.waitFor(() => expect(events).toHaveLength(before));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(events.map((e) => e.type)).not.toContain("cancelled");
    expect(answered).toBe(false);

    await cancelMessage(request.streamId);
    await sending;
    expect(events.at(-1)).toEqual({ type: "cancelled" });
    expect(replay.cancelRequested).toBe(true);
    // Réponse finie : un autre arrêt est sans effet, comme dans le moteur.
    await expect(cancelMessage(request.streamId)).resolves.toBe(null);
  });

  it("stopping a sequence that does not end with cancelled fails the test", async () => {
    const simulator = installSimulator();
    const replay = simulator.queue("model-error", { stepwise: true });
    const request = requestOf("model-error");
    void sendMessage(request, listen().channel);
    await replay.started;

    await cancelMessage(request.streamId);

    expect(takeProblems(simulator)).toEqual([
      `[${SIMULATOR_MARKER}] cancel_message for "model-error", which does not end with "cancelled": replay the "cancelled" sequence to test a cancellation`,
    ]);
  });

  it("lateFinalEvent answers the command before the final event", async () => {
    const simulator = installSimulator();
    simulator.queue("existing-chat", { lateFinalEvent: true });
    const { events, channel } = listen();

    await sendMessage(requestOf("existing-chat"), channel);

    expect(events.at(-1)?.type).not.toBe("done");
    await vi.waitFor(() => expect(events.at(-1)?.type).toBe("done"));
  });

  it("attachmentIds is compared unless ignored: absent is not empty", async () => {
    const simulator = installSimulator();
    const { attachmentIds, ...without } = requestOf("attachments");
    simulator.queue("attachments");
    await expect(sendMessage(without, listen().channel)).rejects.toThrow(
      'attachmentIds is absent, the sequence has ["staged-1","staged-2"]',
    );
    simulator.queue("attachments");
    await expect(
      sendMessage(
        {
          ...requestOf("attachments"),
          attachmentIds: ["staged-2", "staged-1"],
        },
        listen().channel,
      ),
    ).rejects.toThrow('attachmentIds is ["staged-2","staged-1"]');
    simulator.queue("existing-chat");
    await expect(
      sendMessage(
        { ...requestOf("existing-chat"), attachmentIds: [] },
        listen().channel,
      ),
    ).rejects.toThrow("attachmentIds is [], the sequence has absent");
    expect(takeProblems(simulator)).toHaveLength(3);

    simulator.queue("attachments", { ignore: ["attachmentIds"] });
    await expect(
      sendMessage(
        { ...without, streamId: crypto.randomUUID() },
        listen().channel,
      ),
    ).resolves.toBe(null);
    simulator.queue("attachments");
    await expect(
      sendMessage(
        { ...without, attachmentIds, streamId: crypto.randomUUID() },
        listen().channel,
      ),
    ).resolves.toBe(null);
  });

  it("onSend sees each matching send before the replay", async () => {
    const simulator = installSimulator();
    const seen: unknown[] = [];
    simulator.onSend((request, replay) =>
      seen.push({ request, name: replay.name, started: replay.request }),
    );
    simulator.queue("existing-chat");
    await expect(
      sendMessage(
        { ...requestOf("existing-chat"), content: "Hello" },
        listen().channel,
      ),
    ).rejects.toThrow("content is");
    takeProblems(simulator);
    // Requête refusée par la comparaison : pas prévenu.
    expect(seen).toEqual([]);

    simulator.queue("existing-chat");
    const request = requestOf("existing-chat");
    await sendMessage(request, listen().channel);
    // Avant la relecture : le Replay n'a pas encore reçu la requête.
    expect(seen).toEqual([{ request, name: "existing-chat", started: null }]);
  });

  it("what onSend throws refuses the send, which is not replayed", async () => {
    const simulator = installSimulator();
    simulator.onSend(() => {
      throw simulator.fail("refused by the test");
    });
    const replay = simulator.queue("existing-chat");
    const { events, channel } = listen();

    await expect(
      sendMessage(requestOf("existing-chat"), channel),
    ).rejects.toThrow("refused by the test");

    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(events).toEqual([]);
    expect(replay.request).toBe(null);
    expect(takeProblems(simulator)).toEqual([
      `[${SIMULATOR_MARKER}] refused by the test`,
    ]);
  });
});

describe("raw bodies", () => {
  it("stage_bytes gets the bytes and headers that mockIPC would lose", async () => {
    const received: Args[] = [];
    const simulator = installSimulator({
      handlers: {
        stage_bytes: (args) => {
          received.push(args);
          return null;
        },
      },
    });
    const name = "résumé (v2)#1.txt";

    await stageBytes(name, new Uint8Array([104, 105, 0, 255]), 4);

    const expected = {
      bytes: [104, 105, 0, 255],
      headers: { "x-filename": encodeURIComponent(name), "x-size": "4" },
    };
    expect(received).toEqual([expected]);
    expect(simulator.callsTo("stage_bytes")).toEqual([expected]);
  });

  it("an ArrayBuffer, a view or a byte array is a raw body; header names are lower-cased", async () => {
    const simulator = installSimulator({
      handlers: { stage_bytes: () => null },
    });
    const buffer = new Uint8Array([1, 2, 3, 4]).buffer;
    // Fichier collé : octets lus par le File de jsdom.
    const file = new File(["hé"], "x.txt");

    await invoke("stage_bytes", buffer, { headers: { "X-Filename": "a.txt" } });
    await invoke("stage_bytes", new Uint8Array(buffer, 1, 2), {
      headers: [["x-filename", "b.txt"]],
    });
    await invoke("stage_bytes", [5, 6], {
      headers: new Headers({ "x-filename": "c.txt" }),
    });
    await invoke("stage_bytes", await file.arrayBuffer());
    await invoke("stage_bytes", await file.bytes());
    // Arguments ordinaires : inchangés.
    await invoke("stage_bytes", { name: "d.txt" });

    expect(simulator.callsTo("stage_bytes")).toEqual([
      { bytes: [1, 2, 3, 4], headers: { "x-filename": "a.txt" } },
      { bytes: [2, 3], headers: { "x-filename": "b.txt" } },
      { bytes: [5, 6], headers: { "x-filename": "c.txt" } },
      { bytes: [104, 195, 169], headers: {} },
      { bytes: [104, 195, 169], headers: {} },
      { name: "d.txt" },
    ]);
  });
});

describe("Tauri events", () => {
  const DRAG_EVENTS = [
    "tauri://drag-enter",
    "tauri://drag-over",
    "tauri://drag-drop",
    "tauri://drag-leave",
  ];

  function callbacks() {
    return (
      window as unknown as {
        __TAURI_INTERNALS__: { callbacks: Map<number, unknown> };
      }
    ).__TAURI_INTERNALS__.callbacks;
  }

  it("a drop reaches getCurrentWebview().onDragDropEvent until it unlistens", async () => {
    const simulator = installSimulator();
    const warn = vi.spyOn(console, "warn");
    const before = callbacks().size;
    const received: Event<DragDropEvent>[] = [];

    const webview = getCurrentWebview();
    const unlisten = await webview.onDragDropEvent((event) =>
      received.push(event),
    );

    expect(webview.label).toBe("main");
    expect(DRAG_EVENTS.map((name) => simulator.listening(name))).toEqual([
      1, 1, 1, 1,
    ]);
    expect(callbacks().size).toBe(before + 4);
    const paths = ["/home/u/notes.txt", "/home/u/red.png"];
    await simulator.emitEvent("tauri://drag-drop", {
      paths,
      position: { x: 1, y: 2 },
    });
    expect(received).toHaveLength(1);
    expect(received[0]).toMatchObject({
      event: "tauri://drag-drop",
      id: expect.any(Number) as number,
      payload: { type: "drop", paths, position: { x: 1, y: 2 } },
    });
    await simulator.emitEvent("tauri://drag-leave", null);
    expect(received[1]).toMatchObject({ payload: { type: "leave" } });

    // Désinscription immédiate (unregisterListener puis unlisten), sans
    // attendre les promesses.
    unlisten();
    expect(DRAG_EVENTS.map((name) => simulator.listening(name))).toEqual([
      0, 0, 0, 0,
    ]);
    expect(callbacks().size).toBe(before);
    await simulator.emitEvent("tauri://drag-drop", {
      paths,
      position: { x: 1, y: 2 },
    });
    expect(received).toHaveLength(2);
    expect(warn).not.toHaveBeenCalled();
    // Inscriptions : hors des commandes reçues.
    expect(simulator.calls).toEqual([]);
  });

  it("each listener gets { event, id, payload } (copied); once unlistens itself", async () => {
    const simulator = installSimulator();
    const all: Event<unknown>[] = [];
    const first: Event<unknown>[] = [];
    const unlisten = await listenTo("chatonlm://x", (event) => all.push(event));
    await once("chatonlm://x", (event) => first.push(event));
    expect(simulator.listening("chatonlm://x")).toBe(2);

    const payload = { n: 1 };
    await simulator.emitEvent("chatonlm://x", payload);
    await simulator.emitEvent("chatonlm://x", { n: 2 });
    await simulator.emitEvent("chatonlm://other", { n: 3 });

    expect(all.map((event) => event.payload)).toEqual([{ n: 1 }, { n: 2 }]);
    expect(all[0].payload).not.toBe(payload);
    expect(first).toEqual([
      { event: "chatonlm://x", id: expect.any(Number) as number, payload },
    ]);
    expect(first[0].id).not.toBe(all[0].id);
    expect(simulator.listening("chatonlm://x")).toBe(1);
    unlisten();
    expect(simulator.listening("chatonlm://x")).toBe(0);
  });

  it("a listener unlistened by another during the dispatch is skipped", async () => {
    const simulator = installSimulator();
    const warn = vi.spyOn(console, "warn");
    const calls: string[] = [];
    let unlistenSecond = () => {};
    await listenTo("chatonlm://x", () => {
      calls.push("first");
      unlistenSecond();
    });
    unlistenSecond = await listenTo("chatonlm://x", () => calls.push("second"));

    await simulator.emitEvent("chatonlm://x", null);

    expect(calls).toEqual(["first"]);
    expect(warn).not.toHaveBeenCalled();
  });

  it("emitEvent runs in act: the screen shows the update at once", async () => {
    const simulator = installSimulator();
    function Probe() {
      const [text, setText] = useState("waiting");
      useEffect(() => {
        const listening = listenTo<string>("chatonlm://x", (event) =>
          setText(event.payload),
        );
        return () => void listening.then((unlisten) => unlisten());
      }, []);
      return createElement("p", null, text);
    }
    const { unmount } = render(createElement(Probe));
    expect(simulator.listening("chatonlm://x")).toBe(1);

    await simulator.emitEvent("chatonlm://x", "dropped");
    expect(screen.getByText("dropped")).toBeTruthy();

    unmount();
    await vi.waitFor(() => expect(simulator.listening("chatonlm://x")).toBe(0));
  });
});

describe("recorded sequences", () => {
  it("are all there", () => {
    expect(sequenceNames).toEqual([
      "attachments",
      "cancelled",
      "connection-lost",
      "download",
      "download-cancelled",
      "download-offline",
      "edit",
      "existing-chat",
      "http-error",
      "loading",
      "model-error",
      "model-missing",
      "new-chat-thinking",
      "refused-edit-unknown-message",
      "refused-empty",
      "refused-unknown-attachment",
      "unreachable",
      "web-search",
      "web-search-cancelled",
      "web-search-signed-out",
      "web-search-signed-out-late",
    ]);
  });

  // Ce que le frontend suppose du moteur (src/hooks/useSendMessage.ts).
  it.each(sequenceNames)("%s follows the engine's contract", (name) => {
    const { request, events, error } = sequence(name);
    if (error) {
      expect(events).toEqual([]);
      return;
    }
    const types = events.map((e) => e.type);
    // Exactement un événement final, le dernier.
    expect(types.filter((t) => FINAL.includes(t))).toHaveLength(1);
    expect(FINAL).toContain(types.at(-1));
    // Nouvelle conversation : chatCreated d'abord ; sinon jamais.
    expect(types[0] === "chatCreated").toBe(request.chatId === undefined);
    expect(types.filter((t) => t === "chatCreated").length).toBeLessThan(2);
    // Message enregistré tel que tapé.
    const user = events.find((e) => e.type === "userMessage");
    expect(user?.type === "userMessage" && user.message.content).toBe(
      request.content,
    );
    // Fragments seulement après replyStarted ; done désigne la dernière
    // réponse (une par tour de recherche web).
    const started = types.indexOf("replyStarted");
    const firstPiece = types.findIndex(
      (t) => t === "thinking" || t === "content",
    );
    if (firstPiece !== -1) expect(started).toBeLessThan(firstPiece);
    const done = events.at(-1);
    const replies = events.flatMap((e) =>
      e.type === "replyStarted" ? [e.messageId] : [],
    );
    if (done?.type === "done" && replies.length > 0) {
      expect(done.messageId).toBe(replies.at(-1));
    }
    // Outils : les appels d'un tour visent sa réponse, déjà commencée ; ses
    // résultats suivent, un par appel au plus, avant le tour suivant.
    let calls: { messageId: number; count: number; results: number[] } | null =
      null;
    let current: number | null = null;
    for (const event of events) {
      if (event.type === "replyStarted") {
        if (calls) {
          expect(calls.results.length).toBe(calls.count);
        }
        calls = null;
        current = event.messageId;
      } else if (event.type === "toolCalls") {
        expect(event.messageId).toBe(current);
        expect(event.toolCalls.length).toBeGreaterThan(0);
        for (const call of event.toolCalls) {
          expect(call.result).toBeNull();
          expect(call.error).toBeNull();
          expect(typeof JSON.parse(call.arguments)).toBe("object");
        }
        calls = {
          messageId: event.messageId,
          count: event.toolCalls.length,
          results: [],
        };
      } else if (event.type === "toolResult") {
        expect(calls).not.toBeNull();
        expect(event.messageId).toBe(calls!.messageId);
        expect(event.index).toBe(calls!.results.length);
        expect((event.result === null) !== (event.error === null)).toBe(true);
        calls!.results.push(event.index);
      } else if (event.type === "thinking" || event.type === "content") {
        // Plus de fragment pour une réponse qui a appelé ses outils.
        expect(calls).toBeNull();
      }
    }
    if (request.webSearch !== true) {
      expect(types).not.toContain("toolCalls");
    }
    // Chargement du modèle : au plus un par envoi, après le message
    // enregistré et le téléchargement, avant la réponse ; loaded le suit
    // aussitôt, sauf arrêt pendant le chargement (cancelled, sans loaded).
    const loading = types.indexOf("loading");
    expect(types.filter((t) => t === "loading").length).toBeLessThan(2);
    expect(types.filter((t) => t === "loaded").length).toBeLessThan(2);
    if (loading === -1) {
      expect(types).not.toContain("loaded");
    } else {
      expect(loading).toBeGreaterThan(types.indexOf("userMessage"));
      expect(loading).toBeGreaterThan(types.lastIndexOf("download"));
      if (started !== -1) expect(loading).toBeLessThan(started);
      expect([["loaded"], ["cancelled"]]).toContainEqual(
        types.slice(loading + 1, loading + 2),
      );
    }
    // Téléchargement : après le message enregistré, avant la réponse ; un
    // seul download fini, le dernier, complet ; jamais de done: true avant
    // un arrêt ou une erreur.
    const downloads = events.flatMap((e, i) =>
      e.type === "download" ? [{ ...e, at: i }] : [],
    );
    if (downloads.length > 0) {
      expect(downloads[0].at).toBeGreaterThan(types.indexOf("userMessage"));
      if (started !== -1) {
        expect(downloads.at(-1)!.at).toBeLessThan(started);
      }
      const finished = downloads.filter((d) => d.done);
      if (types.at(-1) === "done") {
        expect(finished).toEqual([downloads.at(-1)]);
        expect(finished[0].completed).toBe(finished[0].total);
      } else {
        expect(finished).toEqual([]);
      }
    }
  });
});

describe("the simulator stays out of the app", () => {
  // Code de l'app : tout src/, sauf les tests et leurs utilitaires.
  const SOURCES = import.meta.glob<string>(
    ["/src/**/*.{ts,tsx}", "!/src/**/*.test.{ts,tsx}", "!/src/test/**"],
    { query: "?raw", import: "default", eager: true },
  );

  it("no app file imports src/test", () => {
    const files = Object.keys(SOURCES);
    expect(files).toContain("/src/main.tsx");
    expect(files).toContain("/src/hooks/useSendMessage.ts");
    const offenders = Object.entries(SOURCES)
      .filter(([, text]) =>
        /(?:from|import)\s*\(?\s*["'](?:@\/test\/|(?:\.\.?\/)+(?:[\w.-]+\/)*test\/)|fixtures\/chat|test\/simulator/.test(
          text,
        ),
      )
      .map(([file]) => file);
    expect(offenders).toEqual([]);
  });
});
