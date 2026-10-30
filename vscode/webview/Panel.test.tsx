import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Chat } from "@/bindings/Chat";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Message } from "@/bindings/Message";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import type {
  HostMessage,
  PanelMessage,
  PanelState,
  Receipt,
} from "../shared/protocol";
import { Panel } from "./Panel";

const MODEL = "qwen3:0.6b";

const CONNECTED: PanelState = {
  connection: "connected",
  models: [MODEL, "gemma3:270m"],
  model: MODEL,
  ephemeral: false,
  chatId: null,
  busy: false,
  cloudModels: [],
  generation: 0,
  notice: null,
};

const LOCAL: Receipt = {
  items: [
    {
      kind: "selection",
      label: "Selection",
      path: "src/app.ts",
      lines: [3, 12],
      chars: 840,
    },
    {
      kind: "diagnostic",
      label: "Cannot find name 'x'",
      path: "src/app.ts",
      lines: [7, 7],
      chars: 60,
    },
  ],
  chars: 1200,
  tokens: 300,
  contextLength: 4096,
  destination: "local",
  blocked: [],
};

const CLOUD: Receipt = {
  ...LOCAL,
  destination: "cloud",
  blocked: [
    { label: ".env", reason: "secretFile" },
    { label: "src/config.ts:4", reason: "secretValue" },
  ],
};

afterEach(() => {
  cleanup();
  document.documentElement.classList.remove("dark");
});

