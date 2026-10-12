// Recherche web de bout en bout côté frontend : l'app face au simulateur, qui
// rejoue les séquences web-search*.json enregistrées par les tests du moteur
// Rust contre le faux serveur (modèle chatonlm-search).

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { ServerStatus } from "@/bindings/ServerStatus";
import { healthKey } from "@/hooks/useServerHealth";
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

afterEach(() => {
  streams.resetStreams();
});

const MODEL = "chatonlm-search";

/** Ollama ne répond plus (vérification de santé de la session). */
const UNREACHABLE: ServerStatus = {
  url: "http://127.0.0.1:11434",
  state: "notResponding",
  version: null,
  alternativeUrl: null,
  hostEnv: null,
  httpStatus: null,
  detail: null,
  service: null,
  builtin: false,
  builtinAvailable: false,
  explicitAddress: false,
  logPath: null,
};
const LATEST = "What is the latest Ollama release?";

function setup({
  signedIn = true,
  path = "/c/new",
  recorded = [] as string[],
} = {}) {
  const simulator = installSimulator({ latencyMs: 20 });
  const backend = fakeBackend(simulator, {
    selectedModel: MODEL,
    details: [
      {
        model: MODEL,
        capabilities: ["completion", "tools"],
        thinking: null,
        alwaysThinks: false,
      },
    ],
  });
  backend.settings.webSearchEnabled = true;
  if (signedIn) {
    backend.account = {
      state: "signedIn",
      name: "ada",
      email: "ada@example.com",
      plan: "free",
      avatar: null,
    };
  }
  for (const name of recorded) backend.record(sequence(name));
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** Tape et envoie, une fois le modèle et la recherche web prêts. */
async function send(text: string) {
  const textarea = await screen.findByLabelText("Message");
  await screen.findByRole("button", { name: "Web search", pressed: true });
  await userEvent.type(textarea, `${text}{Enter}`);
}

const callLines = () =>
  [...document.querySelectorAll("[data-role=tool-calls] > div")].map(
    (line) => line.textContent,
  );
const resultBlocks = () => [
  ...document.querySelectorAll<HTMLElement>("[data-role=tool]"),
];
const copyButtons = () =>
  screen
    .queryAllByRole("button", { name: "Copy" })
    .filter((button) => button.title === "Copy");

/** Réponse finie : flux retiré, bouton d'envoi revenu. */
async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    expect(streams.pendingNewStream()).toBeNull();
    screen.getByLabelText("Send message");
  });
}

