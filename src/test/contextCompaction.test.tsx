// Jauge du contexte et compactage (propres à ChatonLM) : l'app face au
// simulateur. Séquences construites par les essais (les séquences
// enregistrées par le moteur viendront avec lui) : compactage automatique
// dans un envoi, réponse coupée à 100 % du contexte.

import {
  act,
  cleanup,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppError } from "@/bindings/AppError";
import type { Compaction } from "@/bindings/Compaction";
import type { ContextUsage } from "@/bindings/ContextUsage";
import type { Message } from "@/bindings/Message";
import type { ServerStatus } from "@/bindings/ServerStatus";
import { resetCompactions } from "@/hooks/useCompactChat";
import { healthKey } from "@/hooks/useServerHealth";
import * as streams from "@/lib/streams";
import { fakeBackend } from "@/test/backend";
import { PARAKEET } from "@/test/dictation";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp, texts } from "@/test/renderApp";
import {
  installSimulator,
  SEQUENCE_TIME,
  type Sequence,
} from "@/test/simulator";
import { VOICE_EN } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  streams.resetStreams();
  resetCompactions();
});

const MODEL = "qwen3:0.6b";
const SIZE = 32_768;
const SUMMARY = "The user asked for a greeting.\nThe assistant said hi.";
const CONTEXT_FULL =
  "The conversation filled the model's context, so the reply was cut. Compact the conversation to continue.";
const AUTO_HINT =
  "When the conversation passes 80%, older messages are summarized automatically.";

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

function message(
  id: number,
  role: "user" | "assistant",
  content: string,
): Message {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? MODEL : null,
    createdAt: SEQUENCE_TIME,
    updatedAt: SEQUENCE_TIME,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

function compaction(fields: Partial<Compaction> = {}): Compaction {
  return {
    id: 1,
    throughMessageId: 2,
    summary: SUMMARY,
    automatic: false,
    createdAt: SEQUENCE_TIME,
    ...fields,
  };
}

/**
 * Conversation « chat-1 » ouverte : un échange, contexte `context`
 * (défaut : 12 500 jetons sur 32 768), compactages `compactions`.
 */
function setup({
  context = { used: 12_500, size: SIZE } as ContextUsage | null,
  compactions = [] as Compaction[],
  language = "en" as "en" | "fr",
  path = "/c/chat-1",
} = {}) {
  const simulator = installSimulator({ latencyMs: 20 });
  const backend = fakeBackend(simulator, { selectedModel: MODEL });
  backend.compaction.summary = SUMMARY;
  backend.settings.language = language;
  backend.settings.effectiveLanguage = language;
  backend.chats.set("chat-1", {
    id: "chat-1",
    title: "Greetings",
    createdAt: SEQUENCE_TIME,
    updatedAt: SEQUENCE_TIME,
    messages: [message(1, "user", "Hello"), message(2, "assistant", "Hi!")],
    context,
    compactions,
  });
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** Bouton de la jauge, une fois la conversation lue. */
function gauge(percent: string) {
  return screen.findByRole("button", { name: `Context: ${percent} used` });
}

async function openPanel(percent = "38%") {
  await userEvent.click(await gauge(percent));
  return screen.getByRole("dialog", { name: "Context" });
}

function compactButton(panel: HTMLElement) {
  return within(panel).getByRole<HTMLButtonElement>("button", {
    name: /^(Compact now|Compacting…)$/,
  });
}

/** Laisse passer les effets différés (focus rendu à la fin d'une réponse). */
async function settle() {
  await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
}

/** Réponse finie : flux retiré, bouton d'envoi revenu. */
async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    screen.getByLabelText("Send message");
  });
}

async function send(text: string) {
  const textarea = await screen.findByLabelText("Message");
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe(MODEL),
  );
  await userEvent.type(textarea, `${text}{Enter}`);
}

