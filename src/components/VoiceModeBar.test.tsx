// Barre du mode vocal face au simulateur (faux moteur de src/test/voiceMode.ts)
// : lancement par le bouton « onde » (la barre remplace le champ, le
// brouillon revient à la fin), libellés par étape (Responding d'après le
// flux de la réponse), avis, micro coupé, pause, niveaux, clavier (mains
// libres, push-to-talk, Échap, Ctrl+Maj+Espace, champs et boutons
// respectés), prérequis (incapacité, boîte d'accord, téléchargements,
// échec), puis les textes en pseudo-langue et en français.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import type { VoiceModePhase } from "@/bindings/VoiceModePhase";
import { setLanguage } from "@/i18n";
import { resetStreams } from "@/lib/streams";
import { fakeBackend, type FakeBackend, thinkingModel } from "@/test/backend";
import { PARAKEET } from "@/test/dictation";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator, type Sequence, sequence } from "@/test/simulator";
import { VOICE_EN } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  resetStreams();
});

const MODEL = "qwen3:0.6b";

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

/**
 * Conversation chat-1, modèle qui sait raisonner, transcription et voix
 * anglaise installées ; `prepare` règle le faux backend avant le rendu.
 */
function setup(prepare?: (backend: FakeBackend) => void, path = "/c/chat-1") {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    selectedModel: MODEL,
    details: [thinkingModel(MODEL, { values: [false, true], default: true })],
  });
  backend.chats.set("chat-1", {
    id: "chat-1",
    title: "Chat",
    createdAt: 1,
    updatedAt: 1,
    messages: [message(1, "user", "Hi"), message(2, "assistant", "Hello!")],
  });
  backend.dictation.install(PARAKEET);
  backend.tts.install(VOICE_EN);
  prepare?.(backend);
  const app = renderApp(path);
  return { simulator, backend, voice: backend.voiceMode, app };
}

const bar = () => document.querySelector<HTMLElement>("[data-voice-mode-bar]");
const talk = () =>
  document.querySelector<HTMLButtonElement>("[data-voice-mode-talk]")!;
const label = () =>
  document.querySelector("[data-voice-mode-label]")?.textContent ?? null;
const notice = () =>
  document.querySelector("[data-voice-mode-notice]")?.textContent ?? null;

/** Bouton « onde », une fois le modèle choisi (sinon inactif). */
async function startButton() {
  let button: HTMLButtonElement | null = null;
  await waitFor(() => {
    button = document.querySelector<HTMLButtonElement>(
      "[data-voice-mode-start]",
    );
    expect(button?.disabled).toBe(false);
  });
  return button!;
}

/** Lance le mode vocal et attend l'écoute. */
async function start() {
  await userEvent.click(await startButton());
  await phase("listening");
}

async function phase(expected: VoiceModePhase) {
  await waitFor(() =>
    expect(bar()?.getAttribute("data-voice-mode-phase")).toBe(expected),
  );
}

/** Touche envoyée à la fenêtre depuis `target` (le corps par défaut). */
function press(init: KeyboardEventInit, target: Element = document.body) {
  return fireEvent.keyDown(target, init);
}

