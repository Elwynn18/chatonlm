// Conversations éphémères (propres à ChatonLM) de bout en bout côté frontend :
// l'app (vraies routes) face au simulateur, qui rejoue la séquence
// « new-chat-ephemeral » enregistrée par le moteur Rust.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { newChatIsEphemeral, resetEphemeral } from "@/lib/ephemeral";
import * as streams from "@/lib/streams";
import { fakeBackend } from "@/test/backend";
import { renderApp, texts } from "@/test/renderApp";
import { installSimulator, sequence } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
// Parcours complets (rendu, simulateur, plusieurs pages).
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  streams.resetStreams();
  resetEphemeral();
});

const HELLO = "Say hello in one word.";

function setup(path: string, recorded: string[] = []) {
  const simulator = installSimulator({ latencyMs: 20 });
  const backend = fakeBackend(simulator, { selectedModel: "qwen3:0.6b" });
  for (const name of recorded) backend.record(sequence(name));
  const app = renderApp(path);
  return { simulator, backend, app };
}

async function send(text: string) {
  const textarea = await screen.findByLabelText("Message");
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe("qwen3:0.6b"),
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

async function showHistory() {
  const toggle = await screen.findByLabelText(/sidebar/);
  if (toggle.getAttribute("aria-label") === "Show sidebar") {
    await userEvent.click(toggle);
  }
}

function ghost() {
  return screen.getByTestId("ephemeral-button");
}

function inHistory(id: string) {
  return document.querySelector(`[data-chat-id='${id}']`) !== null;
}

/** Envoie le premier message d'une nouvelle conversation éphémère. */
async function startEphemeral(
  simulator: ReturnType<typeof installSimulator>,
  app: ReturnType<typeof renderApp>,
) {
  await showHistory();
  await userEvent.click(
    await screen.findByRole("button", {
      name: "Ephemeral chat",
      pressed: false,
    }),
  );
  expect(ghost().getAttribute("aria-pressed")).toBe("true");
  expect(ghost().getAttribute("aria-label")).toBe("Ephemeral chat");
  expect(ghost().getAttribute("title")).toBe("Turn off ephemeral chat");
  expect(screen.getByTestId("ephemeral-banner").textContent).toContain(
    "won't appear in your history",
  );
  const replay = simulator.queue("new-chat-ephemeral");
  await send(HELLO);
  await replyEnded();
  await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
  expect(replay.request?.ephemeral).toBe(true);
  expect(texts("assistant")).toEqual(['"Hello"']);
}

async function open(app: ReturnType<typeof renderApp>, chatId: string) {
  await act(() =>
    app.router.navigate({ to: "/c/$chatId", params: { chatId } }),
  );
}

describe("ephemeral chats", () => {
  it("the ghost kitten makes the new chat ephemeral, kept out of the history and erased once another chat is opened", async () => {
    const { simulator, backend, app } = setup("/c/new");
    await startEphemeral(simulator, app);

    expect(backend.chats.get("chat-1")?.ephemeral).toBe(true);
    // Hors de l'historique, avis et Keep au-dessus de la zone de saisie.
    // Historique relu à la fin de la réponse (attendu par replyEnded).
    expect(simulator.callsTo("list_chats").length).toBeGreaterThan(1);
    expect(inHistory("chat-1")).toBe(false);
    expect(
      (await screen.findByTestId("ephemeral-banner")).textContent,
    ).toContain("Keep");
    // Bouton d'action en aplat roux, plus une bascule.
    expect(ghost().getAttribute("aria-pressed")).toBeNull();
    expect(ghost().className).toContain("bg-accent");
    expect(ghost().getAttribute("aria-label")).toBe("Keep this chat");
    // Le bouton de la page de nouvelle conversation est coupé.
    expect(newChatIsEphemeral()).toBe(false);

    // Réglages puis retour : rien n'est effacé.
    await act(() => app.router.navigate({ to: "/settings" }));
    await open(app, "chat-1");
    await screen.findByTestId("ephemeral-banner");
    expect(simulator.callsTo("forget_chat")).toEqual([]);

    // Nouvelle conversation : l'éphémère est effacée, la nouvelle ordinaire.
    await open(app, "new");
    await waitFor(() =>
      expect(simulator.callsTo("forget_chat")).toEqual([{ id: "chat-1" }]),
    );
    expect(backend.chats.has("chat-1")).toBe(false);
    expect(screen.queryByTestId("ephemeral-banner")).toBeNull();
    expect(ghost().getAttribute("aria-pressed")).toBe("false");
  });

  it("Keep puts the ephemeral chat in the history, and leaving it no longer erases it", async () => {
    const { simulator, backend, app } = setup("/c/new");
    await startEphemeral(simulator, app);

    const banner = await screen.findByTestId("ephemeral-banner");
    await userEvent.click(within(banner).getByRole("button", { name: "Keep" }));
    await waitFor(() => expect(inHistory("chat-1")).toBe(true));
    expect(simulator.callsTo("keep_chat")).toEqual([{ id: "chat-1" }]);
    expect(screen.queryByTestId("ephemeral-banner")).toBeNull();
    // Le bouton du bandeau a disparu : le focus va à la zone de saisie.
    expect(document.activeElement).toBe(screen.getByLabelText("Message"));
    expect(ghost().getAttribute("aria-pressed")).toBeNull();
    expect(ghost().className).not.toContain("bg-accent");
    expect(ghost().getAttribute("aria-label")).toBe("New ephemeral chat");

    await open(app, "new");
    await screen.findByTestId("new-chat");
    expect(simulator.callsTo("forget_chat")).toEqual([]);
    expect(backend.chats.get("chat-1")?.ephemeral).toBeUndefined();
  });

  it("the ghost kitten of an ephemeral chat keeps it too, once on a double click", async () => {
    const { simulator, app } = setup("/c/new");
    await startEphemeral(simulator, app);
    await userEvent.dblClick(
      screen.getByRole("button", { name: "Keep this chat" }),
    );
    await waitFor(() => expect(inHistory("chat-1")).toBe(true));
    expect(simulator.callsTo("keep_chat")).toEqual([{ id: "chat-1" }]);
    // Le second clic n'ouvre pas une nouvelle conversation éphémère.
    expect(app.path()).toBe("/c/chat-1");
    expect(newChatIsEphemeral()).toBe(false);
  });

  it("an ephemeral chat read again (page reloaded) is still erased once left", async () => {
    const { simulator, backend, app } = setup("/c/chat-1", [
      "new-chat-ephemeral",
    ]);
    expect(backend.chats.get("chat-1")?.ephemeral).toBe(true);
    await screen.findByTestId("ephemeral-banner");
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hello"']));

    await open(app, "new");
    await waitFor(() =>
      expect(simulator.callsTo("forget_chat")).toEqual([{ id: "chat-1" }]),
    );
    expect(backend.chats.has("chat-1")).toBe(false);
  });

  it("in an ordinary chat, the ghost kitten opens a new ephemeral chat", async () => {
    const { simulator, app } = setup("/c/chat-1", ["new-chat-thinking"]);
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hello"']));
    expect(screen.queryByTestId("ephemeral-banner")).toBeNull();

    await userEvent.click(
      screen.getByRole("button", { name: "New ephemeral chat" }),
    );
    await waitFor(() => expect(app.path()).toBe("/c/new"));
    expect(ghost().getAttribute("aria-pressed")).toBe("true");
    await screen.findByTestId("ephemeral-banner");
    // Une conversation ordinaire n'est jamais effacée.
    expect(simulator.callsTo("forget_chat")).toEqual([]);
  });

  it("Ctrl+Shift+N opens a new ephemeral chat", async () => {
    const { app } = setup("/c/chat-1", ["new-chat-thinking"]);
    await waitFor(() => expect(texts("assistant")).toEqual(['"Hello"']));

    await userEvent.keyboard("{Control>}{Shift>}N{/Shift}{/Control}");
    await waitFor(() => expect(app.path()).toBe("/c/new"));
    expect(ghost().getAttribute("aria-pressed")).toBe("true");
    await screen.findByTestId("ephemeral-banner");
  });

  it("turned off again, the ghost kitten leaves the new chat ordinary", async () => {
    const { simulator, app } = setup("/c/new");
    const button = await screen.findByRole("button", {
      name: "Ephemeral chat",
      pressed: false,
    });
    await userEvent.click(button);
    await userEvent.click(
      screen.getByRole("button", { name: "Ephemeral chat", pressed: true }),
    );
    expect(screen.queryByTestId("ephemeral-banner")).toBeNull();
    // La séquence ordinaire n'a pas de champ ephemeral : un envoi éphémère
    // serait refusé par le simulateur.
    const replay = simulator.queue("new-chat-thinking");
    await send(HELLO);
    await replyEnded();
    await waitFor(() => expect(app.path()).toBe("/c/chat-1"));
    expect(replay.request?.ephemeral).toBeUndefined();
  });
});