function setHealth(app: ReturnType<typeof setup>["app"], status: ServerStatus) {
  act(() => app.queryClient.setQueryData(healthKey, status));
}

describe("context gauge", () => {
  it("shows the saved context; its panel gives the numbers and the rule", async () => {
    setup();
    const button = await gauge("38%");
    expect(button.getAttribute("aria-haspopup")).toBe("dialog");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    // Secteur de 38 % (pas de disque plein).
    expect(button.querySelector("[data-testid=context-sector]")).not.toBeNull();

    await userEvent.click(button);
    expect(button.getAttribute("aria-expanded")).toBe("true");
    const panel = screen.getByRole("dialog", { name: "Context" });
    expect(panel.textContent).toContain("12,500 / 32,768 tokens (38%)");
    expect(panel.textContent).toContain(AUTO_HINT);
    expect(compactButton(panel).disabled).toBe(false);
    expect(compactButton(panel).textContent).toBe("Compact now");

    // Échap ferme, le focus revient au bouton, sans arrêter quoi que ce soit.
    expect(panel.contains(document.activeElement)).toBe(true);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(button);
    expect(button.getAttribute("aria-expanded")).toBe("false");

    await userEvent.click(button);
    screen.getByRole("dialog", { name: "Context" });
    // Un clic dehors ferme aussi, même sur un texte qui ne prend pas le
    // focus.
    await userEvent.click(screen.getByText("Hi!"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("is written the French way in French", async () => {
    setup({ language: "fr" });
    const button = await screen.findByRole("button", {
      name: /^Contexte\s:\s38\s% utilisé$/u,
    });
    await userEvent.click(button);
    const panel = screen.getByRole("dialog", { name: "Contexte" });
    expect(panel.textContent).toMatch(/12\s500 \/ 32\s768 tokens \(38\s%\)/u);
    expect(panel.textContent).toMatch(/dépasse 80\s%,/u);
  });

  it("draws a full disk from 99.9%, and rounds the percentage down", async () => {
    setup({ context: { used: 32_760, size: SIZE } });
    const button = await gauge("99%");
    expect(button.querySelector("[data-testid=context-sector]")).toBeNull();
    expect(
      button.querySelector("[data-testid=context-full-disk]"),
    ).not.toBeNull();
  });

  it("is bounded at 100% when the estimate passes the size", async () => {
    setup({ context: { used: 40_000, size: SIZE } });
    const button = await gauge("100%");
    // Disque plein, pas de secteur.
    expect(button.querySelector("[data-testid=context-sector]")).toBeNull();
    await userEvent.click(button);
    expect(
      screen.getByRole("dialog", { name: "Context" }).textContent,
    ).toContain("40,000 / 32,768 tokens (100%)");
  });

  it("is hidden when the context is unknown", async () => {
    setup({ context: null });
    await screen.findByText("Hi!");
    expect(screen.queryByRole("button", { name: /^Context:/ })).toBeNull();
  });

  it("is never on the new chat page", async () => {
    setup({ path: "/c/new" });
    await screen.findByLabelText("Message");
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(MODEL),
    );
    expect(screen.queryByRole("button", { name: /^Context:/ })).toBeNull();
  });

  it("has every text translated", async () => {
    setup({ compactions: [compaction()] });
    usePseudoLanguage();
    const button = await screen.findByRole("button", { name: /Context: / });
    await userEvent.click(button);
    const panel = screen.getByRole("dialog");
    expect(
      untranslatedTexts(panel, ["12,500", "32,768", "38%", "80%"]),
    ).toEqual([]);
    const separator = screen.getByTestId("compaction");
    await userEvent.click(within(separator).getByRole("button"));
    expect(untranslatedTexts(separator, [SUMMARY])).toEqual([]);
  });
});

describe("Compact now", () => {
  it("compacts with the composer's model, then shows the separator and the lower gauge", async () => {
    const { simulator } = setup();
    const panel = await openPanel();
    await userEvent.click(compactButton(panel));
    expect(simulator.callsTo("compact_chat")).toEqual([
      { chatId: "chat-1", model: MODEL },
    ]);
    // Conversation relue : séparateur après le dernier message, jauge basse
    // (200 jetons), plus rien à compacter.
    const separator = await screen.findByTestId("compaction");
    const toggle = within(separator).getByRole("button", {
      name: "Context compacted",
    });
    expect(
      separator
        .closest("[data-message-index]")
        ?.getAttribute("data-message-index"),
    ).toBe("1");
    await gauge("0%");
    await waitFor(() => expect(compactButton(panel).disabled).toBe(true));
    expect(compactButton(panel).textContent).toBe("Compact now");

    // Le séparateur déplie le résumé envoyé au modèle.
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByTestId("compaction-summary")).toBeNull();
    await userEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByTestId("compaction-summary").textContent).toBe(SUMMARY);
    // Tout l'historique reste affiché.
    expect(texts("user")).toEqual(["Hello"]);
    expect(texts("assistant")).toEqual(["Hi!"]);
  });

  it("says Compacting… while it runs, and shows its failure in red", async () => {
    const { simulator } = setup();
    let fail: (error: AppError) => void = () => {};
    simulator.handle(
      "compact_chat",
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    const panel = await openPanel();
    await userEvent.click(compactButton(panel));
    await waitFor(() =>
      expect(compactButton(panel).textContent).toBe("Compacting…"),
    );
    expect(compactButton(panel).disabled).toBe(true);
    // Le focus passe au panneau (le bouton est désactivé).
    expect(document.activeElement).toBe(panel);

    act(() => fail({ kind: "ollama", message: "Ollama isn't responding." }));
    const alert = await within(panel).findByRole("alert");
    // Fin du compactage : la zone de saisie ne reprend pas le focus au
    // panneau, qui reste ouvert avec l'échec, sans le rouvrir.
    await settle();
    expect(screen.getByRole("dialog")).toBe(panel);
    expect(document.activeElement).toBe(panel);
    expect(panel.contains(alert)).toBe(true);
    expect(alert.textContent).toBe("Ollama isn't responding.");
    expect(alert.className).toContain("text-red-600");
    expect(compactButton(panel).textContent).toBe("Compact now");
    expect(compactButton(panel).disabled).toBe(false);
  });

  it("while it runs, the composer offers Stop, which stops it", async () => {
    const { simulator } = setup();
    let fail: (error: AppError) => void = () => {};
    simulator.handle(
      "compact_chat",
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    const panel = await openPanel();
    await userEvent.click(compactButton(panel));
    await waitFor(() =>
      expect(compactButton(panel).textContent).toBe("Compacting…"),
    );
    // Pas d'envoi pendant le compactage : Stop à la place.
    expect(screen.queryByLabelText("Send message")).toBeNull();
    await userEvent.click(screen.getByLabelText("Stop"));
    expect(simulator.callsTo("cancel_message")).toEqual([
      { streamId: "compact:chat-1" },
    ]);
    act(() => fail({ kind: "invalid", message: "the compaction was stopped" }));
    await screen.findByLabelText("Send message");
    // Stop, cliqué hors du panneau, l'a fermé ; rouvert, il dit l'arrêt.
    const reopened = await openPanel();
    expect(within(reopened).getByRole("alert").textContent).toBe(
      "the compaction was stopped",
    );
  });

  it("is disabled when nothing follows the last compaction", async () => {
    setup({ compactions: [compaction({ throughMessageId: 2 })] });
    const panel = await openPanel();
    expect(compactButton(panel).disabled).toBe(true);
  });

  it("is enabled when messages follow the last compaction", async () => {
    setup({ compactions: [compaction({ throughMessageId: 1 })] });
    const panel = await openPanel();
    expect(compactButton(panel).disabled).toBe(false);
  });

  it("is disabled while Ollama is lost", async () => {
    const { app } = setup();
    const panel = await openPanel();
    expect(compactButton(panel).disabled).toBe(false);
    setHealth(app, UNREACHABLE);
    await waitFor(() => expect(compactButton(panel).disabled).toBe(true));
  });

  it("is disabled while a reply is being written", async () => {
    const { simulator } = setup();
    const replay = simulator.queue(reply("Next", []), { stepwise: true });
    await send("Next");
    await replay.started;
    await replay.step(2);
    let panel = await openPanel();
    expect(compactButton(panel).disabled).toBe(true);
    // Échap ferme le panneau sans arrêter la réponse.
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(simulator.callsTo("cancel_message")).toEqual([]);
    panel = await openPanel();
    await replay.finish();
    await replyEnded();
    await waitFor(() => expect(compactButton(panel).disabled).toBe(false));
    // Ouvert pendant la réponse, il reste ouvert après sa fin.
    await settle();
    expect(screen.getByRole("dialog")).toBe(panel);
  });

  it("is disabled while a message is being edited", async () => {
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "edit" }));
    const panel = await openPanel();
    expect(compactButton(panel).disabled).toBe(true);
  });

  it("is disabled while the voice mode is open in this chat", async () => {
    const { backend } = setup();
    backend.dictation.install(PARAKEET);
    backend.tts.install(VOICE_EN);
    let start: HTMLButtonElement | null = null;
    await waitFor(() => {
      start = document.querySelector<HTMLButtonElement>(
        "[data-voice-mode-start]",
      );
      expect(start?.disabled).toBe(false);
    });
    const panel = await openPanel();
    expect(compactButton(panel).disabled).toBe(false);
    await userEvent.click(start!);
    await waitFor(() =>
      expect(
        document
          .querySelector("[data-voice-mode-bar]")
          ?.getAttribute("data-voice-mode-phase"),
      ).toBe("listening"),
    );
    const reopened = await openPanel();
    expect(compactButton(reopened).disabled).toBe(true);
  });

  it("disables Retry of the other errors while it runs", async () => {
    for (const [kind, name] of [
      ["loop", "Retry"],
      ["webSearchSignedOut", "Generate without web search"],
    ] as const) {
      const { simulator } = setup();
      simulator.handle("compact_chat", () => new Promise(() => {}));
      simulator.queue(
        reply("Again", [], [{ type: "error", kind, message: "Stopped." }]),
      );
      await send("Again");
      await replyEnded();
      const alert = await screen.findByRole("alert");
      const retry = within(alert).getByRole<HTMLButtonElement>("button", {
        name,
      });
      expect(retry.disabled).toBe(false);
      await userEvent.click(compactButton(await openPanel()));
      await waitFor(() => expect(retry.disabled).toBe(true));
      cleanup();
      streams.resetStreams();
      resetCompactions();
    }
  });
});