describe("voice mode bar", () => {
  it("replaces the message field; End brings the draft back", async () => {
    const { simulator } = setup();
    const field = await screen.findByLabelText<HTMLTextAreaElement>("Message");
    await userEvent.type(field, "My draft");
    await start();
    expect(screen.queryByLabelText("Message")).toBeNull();
    expect(screen.queryByLabelText("Start voice mode")).toBeNull();
    expect(screen.queryByLabelText("Upload multiple files")).toBeNull();
    // Modèle gardé ; le bouton principal a le focus (Espace y agit).
    expect(screen.getByTitle("Select model")).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(talk()));
    expect(label()).toBe("Listening…");
    expect(bar()!.textContent).toContain("Space: I'm done · Esc: end");

    await userEvent.click(
      screen.getByRole("button", { name: "End voice mode" }),
    );
    expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1);
    await waitFor(() => expect(bar()).toBeNull());
    const back = await screen.findByLabelText<HTMLTextAreaElement>("Message");
    expect(back.value).toBe("My draft");
    await waitFor(() => expect(document.activeElement).toBe(back));
  });

  it("shows a label per phase, and what the model does while responding", async () => {
    const { voice, simulator } = setup();
    await start();
    await voice.setState({ phase: "hearing" });
    expect(label()).toBe("Listening…");
    // Parole en cours : indicateur vif.
    expect(talk().className).toContain("bg-neutral-900");
    await voice.setState({ phase: "transcribing" });
    expect(label()).toBe("Transcribing…");

    // Réponse qui charge le modèle, puis raisonne.
    const loading: Sequence = {
      ...sequence("existing-chat"),
      events: [
        sequence("existing-chat").events[0],
        { type: "loading" },
        { type: "loaded" },
        ...sequence("existing-chat").events.slice(1),
      ],
    };
    const replay = simulator.queue(loading, { stepwise: true });
    await voice.utter("Say hello in one word.", "en");
    await replay.started;
    expect(label()).toBe("Thinking…");
    await replay.until("loading");
    await waitFor(() => expect(label()).toBe("Loading model…"));
    await replay.until("loaded");
    await waitFor(() => expect(label()).toBe("Thinking…"));
    await replay.finish();

    await voice.setState({ phase: "speaking" });
    expect(label()).toBe("Speaking…");
    expect(bar()!.textContent).toContain("Space: interrupt · Esc: end");
    await voice.setState({ phase: "paused" });
    expect(label()).toBe("Paused — click to resume");
    await userEvent.click(talk());
    expect(simulator.callsTo("voice_mode_resume")).toHaveLength(1);
    await phase("listening");
  });

  it("paused: the label resumes when clicked, the help says Space resumes", async () => {
    const { voice, simulator } = setup();
    await start();
    await voice.setState({ phase: "paused" });
    expect(bar()!.textContent).toContain("Space: resume · Esc: end");
    await userEvent.click(screen.getByText("Paused — click to resume"));
    expect(simulator.callsTo("voice_mode_resume")).toHaveLength(1);
    await phase("listening");
    // Hors de la pause, le libellé ne fait rien.
    await userEvent.click(screen.getByText("Listening…"));
    expect(simulator.callsTo("voice_mode_resume")).toHaveLength(1);
    // Espace aussi reprend.
    await voice.setState({ phase: "paused" });
    press({ key: " " });
    expect(simulator.callsTo("voice_mode_resume")).toHaveLength(2);
  });

  it("the model's download percent is shown, not announced", async () => {
    const { voice, simulator } = setup();
    await start();
    const recorded = sequence("existing-chat");
    const downloading: Sequence = {
      ...recorded,
      events: [
        recorded.events[0],
        { type: "download", total: 200, completed: 50, done: false },
        { type: "download", total: 200, completed: 100, done: false },
        { type: "download", total: 200, completed: 200, done: true },
        ...recorded.events.slice(1),
      ],
    };
    const replay = simulator.queue(downloading, { stepwise: true });
    await voice.utter("Say hello in one word.", "en");
    await replay.started;
    await replay.step(2);
    const status = () => bar()!.querySelector("[role=status]")!.textContent;
    const percent = () =>
      bar()!.querySelector("[data-voice-mode-percent]")?.textContent ?? null;
    await waitFor(() => expect(percent()).toBe("25%"));
    expect(status()).toBe("Downloading model…");
    await replay.step();
    await waitFor(() => expect(percent()).toBe("50%"));
    expect(status()).toBe("Downloading model…");
    await replay.finish();
    await waitFor(() => expect(percent()).toBeNull());
  });

  it("shows the notice of the last turn", async () => {
    const { voice } = setup();
    await start();
    expect(notice()).toBeNull();
    await voice.setState({ notice: { kind: "unsupportedLanguage" } });
    expect(notice()).toBe(
      "This reply isn't in English, French or Spanish: it's written, not read aloud.",
    );
    // Annoncé avec l'étape.
    expect(
      document
        .querySelector("[data-voice-mode-notice]")!
        .closest("[role=status]"),
    ).toBeTruthy();
    await voice.setState({ notice: { kind: "missingVoice", language: "es" } });
    expect(notice()).toBe(
      "The Spanish voice isn't downloaded: this reply is written, not read aloud.",
    );
    await voice.setState({
      notice: { kind: "readFailed", error: "engineFailed" },
    });
    expect(notice()).toBe("Couldn't read this response aloud.");
    await voice.setState({
      notice: { kind: "failed", message: "The microphone was lost" },
    });
    expect(notice()).toBe("The microphone was lost");
  });

  it("turns the microphone off and on", async () => {
    const { voice, simulator } = setup();
    await start();
    const mute = screen.getByRole("button", { name: "Mute microphone" });
    expect(mute.getAttribute("aria-pressed")).toBe("false");
    await userEvent.click(mute);
    expect(simulator.callsTo("voice_mode_mute")).toEqual([{ muted: true }]);
    await waitFor(() =>
      expect(bar()!.getAttribute("data-voice-mode-muted")).toBe("true"),
    );
    expect(label()).toBe("Microphone off");
    // Bouton bascule : même nom, l'état dit par aria-pressed.
    const pressed = screen.getByRole("button", {
      name: "Mute microphone",
      pressed: true,
    });
    expect(pressed).toBe(mute);
    await userEvent.click(mute);
    expect(simulator.callsTo("voice_mode_mute").at(-1)).toEqual({
      muted: false,
    });
    await waitFor(() => expect(label()).toBe("Listening…"));
    expect(voice.state.muted).toBe(false);
  });

  it("ignores dropped files and pastes during the session", async () => {
    const { simulator, backend } = setup();
    backend.files.set("/a/notes.txt", { size: 3 });
    backend.clipboardImage = { size: 42 };
    await start();
    await waitFor(() =>
      expect(simulator.listening("tauri://drag-drop")).toBe(1),
    );
    const position = { x: 10, y: 20 };
    await simulator.emitEvent("tauri://drag-enter", {
      paths: ["/a/notes.txt"],
      position,
    });
    expect(screen.queryByTestId("drop-overlay")).toBeNull();
    await simulator.emitEvent("tauri://drag-drop", {
      paths: ["/a/notes.txt"],
      position,
    });
    const paste = new Event("paste", { bubbles: true, cancelable: true });
    const shot = new File([new Uint8Array([1])], "", { type: "image/png" });
    Object.defineProperty(paste, "clipboardData", {
      value: {
        types: ["Files"],
        getData: () => "",
        items: [{ kind: "file", getAsFile: () => shot }],
        files: [shot],
      },
    });
    fireEvent(document.body, paste);
    const unreadable = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(unreadable, "clipboardData", {
      value: { types: [], getData: () => "", items: [], files: [] },
    });
    fireEvent(document.body, unreadable);
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(simulator.callsTo("stage_paths")).toEqual([]);
    expect(simulator.callsTo("stage_bytes")).toEqual([]);
    expect(simulator.callsTo("stage_clipboard_image")).toEqual([]);
    expect(paste.defaultPrevented).toBe(false);
    // Fin de la séance : rien n'attend dans la zone de saisie.
    await userEvent.click(
      screen.getByRole("button", { name: "End voice mode" }),
    );
    await screen.findByLabelText("Message");
    expect(screen.queryAllByTestId("composer-file")).toEqual([]);
  });

  it("writes the microphone level while listening, the voice level while speaking", async () => {
    const { voice } = setup();
    await start();
    const level = () => bar()!.style.getPropertyValue("--voice-level");
    await voice.level(0.5, 0.9);
    expect(level()).toBe("0.5");
    await voice.setState({ phase: "speaking" });
    await voice.level(0.2, 0.7);
    expect(level()).toBe("0.7");
    await voice.setState({ phase: "transcribing" });
    await voice.level(0.8, 0.8);
    expect(level()).toBe("0");
  });

  it("no moving level when the system asks for less motion", async () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query.includes("reduce"),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
    try {
      const { voice } = setup();
      await start();
      await voice.level(0.6, 0);
      expect(bar()!.style.getPropertyValue("--voice-level")).toBe("0");
      // Pas de pulsation non plus (variante motion-safe seulement).
      expect(bar()!.innerHTML).not.toMatch(/(?<!motion-safe:)animate-pulse/);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("voice mode keyboard", () => {
  it("hands-free: Space ends the turn while speaking, interrupts the reply", async () => {
    const { voice, simulator } = setup();
    await start();
    // En écoute, rien.
    press({ key: " " });
    expect(simulator.callsTo("voice_mode_end_turn")).toHaveLength(0);
    await voice.setState({ phase: "hearing" });
    // preventDefault : pas de défilement de la page.
    expect(press({ key: " " })).toBe(false);
    expect(simulator.callsTo("voice_mode_end_turn")).toHaveLength(1);
    await phase("transcribing");
    await voice.setState({ phase: "responding" });
    press({ key: " " });
    expect(simulator.callsTo("voice_mode_interrupt")).toHaveLength(1);
    await phase("listening");
    await voice.setState({ phase: "speaking" });
    press({ key: " " }, talk());
    expect(simulator.callsTo("voice_mode_interrupt")).toHaveLength(2);
    await phase("listening");
    // Touche maintenue : rien de plus.
    await voice.setState({ phase: "speaking" });
    press({ key: " ", repeat: true });
    expect(simulator.callsTo("voice_mode_interrupt")).toHaveLength(2);
    expect(simulator.callsTo("voice_mode_hold")).toHaveLength(0);
  });

  it("Space stays with a field and with another focused button", async () => {
    const { voice, simulator } = setup();
    await start();
    await voice.setState({ phase: "hearing" });
    const input = document.createElement("input");
    document.body.append(input);
    try {
      press({ key: " " }, input);
      const mute = screen.getByRole("button", {
        name: "Mute microphone",
      });
      press({ key: " " }, mute);
      expect(simulator.callsTo("voice_mode_end_turn")).toHaveLength(0);
      // Échap dans un champ : non plus.
      press({ key: "Escape" }, input);
      expect(simulator.callsTo("voice_mode_stop")).toHaveLength(0);
    } finally {
      input.remove();
    }
  });

  it("push to talk: Space held talks (repeats ignored), released sends", async () => {
    const { voice, simulator } = setup((backend) => {
      backend.voiceMode.settings.turnMode = "pushToTalk";
    });
    await start();
    expect(talk().getAttribute("aria-label")).toBe("Hold to talk");
    expect(bar()!.textContent).toContain("Hold Space to talk · Esc: end");
    press({ key: " " });
    press({ key: " ", repeat: true });
    press({ key: " ", repeat: true });
    expect(simulator.callsTo("voice_mode_hold")).toEqual([{ pressed: true }]);
    expect(talk().getAttribute("aria-pressed")).toBe("true");
    await phase("hearing");
    fireEvent.keyUp(document.body, { key: " " });
    expect(simulator.callsTo("voice_mode_hold")).toEqual([
      { pressed: true },
      { pressed: false },
    ]);
    await phase("transcribing");
    // Pendant la réponse : appuyer la coupe (le moteur s'en charge).
    await voice.setState({ phase: "speaking" });
    press({ key: " " });
    expect(simulator.callsTo("voice_mode_hold").at(-1)).toEqual({
      pressed: true,
    });
    expect(simulator.callsTo("voice_mode_interrupt")).toHaveLength(0);
    fireEvent.keyUp(document.body, { key: " " });
  });

  it("push to talk: the button held with the pointer", async () => {
    const { simulator } = setup((backend) => {
      backend.voiceMode.settings.turnMode = "pushToTalk";
    });
    await start();
    fireEvent.pointerDown(talk(), { button: 0, pointerId: 1 });
    expect(simulator.callsTo("voice_mode_hold")).toEqual([{ pressed: true }]);
    fireEvent.pointerUp(talk(), { button: 0, pointerId: 1 });
    expect(simulator.callsTo("voice_mode_hold")).toEqual([
      { pressed: true },
      { pressed: false },
    ]);
  });

  it("Escape ends voice mode, not the reply being written; not when a dialog took it", async () => {
    const { simulator } = setup();
    // Réponse tapée au clavier, en cours d'écriture.
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
        events: typed.events
          .filter((event) => event.type !== "chatCreated")
          .map((event) =>
            event.type === "cancelled"
              ? { type: "done" as const, messageId: null }
              : event,
          ),
      },
      { stepwise: true },
    );
    await userEvent.type(field, "Typed{Enter}");
    await keyboard.started;
    await keyboard.until("content");
    await start();
    const dialog = document.createElement("div");
    dialog.setAttribute("aria-modal", "true");
    document.body.append(dialog);
    press({ key: "Escape" });
    expect(simulator.callsTo("voice_mode_stop")).toHaveLength(0);
    dialog.remove();
    press({ key: "Escape" });
    expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1);
    await waitFor(() => expect(bar()).toBeNull());
    // La réponse continue : pas d'arrêt demandé, elle se termine.
    expect(simulator.callsTo("cancel_message")).toHaveLength(0);
    await keyboard.finish();
    await keyboard.done;
  });

  it("Ctrl+Shift+Space starts and ends voice mode, even from the message field", async () => {
    const { simulator } = setup();
    await startButton();
    const field = screen.getByLabelText("Message");
    field.focus();
    press({ key: " ", ctrlKey: true, shiftKey: true }, field);
    await phase("listening");
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(1);
    press({ key: " ", ctrlKey: true, shiftKey: true });
    expect(simulator.callsTo("voice_mode_stop")).toHaveLength(1);
    await waitFor(() => expect(bar()).toBeNull());
  });
});

