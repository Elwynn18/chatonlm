// Envoi d'un message de bout en bout côté frontend : l'app (vraies routes,
// routeur en mémoire) face au simulateur, qui rejoue les séquences
// enregistrées par les tests du moteur Rust.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { ChatEvent } from "@/bindings/ChatEvent";
import { healthKey } from "@/hooks/useServerHealth";
import * as streams from "@/lib/streams";
import { fakeBackend, thinkingModel } from "@/test/backend";
import { composer, renderApp, texts } from "@/test/renderApp";
import { installSimulator, type Sequence, sequence } from "@/test/simulator";

// Parcours complets de l'app (rendu, simulateur, plusieurs requêtes) : sous la
// charge de toute la suite, une étape peut dépasser l'attente par défaut d'1 s.
configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  streams.resetStreams();
});

const HELLO = "Say hello in one word.";

/** Délai des réponses du backend (IPC, SQLite), comme dans l'app réelle. */
const LATENCY_MS = 20;

/**
 * App à `path`, face au simulateur ; `recorded` : séquences déjà en base ;
 * `thinkOff` : qwen3:0.6b sait raisonner (comme dans Ollama 0.35.1) et son
 * bouton Think est mémorisé sur arrêt, comme à l'enregistrement de la
 * séquence existing-chat (think: false).
 */
function setup(
  path: string,
  {
    model = "qwen3:0.6b",
    recorded = [],
    thinkOff = false,
  }: { model?: string; recorded?: string[]; thinkOff?: boolean } = {},
) {
  const simulator = installSimulator({ latencyMs: LATENCY_MS });
  const backend = fakeBackend(simulator, {
    selectedModel: model,
    ...(thinkOff && {
      details: [
        thinkingModel("qwen3:0.6b", { values: [false, true], default: true }),
      ],
      thinkPrefs: { "qwen3:0.6b": false },
    }),
  });
  for (const name of recorded) backend.record(sequence(name));
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** Tape un message et l'envoie (Entrée), une fois le modèle choisi affiché. */
async function send(text: string, model: string) {
  const textarea = await screen.findByLabelText("Message");
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe(model),
  );
  await userEvent.type(textarea, `${text}{Enter}`);
}

/** Attend le bouton Think, mémorisé sur arrêt (avant un envoi : think: false). */
async function thinkIsOff() {
  await screen.findByRole("button", { name: "Think", pressed: false });
}

/** Ouvre la barre latérale : l'historique est alors affiché et relu. */
async function showHistory() {
  const toggle = await screen.findByLabelText(/sidebar/);
  if (toggle.getAttribute("aria-label") === "Show sidebar") {
    await userEvent.click(toggle);
  }
}

/** Le texte a-t-il été affiché, même un instant, depuis l'appel ? */
function watchFor(text: string) {
  let seen = false;
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.textContent?.includes(text)) seen = true;
      }
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
  onTestFinished(() => observer.disconnect());
  return () => seen || document.body.textContent.includes(text);
}

/** Réponse finie : flux retiré, bouton d'envoi revenu. */
async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    expect(streams.pendingNewStream()).toBeNull();
    screen.getByLabelText("Send message");
  });
}

function alertText() {
  return screen.queryByRole("alert")?.textContent ?? null;
}

