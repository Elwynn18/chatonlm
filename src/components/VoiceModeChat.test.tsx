// Mode vocal dans la conversation, face au simulateur : envoi des phrases
// transcrites (réflexion coupée, sauf « Let models think » ; langue de la
// phrase dans `voice`), envoi refusé signalé au moteur, réponse écrite au
// clavier arrêtée d'abord, fin de la séance quand sa page n'est plus
// affichée (autre conversation, réglages) sauf pour la conversation créée
// par le premier envoi vocal, séance orpheline (page rechargée) arrêtée,
// modification d'un message.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import type { SendRequest } from "@/bindings/SendRequest";
import type { VoiceModePhase } from "@/bindings/VoiceModePhase";
import { resetStreams } from "@/lib/streams";
import { fakeBackend, type FakeBackend, thinkingModel } from "@/test/backend";
import { PARAKEET } from "@/test/dictation";
import { renderApp, texts } from "@/test/renderApp";
import { installSimulator, type Sequence, sequence } from "@/test/simulator";
import { VOICE_EN } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  resetStreams();
});

const MODEL = "qwen3:0.6b";
const HELLO = "Say hello in one word.";

function message(id: number, role: "user" | "assistant", content: string) {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? MODEL : null,
    createdAt: id,
    updatedAt: id,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  } satisfies Message;
}

function chat(backend: FakeBackend, id: string) {
  backend.chats.set(id, {
    id,
    title: id,
    createdAt: 1,
    updatedAt: 1,
    messages: [message(1, "user", "Hi"), message(2, "assistant", "Hello!")],
  });
}

/**
 * Conversations chat-1 et chat-2, modèle qui sait raisonner (bascule sur On
 * par défaut), transcription et voix anglaise installées.
 */
function setup(path = "/c/chat-1", prepare?: (backend: FakeBackend) => void) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    selectedModel: MODEL,
    details: [thinkingModel(MODEL, { values: [false, true], default: true })],
  });
  chat(backend, "chat-1");
  chat(backend, "chat-2");
  backend.dictation.install(PARAKEET);
  backend.tts.install(VOICE_EN);
  prepare?.(backend);
  const app = renderApp(path);
  return { simulator, backend, voice: backend.voiceMode, app };
}

const bar = () => document.querySelector<HTMLElement>("[data-voice-mode-bar]");

async function phase(expected: VoiceModePhase) {
  await waitFor(() =>
    expect(bar()?.getAttribute("data-voice-mode-phase")).toBe(expected),
  );
}

async function start() {
  let button: HTMLButtonElement | null = null;
  await waitFor(() => {
    button = document.querySelector<HTMLButtonElement>(
      "[data-voice-mode-start]",
    );
    expect(button?.disabled).toBe(false);
  });
  // Bouton Think lu : la réflexion du modèle est connue.
  await screen.findByRole("button", { name: "Think" });
  await userEvent.click(button!);
  await phase("listening");
}

/** Séquence existing-chat avec ce contenu et ce `think`. */
function reply(content: string, think?: SendRequest["think"]): Sequence {
  const recorded = sequence("existing-chat");
  const request: SendRequest = { ...recorded.request, content };
  if (think === undefined) delete request.think;
  else request.think = think;
  return { ...recorded, request };
}

/**
 * Réponse tapée au clavier (« Typed »), livrée en pas à pas jusqu'à son
 * premier fragment ; elle s'arrête quand on le demande (cancel_message).
 */
async function typedReply(simulator: ReturnType<typeof installSimulator>) {
  const field = await screen.findByLabelText("Message");
  await screen.findByRole("button", { name: "Think" });
  const typed = sequence("cancelled");
  const keyboard = simulator.queue(
    {
      ...typed,
      request: {
        chatId: "chat-1",
        streamId: "stream-1",
        content: "Typed",
        model: MODEL,
        think: true,
      },
      events: typed.events.filter((event) => event.type !== "chatCreated"),
    },
    { stepwise: true },
  );
  await userEvent.type(field, "Typed{Enter}");
  await keyboard.started;
  await keyboard.until("content");
  return keyboard;
}

