// Bandeau « VS Code utilise votre micro », face au simulateur : visible sur
// toute page tant que VS Code tient la séance ou la dictée (lot 4b), jamais
// pour une séance ou une dictée de l'app ; Stop arrête la séance ou abandonne
// la dictée de VS Code (et dit un échec).

import { configure, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import { fakeBackend } from "@/test/backend";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const LISTENING: VoiceModeState = {
  phase: "listening",
  muted: false,
  turn: 0,
  notice: null,
  editor: true,
};

function setup(path: string, state: VoiceModeState) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, { selectedModel: "qwen3:0.6b" });
  backend.voiceMode.state = state;
  renderApp(path);
  return { simulator, backend };
}

const banner = () =>
  document.querySelector<HTMLElement>("[data-testid=editor-voice-banner]");

describe("editor voice banner", () => {
  it("shows on any page while VS Code holds the microphone, and Stop ends it", async () => {
    const { simulator } = setup("/settings", LISTENING);
    await screen.findByText("VS Code is using your microphone");
    const stop = screen.getByRole("button", {
      name: "Stop the VS Code voice session",
    });
    expect(stop.textContent).toBe("Stop");
    await userEvent.click(stop);
    expect(simulator.callsTo("voice_mode_stop_editor")).toHaveLength(1);
    await waitFor(() => expect(banner()).toBeNull());
    expect(simulator.callsTo("voice_mode_stop")).toEqual([]);
  });

  it("is not shown for a session of the page", async () => {
    setup("/c/new", { ...LISTENING, editor: false });
    await screen.findByLabelText("Message");
    expect(banner()).toBeNull();
  });

  it("says when the session couldn't be stopped", async () => {
    const { backend } = setup("/hub", LISTENING);
    backend.voiceMode.failures.set("voice_mode_stop_editor", {
      kind: "invalid",
      message: "boom",
    });
    await userEvent.click(
      await screen.findByRole("button", {
        name: "Stop the VS Code voice session",
      }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't stop the VS Code voice session: boom",
    );
    expect(banner()).not.toBeNull();
  });

  it("shows while VS Code dictates, and Stop drops that dictation", async () => {
    const { simulator, backend } = setup("/hub", {
      ...LISTENING,
      phase: "off",
      editor: false,
    });
    await backend.dictation.setStatus({ phase: "recording", editor: true });
    await screen.findByText("VS Code is using your microphone");
    await userEvent.click(
      screen.getByRole("button", { name: "Stop the VS Code dictation" }),
    );
    expect(simulator.callsTo("dictation_stop_editor")).toHaveLength(1);
    expect(simulator.callsTo("voice_mode_stop_editor")).toEqual([]);
    expect(simulator.callsTo("dictation_cancel")).toEqual([]);
    await waitFor(() => expect(banner()).toBeNull());
  });

  it("is not shown for a dictation of the app, and says a failed stop", async () => {
    const { simulator, backend } = setup("/c/new", {
      ...LISTENING,
      phase: "off",
      editor: false,
    });
    await backend.dictation.setStatus({ phase: "recording", editor: false });
    await screen.findByTestId("new-chat");
    await waitFor(() =>
      expect(simulator.callsTo("dictation_status").length).toBeGreaterThan(0),
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(banner()).toBeNull();
    backend.dictation.failures.set("dictation_stop_editor", {
      kind: "invalid",
      message: "boom",
    });
    await backend.dictation.setStatus({ editor: true });
    await userEvent.click(
      await screen.findByRole("button", { name: "Stop the VS Code dictation" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't stop the VS Code dictation: boom",
    );
    expect(banner()).not.toBeNull();
  });
});
