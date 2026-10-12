// Dictée : statut et téléchargements des modèles de transcription suivis par
// les événements du moteur (réponse en retard sur un événement, relecture
// des modèles à la fin d'un téléchargement), mises à jour des réglages
// (optimistes, en série, relues après un échec), un seul écouteur, monté
// avec l'app.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { DictationSettings } from "@/bindings/DictationSettings";
import type { DictationStatus } from "@/bindings/DictationStatus";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { fakeBackend } from "@/test/backend";
import { type FakeDictation, WHISPER_TINY } from "@/test/dictation";
import { renderHub } from "@/test/renderHub";
import { installSimulator, type Simulator } from "@/test/simulator";
import {
  speechModelsKey,
  useDictationEvents,
  useDictationSettings,
  useDictationStatus,
  useSpeechDownloads,
} from "./useDictation";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(cleanup);

function download(fields: Partial<SpeechDownload>): SpeechDownload {
  return {
    id: WHISPER_TINY,
    name: "Whisper Tiny",
    state: "downloading",
    completedBytes: 0,
    totalBytes: 100,
    error: null,
    ...fields,
  };
}

/** Écouteur de l'app (route racine). */
function Events() {
  useDictationEvents();
  return null;
}

function Probe() {
  const status = useDictationStatus();
  const { downloads, loaded } = useSpeechDownloads();
  const { settings, update } = useDictationSettings();
  return (
    <>
      <p data-testid="phase">{status.data?.phase ?? "loading"}</p>
      <p data-testid="downloads">
        {loaded
          ? downloads.map((d) => `${d.state}:${d.completedBytes}`).join(",") ||
            "none"
          : "loading"}
      </p>
      <p data-testid="settings">
        {settings
          ? `${settings.mode} ${settings.notifyText} ${settings.language}`
          : "loading"}
      </p>
      <button
        type="button"
        onClick={() => {
          update({ mode: "hold" }).catch(() => {});
          update({ notifyText: false }).catch(() => {});
        }}
      >
        two updates
      </button>
    </>
  );
}

/** `prepare` : réglé avant le premier rendu (première lecture). */
function setup(
  prepare?: (simulator: Simulator, dictation: FakeDictation) => void,
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare?.(simulator, backend.dictation);
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Events />
      <Probe />
    </QueryClientProvider>,
  );
  return { simulator, dictation: backend.dictation, client };
}

const text = (id: string) => screen.getByTestId(id).textContent;

async function listening(simulator: Simulator) {
  await waitFor(() => {
    expect(simulator.listening("dictation-status")).toBe(1);
    expect(simulator.listening("speech-downloads")).toBe(1);
  });
}

describe("dictation status", () => {
  it("reads the status, then follows the events", async () => {
    const { simulator, dictation } = setup();
    await waitFor(() => expect(text("phase")).toBe("idle"));
    await listening(simulator);
    await dictation.setStatus({ phase: "recording" });
    await waitFor(() => expect(text("phase")).toBe("recording"));
  });

  it("an answer older than an event does not overwrite it", async () => {
    let answer: ((status: DictationStatus) => void) | null = null;
    const { simulator, dictation } = setup((simulator) =>
      simulator.handle(
        "dictation_status",
        () => new Promise((resolve) => (answer = resolve)),
      ),
    );
    await listening(simulator);
    await waitFor(() => expect(answer).not.toBeNull());
    expect(text("phase")).toBe("loading");
    const older = { ...dictation.status };
    await dictation.setStatus({ phase: "transcribing" });
    act(() => answer!(older));
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(text("phase")).toBe("transcribing");
  });

  it("stops listening when unmounted", async () => {
    const { simulator } = setup();
    await listening(simulator);
    cleanup();
    await waitFor(() => {
      expect(simulator.listening("dictation-status")).toBe(0);
      expect(simulator.listening("speech-downloads")).toBe(0);
    });
  });
});