/** Laisse passer les tâches en attente (envoi qui partirait). */
const settle = () =>
  act(() => new Promise((resolve) => setTimeout(resolve, 50)));

/** Champ `voice` de chaque envoi (absent : envoi au clavier). */
function voiceOf(simulator: ReturnType<typeof installSimulator>) {
  return simulator
    .callsTo("send_message")
    .map((args) => (args.request as SendRequest).voice);
}

describe("voice mode sends", () => {
  it("sends a sentence like a message, thinking off, with its language", async () => {
    const { simulator, voice } = setup();
    await start();
    const replay = simulator.queue(reply(HELLO, false));
    await voice.utter(HELLO, "en");
    await replay.done;
    expect(voiceOf(simulator)).toEqual([{ language: "en" }]);
    await waitFor(() => expect(texts("user")).toContain(HELLO));
    // Toujours en mode vocal.
    expect(bar()).not.toBeNull();
  });

  it("language unknown: voice without language", async () => {
    const { simulator, voice } = setup();
    await start();
    const replay = simulator.queue(reply(HELLO, false));
    await voice.utter(HELLO, null);
    await replay.done;
    expect(voiceOf(simulator)).toEqual([{}]);
  });

  it("“Let models think”: the Think button's choice is sent", async () => {
    const { simulator, voice } = setup("/c/chat-1", (backend) => {
      backend.voiceMode.settings.keepThink = true;
    });
    await start();
    const replay = simulator.queue(reply(HELLO, true));
    await voice.utter(HELLO, "en");
    await replay.done;
    expect(simulator.problems).toEqual([]);
  });

  it("each turn is sent once, in order", async () => {
    const { simulator, voice } = setup();
    await start();
    const first = simulator.queue(reply(HELLO, false));
    await voice.utter(HELLO, "en");
    await first.done;
    // Même tour reçu deux fois : un seul envoi.
    await simulator.emitEvent("voice-mode-utterance", {
      turn: 1,
      text: HELLO,
      language: "en",
    });
    expect(simulator.callsTo("send_message")).toHaveLength(1);
  });

  it("a send refused at once is reported to the engine, shown as its notice", async () => {
    const { simulator, voice } = setup();
    await start();
    const refused = sequence("refused-empty");
    simulator.queue({
      ...refused,
      request: {
        ...refused.request,
        chatId: "chat-1",
        content: HELLO,
        think: false,
      },
    });
    await voice.utter(HELLO, "en");
    await waitFor(() =>
      expect(simulator.callsTo("voice_mode_reply_failed")).toEqual([
        { turn: 1, message: "the message is empty" },
      ]),
    );
    await waitFor(() =>
      expect(
        document.querySelector("[data-voice-mode-notice]")?.textContent,
      ).toBe("the message is empty"),
    );
    await phase("listening");
  });

  it("a reply typed before is stopped first, then the sentence is sent", async () => {
    const { simulator, voice } = setup();
    const field = await screen.findByLabelText("Message");
    await screen.findByRole("button", { name: "Think" });
    const typed = sequence("cancelled");
    const keyboard = simulator.queue(
      {
        ...typed,
        request: {
          chatId: "chat-1",
          streamId: "stream-1",
          content: "Typed",
          model: MODEL,
          think: true,
        },
        events: typed.events.filter((event) => event.type !== "chatCreated"),
      },
      { stepwise: true },
    );
    await userEvent.type(field, "Typed{Enter}");
    await keyboard.started;
    await keyboard.until("content");
    await start();
    const spoken = simulator.queue(reply(HELLO, false));
    await voice.utter(HELLO, "en");
    await waitFor(() =>
      expect(simulator.callsTo("cancel_message")).toHaveLength(1),
    );
    // Pas d'envoi tant que la réponse n'est pas arrêtée.
    expect(simulator.callsTo("send_message")).toHaveLength(1);
    await keyboard.finish();
    await spoken.done;
    expect(simulator.callsTo("send_message")).toHaveLength(2);
    expect(voiceOf(simulator)).toEqual([undefined, { language: "en" }]);
  });
});

