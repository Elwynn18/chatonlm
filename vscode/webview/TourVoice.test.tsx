// Panneau, lot 2 (X) : carte de la visite guidée, « Ask about this part »,
// barre de la séance vocale (chaton en caoutchouc), tours de parole dans la
// conversation, passage joint ; textes traduits.
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Message } from "@/bindings/Message";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import type {
  HostMessage,
  PanelMessage,
  PanelState,
  TourView,
  VoiceView,
} from "../shared/protocol";
import { initialModel, reduce } from "./model";
import { Panel } from "./Panel";

const CONNECTED: PanelState = {
  connection: "connected",
  models: ["qwen3:0.6b"],
  cloudModels: [],
  model: "qwen3:0.6b",
  ephemeral: false,
  chatId: null,
  busy: false,
  generation: 0,
  notice: null,
};

const TOUR: TourView = {
  id: 1,
  kind: "file",
  path: "src/app.ts",
  status: "playing",
  segments: [
    { lines: [1, 3], text: "The imports." },
    { lines: [4, 9], text: "The main loop." },
    { lines: [10, 10], text: "The export." },
  ],
  current: 1,
  receipt: {
    items: [
      {
        kind: "file",
        label: "src/app.ts",
        path: "src/app.ts",
        lines: [1, 10],
        chars: 120,
      },
    ],
    chars: 120,
    tokens: 30,
    contextLength: 8192,
    destination: "local",
    blocked: [],
  },
};

const VOICE: VoiceView = {
  profile: "duck",
  phase: "listening",
  muted: false,
  notice: null,
  shared: [
    {
      kind: "file",
      label: "src/app.ts",
      path: "src/app.ts",
      lines: [3, 40],
      chars: 900,
    },
    { kind: "terminal", label: "npm test", chars: 300 },
  ],
  blocked: [],
  destination: "local",
  noTerminalError: false,
};

afterEach(() => cleanup());

function fromHost(data: HostMessage) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

function renderPanel(state: Partial<PanelState> = {}) {
  const post = vi.fn<(message: PanelMessage) => void>();
  render(<Panel post={post} />);
  fromHost({
    type: "init",
    locale: "en",
    theme: "light",
    state: { ...CONNECTED, ...state },
  });
  return post;
}

const posted = (post: ReturnType<typeof renderPanel>) =>
  post.mock.calls.map(([message]) => message);

