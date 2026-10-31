// Panneau, lot 4b : carte des appels des agents par MCP (qui, quel outil,
// où en est l'appel), arrêt d'un appel en cours (chaque « Stop » nomme son
// outil), annonce du dernier appel en cours, coupure pour tous les agents ;
// textes traduits.
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import type {
  AgentsView,
  HostMessage,
  PanelMessage,
  PanelState,
} from "../shared/protocol";
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

const AGENTS: AgentsView = {
  enabled: true,
  calls: [
    {
      id: "0123456789abcdef",
      tool: "chatonlm_ask",
      agent: "Copilot",
      detail: "Why is the build red?",
      state: "running",
      startedAt: 1000,
      endedAt: null,
    },
    {
      id: "fedcba9876543210",
      tool: "chatonlm_generate_image",
      agent: null,
      detail: "A red kitten",
      state: "failed",
      startedAt: 500,
      endedAt: 900,
    },
  ],
};

afterEach(() => cleanup());

function fromHost(data: HostMessage) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

function renderPanel() {
  const post = vi.fn<(message: PanelMessage) => void>();
  render(<Panel post={post} />);
  fromHost({ type: "init", locale: "en", theme: "light", state: CONNECTED });
  return post;
}

describe("agents card", () => {
  it("shows each call: who, which tool, how it went", () => {
    renderPanel();
    expect(screen.queryByTestId("agents")).toBeNull();
    fromHost({ type: "agents", agents: AGENTS });
    const calls = screen.getAllByTestId("agent-call");
    expect(calls[0].textContent).toContain(
      "Copilot used ChatonLM: ask a local model",
    );
    expect(calls[0].textContent).toContain("Why is the build red?");
    expect(calls[0].textContent).toContain("Running…");
    expect(calls[1].textContent).toContain(
      "An agent used ChatonLM: generate an image",
    );
    expect(calls[1].textContent).toContain("Failed");
    expect(screen.getByRole("status").textContent).toBe(
      "Copilot is using ChatonLM: ask a local model",
    );
    fromHost({ type: "agents", agents: null });
    expect(screen.queryByTestId("agents")).toBeNull();
  });

  it("stops a running call and turns the server off for every agent", () => {
    const post = renderPanel();
    fromHost({ type: "agents", agents: AGENTS });
    fireEvent.click(
      screen.getByRole("button", { name: "Stop: ask a local model" }),
    );
    expect(post).toHaveBeenCalledWith({
      type: "agents",
      command: "stop",
      callId: "0123456789abcdef",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Turn off for agents" }),
    );
    expect(post).toHaveBeenCalledWith({ type: "agents", command: "turnOff" });
    // Rien en cours : la carte se vide.
    fromHost({
      type: "agents",
      agents: {
        enabled: false,
        calls: AGENTS.calls.map((call) => ({ ...call, state: "done" })),
      },
    });
    expect(
      screen.queryByRole("button", { name: "Turn off for agents" }),
    ).toBeNull();
    expect(screen.getByTestId("agents").textContent).toContain(
      "Turned off: agents can no longer use ChatonLM.",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Clear the finished calls" }),
    );
    expect(post).toHaveBeenCalledWith({ type: "agents", command: "clear" });
  });

  it("names each Stop button after its tool, and announces the latest call", () => {
    const post = renderPanel();
    fromHost({ type: "agents", agents: AGENTS });
    // Un second appel pendant le premier (le plus récent d'abord).
    fromHost({
      type: "agents",
      agents: {
        ...AGENTS,
        calls: [
          {
            id: "aaaaaaaaaaaaaaaa",
            tool: "chatonlm_speak",
            agent: null,
            detail: "Build passed.",
            state: "running",
            startedAt: 2000,
            endedAt: null,
          },
          ...AGENTS.calls,
        ],
      },
    });
    expect(screen.getByRole("status").textContent).toBe(
      "An agent is using ChatonLM: read a text aloud",
    );
    const stops = screen
      .getAllByRole("button")
      .filter((button) => button.textContent === "Stop")
      .map((button) => button.getAttribute("aria-label"));
    expect(stops).toEqual([
      "Stop: read a text aloud",
      "Stop: ask a local model",
    ]);
    fireEvent.click(
      screen.getByRole("button", { name: "Stop: read a text aloud" }),
    );
    expect(post).toHaveBeenCalledWith({
      type: "agents",
      command: "stop",
      callId: "aaaaaaaaaaaaaaaa",
    });
  });

  it("translates every text", () => {
    usePseudoLanguage();
    renderPanel();
    const allow = ["Copilot", "Why is the build red?", "A red kitten"];
    const tools = [
      "chatonlm_ask",
      "chatonlm_generate_image",
      "chatonlm_speak",
      "chatonlm_proofread",
      "chatonlm_models",
    ] as const;
    const states = ["running", "done", "failed", "stopped"] as const;
    for (const enabled of [true, false]) {
      fromHost({
        type: "agents",
        agents: {
          enabled,
          calls: tools.map((tool, i) => ({
            ...AGENTS.calls[i % 2],
            id: `${i}`.repeat(16),
            tool,
            state: states[i % states.length],
          })),
        },
      });
      expect(
        untranslatedTexts(screen.getByTestId("agents"), allow),
        String(enabled),
      ).toEqual([]);
    }
  });
});