describe("voice mode: a sentence waiting for a typed reply to stop", () => {
  it("is dropped when voice mode ends during the wait", async () => {
    const { simulator, voice } = setup();
    const keyboard = await typedReply(simulator);
    await start();
    await voice.utter(HELLO, "en");
    await waitFor(() =>
      expect(simulator.callsTo("cancel_message")).toHaveLength(1),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "End voice mode" }),
    );
    await keyboard.finish();
    await settle();
    expect(simulator.callsTo("send_message")).toHaveLength(1);
    expect(simulator.callsTo("voice_mode_reply_failed")).toEqual([]);
  });

  it("is dropped when the reply is interrupted during the wait", async () => {
    const { simulator, voice } = setup();
    const keyboard = await typedReply(simulator);
    await start();
    await voice.utter(HELLO, "en");
    await waitFor(() =>
      expect(simulator.callsTo("cancel_message")).toHaveLength(1),
    );
    fireEvent.keyDown(document.body, { key: " " });
    await phase("listening");
    await keyboard.finish();
    await settle();
    expect(simulator.callsTo("send_message")).toHaveLength(1);
  });

  it("only the latest sentence is sent", async () => {
    const { simulator, voice } = setup();
    const keyboard = await typedReply(simulator);
    await start();
    await voice.utter("First try.", "en");
    await waitFor(() =>
      expect(simulator.callsTo("cancel_message")).toHaveLength(1),
    );
    await voice.utter(HELLO, "en");
    const latest = simulator.queue(reply(HELLO, false));
    await keyboard.finish();
    await latest.done;
    await settle();
    expect(simulator.callsTo("send_message")).toHaveLength(2);
    expect(simulator.callsTo("voice_mode_reply_failed")).toEqual([]);
  });
});

