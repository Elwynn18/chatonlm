// Préchargement du modèle choisi (« Preload the chosen model ») face au
// simulateur : au démarrage, une fois par adresse, modèle et contexte (pas
// la durée avant déchargement), une seconde après le dernier changement ;
// pas sans l'option (rien de lu pour lui), pas pour un modèle non installé,
// pas sans Ollama ; au changement de modèle (sélecteur, conversation
// ouverte) ; l'option activée.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import type { Settings } from "@/bindings/Settings";
import { forgetPreloads, PRELOAD_DELAY_MS } from "@/hooks/usePreloadModel";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { modelsKey } from "@/hooks/useModels";
import { healthKey } from "@/hooks/useServerHealth";
import { settingsKey } from "@/hooks/useSettings";
import { type FakeBackend, fakeBackend, modelInfo } from "@/test/backend";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  forgetPreloads();
  vi.useRealTimers();
});

function setup(
  path: string,
  settings: Partial<Settings> = {},
  prepare: (backend: FakeBackend) => void = () => {},
) {
  const simulator = installSimulator({ latencyMs: 5 });
  const backend = fakeBackend(simulator);
  Object.assign(backend.settings, { preloadModel: true, ...settings });
  prepare(backend);
  const app = renderApp(path);
  const loads = () =>
    simulator.callsTo("load_model").map((args) => args.model as string);
  return { simulator, backend, app, loads };
}

/**
 * Laisse passer les lectures (réglages, modèles, état d'Ollama) et le délai
 * du préchargement : rien n'est parti d'ici, rien ne partira.
 */
const settle = (ms = PRELOAD_DELAY_MS + 400) =>
  act(() => new Promise((resolve) => setTimeout(resolve, ms)));

/** Modèle affiché par le sélecteur. */
async function pickerShows(model: string) {
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe(model),
  );
}

async function choose(model: string) {
  await userEvent.click(screen.getByTitle("Select model"));
  await userEvent.click(await screen.findByRole("option", { name: model }));
  await pickerShows(model);
}