describe("speech model downloads", () => {
  it("reads the list, then follows the events", async () => {
    const { simulator } = setup();
    await waitFor(() => expect(text("downloads")).toBe("none"));
    await listening(simulator);
    await simulator.emitEvent("speech-downloads", [
      download({ completedBytes: 40 }),
    ]);
    await waitFor(() => expect(text("downloads")).toBe("downloading:40"));
  });

  it("an answer older than an event does not overwrite it", async () => {
    let answer: ((list: SpeechDownload[]) => void) | null = null;
    const { simulator } = setup((simulator) =>
      simulator.handle(
        "speech_downloads",
        () => new Promise((resolve) => (answer = resolve)),
      ),
    );
    await listening(simulator);
    await waitFor(() => expect(answer).not.toBeNull());
    await simulator.emitEvent("speech-downloads", [
      download({ state: "done", completedBytes: 100 }),
    ]);
    act(() => answer!([download({ completedBytes: 10 })]));
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(text("downloads")).toBe("done:100");
  });

  it("a finished download makes the speech models read again, once", async () => {
    const { simulator, client } = setup();
    await listening(simulator);
    client.setQueryData(speechModelsKey, []);
    await simulator.emitEvent("speech-downloads", [
      download({ completedBytes: 50 }),
    ]);
    expect(client.getQueryState(speechModelsKey)?.isInvalidated).toBe(false);

    const done = download({ state: "done", completedBytes: 100 });
    await simulator.emitEvent("speech-downloads", [done]);
    expect(client.getQueryState(speechModelsKey)?.isInvalidated).toBe(true);

    // Le même état reçu de nouveau : rien à relire.
    client.setQueryData(speechModelsKey, []);
    await simulator.emitEvent("speech-downloads", [done]);
    expect(client.getQueryState(speechModelsKey)?.isInvalidated).toBe(false);
  });
});

describe("a finished download seen by a read (review 43, F9)", () => {
  it("reads the speech models again", async () => {
    const { simulator, dictation, client } = setup((_, dictation) => {
      dictation.downloads.push(download({ completedBytes: 50 }));
    });
    await waitFor(() => expect(text("downloads")).toBe("downloading:50"));
    client.setQueryData(speechModelsKey, []);
    // Fin manquée par l'événement, vue à la relecture.
    dictation.downloads[0].state = "done";
    await act(() =>
      client.invalidateQueries({ queryKey: ["speech", "downloads"] }),
    );
    await waitFor(() => expect(text("downloads")).toBe("done:50"));
    expect(client.getQueryState(speechModelsKey)?.isInvalidated).toBe(true);
    expect(simulator.listening("speech-downloads")).toBe(1);
  });
});

describe("dictation settings updates", () => {
  /** update_dictation_settings retenu jusqu'à `release()` ou `reject()`. */
  function hold(simulator: Simulator, dictation: FakeDictation) {
    const held: {
      patch: DictationPatch;
      release: () => void;
      reject: (error: unknown) => void;
    }[] = [];
    simulator.handle("update_dictation_settings", (args) => {
      const patch = args.patch as DictationPatch;
      return new Promise<DictationSettings>((resolve, reject) => {
        held.push({
          patch,
          release: () => {
            dictation.settings = { ...dictation.settings, ...patch };
            resolve({ ...dictation.settings });
          },
          reject,
        });
      });
    });
    return held;
  }

  it("show at once, go in order, and the first answer does not undo the second change", async () => {
    const { simulator, dictation } = setup();
    const held = hold(simulator, dictation);
    await waitFor(() => expect(text("settings")).toBe("toggle true auto"));
    act(() => screen.getByRole("button", { name: "two updates" }).click());
    // Les deux changements montrés avant toute réponse.
    await waitFor(() => expect(text("settings")).toBe("hold false auto"));
    // En série : la seconde attend la réponse de la première.
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].patch).toEqual({ mode: "hold" });
    await act(async () => held[0].release());
    // Réponse de la première (notifyText encore vrai) : pas montrée.
    expect(text("settings")).toBe("hold false auto");
    await waitFor(() => expect(held).toHaveLength(2));
    expect(held[1].patch).toEqual({ notifyText: false });
    await act(async () => held[1].release());
    expect(text("settings")).toBe("hold false auto");
  });

  it("a refused change is undone by reading the settings again", async () => {
    const { simulator, dictation } = setup();
    const held = hold(simulator, dictation);
    await waitFor(() => expect(text("settings")).toBe("toggle true auto"));
    act(() => screen.getByRole("button", { name: "two updates" }).click());
    await waitFor(() => expect(held).toHaveLength(1));
    // La seconde est refusée (la première a été enregistrée).
    await act(async () => held[0].release());
    await waitFor(() => expect(held).toHaveLength(2));
    const reads = simulator.callsTo("get_dictation_settings").length;
    await act(async () =>
      held[1].reject({ kind: "database", message: "disk full" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("get_dictation_settings").length).toBe(
        reads + 1,
      ),
    );
    await waitFor(() => expect(text("settings")).toBe("hold true auto"));
  });
});

describe("dictation events in the app", () => {
  it("listen once, whatever the page", async () => {
    const { simulator } = renderHub("/hub/downloads");
    await screen.findByText("No downloads yet.");
    await waitFor(() => {
      expect(simulator.listening("dictation-status")).toBe(1);
      expect(simulator.listening("speech-downloads")).toBe(1);
      expect(simulator.listening("navigate")).toBe(1);
    });
  });
});