describe("voice mode follows its page", () => {
  it("opening another chat ends it", async () => {
    const { simulator, app } = setup();
    await start();
    await app.router.navigate({
      to: "/c/$chatId",
      params: { chatId: "chat-2" },
    });
    await waitFor(() =>
      expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1),
    );
    await screen.findByLabelText("Message");
    expect(bar()).toBeNull();
  });

  it("leaving for the settings ends it", async () => {
    const { simulator, app } = setup();
    await start();
    await app.router.navigate({ to: "/settings" });
    await waitFor(() =>
      expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1),
    );
  });

  it("a new chat: the chat created by the first sentence keeps voice mode", async () => {
    const { simulator, voice, app } = setup("/c/new", (backend) => {
      // Sans réflexion : la séquence new-chat-thinking n'envoie pas think.
      backend.details.delete(MODEL);
    });
    let button: HTMLButtonElement | null = null;
    await waitFor(() => {
      button = document.querySelector<HTMLButtonElement>(
        "[data-voice-mode-start]",
      );
      expect(button?.disabled).toBe(false);
    });
    await userEvent.click(button!);
    await phase("listening");
    const replay = simulator.queue("new-chat-thinking");
    await voice.utter(HELLO, "en");
    await replay.done;
    await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
    await waitFor(() => expect(texts("user")).toContain(HELLO));
    expect(bar()).not.toBeNull();
    expect(simulator.callsTo("voice_mode_stop")).toHaveLength(0);
    expect(voiceOf(simulator)).toEqual([{ language: "en" }]);
    // Le tour suivant part de la page de la conversation créée.
    await voice.setState({ phase: "listening" });
    const next = simulator.queue(reply("And goodbye?"));
    await voice.utter("And goodbye?", "en");
    await next.done;
    expect(next.request?.chatId).toBe("chat-1");
  });

  it("a new chat: its draft and files come back on the created chat's page", async () => {
    const { simulator, voice, backend, app } = setup("/c/new", (b) => {
      b.details.delete(MODEL);
      b.files.set("/home/user/notes.txt", { size: 3 });
      b.picks.push(["/home/user/notes.txt"]);
    });
    const field = await screen.findByLabelText<HTMLTextAreaElement>("Message");
    await userEvent.type(field, "My draft");
    await userEvent.click(screen.getByLabelText("Upload multiple files"));
    await waitFor(() =>
      expect(
        screen.getAllByTestId("composer-file").map((chip) => chip.textContent),
      ).toEqual(["notes.txt"]),
    );
    const [staged] = [...backend.staged.keys()];
    let button: HTMLButtonElement | null = null;
    await waitFor(() => {
      button = document.querySelector<HTMLButtonElement>(
        "[data-voice-mode-start]",
      );
      expect(button?.disabled).toBe(false);
    });
    await userEvent.click(button!);
    await phase("listening");
    // Le champ ne doit plus réapparaître avant la fin de la séance (pas de
    // clignotement au changement de page).
    let fieldBack = false;
    const observer = new MutationObserver(() => {
      if (document.querySelector("textarea[aria-label='Message']")) {
        fieldBack = true;
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    const replay = simulator.queue("new-chat-thinking");
    await voice.utter(HELLO, "en");
    await replay.done;
    await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
    await waitFor(() => expect(texts("user")).toContain(HELLO));
    expect(fieldBack).toBe(false);
    observer.disconnect();
    await userEvent.click(
      screen.getByRole("button", { name: "End voice mode" }),
    );
    const back = await screen.findByLabelText<HTMLTextAreaElement>("Message");
    expect(back.value).toBe("My draft");
    expect(
      screen.getAllByTestId("composer-file").map((chip) => chip.textContent),
    ).toEqual(["notes.txt"]);
    // Le fichier n'a jamais quitté la zone de préparation.
    expect(backend.staged.has(staged)).toBe(true);
    expect(simulator.callsTo("unstage")).toEqual([]);
  });

  it("a session no page holds (reloaded page) is stopped", async () => {
    const { simulator } = setup("/c/chat-1", (backend) => {
      backend.voiceMode.state = {
        phase: "listening",
        muted: false,
        turn: 3,
        notice: null,
        editor: false,
      };
    });
    await waitFor(() =>
      expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1),
    );
    expect(bar()).toBeNull();
  });

  it("a session held by VS Code: the button is off and says so, the page never stops it", async () => {
    const { simulator } = setup("/c/chat-1", (backend) => {
      backend.voiceMode.state = {
        phase: "listening",
        muted: false,
        turn: 0,
        notice: null,
        editor: true,
      };
    });
    const button = await screen.findByRole("button", {
      name: "Voice is in use by VS Code",
    });
    expect((button as HTMLButtonElement).disabled).toBe(true);
    expect(button.getAttribute("title")).toBe("Voice is in use by VS Code");
    // Ni le raccourci ni l'affichage d'une autre page ne la lancent ou
    // l'arrêtent.
    await userEvent.keyboard("{Control>}{Shift>} {/Shift}{/Control}");
    await settle();
    expect(simulator.callsTo("voice_mode_start")).toEqual([]);
    expect(simulator.callsTo("voice_mode_stop")).toEqual([]);
    expect(bar()).toBeNull();
  });

  it("editing a message ends voice mode; it can't start during an edit", async () => {
    const { simulator } = setup();
    await start();
    await userEvent.click(
      (await screen.findAllByRole("button", { name: "edit" }))[0],
    );
    expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1);
    const field = await screen.findByLabelText<HTMLTextAreaElement>("Message");
    expect(field.value).toBe("Hi");
    const button = document.querySelector<HTMLButtonElement>(
      "[data-voice-mode-start]",
    )!;
    expect(button.disabled).toBe(true);
  });
});