describe("web search", () => {
  it("shows the call, its folded result, the dots, then the next answer", async () => {
    const { simulator } = setup();
    const replay = simulator.queue("web-search", { stepwise: true });
    await send(LATEST);
    await replay.started;

    await replay.until("toolCalls");
    await waitFor(() =>
      expect(callLines()).toEqual([`Searching for ${LATEST}…`]),
    );
    // Raisonnement du premier tour clos avec ses appels.
    expect(screen.getByText("Thought for a moment")).toBeTruthy();
    expect(resultBlocks()).toEqual([]);

    await replay.until("toolResult");
    await waitFor(() => expect(resultBlocks()).toHaveLength(1));
    const header = within(resultBlocks()[0]).getByRole("button");
    expect(header.textContent).toBe(`Search results for ${LATEST}`);
    expect(header.getAttribute("aria-expanded")).toBe("false");
    // Entre le résultat et le tour suivant : les points.
    await screen.findByTestId("waiting");

    await replay.finish();
    await replyEnded();
    expect(screen.queryByTestId("waiting")).toBeNull();
    // Relu en base : appel, résultat, réponse finale avec son bouton copier.
    expect(callLines()).toEqual([`Searching for ${LATEST}…`]);
    expect(resultBlocks()).toHaveLength(1);
    const [answer] = texts("assistant");
    expect(answer).toMatch(/^Found: \{"results":/);
    expect(texts("assistant")).toHaveLength(1);
    expect(copyButtons()).toHaveLength(1);

    await userEvent.click(within(resultBlocks()[0]).getByRole("button"));
    expect(resultBlocks()[0].querySelector("pre")?.textContent).toContain(
      `"title": "Result 1 for ${LATEST}"`,
    );
  });

  it("Stop during the tool keeps the call, without result nor dots", async () => {
    const { simulator } = setup();
    const replay = simulator.queue("web-search-cancelled", { stepwise: true });
    const text = "Search slowly for the latest Ollama release.";
    await send(text);
    await replay.started;
    await replay.until("toolCalls");
    await waitFor(() =>
      expect(callLines()).toEqual([`Searching for ${text}…`]),
    );

    await userEvent.click(screen.getByLabelText("Stop"));
    await waitFor(() => expect(replay.cancelRequested).toBe(true));
    await replay.finish();
    await replyEnded();
    expect(callLines()).toEqual([`Searching for ${text}…`]);
    expect(resultBlocks()).toEqual([]);
    expect(screen.queryByTestId("waiting")).toBeNull();
    expect(texts("assistant")).toEqual([]);
  });
});

const SIGNED_OUT_ERROR = "Web search requires an Ollama account";
const GENERATE = "Generate without web search";

/** Bloc d'erreur de la conversation. */
const errorBlock = () => screen.getByRole("alert");

/**
 * Relance du message 1 de « chat-1 » sans recherche web, telle que le moteur
 * l'enregistrerait : message réécrit (même id), puis une nouvelle réponse.
 */
function retrySequence(replyId: number, webSearch = false): Sequence {
  return {
    description: "Retry of the message (built by the test).",
    request: {
      chatId: "chat-1",
      streamId: "stream-1",
      content: LATEST,
      model: MODEL,
      editMessageId: 1,
      ...(webSearch && { webSearch }),
    },
    events: [
      {
        type: "userMessage",
        message: {
          id: 1,
          role: "user",
          content: LATEST,
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
      { type: "replyStarted", messageId: replyId, createdAt: SEQUENCE_TIME },
      { type: "content", text: "Ollama 0.35 is the latest release." },
      { type: "done", messageId: replyId },
    ],
    error: null,
  };
}

describe("web search without an account", () => {
  it("stops with the error, Sign In and Generate without web search", async () => {
    const { simulator, backend } = setup({ signedIn: false });
    simulator.queue("web-search-signed-out");
    await send(LATEST);
    await replyEnded();
    const alert = errorBlock();
    expect(within(alert).getByText("Error")).toBeTruthy();
    expect(within(alert).getByText(SIGNED_OUT_ERROR)).toBeTruthy();
    expect(within(alert).getByRole("button", { name: "Sign In" })).toBeTruthy();
    expect(within(alert).getByRole("button", { name: GENERATE })).toBeTruthy();
    // Aucune réponse de l'assistant ; le message envoyé reste.
    expect(texts("assistant")).toEqual([]);
    expect(texts("user")).toEqual([LATEST]);
    // Rien d'ouvert dans le navigateur.
    expect(simulator.callsTo("account_sign_in")).toEqual([]);
    expect(backend.settings.webSearchEnabled).toBe(true);
  });

  it("Sign In opens the account card of the settings, its Sign In focused", async () => {
    const { simulator, app } = setup({ signedIn: false });
    simulator.queue("web-search-signed-out");
    await send(LATEST);
    await replyEnded();
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: "Sign In" }),
    );
    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(app.router.state.location.search).toEqual({ section: "account" });
    const card = await screen.findByRole("region", { name: "Ollama account" });
    const signIn = await within(card).findByRole("button", { name: "Sign In" });
    await waitFor(() => expect(document.activeElement).toBe(signIn));
    // Rien ne s'ouvre tout seul.
    expect(simulator.callsTo("account_sign_in")).toEqual([]);
  });

  it("signed in meanwhile: the card itself takes the focus", async () => {
    const { simulator, backend } = setup({ signedIn: false });
    simulator.queue("web-search-signed-out");
    await send(LATEST);
    await replyEnded();
    // Relié au terminal entre-temps ; l'erreur a fait relire le compte.
    backend.account = {
      state: "signedIn",
      name: "ada",
      email: "ada@example.com",
      plan: "free",
      avatar: null,
    };
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: "Sign In" }),
    );
    const card = await screen.findByRole("region", { name: "Ollama account" });
    await within(card).findByText("ada");
    await waitFor(() => expect(document.activeElement).toBe(card));
  });

  it("Generate without web search turns it off and sends the message again", async () => {
    const { simulator, backend } = setup({ signedIn: false });
    simulator.queue("web-search-signed-out");
    await send(LATEST);
    await replyEnded();
    const textarea = screen.getByLabelText<HTMLTextAreaElement>("Message");
    await userEvent.type(textarea, "Draft");

    simulator.queue(retrySequence(2));
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: GENERATE }),
    );
    await replyEnded();
    await waitFor(() =>
      expect(texts("assistant")).toEqual([
        "Ollama 0.35 is the latest release.",
      ]),
    );
    // Rien de dupliqué ; erreur partie ; recherche web coupée ; brouillon
    // gardé.
    expect(texts("user")).toEqual([LATEST]);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(backend.settings.webSearchEnabled).toBe(false);
    await screen.findByRole("button", { name: "Web search", pressed: false });
    expect(textarea.value).toBe("Draft");
    // Le bouton cliqué est parti : le focus est revenu à la zone de saisie.
    expect(document.activeElement).toBe(textarea);
    const [request] = simulator.callsTo("send_message").slice(-1);
    expect(request.request).not.toHaveProperty("webSearch");
  });

  it("signed in from the settings: back in the chat, Retry searches again", async () => {
    const { simulator, backend, app } = setup({ signedIn: false });
    simulator.queue("web-search-signed-out");
    await send(LATEST);
    await replyEnded();
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: "Sign In" }),
    );
    await waitFor(() => expect(app.path()).toBe("/settings"));
    // Connexion faite dans le navigateur, relue par la carte.
    backend.account = {
      state: "signedIn",
      name: "ada",
      email: "ada@example.com",
      plan: "free",
      avatar: null,
    };
    const card = await screen.findByRole("region", { name: "Ollama account" });
    await userEvent.click(
      await within(card).findByRole("button", { name: "Sign In" }),
    );
    await within(card).findByText("ada");

    await act(() =>
      app.router.navigate({ to: "/c/$chatId", params: { chatId: "chat-1" } }),
    );
    const alert = await screen.findByRole("alert");
    // Même texte ; Retry à la place de Sign In.
    expect(within(alert).getByText(SIGNED_OUT_ERROR)).toBeTruthy();
    expect(within(alert).queryByRole("button", { name: "Sign In" })).toBeNull();
    expect(within(alert).getByRole("button", { name: GENERATE })).toBeTruthy();

    simulator.queue(retrySequence(2, true));
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await replyEnded();
    await waitFor(() =>
      expect(texts("assistant")).toEqual([
        "Ollama 0.35 is the latest release.",
      ]),
    );
    expect(texts("user")).toEqual([LATEST]);
    expect(backend.settings.webSearchEnabled).toBe(true);
    expect(document.activeElement).toBe(screen.getByLabelText("Message"));
  });

  it("a chat without that error never reads the account", async () => {
    const { simulator } = setup({
      signedIn: false,
      path: "/c/chat-1",
      recorded: ["web-search"],
    });
    await screen.findByRole("button", { name: "Web search", pressed: true });
    await waitFor(() =>
      expect(callLines()).toEqual([`Searching for ${LATEST}…`]),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(simulator.callsTo("account_status")).toEqual([]);
  });

  it("refused late (by the tool): the call stays alone, the retry replaces the reply", async () => {
    const { simulator } = setup();
    simulator.queue("web-search-signed-out-late");
    await send(LATEST);
    await replyEnded();
    expect(callLines()).toEqual([`Searching for ${LATEST}…`]);
    expect(resultBlocks()).toEqual([]);
    expect(within(errorBlock()).getByText(SIGNED_OUT_ERROR)).toBeTruthy();

    simulator.queue(retrySequence(3));
    await userEvent.click(
      within(errorBlock()).getByRole("button", { name: GENERATE }),
    );
    await replyEnded();
    await waitFor(() =>
      expect(texts("assistant")).toEqual([
        "Ollama 0.35 is the latest release.",
      ]),
    );
    // La réponse interrompue (appel sans résultat) a été remplacée.
    expect(callLines()).toEqual([]);
    expect(texts("user")).toEqual([LATEST]);
  });

  it("Generate without web search waits for Ollama to answer again", async () => {
    const { simulator, app } = setup({ signedIn: false });
    simulator.queue("web-search-signed-out");
    await send(LATEST);
    await replyEnded();
    const generate = within(errorBlock()).getByRole<HTMLButtonElement>(
      "button",
      { name: GENERATE },
    );
    expect(generate.disabled).toBe(false);
    act(() =>
      app.queryClient.setQueryData(healthKey, {
        ...UNREACHABLE,
      }),
    );
    await waitFor(() => expect(generate.disabled).toBe(true));
  });

  it("on the new chat page (no chat created), only Sign In is offered", async () => {
    const { simulator } = setup({ signedIn: false });
    simulator.queue({
      description: "Refused before the chat is created (built by the test).",
      request: {
        streamId: "stream-1",
        content: LATEST,
        model: MODEL,
        webSearch: true,
      },
      events: [
        {
          type: "error",
          kind: "webSearchSignedOut",
          message: SIGNED_OUT_ERROR,
        },
      ],
      error: null,
    });
    await send(LATEST);
    await waitFor(() => expect(streams.pendingNewStream()).toBeNull());
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(SIGNED_OUT_ERROR)).toBeTruthy();
    expect(within(alert).getByRole("button", { name: "Sign In" })).toBeTruthy();
    expect(within(alert).queryByRole("button", { name: GENERATE })).toBeNull();
    expect(screen.getByTestId("new-chat")).toBeTruthy();
  });
});