function reply(id: number, model: string): Message {
  return {
    id,
    role: "assistant",
    content: "Hi",
    thinking: "",
    model,
    createdAt: 1,
    updatedAt: 1,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

describe("preloading the chosen model", () => {
  it("loads it once when ChatonLM starts", async () => {
    const { app, loads, backend } = setup("/c/new");
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    expect(backend.running.map((m) => m.name)).toEqual(["qwen3:0.6b"]);
    // Autres pages, retour : pas de nouveau chargement.
    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await act(() =>
      app.router.navigate({ to: "/c/$chatId", params: { chatId: "new" } }),
    );
    await pickerShows("qwen3:0.6b");
    await settle();
    expect(loads()).toEqual(["qwen3:0.6b"]);
  });

  it("reads nothing for it while the option is off", async () => {
    // General ne lit ni la liste des modèles ni l'état d'Ollama.
    const { simulator, loads } = setup("/settings", { preloadModel: false });
    await screen.findByText("Server address");
    await settle();
    expect(loads()).toEqual([]);
    expect(simulator.callsTo("list_models")).toEqual([]);
    expect(simulator.callsTo("server_status")).toEqual([]);
  });

  it("reads the models and the state of Ollama from any page once it is on", async () => {
    const { simulator, loads } = setup("/settings");
    await screen.findByText("Server address");
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    expect(simulator.callsTo("list_models").length).toBeGreaterThan(0);
    expect(simulator.callsTo("server_status").length).toBeGreaterThan(0);
  });

  it("does not load a model that is not installed, then loads the one chosen instead", async () => {
    const { loads } = setup("/c/new", { selectedModel: "llama9:70b" });
    await pickerShows("llama9:70b");
    await settle();
    expect(loads()).toEqual([]);
    await choose("qwen3.5:0.8b");
    await waitFor(() => expect(loads()).toEqual(["qwen3.5:0.8b"]));
  });

  it("loads each model chosen in the picker, once per session", async () => {
    const { loads } = setup("/c/new");
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    await choose("qwen3.5:0.8b");
    await waitFor(() =>
      expect(loads()).toEqual(["qwen3:0.6b", "qwen3.5:0.8b"]),
    );
    await choose("qwen3:0.6b");
    await settle();
    expect(loads()).toEqual(["qwen3:0.6b", "qwen3.5:0.8b"]);
  });

  it("loads the model of the chat that is opened", async () => {
    const { loads } = setup("/c/chat-9", {}, (backend) => {
      backend.chats.set("chat-9", {
        id: "chat-9",
        title: "Old",
        createdAt: 1,
        updatedAt: 1,
        messages: [reply(1, "qwen3.5:0.8b")],
      });
    });
    await pickerShows("qwen3.5:0.8b");
    await waitFor(() => expect(loads()).toContain("qwen3.5:0.8b"));
  });

  it("waits for the choice to stay a second: one load for five steps of Context length", async () => {
    const { backend, loads } = setup("/settings");
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    await screen.findByRole("slider", { name: "Context length" });
    vi.useFakeTimers();
    const slider = () => screen.getByRole("slider", { name: "Context length" });
    act(() => slider().focus());
    for (let i = 0; i < 5; i++) {
      fireEvent.keyDown(slider(), { key: "ArrowRight" });
    }
    // Réponses du backend (5 ms chacune) et rendus.
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(backend.settings.contextLength).toBe(65536);
    await act(() => vi.advanceTimersByTimeAsync(700));
    expect(loads()).toHaveLength(1);
    await act(() => vi.advanceTimersByTimeAsync(300));
    expect(loads()).toHaveLength(2);
    // Chargé avec le dernier contexte (le moteur le lit dans les réglages).
    expect(backend.running.map((m) => m.contextLength)).toEqual([65536]);
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(loads()).toHaveLength(2);

    // Retour sur un cran traversé tout à l'heure : jamais chargé, il l'est.
    for (let i = 0; i < 3; i++) {
      fireEvent.keyDown(slider(), { key: "ArrowLeft" });
    }
    await act(() => vi.advanceTimersByTimeAsync(1500));
    expect(backend.settings.contextLength).toBe(8192);
    expect(loads()).toHaveLength(3);
    expect(backend.running.map((m) => m.contextLength)).toEqual([8192]);
  });

  it("does not load again when the keep alive changes", async () => {
    const { simulator, backend, loads } = setup("/settings?section=models");
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    const slider = await screen.findByRole("slider", {
      name: "Keep models loaded",
    });
    // Déchargé exprès : changer la durée ne doit pas le remettre en mémoire.
    backend.running = [];
    vi.useFakeTimers();
    act(() => slider.focus());
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    await act(() => vi.advanceTimersByTimeAsync(3000));
    expect(simulator.callsTo("update_settings")).toEqual([
      { patch: { keepAlive: 60 } },
    ]);
    expect(loads()).toEqual(["qwen3:0.6b"]);
  });

  it("waits for Ollama to answer", async () => {
    const { app, simulator, backend, loads } = setup(
      "/c/new",
      {},
      (backend) => {
        backend.serverState = "connectionFailed";
      },
    );
    // Comme le moteur : Ollama injoignable, la liste des modèles aussi.
    simulator.handle("list_models", () => {
      if (backend.serverState !== "running") {
        throw { kind: "ollama", message: "Could not connect to Ollama" };
      }
      return backend.models.map(modelInfo);
    });
    await screen.findByText("Ollama isn't responding.");
    await settle();
    expect(loads()).toEqual([]);
    // De retour : les vérifications suivantes le voient (sans attendre
    // leurs délais ici).
    backend.serverState = "running";
    await act(async () => {
      await app.queryClient.invalidateQueries({ queryKey: healthKey });
      await app.queryClient.invalidateQueries({ queryKey: modelsKey });
    });
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
  });

  it("not while Ollama is known to be down, even with the model list read", async () => {
    const { app, backend, loads } = setup("/c/new", { preloadModel: false });
    await pickerShows("qwen3:0.6b");
    backend.serverState = "connectionFailed";
    await screen.findByText("Ollama isn't responding.", undefined, {
      timeout: 5000,
    });
    // Option activée pendant la panne (autre fenêtre de réglages...).
    await act(async () => {
      backend.settings = { ...backend.settings, preloadModel: true };
      await app.queryClient.invalidateQueries({ queryKey: settingsKey });
    });
    await settle();
    expect(loads()).toEqual([]);
  });

  it("not while the last read of the models failed", async () => {
    const { app, simulator, backend, loads } = setup("/c/new", {
      preloadModel: false,
    });
    await pickerShows("qwen3:0.6b");
    // Liste lue, puis Ollama ne répond plus pour elle ; ses nouveaux essais
    // (useModels) passés d'un coup.
    simulator.handle("list_models", () => {
      throw { kind: "ollama", message: "Could not connect to Ollama" };
    });
    vi.useFakeTimers();
    await act(async () => {
      const refetch = app.queryClient.refetchQueries({ queryKey: modelsKey });
      await vi.advanceTimersByTimeAsync(60_000);
      await refetch;
    });
    vi.useRealTimers();
    expect(app.queryClient.getQueryState(modelsKey)?.status).toBe("error");
    expect(app.queryClient.getQueryData(modelsKey)).toBeDefined();
    await act(async () => {
      backend.settings = { ...backend.settings, preloadModel: true };
      await app.queryClient.invalidateQueries({ queryKey: settingsKey });
    });
    await settle();
    expect(loads()).toEqual([]);
  });

  it("ignores a failed load, without trying again", async () => {
    const { app, loads } = setup("/c/new", {}, (backend) => {
      backend.runningErrors.load.set("qwen3:0.6b", {
        kind: "ollama",
        message: "model requires more system memory",
      });
    });
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    await settle();
    expect(screen.queryByRole("alert")).toBeNull();
    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await settle();
    expect(loads()).toEqual(["qwen3:0.6b"]);
  });

  it("turning the option on loads the current model, even again", async () => {
    const { app, backend, loads } = setup("/settings?section=models", {
      preloadModel: false,
    });
    const invalidate = vi.spyOn(app.queryClient, "invalidateQueries");
    const toggle = await screen.findByRole("switch", {
      name: "Preload the chosen model",
    });
    await settle();
    expect(loads()).toEqual([]);
    await userEvent.click(toggle);
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b"]));
    // Le modèle apparaît dans la carte sans attendre la relecture suivante.
    await waitFor(() =>
      expect(invalidate).toHaveBeenCalledWith({ queryKey: runningModelsKey }),
    );
    await waitFor(() =>
      expect(
        screen.getAllByTestId("loaded-model").map((row) => row.dataset.model),
      ).toEqual(["qwen3:0.6b"]),
    );
    // Déchargé, option coupée puis remise : chargé de nouveau.
    await userEvent.click(screen.getByRole("button", { name: "Unload" }));
    await screen.findByText("No models are loaded.");
    await userEvent.click(toggle);
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("false"),
    );
    await userEvent.click(toggle);
    await waitFor(() => expect(loads()).toEqual(["qwen3:0.6b", "qwen3:0.6b"]));
    expect(backend.running.map((m) => m.name)).toEqual(["qwen3:0.6b"]);
  });
});
