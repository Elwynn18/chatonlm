// @vitest-environment node
// Séance vocale tenue par VS Code (lot 2, X) : chaton en caoutchouc (contexte
// de l'éditeur et du terminal, filtré hors machine), tours de parole dans la
// conversation, état, refus, arrêt ; questions sur un passage de la visite.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { deferred, RpcFailure, until } from "./test/fakeBridge";
import {
  cleanUp,
  ofType,
  openEditor,
  setupController,
  TEN_LINES,
} from "./test/setup";
import { Position, resetTestState, Selection, testState } from "./test/vscode";
import { CONTEXT_DELAY_MS } from "./voice";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const voiceOf = (posted: Parameters<typeof ofType>[0]) =>
  ofType(posted, "voice").at(-1)?.voice ?? null;

async function talking() {
  const env = await setupController();
  const editor = openEditor("/work/src/app.ts", TEN_LINES);
  env.fake.handlers["voice.start"] = () => ({
    sessionId: "s1",
    chatId: null,
  });
  env.fake.handlers["chat.get"] = (params) => ({
    id: params.chatId,
    title: "",
    createdAt: 0,
    updatedAt: 0,
    messages: [],
    context: null,
    compactions: [],
    replyRunning: null,
    ephemeral: false,
  });
  return { ...env, editor };
}

function voiceState(
  fake: { notify(m: string, p: unknown): void },
  phase: string,
  muted = false,
) {
  fake.notify("voice.state", {
    state: { phase, muted, turn: 0, notice: null },
  });
}

const STATE = { muted: false, turn: 0, notice: null };
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Barre d'état de la séance (celle qui montre le panneau). */
const voiceItem = () =>
  testState.statusItems.find((item) => item.command === "chatonlm.chat.focus");