describe("sending a message", () => {
  it("a new chat opens its page at once, then shows the reply", async () => {
    const { simulator, app } = setup("/c/new");
    const loading = watchFor("Loading chat...");
    await showHistory();
    const replay = simulator.queue("new-chat-thinking", { stepwise: true });

    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    expect(screen.getByLabelText("Stop")).toBeTruthy();

    // Conversation créée : sa page s'ouvre, déjà remplie (pas de get_chat).
    await replay.step();
    await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
    await screen.findByTestId("chat");
    expect(loading()).toBe(false);
    expect(texts("user")).toEqual([HELLO]);
    expect(screen.getByLabelText("Stop")).toBeTruthy();

    // Message enregistré : l'historique est relu.
    await replay.until("userMessage");
    await waitFor(() =>
      expect(
        document.querySelector("[data-chat-id='chat-1']")?.textContent,
      ).toBe(HELLO),
    );

    // Raisonnement en cours, puis la réponse : le bloc se replie.
    await replay.until("thinking");
    const thinking = await screen.findByRole("button", {
      name: "Thinking...",
    });
    expect(thinking.getAttribute("aria-expanded")).toBe("false");
    await replay.until("content");
    await waitFor(() => expect(texts("assistant")).toEqual(['"']));
    expect(thinking.textContent).toMatch(/^Thought for /);
    expect(simulator.callsTo("get_chat")).toEqual([]);

    await replay.finish();
    await replyEnded();
    expect(texts("user")).toEqual([HELLO]);
    expect(texts("assistant")).toEqual(['"Hello"']);
    // À la fin : conversation relue en base, puis le flux retiré.
    expect(simulator.callsTo("get_chat")).toEqual([{ id: "chat-1" }]);
    expect(loading()).toBe(false);
    expect(alertText()).toBeNull();
  });

  it("a new chat sends the Think setting remembered for the model", async () => {
    const { simulator } = setup("/c/new", { thinkOff: true });
    // Séquence enregistrée sans think : la requête attendue porte think: false.
    const recorded = sequence("new-chat-thinking");
    const replay = simulator.queue({
      ...recorded,
      request: { ...recorded.request, think: false },
    });

    await thinkIsOff();
    await send(HELLO, "qwen3:0.6b");

    await replyEnded();
    expect(replay.request?.think).toBe(false);
    expect(texts("assistant")).toEqual(['"Hello"']);
  });

  it("a message in an existing chat is answered on its page", async () => {
    const { simulator, app } = setup("/c/chat-1", {
      recorded: ["new-chat-thinking"],
      thinkOff: true,
    });
    await showHistory();
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hello"']));
    const reads = simulator.callsTo("get_chat").length;
    const historyReads = simulator.callsTo("list_chats").length;
    // Requête comparée à la séquence enregistrée, think: false compris.
    const replay = simulator.queue("existing-chat");

    await thinkIsOff();
    await send(HELLO, "qwen3:0.6b");

    await replyEnded();
    expect(replay.request?.chatId).toBe("chat-1");
    expect(texts("user")).toEqual([HELLO, HELLO]);
    expect(texts("assistant")).toEqual(['"Hello"', '"Hi!"']);
    expect(app.path()).toBe("/c/chat-1");
    expect(simulator.callsTo("get_chat").length).toBeGreaterThan(reads);
    expect(simulator.callsTo("list_chats").length).toBeGreaterThan(
      historyReads,
    );
  });

  it("stopping a reply keeps the partial answer", async () => {
    const { simulator, app } = setup("/c/new", { model: "chatonlm-stall" });
    const replay = simulator.queue("cancelled");

    await send(HELLO, "chatonlm-stall");
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hi']));
    expect(app.path()).toBe("/c/chat-1");
    await userEvent.click(screen.getByLabelText("Stop"));

    await replyEnded();
    expect(simulator.callsTo("cancel_message")).toEqual([
      { streamId: replay.streamId },
    ]);
    // Réponse partielle relue en base, sans erreur.
    expect(texts("assistant")).toEqual(['"Hi']);
    expect(alertText()).toBeNull();
  });

  it("a model error is shown, then cleared by the next message", async () => {
    const { simulator } = setup("/c/new", {
      model: "chatonlm-error",
      thinkOff: true,
    });
    simulator.queue("model-error");

    await send(HELLO, "chatonlm-error");
    await replyEnded();
    expect(texts("assistant")).toEqual(['"Hi']);
    expect(alertText()).toContain("model runner has unexpectedly stopped");

    // Autre modèle, puis un autre message : l'erreur part dès l'envoi.
    await userEvent.click(screen.getByTitle("Select model"));
    await userEvent.click(screen.getByRole("option", { name: "qwen3:0.6b" }));
    const replay = simulator.queue("existing-chat", { stepwise: true });
    await thinkIsOff();
    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    expect(alertText()).toBeNull();

    await replay.finish();
    await replyEnded();
    expect(texts("assistant")).toEqual(['"Hi', '"Hi!"']);
    expect(alertText()).toBeNull();
  });

  it.each([
    ["http-error", "502 Bad Gateway", []],
    ["model-missing", "404 Not Found: model 'chatonlm-vanished' not found", []],
    ["unreachable", "can't connect to Ollama at http://127.0.0.1:11434", []],
    [
      "connection-lost",
      "Connection lost: error decoding response body",
      ['"Hi'],
    ],
  ])("%s: the error is shown under the messages", async (name, text, kept) => {
    const { model } = sequence(name).request;
    const { simulator, app } = setup("/c/new", { model });
    simulator.queue(name);

    await send(HELLO, model);

    await replyEnded();
    expect(app.path()).toBe("/c/chat-1");
    expect(texts("user")).toEqual([HELLO]);
    expect(texts("assistant")).toEqual(kept);
    const alert = within(screen.getByTestId("chat")).getByRole("alert");
    expect(alert.textContent).toBe(`Error${text}`);
  });

  it.each(["unreachable", "connection-lost"])(
    "%s: Ollama is checked again at once",
    async (name) => {
      const { model } = sequence(name).request;
      const { simulator, backend, app } = setup("/c/new", { model });
      const invalidate = vi.spyOn(app.queryClient, "invalidateQueries");
      const replay = simulator.queue(name, { stepwise: true });
      await send(HELLO, model);
      await replay.started;
      await replay.step(replay.remaining - 1);
      await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
      const checks = () => simulator.callsTo("server_status").length;
      const before = checks();
      backend.serverState = "connectionFailed";

      const sentAt = Date.now();
      await replay.finish();

      expect(invalidate).toHaveBeenCalledWith({ queryKey: healthKey });
      await screen.findByText("Ollama isn't responding.");
      expect(checks()).toBeGreaterThan(before);
      const check = simulator.calls.filter(
        (c) => c.command === "server_status",
      )[before];
      // Bien avant la vérification suivante, prévue 3 s après la précédente.
      expect(check.at - sentAt).toBeLessThan(1000);
    },
  );

  it("three dots wait for the reply, then give way to the error", async () => {
    const { simulator } = setup("/c/new");
    const replay = simulator.queue("http-error", { stepwise: true });
    await send(HELLO, "qwen3:0.6b");
    await replay.started;

    await replay.until("userMessage");
    await waitFor(() => expect(texts("user")).toEqual([HELLO]));
    expect(screen.queryByTestId("waiting")).toBeNull();
    // Après 750 ms sans réponse.
    await screen.findByTestId("waiting", undefined, { timeout: 2000 });

    await replay.finish();
    await replyEnded();
    expect(screen.queryByTestId("waiting")).toBeNull();
    expect(alertText()).toBe("Error502 Bad Gateway");
  });

  it("a model error does not check Ollama again", async () => {
    const { simulator, app } = setup("/c/new", { model: "chatonlm-error" });
    const invalidate = vi.spyOn(app.queryClient, "invalidateQueries");
    simulator.queue("model-error");

    await send(HELLO, "chatonlm-error");
    await replyEnded();

    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: healthKey });
  });

  it("a new chat refused before its creation shows the error on the new chat page", async () => {
    const { simulator, app } = setup("/c/new");
    await screen.findByTestId("new-chat");
    // Message fait d'espaces : la zone de saisie ne l'envoie pas, le moteur
    // le refuse.
    simulator.queue("refused-empty");

    await act(() =>
      app.send({ chatId: "new", content: "   ", model: "qwen3:0.6b" }),
    );

    const page = screen.getByTestId("new-chat");
    await waitFor(() =>
      expect(within(page).getByRole("alert").textContent).toBe(
        "Errorthe message is empty",
      ),
    );
    expect(app.path()).toBe("/c/new");
    expect(streams.pendingNewStream()).toBeNull();

    // Refus suivant (erreur simulée) : la précédente part dès l'envoi.
    const refusal: Sequence = {
      ...sequence("refused-empty"),
      description: "The database is locked: the send is refused.",
      error: { kind: "database", message: "database is locked" },
    };
    refusal.request.content = "Hello";
    const replay = simulator.queue(refusal, { stepwise: true });
    await send("Hello", "qwen3:0.6b");
    await replay.started;
    expect(alertText()).toBeNull();

    await replay.finish();
    await waitFor(() =>
      expect(within(page).getByRole("alert").textContent).toBe(
        "Errordatabase is locked",
      ),
    );
    expect(composer().value).toBe("");
    expect(app.path()).toBe("/c/new");
  });

  it("the final event arriving after the command's answer is still applied", async () => {
    const { simulator, backend } = setup("/c/chat-1", {
      recorded: ["new-chat-thinking"],
      thinkOff: true,
    });
    const timeline: string[] = [];
    simulator.onEvent((event) => timeline.push(event.type));
    simulator.handle("get_chat", (args) => {
      timeline.push("get_chat");
      return backend.chats.get(args.id as string);
    });
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hello"']));
    simulator.queue("existing-chat", { lateFinalEvent: true });

    await thinkIsOff();
    await send(HELLO, "qwen3:0.6b");

    await replyEnded();
    expect(texts("assistant")).toEqual(['"Hello"', '"Hi!"']);
    // Relu en base après l'événement final, pas dès la réponse.
    const sending = timeline.slice(timeline.indexOf("userMessage"));
    expect(sending.indexOf("get_chat")).toBeGreaterThan(
      sending.indexOf("done"),
    );
  });
});

