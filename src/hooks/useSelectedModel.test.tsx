// Modèle choisi à l'ouverture d'une conversation d'avant la 0.10 (anciens
// noms d'Ollama dans l'historique) face au simulateur, qui a des modèles
// repris (`@ollama/…`) et de Hugging Face, et la table des noms repris du
// moteur ; fin d'une reprise suivie hors de Settings › Models : réglages et
// modèles relus.

import { act, configure, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { renderApp, shownModel } from "@/test/renderApp";
import { installSimulator, type Simulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const OLLAMA = "@ollama/qwen3:0.6b";
const HF = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
/** Identifiant de repli d'un modèle repris (hôte et espace gardés). */
const FALLBACK = "@ollama/registry.ollama.ai/library/llama3:latest";

function reply(model: string): Message {
  return {
    id: 2,
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

/**
 * Conversation `old` dont le dernier message porte `model`, ouverte (ou la
 * page `path`).
 */
function setup(
  model: string,
  selectedModel: string,
  prepare: (backend: FakeBackend) => void = () => {},
  path = "/c/old",
) {
  const simulator = installSimulator({ latencyMs: 5 });
  const backend = fakeBackend(simulator, {
    selectedModel,
    models: [OLLAMA, HF, FALLBACK],
  });
  backend.renamed.set("llama3", FALLBACK);
  backend.chats.set("old", {
    id: "old",
    title: "Old",
    createdAt: 1,
    updatedAt: 1,
    messages: [reply(model)],
  });
  prepare(backend);
  const app = renderApp(path);
  return { simulator, backend, app };
}

const selections = (simulator: Simulator) =>
  simulator
    .callsTo("update_settings")
    .map((args) => (args.patch as SettingsPatch).selectedModel)
    .filter((model) => model !== undefined);

const settle = () =>
  act(() => new Promise((resolve) => setTimeout(resolve, 200)));

describe("the model of a chat from before 0.10", () => {
  it("an old Ollama name taken over keeps its new id", async () => {
    const { simulator, backend } = setup("qwen3:0.6b", OLLAMA);
    await screen.findByTestId("chat");
    await settle();
    expect(selections(simulator)).toEqual([]);
    expect(backend.settings.selectedModel).toBe(OLLAMA);
    expect(shownModel()).toBe(OLLAMA);
  });

  it("an old name becomes the selected model under its new id", async () => {
    const { simulator, backend } = setup("qwen3:0.6b", HF);
    await waitFor(() => expect(backend.settings.selectedModel).toBe(OLLAMA));
    expect(selections(simulator)).toEqual([OLLAMA]);
    await waitFor(() => expect(shownModel()).toBe(OLLAMA));
  });

  it("an old Hugging Face name of Ollama becomes its id", async () => {
    const { backend } = setup(`hf.co/${HF}`, OLLAMA);
    await waitFor(() => expect(backend.settings.selectedModel).toBe(HF));
  });

  it("an old name renamed otherwise is translated by the engine", async () => {
    const { simulator, backend } = setup("llama3", OLLAMA);
    await waitFor(() => expect(backend.settings.selectedModel).toBe(FALLBACK));
    await settle();
    expect(selections(simulator)).toEqual(["llama3"]);
    await waitFor(() => expect(shownModel()).toBe(FALLBACK));
  });

  it("an old name that was never taken over leaves the selected model", async () => {
    const { simulator, backend } = setup("phi4:14b", OLLAMA);
    await waitFor(() =>
      expect(selections(simulator)).toEqual(["phi4:14b", OLLAMA]),
    );
    await settle();
    expect(backend.settings.selectedModel).toBe(OLLAMA);
    expect(shownModel()).toBe(OLLAMA);
  });
});

describe("the end of an import of Ollama models", () => {
  it("reads the settings and the models again, outside Settings › Models", async () => {
    const { simulator, backend } = setup(
      OLLAMA,
      "llama3",
      // Premier lancement : réglage encore sous l'ancien nom, modèle pas
      // encore repris.
      (backend) => {
        backend.models = [OLLAMA, HF];
      },
      "/c/new",
    );
    await waitFor(() => expect(shownModel()).toBe("llama3"));
    await waitFor(() =>
      expect(simulator.listening("model-import")).toBeGreaterThan(0),
    );
    const reads = simulator.callsTo("get_settings").length;
    // La reprise automatique finit : modèle repris, réglage réécrit.
    backend.models = [FALLBACK, OLLAMA, HF];
    backend.settings = { ...backend.settings, selectedModel: FALLBACK };
    backend.modelImport.state = {
      running: null,
      last: {
        origin: "builtin",
        dir: "/data/ollama",
        imported: [{ name: "llama3:latest", id: FALLBACK }],
        redownload: [],
        failed: [],
        cancelled: false,
        error: null,
      },
    };
    await backend.modelImport.emit();
    await waitFor(() =>
      expect(simulator.callsTo("get_settings").length).toBeGreaterThan(reads),
    );
    await waitFor(() => expect(shownModel()).toBe(FALLBACK));
  });
});