function message(id: number, role: Message["role"], content: string): Message {
  return {
    id,
    role,
    content,
    thinking: "",
    model: MODEL,
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

/** Message de l'hôte reçu par la page. */
function fromHost(data: HostMessage) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

function stream(requestId: number, ...events: ChatEvent[]) {
  for (const event of events) fromHost({ type: "event", requestId, event });
}

function renderPanel(
  state: Partial<PanelState> = {},
  init: Partial<Extract<HostMessage, { type: "init" }>> = {},
) {
  const post = vi.fn<(message: PanelMessage) => void>();
  render(<Panel post={post} />);
  fromHost({
    type: "init",
    locale: "en",
    theme: "light",
    state: { ...CONNECTED, ...state },
    ...init,
  });
  return post;
}

const posted = (post: ReturnType<typeof renderPanel>) =>
  post.mock.calls.map(([message]) => message);

function composer() {
  return screen.getByRole("textbox", { name: "Message to ChatonLM" });
}

describe("Panel", () => {
  it("says it is ready, and waits for init before showing anything", () => {
    const post = vi.fn<(message: PanelMessage) => void>();
    const { container } = render(<Panel post={post} />);
    expect(post).toHaveBeenCalledWith({ type: "ready" });
    expect(container.textContent).toBe("");
  });

  it("ignores messages that are not from the host protocol", () => {
    const post = vi.fn<(message: PanelMessage) => void>();
    const { container } = render(<Panel post={post} />);
    act(() => {
      window.dispatchEvent(
        new MessageEvent("message", { data: { type: "other" } }),
      );
    });
    expect(container.textContent).toBe("");
  });

  describe("empty states", () => {
    it("offers to open the app when it is closed", () => {
      const post = renderPanel({ connection: "appClosed" });
      expect(screen.getByText("ChatonLM isn't running")).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Open ChatonLM" }));
      expect(posted(post)).toContainEqual({ type: "launchApp" });
      expect(screen.queryByRole("textbox")).toBeNull();
    });

    it("explains how to turn the link on", () => {
      const post = renderPanel({ connection: "bridgeOff" });
      expect(
        screen.getByText(/Turn on the VS Code link in ChatonLM settings/),
      ).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Open ChatonLM" }));
      expect(posted(post)).toContainEqual({ type: "openApp" });
    });

    it("offers to pair", () => {
      const post = renderPanel({ connection: "notPaired" });
      fireEvent.click(screen.getByRole("button", { name: "Pair" }));
      expect(posted(post)).toContainEqual({ type: "pair" });
    });

    it("tells about an incompatible version, and an error with its text", () => {
      renderPanel({ connection: "incompatible" });
      expect(screen.getByTestId("screen-incompatible")).toBeTruthy();
      fromHost({
        type: "state",
        state: { ...CONNECTED, connection: "error", error: "Socket closed" },
      });
      expect(screen.getByText("Socket closed")).toBeTruthy();
      expect(
        screen.getByRole("img", { name: "Couldn't reach ChatonLM" }),
      ).toBeTruthy();
    });

    it("waits while connecting", () => {
      renderPanel({ connection: "connecting" });
      expect(screen.getByTestId("screen-connecting")).toBeTruthy();
    });

    it("sends to the app when no model is installed", () => {
      const post = renderPanel({ models: [], model: null });
      expect(screen.getByTestId("screen-noModels")).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Open ChatonLM" }));
      expect(posted(post)).toContainEqual({ type: "openApp" });
      // Modèle téléchargé dans l'app : la liste se relit sur demande.
      fireEvent.click(screen.getByRole("button", { name: "Check again" }));
      expect(posted(post)).toContainEqual({ type: "refresh" });
    });

    it("shows an empty chat with the quick actions", () => {
      const post = renderPanel();
      expect(screen.getByTestId("screen-empty")).toBeTruthy();
      const actions = screen.getByRole("group", { name: "Quick actions" });
      fireEvent.click(within(actions).getByRole("button", { name: "Explain" }));
      fireEvent.click(
        within(actions).getByRole("button", { name: "Commit message" }),
      );
      expect(posted(post)).toContainEqual({
        type: "action",
        action: "explain",
      });
      expect(posted(post)).toContainEqual({
        type: "action",
        action: "commitMessage",
      });
    });
  });

  describe("sending", () => {
    it("sends with Enter and goes to a new line with Shift+Enter", async () => {
      const post = renderPanel();
      const user = userEvent.setup();
      await user.click(composer());
      await user.keyboard("first{Shift>}{Enter}{/Shift}second");
      expect(composer()).toHaveProperty("value", "first\nsecond");
      expect(posted(post).filter((m) => m.type === "send")).toEqual([]);

      await user.keyboard("{Enter}");
      expect(posted(post)).toContainEqual({
        type: "send",
        text: "first\nsecond",
      });
      expect(composer()).toHaveProperty("value", "");
      // Montré tout de suite, avant que l'hôte ne réponde.
      expect(screen.getByText(/first\s+second/)).toBeTruthy();
    });

    it("does not send an empty message, nor without a model", async () => {
      const post = renderPanel({ model: null });
      const user = userEvent.setup();
      await user.click(composer());
      await user.keyboard("hello{Enter}");
      expect(posted(post).filter((m) => m.type === "send")).toEqual([]);
      expect(
        screen.getByRole("button", { name: "Send message" }),
      ).toHaveProperty("disabled", true);
    });

    it("streams the reply, with folded thinking, then offers to copy it", async () => {
      const post = renderPanel({ chatId: "c1" });
      const user = userEvent.setup();
      await user.click(composer());
      await user.keyboard("Explain{Enter}");
      stream(
        1,
        { type: "userMessage", message: message(10, "user", "Explain") },
        { type: "replyStarted", messageId: 11, createdAt: 2 },
      );
      expect(
        screen.getByRole("status", { name: "Writing a reply…" }),
      ).toBeTruthy();
      stream(
        1,
        { type: "thinking", text: "Let me think" },
        { type: "content", text: "It is **bold" },
      );
      // Markdown incomplet réparé pendant le flux (gras pas encore fermé).
      expect(
        screen.getByText("bold").closest('[data-streamdown="strong"]'),
      ).not.toBeNull();
      expect(screen.queryByRole("button", { name: "Copy" })).toBeNull();
      // Stop pendant la réponse, Entrée n'envoie rien.
      expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();

      stream(1, {
        type: "content",
        text: "** and [docs](https://chatonlm.com/docs).",
      });
      stream(1, { type: "done", messageId: 11 });
      const thinking = screen.getByRole("button", { name: /Thought for/ });
      expect(thinking.getAttribute("aria-expanded")).toBe("false");

      fireEvent.click(screen.getByRole("link", { name: "docs" }));
      expect(posted(post)).toContainEqual({
        type: "openLink",
        url: "https://chatonlm.com/docs",
      });
      fireEvent.click(screen.getByRole("button", { name: "Copy" }));
      expect(posted(post)).toContainEqual({
        type: "copy",
        text: "It is **bold** and [docs](https://chatonlm.com/docs).",
      });
      expect(screen.getByRole("button", { name: "Send message" })).toBeTruthy();
    });

    it("stops a reply, and never sends while one is written", async () => {
      const post = renderPanel({ chatId: "c1" });
      const user = userEvent.setup();
      stream(
        2,
        { type: "userMessage", message: message(10, "user", "From editor") },
        { type: "replyStarted", messageId: 11, createdAt: 2 },
        { type: "content", text: "Partial" },
      );
      await user.click(composer());
      await user.keyboard("more{Enter}");
      expect(posted(post).filter((m) => m.type === "send")).toEqual([]);
      expect(composer()).toHaveProperty("value", "more");

      fireEvent.click(screen.getByRole("button", { name: "Stop" }));
      expect(posted(post)).toContainEqual({ type: "stop" });
      stream(2, { type: "cancelled" });
      expect(screen.getByText("Partial")).toBeTruthy();
      expect(screen.getByRole("button", { name: "Send message" })).toBeTruthy();
    });

    it("stops a send the host has not taken yet", async () => {
      const post = renderPanel({ chatId: "c1" });
      const user = userEvent.setup();
      await user.click(composer());
      await user.keyboard("hello{Enter}");
      fireEvent.click(screen.getByRole("button", { name: "Stop" }));
      expect(posted(post)).toContainEqual({ type: "stop" });
      expect(screen.getByRole("button", { name: "Send message" })).toBeTruthy();
    });

    it("shows the Stop button while the host says a reply is written", () => {
      renderPanel({ chatId: "c1", busy: true });
      expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
      expect(screen.getByRole("combobox", { name: "Model" })).toHaveProperty(
        "disabled",
        true,
      );
    });

    it("shows an error of the engine", () => {
      renderPanel({ chatId: "c1" });
      stream(3, {
        type: "error",
        kind: "unreachable",
        message: "Ollama is not running",
      });
      expect(screen.getByRole("alert").textContent).toContain(
        "Ollama is not running",
      );
    });

    it("shows the model loading before the reply", () => {
      renderPanel({ chatId: "c1" });
      stream(3, { type: "loading" });
      expect(screen.getByText("Loading model…")).toBeTruthy();
    });

    it("shows a saved chat read again by the host", () => {
      renderPanel({ chatId: "c1" });
      fromHost({
        type: "chat",
        chat: chat("c1", [
          message(1, "user", "Old question"),
          message(2, "assistant", "Old answer"),
        ]),
      });
      expect(screen.getByText("Old question")).toBeTruthy();
      expect(screen.getByText("Old answer")).toBeTruthy();
      // Une autre conversation que celle de l'état n'est pas montrée.
      fromHost({
        type: "chat",
        chat: chat("c9", [message(5, "user", "Other")]),
      });
      expect(screen.queryByText("Other")).toBeNull();
    });
  });

  describe("receipt", () => {
    function sendWith(receipt: Receipt) {
      stream(5, {
        type: "userMessage",
        message: message(10, "user", "Fix it"),
      });
      fromHost({ type: "receipt", requestId: 5, receipt });
    }

    it("shows the receipt the host gives back for a saved message", () => {
      // Envoi fait panneau fermé : seuls les reçus gardés et la conversation
      // relue arrivent.
      renderPanel({ chatId: "c1" });
      fromHost({ type: "receipts", receipts: { 10: LOCAL } });
      fromHost({
        type: "chat",
        chat: chat("c1", [message(10, "user", "Fix it")]),
      });
      expect(
        screen.getByRole("button", {
          name: "2 items · ~300 tokens · stays on this computer",
        }),
      ).toBeTruthy();
    });

    it("sums up what stays on this computer, folded", () => {
      renderPanel({ chatId: "c1" });
      sendWith(LOCAL);
      const toggle = screen.getByRole("button", {
        name: "2 items · ~300 tokens · stays on this computer",
      });
      expect(toggle.getAttribute("aria-expanded")).toBe("false");
      const details = document.getElementById(
        toggle.getAttribute("aria-controls")!,
      )!;
      expect(details.hidden).toBe(true);

      fireEvent.click(toggle);
      expect(details.hidden).toBe(false);
      expect(details.textContent).toContain(
        "The model runs on this computer: nothing leaves it.",
      );
      expect(details.textContent).toContain(
        "About 7% of the model's context (4096 tokens).",
      );
      expect(details.textContent).toContain(
        "Selection Selection · src/app.ts · lines 3–12 · 840 characters",
      );
      expect(details.textContent).toContain("line 7 · 60 characters");
      expect(screen.queryByTestId("receipt-blocked")).toBeNull();
    });

    it("says what goes to the cloud and what was removed, and why", () => {
      renderPanel({ chatId: "c1" });
      sendWith(CLOUD);
      const toggle = screen.getByRole("button", {
        name: "2 items · ~300 tokens · goes to the cloud",
      });
      fireEvent.click(toggle);
      expect(screen.getByTestId("receipt").textContent).toContain(
        "The model runs in the cloud: this text is sent over the network.",
      );
      const blocked = screen.getByTestId("receipt-blocked");
      expect(blocked.textContent).toBe(
        "Removed before sending:.env · file that may hold secretssrc/config.ts:4 · line that looks like a secret",
      );
    });

    it("says a typed message goes alone, or with the earlier messages", () => {
      renderPanel({ chatId: "c1" });
      const typed = { ...LOCAL, items: [], tokens: 3 };
      sendWith(typed);
      expect(
        screen.getByRole("button", {
          name: "your message only · ~3 tokens · stays on this computer",
        }),
      ).toBeTruthy();
      cleanup();
      renderPanel({ chatId: "c1" });
      sendWith({ ...typed, tokens: 40, history: { messages: 4, chars: 150 } });
      const toggle = screen.getByRole("button", {
        name: "your message · + 4 earlier messages · ~40 tokens · stays on this computer",
      });
      fireEvent.click(toggle);
      expect(screen.getByTestId("receipt-history").textContent).toBe(
        "Also sent, as they are: 4 earlier messages of this chat (150 characters).",
      );
    });

    it("says when the text goes to another computer, or maybe leaves", () => {
      renderPanel({ chatId: "c1" });
      sendWith({ ...LOCAL, destination: "remote" });
      fireEvent.click(
        screen.getByRole("button", {
          name: "2 items · ~300 tokens · goes to another computer",
        }),
      );
      const receipt = screen.getByTestId("receipt");
      expect(receipt.textContent).toContain(
        "The model runs on another computer (model location set in ChatonLM): this text is sent over the network.",
      );
      expect(receipt.textContent).toContain(
        "Secrets are removed as best as possible",
      );
      cleanup();
      renderPanel({ chatId: "c1" });
      sendWith({ ...LOCAL, destination: "unknown" });
      expect(
        screen.getByRole("button", {
          name: "2 items · ~300 tokens · may leave this computer",
        }),
      ).toBeTruthy();
    });

    it("stays under the message once the chat is read again", () => {
      renderPanel({ chatId: "c1" });
      sendWith(LOCAL);
      stream(5, { type: "done", messageId: null });
      fromHost({
        type: "chat",
        chat: chat("c1", [message(10, "user", "Fix it")]),
      });
      expect(screen.getByTestId("receipt").dataset.destination).toBe("local");
    });
  });

  describe("header", () => {
    it("starts a new chat, or an ephemeral one with the ghost kitten", () => {
      const post = renderPanel({ chatId: "c1" });
      const ghost = screen.getByRole("button", { name: "Ephemeral chat" });
      expect(ghost.getAttribute("aria-pressed")).toBe("false");
      expect(ghost.title).toBe("Start an ephemeral chat");
      fireEvent.click(ghost);
      expect(posted(post)).toContainEqual({ type: "newChat", ephemeral: true });

      fromHost({
        type: "state",
        state: { ...CONNECTED, chatId: null, ephemeral: true },
      });
      expect(ghost.getAttribute("aria-pressed")).toBe("true");
      expect(ghost.title).toBe("Turn off ephemeral chat");
      expect(screen.getByTestId("ephemeral-notice")).toBeTruthy();
      fireEvent.click(ghost);
      expect(posted(post)).toContainEqual({
        type: "newChat",
        ephemeral: false,
      });

      post.mockClear();
      fireEvent.click(screen.getByRole("button", { name: "New chat" }));
      expect(posted(post)).toEqual([{ type: "newChat", ephemeral: false }]);
    });

    it("keeps an ephemeral chat with the ghost kitten or Keep, like the app", () => {
      const post = renderPanel({ chatId: "e1", ephemeral: true });
      const ghost = screen.getByRole("button", { name: "Keep this chat" });
      expect(ghost.getAttribute("aria-pressed")).toBeNull();
      fireEvent.click(ghost);
      expect(posted(post)).toEqual([{ type: "ready" }, { type: "keep" }]);
      fireEvent.click(
        within(screen.getByTestId("ephemeral-notice")).getByRole("button", {
          name: "Keep",
        }),
      );
      expect(posted(post).at(-1)).toEqual({ type: "keep" });
      // Pas encore commencée : rien à garder.
      cleanup();
      renderPanel({ chatId: null, ephemeral: true });
      expect(
        within(screen.getByTestId("ephemeral-notice")).queryByRole("button"),
      ).toBeNull();
    });

    it("says why a new chat was opened for a model off this computer", () => {
      renderPanel({ notice: "offMachineNewChat" });
      expect(screen.getByTestId("notice").textContent).toBe(
        "This model runs outside this computer: a new chat was started, so the code of the previous one stays here.",
      );
    });

    it("forgets a reply still running when a new chat starts", () => {
      renderPanel({ chatId: "c1", busy: true });
      stream(
        5,
        { type: "userMessage", message: message(10, "user", "Old question") },
        { type: "replyStarted", messageId: 11, createdAt: 2 },
        { type: "content", text: "Old answer" },
      );
      fromHost({
        type: "state",
        state: { ...CONNECTED, chatId: null, busy: true, generation: 1 },
      });
      expect(screen.queryByText("Old question")).toBeNull();
      expect(screen.queryByText("Old answer")).toBeNull();
    });

    it("marks cloud models, and asks to choose one", () => {
      renderPanel({
        models: [MODEL, "gpt-oss:20b-cloud"],
        cloudModels: ["gpt-oss:20b-cloud"],
        model: null,
      });
      const picker = screen.getByRole("combobox", { name: "Model" });
      expect(
        [...picker.querySelectorAll("option")].map((o) => o.textContent),
      ).toEqual(["Choose a model", MODEL, "gpt-oss:20b-cloud (cloud)"]);
    });

    it("forgets the messages of the previous chat on a new chat", () => {
      renderPanel({ chatId: "c1" });
      fromHost({ type: "chat", chat: chat("c1", [message(1, "user", "Old")]) });
      fromHost({ type: "state", state: { ...CONNECTED, chatId: null } });
      expect(screen.queryByText("Old")).toBeNull();
      expect(screen.getByTestId("screen-empty")).toBeTruthy();
    });

    it("chooses the model", () => {
      const post = renderPanel();
      fireEvent.change(screen.getByRole("combobox", { name: "Model" }), {
        target: { value: "gemma3:270m" },
      });
      expect(posted(post)).toContainEqual({
        type: "selectModel",
        model: "gemma3:270m",
      });
    });
  });

  describe("theme and language", () => {
    it("follows the theme of VS Code", () => {
      renderPanel({}, { theme: "dark" });
      expect(document.documentElement.classList.contains("dark")).toBe(true);
      fromHost({ type: "theme", theme: "light" });
      expect(document.documentElement.classList.contains("dark")).toBe(false);
    });

    it("speaks the language of VS Code", () => {
      renderPanel({ connection: "appClosed" }, { locale: "fr" });
      expect(
        screen.getByRole("button", { name: "Ouvrir ChatonLM" }),
      ).toBeTruthy();
    });

    it("writes no text outside the catalogs", () => {
      usePseudoLanguage();
      const allow = [
        MODEL,
        "gemma3:270m",
        "Fix it",
        "src/app.ts",
        "Selection",
        "Cannot find name 'x'",
        ".env",
        "src/config.ts:4",
        "gpt-oss:20b-cloud",
        "Answer",
        "Socket closed",
        "Thinking text",
      ];
      for (const connection of [
        "connecting",
        "notPaired",
        "appClosed",
        "bridgeOff",
        "incompatible",
      ] as const) {
        renderPanel({ connection });
        expect(untranslatedTexts(document.body, allow), connection).toEqual([]);
        cleanup();
      }
      renderPanel({ connection: "error", error: "Socket closed" });
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
      cleanup();
      renderPanel({ models: [], model: null });
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
      cleanup();
      renderPanel({
        models: [MODEL, "gpt-oss:20b-cloud"],
        cloudModels: ["gpt-oss:20b-cloud"],
        model: null,
        notice: "offMachineNewChat",
      });
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
      cleanup();

      renderPanel({ chatId: "c1", ephemeral: true });
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
      stream(
        5,
        { type: "userMessage", message: message(10, "user", "Fix it") },
        { type: "loading" },
      );
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
      fromHost({
        type: "receipt",
        requestId: 5,
        receipt: { ...CLOUD, history: { messages: 2, chars: 40 } },
      });
      stream(
        5,
        { type: "replyStarted", messageId: 11, createdAt: 2 },
        { type: "thinking", text: "Thinking text" },
        { type: "content", text: "Answer" },
        { type: "done", messageId: 11 },
      );
      fireEvent.click(
        screen.getByRole("button", { name: /goes to the cloud/ }),
      );
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
    });
  });
});