describe("Talk it through (rubber duck)", () => {
  it("starts with what the editor shows and the last failed command", async () => {
    const { fake, controller, posted, terminal } = await talking();
    terminal.fail({
      command: "npm test",
      exitCode: 1,
      output: "Expected 1, got 2",
    });
    await controller.handle({ type: "voice", command: "start" });
    const [start] = fake.calls("voice.start");
    expect(start.profile).toBe("duck");
    expect(start.model).toBe("qwen3:0.6b");
    expect(start.chatId).toBeUndefined();
    expect(start.ephemeral).toBeUndefined();
    const context = start.context as string;
    expect(context).toContain("What the user has open in VS Code:");
    expect(context).toContain("`src/app.ts`, lines 1–10:");
    expect(context).toContain("line 10");
    expect(context).toContain("Last command that failed in the terminal:");
    expect(context).toContain("$ npm test\nExpected 1, got 2\n(exit code 1)");

    const view = voiceOf(posted);
    expect(view).toMatchObject({
      profile: "duck",
      phase: "starting",
      muted: false,
      destination: "local",
      noTerminalError: false,
      blocked: [],
    });
    expect(view?.shared.map((item) => [item.kind, item.label])).toEqual([
      ["file", "src/app.ts"],
      ["terminal", "npm test"],
    ]);
  });

  it("says when no terminal error is known, and shares the selection", async () => {
    const { fake, controller, posted, editor } = await talking();
    editor.selection = new Selection(new Position(2, 0), new Position(3, 6));
    await controller.startVoice("duck");
    const context = fake.calls("voice.start")[0].context as string;
    expect(context).toContain("`src/app.ts`, lines 3–4:");
    expect(context).not.toContain("line 5");
    expect(context).not.toContain("terminal");
    expect(voiceOf(posted)?.noTerminalError).toBe(true);
    expect(voiceOf(posted)?.shared[0]).toMatchObject({
      kind: "selection",
      lines: [3, 4],
    });
  });

  it("removes secrets from the context when the model is off this computer", async () => {
    const { fake, controller, posted, terminal } = await talking();
    openEditor(
      "/work/src/config.ts",
      'const a = 1;\nconst apiKey = "abcd1234efgh";\nconst b = 2;',
    );
    terminal.fail({
      command: "deploy",
      exitCode: 3,
      output: "using TOKEN=supersecret99",
    });
    fake.local = false;
    await controller.startVoice("duck");
    const context = fake.calls("voice.start")[0].context as string;
    expect(context).not.toContain("abcd1234efgh");
    expect(context).not.toContain("supersecret99");
    expect(context).toContain("[secret removed by ChatonLM]");
    const view = voiceOf(posted);
    expect(view?.destination).toBe("remote");
    expect(view?.blocked.map((b) => b.reason)).toEqual([
      "secretValue",
      "secretValue",
    ]);
  });

  it("follows the phases, the level and the turns of the session", async () => {
    const { fake, controller, posted } = await talking();
    await controller.startVoice("duck");
    voiceState(fake, "listening");
    await until(() => voiceOf(posted)?.phase === "listening");
    fake.notify("voice.level", { level: { input: 0.4, output: 0 } });
    await until(() => ofType(posted, "voiceLevel").length === 1);
    expect(ofType(posted, "voiceLevel")[0].level).toBe(0.4);
    voiceState(fake, "listening", true);
    await until(() => voiceOf(posted)?.muted === true);

    // Un tour de parole : relayé comme un envoi, sous un numéro négatif.
    const events = [
      { type: "chatCreated", chatId: "voice-chat" },
      {
        type: "userMessage",
        message: { id: 1, role: "user", content: "Mon test échoue" },
      },
      { type: "replyStarted", messageId: 2, createdAt: 0 },
      { type: "content", text: "Que devrait-il rendre ?" },
      { type: "done", messageId: 2 },
    ];
    for (const event of events) fake.notify("voice.chat", { event, turn: 1 });
    await until(() => ofType(posted, "event").length === events.length);
    const relayed = ofType(posted, "event");
    expect(new Set(relayed.map((m) => m.requestId)).size).toBe(1);
    expect(relayed[0].requestId).toBeLessThan(0);
    expect(relayed.map((m) => m.event.type)).toEqual(events.map((e) => e.type));
    expect(controller.state.chatId).toBe("voice-chat");
    // Tour fini : la conversation est relue.
    await until(() =>
      fake.calls("chat.get").some((p) => p.chatId === "voice-chat"),
    );

    // Tour suivant : un autre numéro.
    fake.notify("voice.chat", {
      event: { type: "replyStarted", messageId: 4, createdAt: 0 },
      turn: 2,
    });
    await until(() => ofType(posted, "event").length === events.length + 1);
    const next = ofType(posted, "event").at(-1);
    expect(next?.requestId).not.toBe(relayed[0].requestId);

    // Fin de la séance pendant une réponse : la vue est retirée, mais la
    // réponse continue de s'écrire dans l'app, sous le même numéro.
    await controller.handle({ type: "voice", command: "stop" });
    expect(fake.calls("voice.stop")).toHaveLength(1);
    expect(voiceOf(posted)).toBeNull();
    expect(
      ofType(posted, "event").some((m) => m.event.type === "cancelled"),
    ).toBe(false);
    fake.notify("voice.chat", {
      event: { type: "done", messageId: 4 },
      turn: 2,
    });
    await until(() => ofType(posted, "event").at(-1)?.event.type === "done");
    expect(ofType(posted, "event").at(-1)?.requestId).toBe(next?.requestId);
  });

  it("mutes and unmutes the microphone", async () => {
    const { fake, controller, posted } = await talking();
    await controller.startVoice("duck");
    await controller.handle({ type: "voice", command: "mute" });
    expect(fake.calls("voice.mute")).toEqual([{ muted: true }]);
    expect(voiceOf(posted)?.muted).toBe(true);
    await controller.handle({ type: "voice", command: "unmute" });
    expect(fake.calls("voice.mute").at(-1)).toEqual({ muted: false });
  });

  it("sends a new context after the editor changes", async () => {
    const { fake, controller, editor } = await talking();
    await controller.startVoice("duck");
    editor.selection = new Selection(new Position(6, 0), new Position(6, 6));
    testState.events.selection.fire({});
    testState.events.selection.fire({});
    await until(
      () => fake.calls("voice.context").length === 1,
      CONTEXT_DELAY_MS + 1000,
    );
    const text = fake.calls("voice.context")[0].text as string;
    expect(text).toContain("`src/app.ts`, line 7:");
    // Rien de changé : rien de renvoyé.
    testState.events.selection.fire({});
    await new Promise((resolve) => setTimeout(resolve, CONTEXT_DELAY_MS + 100));
    expect(fake.calls("voice.context")).toHaveLength(1);
  });

  it("ends when the app ends it, and when the link is lost", async () => {
    const { fake, controller, posted } = await talking();
    await controller.startVoice("duck");
    fake.notify("voice.state", {
      state: {
        phase: "off",
        muted: false,
        turn: 0,
        notice: { kind: "failed", message: "Micro débranché." },
      },
    });
    await until(() => voiceOf(posted) === null);
    expect(testState.shown.at(-1)).toEqual({
      level: "error",
      text: "Micro débranché.",
    });

    await controller.startVoice("duck");
    expect(voiceOf(posted)).not.toBeNull();
    fake.dropAll();
    await until(() => voiceOf(posted) === null);
  });

  it("explains each refusal of the app", async () => {
    const { fake, controller, posted, deps } = await talking();
    const refuse = (code: number, data: unknown) => {
      fake.handlers["voice.start"] = () => {
        throw new RpcFailure(code, "refused", data);
      };
    };
    refuse(-32032, { owner: "page" });
    await controller.startVoice("duck");
    expect(testState.shown.at(-1)?.text).toBe(
      "Voice is already in use in ChatonLM.",
    );
    expect(voiceOf(posted)).toBeNull();

    refuse(-32033, { message: "Aucun micro trouvé." });
    await controller.startVoice("duck");
    expect(testState.shown.at(-1)?.text).toBe("Aucun micro trouvé.");

    refuse(-32031, {});
    testState.choice = "Open ChatonLM";
    await controller.startVoice("duck");
    expect(testState.shown.at(-1)?.text).toMatch(/speech recognition model/);
    expect(deps.launchApp).toHaveBeenCalledWith(false);
  });

  it("keeps the chat of the panel, and an ephemeral one stays ephemeral", async () => {
    const { fake, controller } = await talking();
    controller.newChat(true);
    await controller.startVoice("duck");
    expect(fake.calls("voice.start")[0].ephemeral).toBe(true);
    await controller.handle({ type: "voice", command: "stop" });

    fake.handlers["voice.start"] = () => ({ sessionId: "s2", chatId: "c9" });
    await controller.startVoice("duck");
    expect(controller.state.chatId).toBe("c9");
    await controller.handle({ type: "voice", command: "stop" });
    await controller.startVoice("duck");
    expect(fake.calls("voice.start").at(-1)?.chatId).toBe("c9");
    expect(fake.calls("voice.start").at(-1)?.ephemeral).toBeUndefined();

    // Nouvelle conversation : la séance s'arrête.
    controller.newChat(false);
    await until(() => fake.calls("voice.stop").length === 3);
    expect(controller.voice.active).toBe(false);
  });

  it("explains a refusal that the app published as the end of the session first", async () => {
    // Ordre de l'app : `starting`, puis `off` (état d'échec), puis l'erreur.
    const { fake, controller, posted, deps } = await talking();
    const refuse = (code: number, data: unknown) => {
      fake.handlers["voice.start"] = (_params, { notify }) => {
        notify("voice.state", { state: { ...STATE, phase: "starting" } });
        notify("voice.state", { state: { ...STATE, phase: "off" } });
        throw new RpcFailure(code, "refused", data);
      };
    };
    refuse(-32031, {});
    testState.choice = "Open ChatonLM";
    await controller.startVoice("duck");
    expect(testState.shown.at(-1)?.text).toMatch(/speech recognition model/);
    expect(deps.launchApp).toHaveBeenCalledWith(false);
    expect(voiceOf(posted)).toBeNull();

    refuse(-32030, { language: "fr" });
    await controller.startVoice("duck");
    expect(testState.shown.at(-1)?.text).toMatch(/no voice for this language/);
    expect(deps.launchApp).toHaveBeenCalledTimes(2);

    refuse(-32033, { message: "Aucun micro trouvé." });
    testState.choice = undefined;
    await controller.startVoice("duck");
    expect(testState.shown.at(-1)?.text).toBe("Aucun micro trouvé.");
    expect(voiceOf(posted)).toBeNull();
    expect(testState.shown).toHaveLength(3);
  });

  it("ends a session the app ended before its answer, with its notice", async () => {
    const { fake, controller, posted } = await talking();
    fake.handlers["voice.start"] = (_params, { notify }) => {
      notify("voice.state", {
        state: {
          ...STATE,
          phase: "off",
          notice: { kind: "failed", message: "Micro débranché." },
        },
      });
      return { sessionId: "s1", chatId: null };
    };
    await controller.startVoice("duck");
    expect(voiceOf(posted)).toBeNull();
    expect(testState.shown.at(-1)?.text).toBe("Micro débranché.");
    expect(controller.voice.active).toBe(false);
  });

  it("stops again a session stopped while it was starting", async () => {
    const { fake, controller, posted } = await talking();
    const answer = deferred<unknown>();
    fake.handlers["voice.start"] = () => answer.promise;
    const starting = controller.startVoice("duck");
    await until(() => fake.calls("voice.start").length === 1);
    await controller.handle({ type: "voice", command: "stop" });
    expect(fake.calls("voice.stop")).toHaveLength(1);
    expect(voiceOf(posted)).toBeNull();
    // L'app a ouvert le micro après avoir traité l'arrêt : il lui est redit.
    answer.resolve({ sessionId: "s1", chatId: null });
    await starting;
    await until(() => fake.calls("voice.stop").length === 2);
    expect(voiceOf(posted)).toBeNull();

    // Filet : un état d'une séance que VS Code ne montre plus.
    voiceState(fake, "listening");
    await until(() => fake.calls("voice.stop").length === 3);
    voiceState(fake, "speaking");
    await sleep(30);
    expect(fake.calls("voice.stop")).toHaveLength(3);
    expect(voiceOf(posted)).toBeNull();
  });

  it("never stops a newer session for the late answer of an older one", async () => {
    const { fake, controller, posted } = await talking();
    const first = deferred<unknown>();
    let starts = 0;
    fake.handlers["voice.start"] = () =>
      ++starts === 1 ? first.promise : { sessionId: "s2", chatId: null };
    const older = controller.startVoice("duck");
    await until(() => fake.calls("voice.start").length === 1);
    await controller.startVoice("duck");
    expect(fake.calls("voice.start")).toHaveLength(2);
    expect(fake.calls("voice.stop")).toHaveLength(1);
    first.resolve({ sessionId: "s1", chatId: null });
    await older;
    await sleep(30);
    expect(fake.calls("voice.stop")).toHaveLength(1);
    expect(voiceOf(posted)?.phase).toBe("starting");
  });

  it("sends the context changed while the session was starting", async () => {
    const { fake, controller, posted } = await talking();
    const answer = deferred<unknown>();
    fake.handlers["voice.start"] = () => answer.promise;
    const starting = controller.startVoice("duck");
    await until(() => fake.calls("voice.start").length === 1);
    await controller.voice.setContext(
      { text: "nouveau contexte", shared: [], blocked: [] },
      true,
    );
    // Pas encore de séance dans l'app : rien ne part avant sa réponse.
    expect(fake.calls("voice.context")).toEqual([]);
    expect(voiceOf(posted)?.shared).toEqual([]);
    answer.resolve({ sessionId: "s1", chatId: null });
    await starting;
    await until(() => fake.calls("voice.context").length === 1);
    expect(fake.calls("voice.context")[0]).toEqual({
      text: "nouveau contexte",
    });
  });

  it("resumes a paused session", async () => {
    const { fake, controller } = await talking();
    await controller.startVoice("duck");
    // En écoute : rien à reprendre.
    voiceState(fake, "listening");
    await until(() => controller.voice.view?.phase === "listening");
    await controller.handle({ type: "voice", command: "resume" });
    expect(fake.calls("voice.resume")).toEqual([]);

    voiceState(fake, "paused");
    await until(() => controller.voice.view?.phase === "paused");
    await controller.handle({ type: "voice", command: "resume" });
    expect(fake.calls("voice.resume")).toEqual([{}]);

    fake.handlers["voice.resume"] = () => {
      throw new RpcFailure(-32033, "refused", {
        message: "Aucun micro trouvé.",
      });
    };
    await controller.handle({ type: "voice", command: "resume" });
    expect(testState.shown.at(-1)?.text).toBe("Aucun micro trouvé.");
  });

  it("shows the session in the status bar, and stops it from the palette", async () => {
    const { fake, controller } = await talking();
    await controller.startVoice("duck");
    expect(testState.contexts["chatonlm.talking"]).toBe(true);
    expect(voiceItem()?.visible).toBe(true);
    expect(voiceItem()?.text).toContain("ChatonLM: getting ready to listen");
    voiceState(fake, "listening");
    await until(
      () => voiceItem()?.text.includes("ChatonLM is listening") === true,
    );
    voiceState(fake, "listening", true);
    await until(() => voiceItem()?.text.includes("microphone off") === true);

    await controller.voice.stop();
    expect(fake.calls("voice.stop")).toHaveLength(1);
    expect(voiceItem()?.visible).toBe(false);
    expect(testState.contexts["chatonlm.talking"]).toBe(false);
  });

  it("lets a reply end after the session, and keeps the turns apart", async () => {
    const { fake, controller, posted } = await talking();
    await controller.startVoice("duck");
    const turn = (n: number, ...events: unknown[]) => {
      for (const event of events) fake.notify("voice.chat", { event, turn: n });
    };
    turn(
      1,
      { type: "chatCreated", chatId: "voice-chat" },
      { type: "replyStarted", messageId: 2, createdAt: 0 },
      { type: "content", text: "Pre" },
    );
    await until(() => ofType(posted, "event").length === 3);
    const first = ofType(posted, "event")[0].requestId;
    await controller.handle({ type: "voice", command: "stop" });

    // La réponse continue dans l'app : relayée, pas annulée.
    turn(1, { type: "content", text: "mière" });
    await until(() => ofType(posted, "event").length === 4);
    expect(ofType(posted, "event").at(-1)).toEqual({
      type: "event",
      requestId: first,
      event: { type: "content", text: "mière" },
    });
    // Rien ne part dans la conversation avant sa fin (l'app refuserait).
    await controller.handle({ type: "send", text: "hello" });
    await controller.startVoice("duck");
    expect(fake.calls("chat.send")).toEqual([]);
    expect(fake.calls("voice.start")).toHaveLength(1);
    expect(testState.shown.at(-1)?.text).toBe(
      "A spoken reply is still being written. Wait for it.",
    );

    turn(1, { type: "done", messageId: 2 });
    await until(() => ofType(posted, "event").length === 5);
    // Nouvelle séance : ses tours (numérotés sur toute la connexion) sont à part.
    await controller.startVoice("duck");
    expect(fake.calls("voice.start")).toHaveLength(2);
    turn(2, { type: "replyStarted", messageId: 4, createdAt: 0 });
    await until(() => ofType(posted, "event").length === 6);
    expect(ofType(posted, "event").at(-1)?.requestId).not.toBe(first);
  });

  it("keeps a reply of a chat left since out of the new one", async () => {
    const { fake, controller, posted } = await talking();
    await controller.startVoice("duck");
    fake.notify("voice.chat", {
      event: { type: "replyStarted", messageId: 2, createdAt: 0 },
      turn: 1,
    });
    await until(() => ofType(posted, "event").length === 1);
    controller.newChat(false);
    await until(() => fake.calls("voice.stop").length === 1);
    fake.notify("voice.chat", {
      event: { type: "content", text: "ancien" },
      turn: 1,
    });
    fake.notify("voice.chat", {
      event: { type: "done", messageId: 2 },
      turn: 1,
    });
    await sleep(30);
    expect(ofType(posted, "event")).toHaveLength(1);
    // La nouvelle conversation n'attend pas la réponse de l'ancienne.
    fake.handlers["chat.send"] = () => ({ chatId: "c2", outcome: "done" });
    await controller.handle({ type: "send", text: "hello" });
    expect(fake.calls("chat.send")).toHaveLength(1);
  });

  it("refuses a typed message while the session talks in the chat", async () => {
    const { fake, controller } = await talking();
    await controller.startVoice("duck");
    await controller.handle({ type: "send", text: "hello" });
    expect(fake.calls("chat.send")).toEqual([]);
    expect(testState.shown.at(-1)?.text).toBe(
      "A voice session is on. Stop it first.",
    );
  });
});