/** Modèle absent du serveur, téléchargé au premier envoi. */
const PULL = "chatonlm-pull";
/** Taille totale des événements de téléchargement construits ici (« 4.7 GB »). */
const LAYER = 5_046_586_573;
const OFFLINE =
  "Unable to download model. Please check your internet connection to download the model for offline use.";

function download(completed: number, done = false): ChatEvent {
  return { type: "download", total: LAYER, completed, done };
}

/**
 * Envoi d'une nouvelle conversation vers un modèle absent, construit d'après
 * le contrat du moteur (création, message, progression du téléchargement),
 * suivi de `tail` ; `existing` : message dans la conversation chat-1 déjà
 * enregistrée, sans création.
 */
function pullSequence(tail: ChatEvent[], existing = false): Sequence {
  const base = sequence("cancelled");
  const [created, user] = base.events;
  const request = existing
    ? { ...base.request, chatId: "chat-1", model: "qwen3:0.6b" }
    : { ...base.request, model: PULL };
  if (existing && user.type === "userMessage") user.message.id = 3;
  return {
    description: "The model is downloaded before the reply.",
    request,
    events: [
      ...(existing ? [] : [created]),
      user,
      download(0),
      download(Math.ceil(LAYER / 4)),
      download(Math.ceil(LAYER / 2)),
      ...tail,
    ],
    error: null,
  };
}