describe("voice mode prerequisites", () => {
  it("this computer can't: the reason above the composer, no bar", async () => {
    const { simulator } = setup((backend) => {
      backend.voiceMode.unavailable = "Voice mode needs ONNX Runtime";
    });
    await userEvent.click(await startButton());
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "Voice mode needs ONNX Runtime",
      ),
    );
    expect(bar()).toBeNull();
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(0);
    expect(screen.getByLabelText("Message")).toBeTruthy();
  });

  it("refused start: the engine's message above the composer", async () => {
    setup((backend) => {
      backend.voiceMode.failures.set("voice_mode_start", {
        kind: "invalid",
        message: "Finish the dictation first",
      });
    });
    await userEvent.click(await startButton());
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "Finish the dictation first",
      ),
    );
    expect(bar()).toBeNull();
  });

  it("missing files: sizes asked first, downloaded, then voice mode starts", async () => {
    const { simulator, backend } = setup((b) => {
      b.dictation.models.find((m) => m.id === PARAKEET)!.installed = false;
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
    });
    await userEvent.click(await startButton());
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByText("Download 2 files for voice mode?"),
    ).toBeTruthy();
    expect(
      within(dialog).getByText(
        "Voice mode needs these files. They're downloaded once from the Internet and stay on this computer.",
      ),
    ).toBeTruthy();
    await waitFor(() =>
      expect(
        [...dialog.querySelectorAll("li")].map((item) => item.textContent),
      ).toEqual([
        "Speech recognition: Parakeet TDT 0.6B v3 (640 MB)",
        "Voice: English · Alba (175 MB)",
      ]),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Download and start" }),
    );
    await waitFor(() => {
      expect(simulator.callsTo("speech_download")).toEqual([{ id: PARAKEET }]);
      expect(simulator.callsTo("voice_download")).toEqual([{ id: VOICE_EN }]);
    });
    expect(bar()!.getAttribute("data-voice-mode-launch")).toBe("downloading");
    await waitFor(() =>
      expect(label()).toBe("Downloading voice mode files… 0%"),
    );
    const speechTotal = backend.dictation.downloads[0].totalBytes;
    const voiceTotal = backend.tts.downloads[0].totalBytes;
    await backend.dictation.progress(PARAKEET, speechTotal / 2);
    await waitFor(() =>
      expect(label()).toBe(
        `Downloading voice mode files… ${Math.floor((speechTotal / 2 / (speechTotal + voiceTotal)) * 100)}%`,
      ),
    );
    // Annoncé : l'étape, pas chaque pourcentage.
    expect(bar()!.querySelector("[role=status]")!.textContent).toBe(
      "Downloading voice mode files…",
    );
    await backend.dictation.finish(PARAKEET);
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(0);
    await backend.tts.finish(VOICE_EN);
    await phase("listening");
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(1);
  });

  it("an old failed download doesn't count; a new failure ends the launch", async () => {
    const { simulator, backend } = setup((b) => {
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
      // Échec d'avant, encore listé.
      b.tts.downloads.push({
        id: VOICE_EN,
        name: "Pocket TTS · English (Alba)",
        state: "failed",
        completedBytes: 10,
        totalBytes: 183_500_000,
        error: "old failure",
      });
    });
    // Liste déjà connue de l'app (ModelHub ouvert plus tôt) : en cache.
    await startButton();
    await backend.tts.emitDownloads();
    await userEvent.click(await startButton());
    const dialog = await screen.findByRole("alertdialog");
    // Seule la voix manque.
    await waitFor(() => expect(dialog.querySelectorAll("li")).toHaveLength(1));
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Download and start" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("voice_download")).toHaveLength(1),
    );
    expect(simulator.callsTo("speech_download")).toHaveLength(0);
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(bar()).not.toBeNull();
    await backend.tts.progress(VOICE_EN, 1000);
    await backend.tts.fail(VOICE_EN, "network down");
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "Couldn't download the files voice mode needs. Try again.",
      ),
    );
    expect(bar()).toBeNull();
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(0);
  });

  it("one missing file: the dialog speaks of one file", async () => {
    setup((b) => {
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
    });
    await userEvent.click(await startButton());
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByText("Download a file for voice mode?"),
    ).toBeTruthy();
    expect(
      within(dialog).getByText(
        "Voice mode needs this file. It's downloaded once from the Internet and stays on this computer.",
      ),
    ).toBeTruthy();
    act(() => setLanguage("fr"));
    expect(dialog.querySelector("h2")?.textContent).toBe(
      "Télécharger un fichier pour le mode vocal\u00a0?",
    );
  });

  it("a file installed while the dialog was open isn't downloaded again", async () => {
    const { simulator, backend } = setup((b) => {
      b.dictation.models.find((m) => m.id === PARAKEET)!.installed = false;
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
    });
    await userEvent.click(await startButton());
    const dialog = await screen.findByRole("alertdialog");
    await waitFor(() => expect(dialog.querySelectorAll("li")).toHaveLength(2));
    // Téléchargement du ModelHub fini pendant que la boîte est ouverte.
    backend.dictation.install(PARAKEET);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Download and start" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("voice_download")).toEqual([{ id: VOICE_EN }]),
    );
    expect(simulator.callsTo("speech_download")).toEqual([]);
    await backend.tts.finish(VOICE_EN);
    await phase("listening");
  });

  it("everything installed while the dialog was open: starts at once", async () => {
    const { simulator, backend } = setup((b) => {
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
    });
    await userEvent.click(await startButton());
    const dialog = await screen.findByRole("alertdialog");
    backend.tts.install(VOICE_EN);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Download and start" }),
    );
    await phase("listening");
    expect(simulator.callsTo("voice_download")).toEqual([]);
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(1);
  });

  it("refusing the download: nothing is downloaded, no bar", async () => {
    const { simulator } = setup((b) => {
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
    });
    await userEvent.click(await startButton());
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    await waitFor(() => expect(bar()).toBeNull());
    expect(simulator.callsTo("voice_download")).toHaveLength(0);
    expect(simulator.callsTo("voice_mode_start")).toHaveLength(0);
    expect(screen.getByLabelText("Message")).toBeTruthy();
  });
});