describe("Ask about this part of the tour", () => {
  async function onSegment() {
    const env = await talking();
    env.fake.handlers["tour.plan"] = () => ({
      segments: [
        { lines: [1, 2], text: "Le début." },
        { lines: [3, 5], text: "La boucle." },
      ],
    });
    env.fake.handlers["speech.read"] = () => ({ readId: "r1" });
    env.fake.handlers["chat.send"] = () => ({ chatId: "c1", outcome: "done" });
    await env.controller.tour.start(false);
    env.fake.notify("speech.event", {
      readId: "r1",
      type: "started",
      segmentId: "1:1",
    });
    await until(() => ofType(env.posted, "tour").at(-1)?.tour?.current === 1);
    return env;
  }

  it("typed: pauses the tour and sends the part with the next message", async () => {
    const { fake, controller, posted } = await onSegment();
    await controller.handle({ type: "tourAsk", voice: false });
    expect(ofType(posted, "tour").at(-1)?.tour?.status).toBe("paused");
    await until(() => fake.calls("speech.stop").length === 1);
    expect(controller.state.attachment).toEqual({
      path: "src/app.ts",
      lines: [3, 5],
    });

    await controller.handle({
      type: "send",
      text: "À propos de ce passage : pourquoi i ?",
    });
    const [sent] = fake.calls("chat.send");
    const content = sent.content as string;
    expect(content.startsWith("À propos de ce passage : pourquoi i ?")).toBe(
      true,
    );
    expect(content).toContain("`src/app.ts`, lines 3–5:");
    expect(content).toContain("line 3\nline 4\nline 5");
    expect(content).not.toContain("line 6");
    const receipt = ofType(posted, "receipt").at(-1)?.receipt;
    expect(receipt?.items).toEqual([
      expect.objectContaining({ path: "src/app.ts", lines: [3, 5] }),
    ]);
    // Seulement avec ce message.
    expect(controller.state.attachment).toBeNull();
  });

  it("typed: the part can be taken off before sending", async () => {
    const { fake, controller } = await onSegment();
    await controller.handle({ type: "tourAsk", voice: false });
    await controller.handle({ type: "clearAttachment" });
    expect(controller.state.attachment).toBeNull();
    await controller.handle({ type: "send", text: "hello" });
    expect(fake.calls("chat.send")[0].content).toBe("hello");
  });

  it("aloud: opens a tour session with the part, and stops reading when the user speaks", async () => {
    const { fake, controller, posted } = await onSegment();
    await controller.handle({ type: "tourAsk", voice: true });
    const [start] = fake.calls("voice.start");
    expect(start.profile).toBe("tour");
    const context = start.context as string;
    expect(context).toContain("The tour said: La boucle.");
    expect(context).toContain("line 3\nline 4\nline 5");
    expect(context).not.toContain("terminal");
    expect(voiceOf(posted)?.profile).toBe("tour");

    // Visite reprise, puis l'utilisateur parle : la lecture s'arrête.
    await controller.tourCommand("resume");
    expect(ofType(posted, "tour").at(-1)?.tour?.status).toBe("playing");
    voiceState(fake, "hearing");
    await until(() => ofType(posted, "tour").at(-1)?.tour?.status === "paused");

    // Visite arrêtée : la question à la voix finit avec elle.
    await controller.tourCommand("stop");
    await until(() => fake.calls("voice.stop").length === 1);
    expect(voiceOf(posted)).toBeNull();
  });

  it("aloud: removes secrets from what the tour said when the model is off this computer", async () => {
    const env = await talking();
    const { fake, controller } = env;
    fake.handlers["tour.plan"] = () => ({
      segments: [{ lines: [1, 2], text: 'Ici, password = "hunter22".' }],
    });
    fake.handlers["speech.read"] = () => ({ readId: "r1" });
    fake.local = false;
    await controller.tour.start(false);
    fake.notify("speech.event", {
      readId: "r1",
      type: "started",
      segmentId: "1:0",
    });
    await until(() => ofType(env.posted, "tour").at(-1)?.tour?.current === 0);
    await controller.handle({ type: "tourAsk", voice: true });
    const context = fake.calls("voice.start")[0].context as string;
    expect(context).not.toContain("hunter22");
    expect(context).toContain("The tour said: [secret removed by ChatonLM]");
  });
});