const REPLY: ChatEvent[] = [
  download(LAYER, true),
  { type: "replyStarted", messageId: 2, createdAt: 0 },
  { type: "content", text: "Hello" },
  { type: "done", messageId: 2 },
];

/** Bloc de téléchargement affiché, ou null. */
const downloadBlock = () =>
  document.querySelector<HTMLElement>("[data-testid=downloading]");

/** Le modèle porte-t-il l'icône « non téléchargé » dans le sélecteur ? */
async function notDownloaded(model: string) {
  await userEvent.click(screen.getByTitle("Select model"));
  const option = await screen.findByRole("option", { name: model });
  const missing = within(option).queryByLabelText("Not downloaded") !== null;
  await userEvent.keyboard("{Escape}");
  return missing;
}

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

describe("downloading a missing model", () => {
  it("shows the download without dots, fades it out, then the reply comes", async () => {
    const { simulator, app } = setup("/c/new", { model: PULL });
    const replay = simulator.queue(pullSequence(REPLY), { stepwise: true });
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(PULL),
    );
    expect(await notDownloaded(PULL)).toBe(true);

    await send(HELLO, PULL);
    await replay.started;
    await replay.until("download");
    await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
    await waitFor(() =>
      expect(downloadBlock()?.textContent).toBe(
        "Downloading model0.0 GB / 4.7 GB (0%)",
      ),
    );
    expect(screen.getByLabelText("Stop")).toBeTruthy();
    await replay.step(2);
    await waitFor(() =>
      expect(downloadBlock()?.textContent).toContain("2.4 GB / 4.7 GB (50%)"),
    );
    expect(downloadBlock()?.className).toContain("opacity-100");
    // Pas de trois points pendant le téléchargement.
    await delay(1000);
    expect(screen.queryByTestId("waiting")).toBeNull();

    // Fini : le bloc s'efface, liste des modèles et détails relus.
    const listReads = simulator.callsTo("list_models").length;
    const detailReads = () =>
      simulator.callsTo("model_details").filter((a) => a.model === PULL).length;
    const detailsBefore = detailReads();
    await replay.step();
    await waitFor(() =>
      expect(downloadBlock()?.className).toContain("opacity-0 -translate-y-8"),
    );
    expect(downloadBlock()?.textContent).toContain("4.7 GB / 4.7 GB (100%)");
    await waitFor(() => {
      expect(simulator.callsTo("list_models").length).toBeGreaterThan(
        listReads,
      );
      expect(detailReads()).toBeGreaterThan(detailsBefore);
    });
    // Les points reviennent jusqu'à la réponse.
    await screen.findByTestId("waiting", undefined, { timeout: 2000 });

    await replay.finish();
    await replyEnded();
    expect(texts("assistant")).toEqual(["Hello"]);
    expect(downloadBlock()).toBeNull();
    expect(screen.queryByTestId("waiting")).toBeNull();
    expect(alertText()).toBeNull();
    // Installé : plus d'icône de téléchargement.
    expect(await notDownloaded(PULL)).toBe(false);
  }, 30_000);

  it("Stop during the download of a new chat cancels it", async () => {
    const { simulator, app } = setup("/c/new", { model: PULL });
    const replay = simulator.queue(pullSequence([{ type: "cancelled" }]), {
      stepwise: true,
    });
    await send(HELLO, PULL);
    await replay.started;
    // Avant la création : Stop déjà là, sur la page de nouvelle conversation.
    expect(screen.getByLabelText("Stop")).toBeTruthy();
    await replay.step(3);
    await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
    await waitFor(() => expect(downloadBlock()).not.toBeNull());

    await userEvent.click(screen.getByLabelText("Stop"));
    expect(simulator.callsTo("cancel_message")).toEqual([
      { streamId: replay.streamId },
    ]);
    await replay.finish();
    await replyEnded();
    expect(downloadBlock()).toBeNull();
    expect(texts("user")).toEqual([HELLO]);
    expect(texts("assistant")).toEqual([]);
    expect(alertText()).toBeNull();
  }, 30_000);

  it("Stop during the download in an existing chat cancels it", async () => {
    const { simulator } = setup("/c/chat-1", {
      recorded: ["new-chat-thinking"],
    });
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hello"']));
    const replay = simulator.queue(
      pullSequence([{ type: "cancelled" }], true),
      { stepwise: true, ignore: ["think"] },
    );
    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    await replay.until("download");
    await waitFor(() => expect(downloadBlock()).not.toBeNull());

    await userEvent.click(screen.getByLabelText("Stop"));
    expect(simulator.callsTo("cancel_message")).toEqual([
      { streamId: replay.streamId },
    ]);
    await replay.finish();
    await replyEnded();
    expect(downloadBlock()).toBeNull();
    expect(texts("user")).toEqual([HELLO, HELLO]);
    expect(texts("assistant")).toEqual(['"Hello"']);
  }, 30_000);

  // Séquences enregistrées par le moteur sur le faux serveur (cargo test).
  it.each([
    ["download", '"Hi!"', null],
    ["download-cancelled", null, null],
    ["download-offline", null, `Error${OFFLINE}`],
  ])(
    "the engine's %s sequence plays through the app",
    async (name, answer, error) => {
      const recorded = sequence(name);
      const { simulator } = setup("/c/new", { model: recorded.request.model });
      const seen = watchFor("Downloading model");
      simulator.queue(name);
      await send(HELLO, recorded.request.model);
      if (name === "download-cancelled") {
        // L'arrêt rejoué attend le clic sur Stop, pendant le téléchargement.
        await waitFor(() => expect(downloadBlock()).not.toBeNull());
        await userEvent.click(screen.getByLabelText("Stop"));
      }

      await replyEnded();
      expect(seen()).toBe(name !== "download-offline");
      expect(downloadBlock()).toBeNull();
      expect(texts("user")).toEqual([HELLO]);
      expect(texts("assistant")).toEqual(answer === null ? [] : [answer]);
      expect(alertText()).toBe(error);
    },
    30_000,
  );

  it("a failed download shows its error and no block", async () => {
    const { simulator, app } = setup("/c/new", { model: PULL });
    const invalidate = vi.spyOn(app.queryClient, "invalidateQueries");
    simulator.queue(
      pullSequence([{ type: "error", kind: "download", message: OFFLINE }]),
    );
    await send(HELLO, PULL);

    await replyEnded();
    expect(app.path()).toBe("/c/chat-1");
    expect(downloadBlock()).toBeNull();
    expect(texts("assistant")).toEqual([]);
    expect(alertText()).toBe(`Error${OFFLINE}`);
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: healthKey });
  }, 30_000);
});