describe("voice mode texts", () => {
  it("are all translated: button, dialog, bar", async () => {
    const { voice, backend } = setup((b) => {
      b.tts.voices.find((v) => v.id === VOICE_EN)!.installed = false;
    });
    usePseudoLanguage();
    const button = await startButton();
    expect(untranslatedTexts(button.parentElement!)).toEqual([]);
    await userEvent.click(button);
    const dialog = await screen.findByRole("alertdialog");
    await waitFor(() => expect(dialog.querySelectorAll("li")).toHaveLength(1));
    expect(untranslatedTexts(dialog, ["Alba"])).toEqual([]);
    await userEvent.click(within(dialog).getAllByRole("button").at(-1)!);
    await waitFor(() =>
      expect(bar()!.getAttribute("data-voice-mode-launch")).toBe("downloading"),
    );
    expect(untranslatedTexts(bar()!)).toEqual([]);
    await backend.tts.finish(VOICE_EN);
    await phase("listening");
    expect(untranslatedTexts(bar()!)).toEqual([]);
    await voice.setState({
      phase: "speaking",
      muted: true,
      notice: { kind: "missingVoice", language: "fr" },
    });
    expect(untranslatedTexts(bar()!)).toEqual([]);
  });

  it("in French and in Spanish", async () => {
    const { voice } = setup();
    await start();
    act(() => setLanguage("fr"));
    expect(label()).toBe("À l'écoute…");
    expect(bar()!.textContent).toContain("Espace : j'ai fini");
    expect(
      screen.getByRole("button", { name: "Terminer le mode vocal" }),
    ).toBeTruthy();
    act(() => setLanguage("es"));
    await voice.setState({ phase: "paused" });
    expect(label()).toBe("En pausa — haz clic para reanudar");
  });
});
