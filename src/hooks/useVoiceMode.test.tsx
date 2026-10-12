// Magasin du mode vocal, hors de l'app : lancement (prérequis, accord,
// démarrage, arrêt pendant le démarrage), état reçu, séance orpheline,
// phrases gardées jusqu'à ce que la page soit prête, niveaux sans rendu de
// la page, échec effacé quand la page change, lecture de l'état plus
// ancienne qu'un événement.

import { renderHook, waitFor } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VoiceModeRequirements } from "@/bindings/VoiceModeRequirements";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import type { VoiceModeUtterance } from "@/bindings/VoiceModeUtterance";
import {
  adoptVoiceModeChat,
  applyVoiceModeLevel,
  applyVoiceModeState,
  applyVoiceModeUtterance,
  registerVoiceModeUtterances,
  startVoiceMode,
  stopVoiceMode,
  subscribeVoiceMode,
  subscribeVoiceModeLevel,
  useVoiceModeFor,
  voiceModeConfirmed,
  voiceModeDownloading,
  voiceModePageShown,
  voiceModeSnapshot,
} from "./useVoiceMode";

afterEach(() => {
  clearMocks();
});

const READY: VoiceModeRequirements = {
  speechModel: "parakeet-tdt-0.6b-v3",
  speechModelInstalled: true,
  speechModelBytes: 0,
  voiceModel: "pocket-tts-en",
  voiceLanguage: "en",
  voiceInstalled: true,
  voiceBytes: 0,
  unavailable: null,
};

const LISTENING: VoiceModeState = {
  phase: "listening",
  muted: false,
  turn: 0,
  notice: null,
};

/** Faux IPC : commandes reçues ; voice_mode_start attend `startGate`. */
function engine({
  requirements = READY,
  startGate = Promise.resolve(),
}: {
  requirements?: VoiceModeRequirements;
  startGate?: Promise<void>;
} = {}) {
  const calls: string[] = [];
  mockIPC(async (command) => {
    calls.push(command);
    switch (command) {
      case "voice_mode_requirements":
        return requirements;
      case "voice_mode_start":
        await startGate;
        return null;
      case "voice_mode_stop":
        return null;
    }
    throw new Error(`unexpected command ${command}`);
  });
  return calls;
}

function utterance(turn: number, text = `turn ${turn}`): VoiceModeUtterance {
  return { turn, text, language: "en" };
}