/**
 * Envoi d'une nouvelle conversation dont le modèle est d'abord chargé en
 * mémoire, construit d'après le contrat du moteur (création, message,
 * loading), suivi de `tail`.
 */
function loadSequence(tail: ChatEvent[]): Sequence {
  const base = sequence("cancelled");
  const [created, user] = base.events;
  return {
    description: "The model is loaded into memory before the reply.",
    request: { ...base.request, model: "qwen3:0.6b" },
    events: [created, user, { type: "loading" }, ...tail],
    error: null,
  };
}

const LOADED_REPLY: ChatEvent[] = [
  { type: "loaded" },
  { type: "replyStarted", messageId: 2, createdAt: 0 },
  { type: "content", text: "Hello" },
  { type: "done", messageId: 2 },
];

const loadingLine = () => screen.queryByTestId("loading-model");

describe("loading the model into memory", () => {
  it("shows Loading model... instead of the dots, then the dots, then the reply", async () => {
    const { simulator } = setup("/c/new");
    const replay = simulator.queue(loadSequence(LOADED_REPLY), {
      stepwise: true,
    });
    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    await replay.until("loading");
    // Après 750 ms, à la place des trois points.
    const line = await screen.findByTestId("loading-model", undefined, {
      timeout: 2000,
    });
    expect(line.textContent).toBe("Loading model...");
    // Dans la région d'annonce, présente depuis l'envoi.
    expect(line.closest("[role=status]")).toBe(
      screen.getByTestId("loading-status"),
    );
    expect(screen.queryByTestId("waiting")).toBeNull();
    expect(screen.getByLabelText("Stop")).toBeTruthy();

    await replay.step();
    await waitFor(() => expect(loadingLine()).toBeNull());
    expect(screen.getByTestId("waiting")).toBeTruthy();

    await replay.finish();
    await replyEnded();
    expect(texts("assistant")).toEqual(["Hello"]);
    expect(loadingLine()).toBeNull();
    // Rien de gardé dans l'historique.
    expect(screen.getByTestId("chat").textContent).not.toContain(
      "Loading model",
    );
  }, 30_000);

  it("shows the loading again when coming back to the chat", async () => {
    const { simulator, app } = setup("/c/new");
    const replay = simulator.queue(loadSequence(LOADED_REPLY), {
      stepwise: true,
    });
    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    await replay.until("loading");
    await screen.findByTestId("loading-model", undefined, { timeout: 2000 });

    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    expect(loadingLine()).toBeNull();
    await act(() =>
      app.router.navigate({
        to: "/c/$chatId",
        params: { chatId: "chat-1" },
      }),
    );
    await screen.findByTestId("loading-model", undefined, { timeout: 2000 });

    await replay.finish();
    await replyEnded();
    expect(texts("assistant")).toEqual(["Hello"]);
    expect(loadingLine()).toBeNull();
  }, 30_000);

  it("Stop during the loading cancels it, without a reply", async () => {
    const { simulator } = setup("/c/new");
    const replay = simulator.queue(loadSequence([{ type: "cancelled" }]), {
      stepwise: true,
    });
    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    await replay.until("loading");
    await screen.findByTestId("loading-model", undefined, { timeout: 2000 });

    await userEvent.click(screen.getByLabelText("Stop"));
    expect(simulator.callsTo("cancel_message")).toEqual([
      { streamId: replay.streamId },
    ]);
    await replay.finish();
    await replyEnded();
    expect(loadingLine()).toBeNull();
    expect(screen.queryByTestId("waiting")).toBeNull();
    expect(texts("user")).toEqual([HELLO]);
    expect(texts("assistant")).toEqual([]);
    expect(alertText()).toBeNull();
  }, 30_000);

  // Séquence enregistrée par le moteur sur le faux serveur (cargo test) :
  // modèle chargé en 1,5 s.
  it("the engine's loading sequence plays through the app", async () => {
    const { model } = sequence("loading").request;
    const { simulator } = setup("/c/new", { model });
    const seen = watchFor("Loading model...");
    const replay = simulator.queue("loading", { stepwise: true });
    await send(HELLO, model);
    await replay.started;
    await replay.until("loading");
    await screen.findByTestId("loading-model", undefined, { timeout: 2000 });

    await replay.until("loaded");
    await waitFor(() => expect(loadingLine()).toBeNull());
    await replay.finish();
    await replyEnded();
    expect(seen()).toBe(true);
    expect(texts("assistant")).toEqual(['"Hello"']);
    expect(loadingLine()).toBeNull();
    expect(alertText()).toBeNull();
  }, 30_000);

  it("a failed load gives way to the error of the reply", async () => {
    const { simulator } = setup("/c/new");
    const MEMORY =
      "model requires more system memory (64.0 GiB) than is available (8.0 GiB)";
    const replay = simulator.queue(
      loadSequence([
        { type: "loaded" },
        { type: "error", kind: "model", message: MEMORY },
      ]),
      { stepwise: true },
    );
    await send(HELLO, "qwen3:0.6b");
    await replay.started;
    await replay.until("loading");
    await screen.findByTestId("loading-model", undefined, { timeout: 2000 });

    await replay.finish();
    await replyEnded();
    expect(loadingLine()).toBeNull();
    expect(alertText()).toBe(`Error${MEMORY}`);
  }, 30_000);
});