/** Envoi de `content` dans chat-1 : `middle` entre le message et la réponse. */
function reply(
  content: string,
  middle: Sequence["events"],
  end: Sequence["events"] = [
    { type: "content", text: "Sure." },
    { type: "context", used: 900, size: SIZE },
    { type: "done", messageId: 4 },
  ],
): Sequence {
  return {
    description: "A reply in chat-1 (built by the test).",
    request: { chatId: "chat-1", streamId: "stream-1", content, model: MODEL },
    events: [
      { type: "userMessage", message: message(3, "user", content) },
      ...middle,
      { type: "replyStarted", messageId: 4, createdAt: SEQUENCE_TIME },
      ...end,
    ],
    error: null,
  };
}

describe("automatic compaction during a send", () => {
  it("shows Compacting conversation…, then the separator, and a live gauge", async () => {
    const { simulator } = setup({ context: { used: 27_000, size: SIZE } });
    const auto = compaction({ automatic: true, throughMessageId: 2 });
    const replay = simulator.queue(
      reply("Next", [
        { type: "compacting" },
        {
          type: "compacted",
          compaction: auto,
          context: { used: 300, size: SIZE },
        },
      ]),
      { stepwise: true },
    );
    await gauge("82%");
    await send("Next");
    await replay.started;

    await replay.until("compacting");
    const status = screen.getByTestId("loading-status");
    await waitFor(() =>
      expect(status.textContent).toBe("Compacting conversation…"),
    );
    expect(screen.queryByTestId("waiting")).toBeNull();

    await replay.until("compacted");
    // Séparateur dès l'événement, après le message résumé ; jauge vivante.
    const separator = await screen.findByRole("button", {
      name: "Context compacted automatically",
    });
    expect(
      separator
        .closest("[data-message-index]")
        ?.getAttribute("data-message-index"),
    ).toBe("1");
    await gauge("0%");
    expect(status.textContent).not.toContain("Compacting conversation…");

    await replay.until("context");
    await gauge("2%");

    await replay.finish();
    await replyEnded();
    // Relu en base : un seul séparateur, la jauge garde la dernière valeur.
    expect(screen.getAllByTestId("compaction")).toHaveLength(1);
    await gauge("2%");
    expect(texts("assistant")).toEqual(["Hi!", "Sure."]);
  });
});