describe("voice mode store", () => {
  it("launch: requirements, start, then the engine's state clears the launch", async () => {
    const calls = engine();
    await startVoiceMode("chat-1");
    expect(calls).toEqual(["voice_mode_requirements", "voice_mode_start"]);
    // Pas encore d'état actif : le démarrage reste affiché.
    expect(voiceModeSnapshot().launch).toEqual({ step: "starting" });
    applyVoiceModeState(LISTENING);
    expect(voiceModeSnapshot()).toMatchObject({
      owner: "chat-1",
      launch: null,
      state: LISTENING,
    });
    // Fin par le moteur (fenêtre masquée...) : plus de propriétaire.
    applyVoiceModeState({ ...LISTENING, phase: "off" });
    expect(voiceModeSnapshot().owner).toBeNull();
  });

  it("missing files: asks first; an old “off” state doesn't end the launch", async () => {
    const calls = engine({
      requirements: { ...READY, voiceInstalled: false, voiceBytes: 10 },
    });
    await startVoiceMode("chat-1");
    expect(voiceModeSnapshot().launch?.step).toBe("asking");
    applyVoiceModeState({ ...LISTENING, phase: "off" });
    expect(voiceModeSnapshot().owner).toBe("chat-1");
    // Accord : la boîte se ferme, les prérequis relus disent ce qui manque.
    const run = voiceModeConfirmed();
    expect(run).not.toBeNull();
    expect(voiceModeSnapshot().launch).toEqual({ step: "checking" });
    expect(
      voiceModeDownloading(run!, {
        ...READY,
        voiceInstalled: false,
        voiceBytes: 10,
      }),
    ).toEqual({ speech: null, voice: "pocket-tts-en" });
    expect(voiceModeSnapshot().launch).toMatchObject({
      step: "downloading",
      speech: null,
      voice: "pocket-tts-en",
    });
    expect(calls).toEqual(["voice_mode_requirements"]);
  });

  it("files installed while the dialog was open: starts without downloading", async () => {
    const calls = engine({
      requirements: { ...READY, speechModelInstalled: false },
    });
    await startVoiceMode("chat-1");
    const run = voiceModeConfirmed()!;
    expect(voiceModeDownloading(run, READY)).toBeNull();
    expect(voiceModeSnapshot().launch).toEqual({ step: "starting" });
    await waitFor(() => expect(calls).toContain("voice_mode_start"));
  });

  it("ended while the requirements are read again: nothing more", async () => {
    engine({ requirements: { ...READY, voiceInstalled: false } });
    await startVoiceMode("chat-1");
    const run = voiceModeConfirmed()!;
    stopVoiceMode();
    expect(
      voiceModeDownloading(run, { ...READY, voiceInstalled: false }),
    ).toBeNull();
    expect(voiceModeSnapshot()).toMatchObject({ owner: null, launch: null });
  });

  it("stopped while starting: the engine is stopped again once started", async () => {
    let open: () => void = () => {};
    const calls = engine({
      startGate: new Promise<void>((resolve) => (open = resolve)),
    });
    const starting = startVoiceMode("chat-1");
    await waitFor(() => expect(calls).toContain("voice_mode_start"));
    stopVoiceMode();
    expect(voiceModeSnapshot().owner).toBeNull();
    open();
    await starting;
    expect(calls.filter((c) => c === "voice_mode_stop")).toHaveLength(2);
  });

  it("an active session nobody holds is stopped", () => {
    const calls = engine();
    applyVoiceModeState(LISTENING);
    expect(calls).toEqual(["voice_mode_stop"]);
  });

  it("sentences wait for the page of the session, then go to it once", async () => {
    engine();
    await startVoiceMode("new");
    applyVoiceModeState(LISTENING);
    applyVoiceModeUtterance(utterance(1));
    adoptVoiceModeChat("new", "chat-9");
    const received: string[] = [];
    // Page d'une autre conversation : rien.
    const other = registerVoiceModeUtterances("chat-2", (u) =>
      received.push(`chat-2 ${u.text}`),
    );
    const mine = registerVoiceModeUtterances("chat-9", (u) =>
      received.push(u.text),
    );
    applyVoiceModeUtterance(utterance(2));
    applyVoiceModeUtterance(utterance(2));
    expect(received).toEqual(["turn 1", "turn 2"]);
    other();
    mine();
  });

  it("levels don't wake the store's subscribers", () => {
    const store = vi.fn();
    const levels = vi.fn();
    const stopStore = subscribeVoiceMode(store);
    const stopLevels = subscribeVoiceModeLevel(levels);
    applyVoiceModeLevel({ input: 0.4, output: 0 });
    expect(levels).toHaveBeenCalledWith({ input: 0.4, output: 0 });
    expect(store).not.toHaveBeenCalled();
    stopStore();
    stopLevels();
  });

  it("a launch failure is shown on its page only, until the page changes", async () => {
    engine({ requirements: { ...READY, unavailable: "Not on this CPU" } });
    await startVoiceMode("chat-1");
    const mine = renderHook(() => useVoiceModeFor("chat-1"));
    const other = renderHook(() => useVoiceModeFor("chat-2"));
    expect(mine.result.current).toMatchObject({
      shown: false,
      error: "Not on this CPU",
    });
    expect(other.result.current.error).toBeNull();
    voiceModePageShown("chat-1");
    expect(voiceModeSnapshot().error).not.toBeNull();
    voiceModePageShown(null);
    expect(voiceModeSnapshot().error).toBeNull();
  });

  it("the page of another chat ends the session or its launch", async () => {
    const calls = engine({
      requirements: { ...READY, voiceInstalled: false },
    });
    await startVoiceMode("chat-1");
    voiceModePageShown("chat-1");
    expect(voiceModeSnapshot().owner).toBe("chat-1");
    voiceModePageShown("chat-2");
    expect(voiceModeSnapshot()).toMatchObject({ owner: null, launch: null });
    // Lancement abandonné avant le démarrage : rien à arrêter au moteur.
    expect(calls).not.toContain("voice_mode_stop");
  });
});