function message(id: number, role: Message["role"], content: string): Message {
  return {
    id,
    role,
    content,
    thinking: "",
    model: "qwen3:0.6b",
    createdAt: 1,
    updatedAt: 1,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

describe("Tour card", () => {
  it("shows where the tour is, and drives it", () => {
    const post = renderPanel();
    fromHost({ type: "tour", tour: TOUR });
    expect(screen.getByText("Tour of src/app.ts")).toBeTruthy();
    expect(screen.getByTestId("tour-progress").textContent).toBe("Part 2 of 3");
    const current = screen.getByRole("button", { current: "step" });
    expect(current.textContent).toContain("The main loop.");
    expect(current.textContent).toContain("lines 4–9");

    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    fireEvent.click(screen.getByRole("button", { name: "Previous part" }));
    fireEvent.click(screen.getByRole("button", { name: "Next part" }));
    fireEvent.click(screen.getByRole("button", { name: "Stop the tour" }));
    fireEvent.click(screen.getByText(/The export\./));
    expect(posted(post)).toEqual(
      expect.arrayContaining([
        { type: "tour", command: "pause" },
        { type: "tour", command: "previous" },
        { type: "tour", command: "next" },
        { type: "tour", command: "stop" },
        { type: "tourGoto", index: 2 },
      ]),
    );

    fromHost({ type: "tour", tour: { ...TOUR, status: "paused", current: 0 } });
    expect(screen.getByTestId("tour-progress").textContent).toBe(
      "Part 1 of 3, paused",
    );
    expect(
      screen.getByRole("button", { name: "Previous part" }),
    ).toHaveProperty("disabled", true);
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(posted(post)).toContainEqual({ type: "tour", command: "resume" });
  });

  it("asks about the part: typed, with the text ready in the box, or aloud", () => {
    const post = renderPanel();
    fromHost({ type: "tour", tour: TOUR });
    fireEvent.click(
      screen.getByRole("button", { name: "Ask about this part" }),
    );
    expect(posted(post)).toContainEqual({ type: "tourAsk", voice: false });
    const box = screen.getByRole("textbox", { name: "Message to ChatonLM" });
    expect(box).toHaveProperty("value", "About this part: ");
    expect(document.activeElement).toBe(box);

    // L'hôte joint le passage : il est montré, et peut être retiré.
    fromHost({
      type: "state",
      state: {
        ...CONNECTED,
        attachment: { path: "src/app.ts", lines: [4, 9] },
      },
    });
    expect(screen.getByTestId("attachment").textContent).toContain(
      "About src/app.ts, lines 4–9",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Don't attach this part" }),
    );
    expect(posted(post)).toContainEqual({ type: "clearAttachment" });

    fireEvent.click(screen.getByRole("button", { name: "Ask aloud" }));
    expect(posted(post)).toContainEqual({ type: "tourAsk", voice: true });
  });

  it("says the tour is being prepared, and closes a finished one", () => {
    renderPanel();
    fromHost({
      type: "tour",
      tour: { ...TOUR, status: "planning", segments: [], current: -1 },
    });
    expect(screen.getByTestId("tour-progress").textContent).toBe(
      "Preparing the tour…",
    );
    expect(screen.queryByRole("button", { name: "Ask about this part" })).toBe(
      null,
    );
    fromHost({ type: "tour", tour: { ...TOUR, status: "ended" } });
    expect(screen.getByTestId("tour-progress").textContent).toBe("Tour over");
    expect(screen.queryByRole("button", { current: "step" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByTestId("tour")).toBeNull();
    // Une nouvelle visite se montre de nouveau.
    fromHost({ type: "tour", tour: { ...TOUR, id: 2 } });
    expect(screen.getByTestId("tour")).toBeTruthy();
  });
});

describe("Voice session", () => {
  it("starts from the microphone button of the box", () => {
    const post = renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Talk it through" }));
    expect(posted(post)).toContainEqual({ type: "voice", command: "start" });
  });

  it("replaces the box while it runs: phase, level, mute, end, what is shared", () => {
    const post = renderPanel();
    fromHost({ type: "voice", voice: VOICE });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByTestId("voice-phase").textContent).toBe("Listening…");
    fromHost({ type: "voiceLevel", level: 0.42 });
    expect(
      screen
        .getByRole("meter", { name: "Microphone level" })
        .getAttribute("aria-valuenow"),
    ).toBe("42");
    expect(screen.getByTestId("voice-shared").textContent).toBe(
      "Shared with the model: src/app.ts, lines 3–40 · last failed command (npm test)",
    );
    fireEvent.click(screen.getByRole("button", { name: "Mute microphone" }));
    fireEvent.click(
      screen.getByRole("button", { name: "End the voice session" }),
    );
    expect(posted(post)).toEqual(
      expect.arrayContaining([
        { type: "voice", command: "mute" },
        { type: "voice", command: "stop" },
      ]),
    );

    fromHost({
      type: "voice",
      voice: {
        ...VOICE,
        phase: "responding",
        muted: true,
        shared: [],
        noTerminalError: true,
        blocked: [{ label: "x", reason: "secretValue" }],
      },
    });
    expect(screen.getByTestId("voice-phase").textContent).toBe(
      "Microphone off",
    );
    expect(
      screen
        .getByRole("meter", { name: "Microphone level" })
        .getAttribute("aria-valuenow"),
    ).toBe("0");
    expect(screen.getByTestId("voice-no-terminal").textContent).toBe(
      "No terminal error available",
    );
    expect(screen.getByText(/1 item removed before sending/)).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Turn the microphone back on" }),
    );
    expect(posted(post)).toContainEqual({ type: "voice", command: "unmute" });

    // Séance finie : la zone de saisie revient.
    fromHost({ type: "voice", voice: null });
    expect(
      screen.getByRole("textbox", { name: "Message to ChatonLM" }),
    ).toBeTruthy();
  });

  it("offers to resume a paused session", () => {
    const post = renderPanel();
    fromHost({ type: "voice", voice: VOICE });
    expect(screen.queryByRole("button", { name: "Resume listening" })).toBe(
      null,
    );
    fromHost({ type: "voice", voice: { ...VOICE, phase: "paused" } });
    expect(screen.getByTestId("voice-phase").textContent).toBe("Paused");
    fireEvent.click(screen.getByRole("button", { name: "Resume listening" }));
    expect(posted(post)).toContainEqual({ type: "voice", command: "resume" });
  });

  it("says where what is said goes", () => {
    renderPanel();
    fromHost({ type: "voice", voice: VOICE });
    const where = () => screen.getByTestId("voice-destination");
    expect(where().textContent).toBe(
      "The model runs on this computer: what you say stays on it.",
    );
    for (const [destination, text] of [
      [
        "cloud",
        "The model runs in the cloud: what you say is sent over the network.",
      ],
      [
        "remote",
        "The model runs on another computer: what you say is sent over the network.",
      ],
      ["unknown", /handled as if it left this computer/],
    ] as const) {
      fromHost({ type: "voice", voice: { ...VOICE, destination } });
      expect(where().getAttribute("data-destination")).toBe(destination);
      expect(where().textContent).toMatch(text);
      // Hors de cette machine : marqué en roux.
      expect(where().querySelector("svg")?.getAttribute("class")).toContain(
        "text-accent-fg",
      );
    }
  });

  it("shows the spoken turns in the chat", () => {
    renderPanel();
    fromHost({ type: "voice", voice: { ...VOICE, phase: "speaking" } });
    const events: ChatEvent[] = [
      { type: "userMessage", message: message(1, "user", "My test fails") },
      { type: "replyStarted", messageId: 2, createdAt: 1 },
      { type: "content", text: "What should it return?" },
      { type: "done", messageId: 2 },
    ];
    for (const event of events)
      fromHost({ type: "event", requestId: -1, event });
    expect(screen.getByText("My test fails")).toBeTruthy();
    expect(screen.getByText("What should it return?")).toBeTruthy();
    expect(screen.getByTestId("voice-phase").textContent).toBe("Speaking…");
  });

  it("has every text translated", () => {
    usePseudoLanguage();
    const allow = [
      "qwen3:0.6b",
      "src/app.ts",
      "npm test",
      "The imports.",
      "The main loop.",
      "The export.",
    ];
    renderPanel({ attachment: { path: "src/app.ts", lines: [4, 9] } });
    fromHost({ type: "tour", tour: TOUR });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    fromHost({
      type: "tour",
      tour: { ...TOUR, status: "paused", kind: "diff" },
    });
    fromHost({ type: "voice", voice: { ...VOICE, noTerminalError: true } });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    for (const phase of [
      "starting",
      "transcribing",
      "responding",
      "speaking",
      "paused",
    ] as const) {
      fromHost({ type: "voice", voice: { ...VOICE, phase, profile: "tour" } });
      expect(untranslatedTexts(document.body, allow), phase).toEqual([]);
    }
    for (const destination of ["cloud", "remote", "unknown"] as const) {
      fromHost({ type: "voice", voice: { ...VOICE, destination } });
      expect(untranslatedTexts(document.body, allow), destination).toEqual([]);
    }
    fromHost({
      type: "tour",
      tour: { ...TOUR, kind: "selection", status: "ended" },
    });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });
});

describe("model, spoken turns", () => {
  it("never binds a spoken turn to a typed message still waiting", () => {
    let model = reduce(initialModel(), {
      type: "host",
      message: { type: "init", locale: "en", theme: "light", state: CONNECTED },
      now: 0,
    });
    model = reduce(model, { type: "sent", text: "typed" });
    model = reduce(model, {
      type: "host",
      message: {
        type: "event",
        requestId: -1,
        event: { type: "replyStarted", messageId: 9, createdAt: 0 },
      },
      now: 0,
    });
    expect(model.turns).toHaveLength(2);
    expect(model.turns[0]).toMatchObject({ text: "typed", requestId: null });
    expect(model.turns[1].requestId).toBe(-1);
  });

  it("drops the microphone level when the session ends", () => {
    let model = reduce(initialModel(), {
      type: "host",
      message: { type: "voice", voice: VOICE },
      now: 0,
    });
    model = reduce(model, {
      type: "host",
      message: { type: "voiceLevel", level: 0.8 },
      now: 0,
    });
    expect(model.voiceLevel).toBe(0.8);
    model = reduce(model, {
      type: "host",
      message: { type: "voice", voice: null },
      now: 0,
    });
    expect(model.voiceLevel).toBe(0);
    // Niveau reçu hors séance : ignoré.
    model = reduce(model, {
      type: "host",
      message: { type: "voiceLevel", level: 0.5 },
      now: 0,
    });
    expect(model.voiceLevel).toBe(0);
  });
});