describe("a reply cut because the context is full", () => {
  function contextFull(): Sequence {
    return reply(
      "Tell me everything",
      [],
      [
        { type: "content", text: "Partial answer" },
        { type: "context", used: SIZE, size: SIZE },
        { type: "error", kind: "contextFull", message: CONTEXT_FULL },
      ],
    );
  }

  it("offers Compact now under the error, which goes away once compacted", async () => {
    const { simulator } = setup();
    simulator.queue(contextFull());
    await send("Tell me everything");
    await replyEnded();
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(CONTEXT_FULL)).toBeTruthy();
    // La réponse coupée est gardée.
    await waitFor(() =>
      expect(texts("assistant")).toEqual(["Hi!", "Partial answer"]),
    );
    await gauge("100%");

    await userEvent.click(
      within(alert).getByRole("button", { name: "Compact now" }),
    );
    expect(simulator.callsTo("compact_chat")).toEqual([
      { chatId: "chat-1", model: MODEL },
    ]);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    const separator = await screen.findByTestId("compaction");
    expect(
      separator
        .closest("[data-message-index]")
        ?.getAttribute("data-message-index"),
    ).toBe("3");
  });

  it("shows Compacting… and the failure under the error", async () => {
    const { simulator } = setup();
    let fail: (error: AppError) => void = () => {};
    simulator.handle(
      "compact_chat",
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    simulator.queue(contextFull());
    await send("Tell me everything");
    await replyEnded();
    const alert = await screen.findByRole("alert");
    await userEvent.click(
      within(alert).getByRole("button", { name: "Compact now" }),
    );
    const running = await within(alert).findByRole<HTMLButtonElement>(
      "button",
      { name: "Compacting…" },
    );
    expect(running.disabled).toBe(true);
    act(() => fail({ kind: "ollama", message: "The model failed." }));
    expect(await within(alert).findByText("The model failed.")).toBeTruthy();
    expect(
      within(alert).getByRole<HTMLButtonElement>("button", {
        name: "Compact now",
      }).disabled,
    ).toBe(false);
  });

  it("Compact now under the error waits for Ollama to answer again", async () => {
    const { simulator, app } = setup();
    simulator.queue(contextFull());
    await send("Tell me everything");
    await replyEnded();
    const alert = await screen.findByRole("alert");
    const button = within(alert).getByRole<HTMLButtonElement>("button", {
      name: "Compact now",
    });
    expect(button.disabled).toBe(false);
    setHealth(app, UNREACHABLE);
    await waitFor(() => expect(button.disabled).toBe(true));
  });
});