describe("sending attached files", () => {
  const VISION = "qwen3.5:0.8b";
  const NOTES = "/home/u/notes.txt";
  const RED = "/home/u/red.png";

  /**
   * Nouvelle conversation avec VISION (capacité vision) ; `picked` : chemins
   * rendus par le sélecteur au clic sur « + ».
   */
  function setupFiles(picked: string[]) {
    const simulator = installSimulator({ latencyMs: LATENCY_MS });
    const backend = fakeBackend(simulator, {
      selectedModel: VISION,
      models: [VISION],
      details: [
        {
          model: VISION,
          capabilities: ["completion", "vision"],
          thinking: null,
          alwaysThinks: false,
        },
      ],
      files: { [NOTES]: { size: 13 }, [RED]: { size: 120 } },
    });
    backend.picks.push(picked);
    const app = renderApp("/c/new");
    return { simulator, backend, app };
  }

  /** Clique sur « + » et attend les pastilles de `names`. */
  async function attach(names: string[]) {
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(VISION),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    await waitFor(() =>
      expect(
        screen.queryAllByTestId("composer-file").map((c) => c.textContent),
      ).toEqual(names),
    );
  }

  it("the files go with the message by their ids, then show in the sent message", async () => {
    const { simulator, backend } = setupFiles([NOTES, RED]);
    // Séquence enregistrée avec think: false (bouton Think de qwen3.5).
    const replay = simulator.queue("attachments", {
      stepwise: true,
      ignore: ["think"],
    });
    await attach(["notes.txt", "red.png"]);
    await userEvent.type(composer(), "Describe these files.{Enter}");
    await replay.started;
    expect(replay.request?.attachmentIds).toEqual(["staged-1", "staged-2"]);
    expect(screen.queryByTestId("composer-files")).toBeNull();

    // Avant l'enregistrement : la pastille et un carré vide pour l'image.
    await replay.step();
    await screen.findByTestId("chat");
    await waitFor(() =>
      expect(texts("user")).toEqual(["notes.txtDescribe these files."]),
    );
    expect(screen.getByTestId("thumbnail-placeholder")).toBeTruthy();

    // Enregistré : l'image vient de la base ; les fichiers ont quitté la zone.
    await replay.until("userMessage");
    const image = await screen.findByRole("img", { name: "red.png" });
    expect(image.getAttribute("src")).toBe("chatonlm://localhost/attachment/2");
    expect(backend.staged.size).toBe(0);

    await replay.finish();
    await replyEnded();
    expect(texts("user")).toEqual(["notes.txtDescribe these files."]);
    expect(screen.getByRole("img", { name: "red.png" })).toBeTruthy();
    expect(simulator.callsTo("unstage")).toEqual([]);
  });

  it("a send refused at once releases its files", async () => {
    const { simulator, backend, app } = setupFiles([NOTES, RED]);
    simulator.queue("refused-unknown-attachment", {
      ignore: ["think", "attachmentIds"],
    });
    await attach(["notes.txt", "red.png"]);
    await userEvent.type(composer(), "Describe these files.{Enter}");

    const page = screen.getByTestId("new-chat");
    await waitFor(() =>
      expect(within(page).getByRole("alert").textContent).toBe(
        "Errorunknown attachment: staged-unknown",
      ),
    );
    // Le composer vidé ne les renverra pas : libérés.
    await waitFor(() =>
      expect(simulator.callsTo("unstage")).toEqual([
        { ids: ["staged-1", "staged-2"] },
      ]),
    );
    expect(backend.staged.size).toBe(0);
    expect(app.path()).toBe("/c/new");
  });
});

describe("editing a message", () => {
  const VISION = "qwen3.5:0.8b";
  const EXTRA = "/home/u/extra.md";

  /** chat-1 enregistrée par la séquence attachments (message 1, réponse 2). */
  function setupEdit() {
    const simulator = installSimulator({ latencyMs: LATENCY_MS });
    const backend = fakeBackend(simulator, {
      selectedModel: VISION,
      models: [VISION],
      details: [
        {
          model: VISION,
          capabilities: ["completion", "vision"],
          thinking: null,
          alwaysThinks: false,
        },
      ],
      files: { [EXTRA]: { size: 18 } },
    });
    backend.record(sequence("attachments"));
    backend.picks.push([EXTRA]);
    const app = renderApp("/c/chat-1");
    return { simulator, backend, app };
  }

  /** Commence l'édition du premier message, change ses pièces jointes et son texte, et l'envoie. */
  async function editFirstMessage() {
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hi!"']));
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(VISION),
    );
    await userEvent.click(screen.getByRole("button", { name: "edit" }));
    expect(composer().value).toBe("Describe these files.");
    const chips = () =>
      screen.queryAllByTestId("composer-file").map((c) => c.textContent);
    expect(chips()).toEqual(["notes.txt", "red.png"]);
    await userEvent.click(
      screen.getByRole("button", { name: "Remove red.png" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    await waitFor(() => expect(chips()).toEqual(["notes.txt", "extra.md"]));
    await userEvent.clear(composer());
    await userEvent.type(composer(), "Describe this file instead.{Enter}");
  }

  it("replaces the message and the following ones, as the engine recorded it", async () => {
    const { simulator, backend } = setupEdit();
    // Séquence enregistrée avec think: false (bouton Think de qwen3.5).
    const replay = simulator.queue("edit", {
      stepwise: true,
      ignore: ["think"],
    });
    await editFirstMessage();
    await replay.started;
    // editMessageId, keptAttachmentIds et attachmentIds comparés par le
    // simulateur ; la réponse d'avant a déjà disparu.
    expect(texts("assistant")).toEqual([]);
    expect(texts("user")).toEqual([
      "notes.txtextra.mdDescribe this file instead.",
    ]);

    await replay.finish();
    await replyEnded();
    expect(texts("user")).toEqual([
      "notes.txtextra.mdDescribe this file instead.",
    ]);
    expect(texts("assistant")).toEqual(['"Hi!"']);
    expect(screen.queryByRole("img", { name: "red.png" })).toBeNull();
    // Base simulée : le message modifié et la nouvelle réponse seulement.
    expect(backend.chats.get("chat-1")?.messages.map((m) => m.id)).toEqual([
      1, 3,
    ]);
    expect(backend.staged.size).toBe(0);
  });

  it("a refused edit leaves the chat as it was and releases the new file", async () => {
    const { simulator, backend } = setupEdit();
    simulator.queue("refused-edit-unknown-message", {
      ignore: ["think", "editMessageId", "keptAttachmentIds", "attachmentIds"],
    });
    await editFirstMessage();
    await waitFor(() => expect(alertText()).toBe("Errorunknown message: 99"));
    expect(texts("user")).toEqual(["notes.txtDescribe these files."]);
    expect(texts("assistant")).toEqual(['"Hi!"']);
    await waitFor(() =>
      expect(simulator.callsTo("unstage")).toEqual([{ ids: ["staged-1"] }]),
    );
    expect(backend.staged.size).toBe(0);
  });
});
