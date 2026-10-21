// Onglet « Models » des réglages face au simulateur : durée avant
// déchargement (curseur), préchargement (interrupteur), modèles en mémoire
// (résumé, détails, Unload, Unload all), chargement d'un modèle installé,
// erreurs, relecture toutes les 2 s tant que l'onglet est affiché.

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
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { forgetPreloads } from "@/hooks/usePreloadModel";
import { machineMemoryKey } from "@/hooks/useMachineMemory";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { setLanguage } from "@/i18n";
import {
  type FakeBackend,
  fakeBackend,
  RUNNING_SIZE,
  runningModel,
} from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator, type Simulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  forgetPreloads();
});

const KEEP_DESCRIPTION =
  "How long a model stays in memory after its last use. Default follows the Ollama server (5 minutes unless OLLAMA_KEEP_ALIVE is set).";
const PRELOAD_DESCRIPTION =
  "Load the model selected in the chat as soon as you choose it and when ChatonLM starts, so your first message doesn't wait.";

const MINUTE = 60_000;

/** Onglet Models affiché, après `prepare` du faux backend. */
function setup(
  prepare: (backend: FakeBackend) => void = () => {},
  settings: Partial<Settings> = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    models: ["qwen3:0.6b", "qwen3.5:0.8b", "gemma3:270m"],
  });
  Object.assign(backend.settings, settings);
  prepare(backend);
  const app = renderApp("/settings?section=models");
  return { simulator, backend, app };
}

/** Promesse tenue par le test. */
function deferred() {
  let resolve: () => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<null>((ok, fail) => {
    resolve = () => ok(null);
    reject = fail;
  });
  return { promise, resolve, reject };
}

const patches = (simulator: Simulator) =>
  simulator
    .callsTo("update_settings")
    .map((args) => args.patch as SettingsPatch);

const keepSlider = () =>
  screen.getByRole("slider", { name: "Keep models loaded" });
const preloadSwitch = () =>
  screen.getByRole<HTMLButtonElement>("switch", {
    name: "Preload the chosen model",
  });
const unloadOnQuitSwitch = () =>
  screen.getByRole<HTMLButtonElement>("switch", {
    name: "Unload models when quitting",
  });
const uncensoredSwitch = () =>
  screen.getByRole<HTMLButtonElement>("switch", {
    name: "Show uncensored models",
  });
const cloudSwitch = () =>
  screen.getByRole<HTMLButtonElement>("switch", { name: "Use cloud models" });
const SERVER_CLOUD_OFF =
  "This Ollama server has its cloud turned off: cloud models stay hidden.";

/** Carte « Loaded models », liste lue. */
async function loadedCard() {
  const card = await screen.findByTestId("loaded-models");
  await waitFor(() =>
    expect(within(card).queryByTestId("loaded-models-loading")).toBeNull(),
  );
  return card;
}

/** Lignes des modèles en mémoire : [nom, détails]. */
function rows() {
  return screen
    .queryAllByTestId("loaded-model")
    .map((row) => [
      row.getAttribute("data-model"),
      row.querySelector("p")?.textContent,
    ]);
}

/**
 * Bouton inactif pendant une action, mais focalisable (aria-disabled) : il
 * garde le focus.
 */
const inactive = (button: HTMLElement) =>
  button.getAttribute("aria-disabled") === "true" &&
  !(button as HTMLButtonElement).disabled;

const loadSelect = () =>
  screen.getByLabelText<HTMLSelectElement>("Load a model");
const optionTexts = (select: HTMLSelectElement) =>
  [...select.options].map((option) => option.text);

describe("Settings: Models", () => {
  describe("loading settings", () => {
    it("shows the keep-alive steps and the preload switch, off by default", async () => {
      setup();
      await screen.findByTestId("models-settings");
      expect(
        screen.getByTestId("settings-tab-models").getAttribute("aria-current"),
      ).toBe("page");
      const slider = keepSlider();
      expect(slider.getAttribute("aria-valuetext")).toBe("Default");
      const description = document.getElementById(
        slider.getAttribute("aria-describedby")!,
      );
      expect(description?.textContent).toBe(KEEP_DESCRIPTION);
      const steps = slider.closest("[class*=justify-between]")!;
      expect(
        [...steps.querySelectorAll(".text-xs")].map(
          (label) => label.textContent,
        ),
      ).toEqual([
        "Default",
        "1 min",
        "5 min",
        "15 min",
        "30 min",
        "1 hour",
        "4 hours",
        "Always",
      ]);
      expect(preloadSwitch().getAttribute("aria-checked")).toBe("false");
      expect(
        document.getElementById(
          preloadSwitch().getAttribute("aria-describedby")!,
        )?.textContent,
      ).toBe(PRELOAD_DESCRIPTION);
      expect(screen.getByText("Keep models loaded")).toBeTruthy();
      expect(screen.getByText("Preload the chosen model")).toBeTruthy();
      // Rien de General ici (Reset n'y touche pas).
      expect(screen.queryByRole("button", { name: "Reset to defaults" })).toBe(
        null,
      );
    });

    it.each([
      [-1, "Always"],
      [3600, "1 hour"],
      [14400, "4 hours"],
    ])("shows the saved keep alive %i as %s", async (keepAlive, label) => {
      setup(() => {}, { keepAlive });
      await screen.findByTestId("models-settings");
      expect(keepSlider().getAttribute("aria-valuetext")).toBe(label);
    });

    it("saves each step with its value in seconds, then shows Saved", async () => {
      const { simulator, backend } = setup();
      await screen.findByTestId("models-settings");
      act(() => keepSlider().focus());
      fireEvent.keyDown(keepSlider(), { key: "ArrowRight" });
      await screen.findByText("Saved");
      await waitFor(() =>
        expect(keepSlider().getAttribute("aria-valuetext")).toBe("1 min"),
      );
      fireEvent.keyDown(keepSlider(), { key: "End" });
      await waitFor(() =>
        expect(keepSlider().getAttribute("aria-valuetext")).toBe("Always"),
      );
      fireEvent.keyDown(keepSlider(), { key: "ArrowLeft" });
      await waitFor(() =>
        expect(keepSlider().getAttribute("aria-valuetext")).toBe("4 hours"),
      );
      expect(patches(simulator)).toEqual([
        { keepAlive: 60 },
        { keepAlive: -1 },
        { keepAlive: 14400 },
      ]);
      expect(backend.settings.keepAlive).toBe(14400);
    });

    it("keeps the chosen step while it is saved, and goes back on an error", async () => {
      const { simulator, backend } = setup(() => {}, { keepAlive: 300 });
      const held = deferred();
      simulator.handle("update_settings", () => held.promise);
      await screen.findByTestId("models-settings");
      act(() => keepSlider().focus());
      fireEvent.keyDown(keepSlider(), { key: "ArrowRight" });
      await waitFor(() => expect(patches(simulator)).toHaveLength(1));
      expect(keepSlider().getAttribute("aria-valuetext")).toBe("15 min");
      await act(async () =>
        held.reject({ kind: "database", message: "disk full" }),
      );
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Couldn't save this setting: disk full",
      );
      expect(keepSlider().getAttribute("aria-valuetext")).toBe("5 min");
      expect(screen.queryByText("Saved")).toBeNull();
      expect(backend.settings.keepAlive).toBe(300);
    });

    it("turns preloading on and off", async () => {
      const { simulator } = setup();
      await screen.findByTestId("models-settings");
      await userEvent.click(preloadSwitch());
      await screen.findByText("Saved");
      await waitFor(() =>
        expect(preloadSwitch().getAttribute("aria-checked")).toBe("true"),
      );
      await userEvent.click(preloadSwitch());
      await waitFor(() =>
        expect(preloadSwitch().getAttribute("aria-checked")).toBe("false"),
      );
      expect(patches(simulator)).toEqual([
        { preloadModel: true },
        { preloadModel: false },
      ]);
    });

    it("shows the uncensored section of ModelHub on demand, hidden by default", async () => {
      const { simulator, backend } = setup();
      await screen.findByTestId("models-settings");
      expect(uncensoredSwitch().getAttribute("aria-checked")).toBe("false");
      expect(
        document.getElementById(
          uncensoredSwitch().getAttribute("aria-describedby")!,
        )?.textContent,
      ).toBe(
        "Adds an “Uncensored” section to ModelHub: community versions of official models that no longer refuse to answer. They can write offensive or dangerous content, and ChatonLM doesn't check them.",
      );
      await userEvent.click(uncensoredSwitch());
      await screen.findByText("Saved");
      await waitFor(() =>
        expect(uncensoredSwitch().getAttribute("aria-checked")).toBe("true"),
      );
      expect(backend.settings.showUncensored).toBe(true);
      await userEvent.click(uncensoredSwitch());
      await waitFor(() =>
        expect(uncensoredSwitch().getAttribute("aria-checked")).toBe("false"),
      );
      expect(patches(simulator)).toEqual([
        { showUncensored: true },
        { showUncensored: false },
      ]);
    });

    it("unloads the models when quitting unless turned off, on by default", async () => {
      const { simulator, backend } = setup();
      await screen.findByTestId("models-settings");
      expect(unloadOnQuitSwitch().getAttribute("aria-checked")).toBe("true");
      expect(
        document.getElementById(
          unloadOnQuitSwitch().getAttribute("aria-describedby")!,
        )?.textContent,
      ).toBe(
        "When you quit ChatonLM, the models Ollama still holds in memory are unloaded, freeing system memory and VRAM. Closing the window while ChatonLM keeps running in the system tray leaves them loaded.",
      );
      await userEvent.click(unloadOnQuitSwitch());
      await screen.findByText("Saved");
      await waitFor(() =>
        expect(unloadOnQuitSwitch().getAttribute("aria-checked")).toBe("false"),
      );
      expect(backend.settings.unloadOnQuit).toBe(false);
      await userEvent.click(unloadOnQuitSwitch());
      await waitFor(() =>
        expect(unloadOnQuitSwitch().getAttribute("aria-checked")).toBe("true"),
      );
      expect(patches(simulator)).toEqual([
        { unloadOnQuit: false },
        { unloadOnQuit: true },
      ]);
    });

    it("uses the cloud models only once turned on, off by default", async () => {
      const { simulator, backend } = setup();
      await screen.findByTestId("models-settings");
      expect(cloudSwitch().getAttribute("aria-checked")).toBe("false");
      expect(
        document.getElementById(cloudSwitch().getAttribute("aria-describedby")!)
          ?.textContent,
      ).toBe(
        "Adds Ollama's cloud models to the model picker. They run on ollama.com, not on this computer: the messages you send them leave this computer. They need an Ollama account (Account tab) and count against its plan.",
      );
      // Coupé : l'état du cloud du serveur n'est pas demandé.
      expect(simulator.callsTo("cloud_status")).toEqual([]);
      await userEvent.click(cloudSwitch());
      await screen.findByText("Saved");
      await waitFor(() =>
        expect(cloudSwitch().getAttribute("aria-checked")).toBe("true"),
      );
      expect(backend.settings.cloudModels).toBe(true);
      expect(screen.queryByText(SERVER_CLOUD_OFF)).toBeNull();
      await userEvent.click(cloudSwitch());
      await waitFor(() =>
        expect(cloudSwitch().getAttribute("aria-checked")).toBe("false"),
      );
      expect(patches(simulator)).toEqual([
        { cloudModels: true },
        { cloudModels: false },
      ]);
    });

    it("says when the server has its cloud turned off", async () => {
      setup(
        (backend) => {
          backend.cloud = { disabled: true, source: "env" };
        },
        { cloudModels: true },
      );
      await screen.findByTestId("models-settings");
      expect(await screen.findByText(SERVER_CLOUD_OFF)).toBeTruthy();
    });

    it("shows why the switch was not saved", async () => {
      const { backend } = setup();
      backend.settingsErrors.update = { kind: "database", message: "locked" };
      await screen.findByTestId("models-settings");
      await userEvent.click(preloadSwitch());
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Couldn't save this setting: locked",
      );
      expect(preloadSwitch().getAttribute("aria-checked")).toBe("false");
    });
  });

  describe("loaded models", () => {
    it("says when no model is loaded, without Unload all", async () => {
      setup();
      const card = await loadedCard();
      expect(within(card).getByRole("heading").textContent).toBe(
        "Loaded models",
      );
      expect(within(card).getByText("No models are loaded.")).toBeTruthy();
      expect(within(card).queryByRole("button", { name: "Unload all" })).toBe(
        null,
      );
      expect(rows()).toEqual([]);
    });

    it("lists each model with its size, processor, context and unloading time", async () => {
      const now = Date.now();
      setup((backend) => {
        backend.running = [
          runningModel("qwen3:0.6b", {
            expiresAt: now + 4 * MINUTE + 30_000,
          }),
          runningModel("big-split:8b", {
            size: 4 * RUNNING_SIZE,
            sizeVram: 3 * RUNNING_SIZE,
            contextLength: 32768,
            expiresAt: null,
            kept: true,
          }),
          runningModel("cpu-only:1b", {
            sizeVram: 0,
            contextLength: 8000,
            expiresAt: now - MINUTE,
          }),
          runningModel("odd:1b", { contextLength: null, expiresAt: null }),
        ];
      });
      await loadedCard();
      expect(rows()).toEqual([
        ["qwen3:0.6b", "1.0 GB · 100% GPU · 4k context · Unloads in 4 minutes"],
        [
          "big-split:8b",
          "4.0 GB · 25%/75% CPU/GPU · 32k context · Stays loaded",
        ],
        ["cpu-only:1b", "1.0 GB · 100% CPU · 8000 context · Unloads soon"],
        ["odd:1b", "1.0 GB · 100% GPU"],
      ]);
      expect(screen.getByTestId("loaded-models-summary").textContent).toBe(
        "4 models loaded · 7.0 GB in memory",
      );
      // Escargot : le seul modèle partagé avec le processeur.
      const snails = screen.getAllByTestId("slow-model");
      expect(snails.map((snail) => snail.getAttribute("aria-label"))).toEqual([
        "Slower replies: 25% of big-split:8b runs on the processor (not enough graphics memory)",
      ]);
      expect(
        snails[0]
          .closest("[data-testid=loaded-model]")
          ?.getAttribute("data-model"),
      ).toBe("big-split:8b");
    });

    it("says 1 model loaded, and shows a Hugging Face name like the picker", async () => {
      setup((backend) => {
        backend.running = [
          runningModel("hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M", {
            expiresAt: Date.now() + 90_000,
          }),
        ];
      });
      await loadedCard();
      expect(screen.getByTestId("loaded-models-summary").textContent).toBe(
        "1 model loaded · 1.0 GB in memory",
      );
      const row = screen.getByTestId("loaded-model");
      // Titre et pastille d'étiquette, comme le sélecteur.
      expect(row.querySelector("[title]")?.textContent).toBe(
        "SmolLM2-135M-InstructQ4_K_M",
      );
      expect(row.querySelector("p")?.textContent).toBe(
        "1.0 GB · 100% GPU · 4k context · Unloads in 1 minute",
      );
    });

    it("unloads a model: Unloading… until the list is read again", async () => {
      const { simulator, backend } = setup((backend) => {
        backend.running = [
          runningModel("qwen3:0.6b"),
          runningModel("gemma3:270m"),
        ];
      });
      const held = deferred();
      simulator.handle("unload_model", (args) =>
        held.promise.then(() => {
          backend.running = backend.running.filter(
            (m) => m.name !== args.model,
          );
          return null;
        }),
      );
      await loadedCard();
      const row = screen
        .getAllByTestId("loaded-model")
        .find((r) => r.getAttribute("data-model") === "qwen3:0.6b")!;
      await userEvent.click(
        within(row).getByRole("button", { name: "Unload" }),
      );
      const busy = within(row).getByRole<HTMLButtonElement>("button", {
        name: "Unloading…",
      });
      expect(inactive(busy)).toBe(true);
      expect(inactive(screen.getByRole("button", { name: "Unload all" }))).toBe(
        true,
      );
      expect(simulator.callsTo("unload_model")).toEqual([
        { model: "qwen3:0.6b" },
      ]);
      // Inactif : un second clic ne relance rien.
      await userEvent.click(busy);
      expect(simulator.callsTo("unload_model")).toHaveLength(1);
      // Déchargé, liste pas encore relue : la ligne attend toujours.
      const list = deferred();
      const read = simulator.callsTo("running_models").length;
      simulator.handle("running_models", () =>
        list.promise.then(() => backend.running),
      );
      await act(async () => held.resolve());
      await waitFor(() =>
        expect(simulator.callsTo("running_models").length).toBeGreaterThan(
          read,
        ),
      );
      expect(within(row).getByRole("button").textContent).toBe("Unloading…");
      await act(async () => list.resolve());
      await waitFor(() =>
        expect(rows().map(([name]) => name)).toEqual(["gemma3:270m"]),
      );
      expect(screen.queryByText("Unloading…")).toBeNull();
      expect(screen.getByTestId("loaded-models-summary").textContent).toBe(
        "1 model loaded · 1.0 GB in memory",
      );
    });

    it("says why a model could not be unloaded", async () => {
      setup((backend) => {
        backend.running = [runningModel("qwen3:0.6b")];
        backend.runningErrors.unload = {
          kind: "ollama",
          message: "Ollama returned HTTP 500",
        };
      });
      await loadedCard();
      await userEvent.click(screen.getByRole("button", { name: "Unload" }));
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Couldn't unload qwen3:0.6b: Ollama returned HTTP 500",
      );
      expect(rows().map(([name]) => name)).toEqual(["qwen3:0.6b"]);
      expect(screen.getByRole("button", { name: "Unload" })).toBeTruthy();
    });

    it("unloads every model with Unload all", async () => {
      const { simulator, backend } = setup((backend) => {
        backend.running = [
          runningModel("qwen3:0.6b"),
          runningModel("gemma3:270m"),
        ];
      });
      const held = deferred();
      simulator.handle("unload_all_models", () =>
        held.promise.then(() => {
          backend.running = [];
          return null;
        }),
      );
      await loadedCard();
      await userEvent.click(screen.getByRole("button", { name: "Unload all" }));
      // Le bouton et chaque ligne attendent.
      const busy = screen.getAllByRole<HTMLButtonElement>("button", {
        name: "Unloading…",
      });
      expect(busy).toHaveLength(3);
      expect(busy.every(inactive)).toBe(true);
      expect(
        within(
          screen.getByTestId("loaded-models-summary").parentElement!,
        ).getByRole("button").textContent,
      ).toBe("Unloading…");
      await act(async () => held.resolve());
      await screen.findByText("No models are loaded.");
      expect(simulator.callsTo("unload_all_models")).toHaveLength(1);
      expect(simulator.callsTo("unload_model")).toEqual([]);
    });

    it("loads an installed model that is not in memory", async () => {
      const { simulator, backend } = setup(
        (backend) => {
          backend.models.push("gpt-oss:20b-cloud");
          backend.running = [runningModel("qwen3:0.6b")];
        },
        { selectedModel: "gemma3:270m", contextLength: 8192, keepAlive: -1 },
      );
      const held = deferred();
      simulator.handle("load_model", async (args) => {
        await held.promise;
        backend.running.push(
          runningModel(args.model as string, {
            contextLength: 8192,
            expiresAt: null,
            kept: true,
          }),
        );
        return null;
      });
      await loadedCard();
      // Ni ceux déjà en mémoire, ni ceux du nuage ; le modèle choisi d'abord.
      await waitFor(() =>
        expect(optionTexts(loadSelect())).toEqual([
          "qwen3.5:0.8b",
          "gemma3:270m",
        ]),
      );
      expect(loadSelect().value).toBe("gemma3:270m");
      await userEvent.selectOptions(loadSelect(), "qwen3.5:0.8b");
      await userEvent.click(screen.getByRole("button", { name: "Load" }));
      const busy = screen.getByRole<HTMLButtonElement>("button", {
        name: "Loading…",
      });
      expect(inactive(busy)).toBe(true);
      expect(loadSelect().disabled).toBe(true);
      expect(simulator.callsTo("load_model")).toEqual([
        { model: "qwen3.5:0.8b" },
      ]);
      await act(async () => held.resolve());
      await waitFor(() =>
        expect(rows()).toEqual([
          [
            "qwen3:0.6b",
            "1.0 GB · 100% GPU · 4k context · Unloads in 4 minutes",
          ],
          ["qwen3.5:0.8b", "1.0 GB · 100% GPU · 8k context · Stays loaded"],
        ]),
      );
      expect(screen.getByRole("button", { name: "Load" })).toBeTruthy();
      expect(optionTexts(loadSelect())).toEqual(["gemma3:270m"]);
    });

    it("says why a model could not be loaded", async () => {
      setup((backend) => {
        backend.runningErrors.load.set("qwen3:0.6b", {
          kind: "ollama",
          message:
            "model requires more system memory (64.0 GiB) than is available (8.0 GiB)",
        });
      });
      await loadedCard();
      await waitFor(() => expect(loadSelect().value).toBe("qwen3:0.6b"));
      await userEvent.click(screen.getByRole("button", { name: "Load" }));
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Couldn't load qwen3:0.6b: model requires more system memory (64.0 GiB) than is available (8.0 GiB)",
      );
      expect(screen.getByText("No models are loaded.")).toBeTruthy();
      expect(inactive(screen.getByRole("button", { name: "Load" }))).toBe(
        false,
      );
    });

    it("has nothing to load once every model is in memory", async () => {
      const { simulator } = setup((backend) => {
        backend.models = ["qwen3"];
        // /api/ps nomme « qwen3:latest » le modèle installé « qwen3 ».
        backend.running = [runningModel("qwen3:latest")];
      });
      await loadedCard();
      await waitFor(() =>
        expect(optionTexts(loadSelect())).toEqual(["No other model installed"]),
      );
      expect(loadSelect().disabled).toBe(true);
      const load = screen.getByRole("button", { name: "Load" });
      expect(inactive(load)).toBe(true);
      await userEvent.click(load);
      expect(simulator.callsTo("load_model")).toEqual([]);
    });

    it("says when Ollama can't be reached, and tries again", async () => {
      const { backend } = setup((backend) => {
        backend.runningErrors.list = {
          kind: "ollama",
          message: "Could not connect to Ollama at http://127.0.0.1:11434",
        };
      });
      const card = await screen.findByTestId("loaded-models");
      expect((await within(card).findByRole("alert")).textContent).toBe(
        "Couldn't reach Ollama: Could not connect to Ollama at http://127.0.0.1:11434",
      );
      expect(within(card).queryByLabelText("Load a model")).toBeNull();
      backend.runningErrors.list = null;
      backend.running = [runningModel("qwen3:0.6b")];
      await userEvent.click(
        within(card).getByRole("button", { name: "Try again" }),
      );
      await waitFor(() =>
        expect(rows().map(([name]) => name)).toEqual(["qwen3:0.6b"]),
      );
      expect(within(card).queryByRole("alert")).toBeNull();
    });

    it("reads the list again every 2 seconds, unloading times included, only while the tab is shown", async () => {
      const { simulator, backend } = setup((backend) => {
        backend.running = [
          runningModel("qwen3:0.6b", { expiresAt: Date.now() + 61_500 }),
        ];
      });
      await loadedCard();
      expect(rows()[0][1]).toContain("Unloads in 1 minute");
      // Même réponse relue : l'échéance est recalculée.
      await waitFor(
        () => expect(rows()[0][1]).toMatch(/Unloads in 5\d seconds$/),
        { timeout: 4000 },
      );
      // Un modèle chargé ailleurs (chat) apparaît sans action ici.
      backend.running.push(runningModel("gemma3:270m"));
      await waitFor(() => expect(rows()).toHaveLength(2), { timeout: 4000 });

      await userEvent.click(screen.getByTestId("settings-tab-general"));
      await screen.findByText("Server address");
      const reads = simulator.callsTo("running_models").length;
      await act(() => new Promise((resolve) => setTimeout(resolve, 2500)));
      expect(simulator.callsTo("running_models")).toHaveLength(reads);
    });
  });

  describe("focus and errors", () => {
    const unloadButton = (model: string) =>
      within(
        screen
          .getAllByTestId("loaded-model")
          .find((row) => row.dataset.model === model)!,
      ).getByRole("button");
    const title = () => screen.getByRole("heading", { name: "Loaded models" });

    it("an unloaded row gives the focus to the row in its place, then the previous one, then the title", async () => {
      setup((backend) => {
        backend.running = ["a:1b", "b:1b", "c:1b"].map((name) =>
          runningModel(name),
        );
      });
      await loadedCard();
      await userEvent.click(unloadButton("b:1b"));
      await waitFor(() =>
        expect(rows().map(([name]) => name)).toEqual(["a:1b", "c:1b"]),
      );
      expect(document.activeElement).toBe(unloadButton("c:1b"));
      // Dernière ligne : la précédente.
      await userEvent.keyboard("{Enter}");
      await waitFor(() =>
        expect(rows().map(([name]) => name)).toEqual(["a:1b"]),
      );
      expect(document.activeElement).toBe(unloadButton("a:1b"));
      // Plus de ligne : le titre de la carte.
      await userEvent.keyboard("{Enter}");
      await screen.findByText("No models are loaded.");
      expect(document.activeElement).toBe(title());
    });

    it("Unload all and Try again leave the focus on the card title", async () => {
      const { backend } = setup((backend) => {
        backend.running = [runningModel("a:1b"), runningModel("b:1b")];
      });
      await loadedCard();
      await userEvent.click(screen.getByRole("button", { name: "Unload all" }));
      await screen.findByText("No models are loaded.");
      expect(document.activeElement).toBe(title());

      backend.runningErrors.list = { kind: "ollama", message: "down" };
      await screen.findByTestId("loaded-models-error", undefined, {
        timeout: 4000,
      });
      backend.runningErrors.list = null;
      await userEvent.click(screen.getByRole("button", { name: "Try again" }));
      await waitFor(() =>
        expect(screen.queryByTestId("loaded-models-error")).toBeNull(),
      );
      expect(document.activeElement).toBe(title());
    });

    it("Try again stays available while the list is read again by itself", async () => {
      const { simulator, backend } = setup((backend) => {
        backend.runningErrors.list = { kind: "ollama", message: "down" };
      });
      await screen.findByTestId("loaded-models-error");
      // Relecture automatique retenue.
      let answer: () => void = () => {};
      simulator.handle(
        "running_models",
        () =>
          new Promise((resolve) => {
            answer = () => resolve(backend.running);
          }),
      );
      const reads = simulator.callsTo("running_models").length;
      await waitFor(
        () =>
          expect(simulator.callsTo("running_models").length).toBeGreaterThan(
            reads,
          ),
        { timeout: 4000 },
      );
      const retry = screen.getByRole<HTMLButtonElement>("button", {
        name: "Try again",
      });
      expect(retry.disabled).toBe(false);
      expect(retry.getAttribute("aria-disabled")).toBeNull();
      // Le clic rejoint la relecture en cours.
      await userEvent.click(retry);
      expect(simulator.callsTo("running_models")).toHaveLength(reads + 1);
      backend.running = [runningModel("qwen3:0.6b")];
      await act(async () => answer());
      await waitFor(() =>
        expect(rows().map(([name]) => name)).toEqual(["qwen3:0.6b"]),
      );
    });

    it("Try again joins a re-read in progress, also after a list was shown", async () => {
      const { simulator, backend } = setup((backend) => {
        backend.running = [runningModel("qwen3:0.6b")];
      });
      await loadedCard();
      backend.runningErrors.list = { kind: "ollama", message: "down" };
      await screen.findByTestId("loaded-models-error", undefined, {
        timeout: 4000,
      });
      let answer: () => void = () => {};
      simulator.handle(
        "running_models",
        () =>
          new Promise((resolve) => {
            answer = () => resolve(backend.running);
          }),
      );
      const reads = simulator.callsTo("running_models").length;
      await waitFor(
        () =>
          expect(simulator.callsTo("running_models").length).toBeGreaterThan(
            reads,
          ),
        { timeout: 4000 },
      );
      await userEvent.click(screen.getByRole("button", { name: "Try again" }));
      // Pas de lecture de plus : la relecture en cours n'est pas annulée.
      expect(simulator.callsTo("running_models")).toHaveLength(reads + 1);
      await act(async () => answer());
      await waitFor(() =>
        expect(screen.queryByTestId("loaded-models-error")).toBeNull(),
      );
      expect(rows().map(([name]) => name)).toEqual(["qwen3:0.6b"]);
    });

    it("a new action clears the last error", async () => {
      const { backend } = setup((backend) => {
        backend.running = [runningModel("gemma3:270m")];
        backend.runningErrors.load.set("qwen3:0.6b", {
          kind: "ollama",
          message: "out of memory",
        });
      });
      await loadedCard();
      await waitFor(() => expect(loadSelect().value).toBe("qwen3:0.6b"));
      await userEvent.click(screen.getByRole("button", { name: "Load" }));
      expect((await screen.findByTestId("load-error")).textContent).toBe(
        "Couldn't load qwen3:0.6b: out of memory",
      );
      // Unload d'une autre ligne : l'échec du chargement disparaît.
      backend.runningErrors.unload = { kind: "ollama", message: "busy" };
      await userEvent.click(unloadButton("gemma3:270m"));
      expect((await screen.findByTestId("unload-error")).textContent).toBe(
        "Couldn't unload gemma3:270m: busy",
      );
      expect(screen.queryByTestId("load-error")).toBeNull();
      // Load de nouveau : l'échec du déchargement disparaît.
      backend.runningErrors.load.clear();
      await userEvent.click(screen.getByRole("button", { name: "Load" }));
      await waitFor(() =>
        expect(rows().map(([name]) => name)).toEqual([
          "gemma3:270m",
          "qwen3:0.6b",
        ]),
      );
      expect(screen.queryByTestId("unload-error")).toBeNull();
      // Unload all : l'échec d'un chargement disparaît aussi.
      backend.runningErrors.load.set("qwen3.5:0.8b", {
        kind: "ollama",
        message: "out of memory",
      });
      await waitFor(() => expect(loadSelect().value).toBe("qwen3.5:0.8b"));
      await userEvent.click(screen.getByRole("button", { name: "Load" }));
      await screen.findByTestId("load-error");
      await userEvent.click(screen.getByRole("button", { name: "Unload all" }));
      expect((await screen.findByTestId("unload-error")).textContent).toBe(
        "Couldn't unload the models: busy",
      );
      expect(screen.queryByTestId("load-error")).toBeNull();
    });

    it("an error goes once the list shows it no longer applies", async () => {
      const { app, backend } = setup((backend) => {
        backend.running = [runningModel("gemma3:270m")];
        backend.runningErrors.load.set("qwen3:0.6b", {
          kind: "ollama",
          message: "out of memory",
        });
        backend.runningErrors.unload = { kind: "ollama", message: "busy" };
      });
      const reread = () =>
        act(() =>
          app.queryClient.invalidateQueries({ queryKey: runningModelsKey }),
        );
      await loadedCard();
      await waitFor(() => expect(loadSelect().value).toBe("qwen3:0.6b"));
      await userEvent.click(screen.getByRole("button", { name: "Load" }));
      await screen.findByTestId("load-error");
      // Chargé ailleurs (chat) : l'échec n'a plus lieu d'être.
      backend.running.push(runningModel("qwen3:0.6b"));
      await reread();
      await waitFor(() => expect(rows()).toHaveLength(2));
      expect(screen.queryByTestId("load-error")).toBeNull();

      // Déchargement raté, puis le modèle quitte la mémoire de lui-même.
      await userEvent.click(unloadButton("gemma3:270m"));
      await screen.findByTestId("unload-error");
      backend.running = backend.running.filter((m) => m.name !== "gemma3:270m");
      await reread();
      await waitFor(() => expect(rows()).toHaveLength(1));
      expect(screen.queryByTestId("unload-error")).toBeNull();

      // Unload all raté, puis plus rien en mémoire.
      await userEvent.click(screen.getByRole("button", { name: "Unload all" }));
      await screen.findByTestId("unload-error");
      backend.running = [];
      await reread();
      await screen.findByText("No models are loaded.");
      expect(screen.queryByTestId("unload-error")).toBeNull();
    });
  });
});

describe("Settings: Models, system prompt", () => {
  const DESCRIPTION =
    "Sent to the model at the start of every chat, voice messages and Easy Link bots included, and never shown in the chat. It takes the place of the model's built-in system prompt. Leave empty to send none.";
  const field = () =>
    screen.getByRole<HTMLTextAreaElement>("textbox", { name: "System prompt" });

  it("shows the saved prompt and saves a change when the field is left", async () => {
    const { simulator, backend } = setup(() => {}, {
      systemPrompt: "Be brief.",
    });
    await screen.findByRole("heading", { name: "Instructions" });
    expect(field().value).toBe("Be brief.");
    expect(field().placeholder).toBe(
      "For example: Answer in French, briefly and plainly.",
    );
    expect(
      document.getElementById(field().getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe(DESCRIPTION);

    await userEvent.clear(field());
    await userEvent.type(field(), "Answer in French.");
    // Rien d'envoyé pendant la frappe.
    expect(patches(simulator)).toEqual([]);
    await userEvent.tab();
    await waitFor(() =>
      expect(patches(simulator)).toEqual([
        { systemPrompt: "Answer in French." },
      ]),
    );
    expect(backend.settings.systemPrompt).toBe("Answer in French.");
    await screen.findByText("Saved");

    // Quitté sans changement : rien de renvoyé.
    await userEvent.click(field());
    await userEvent.tab();
    expect(patches(simulator)).toHaveLength(1);
  });

  it("saves a prompt made of spaces as empty, and Clear empties it", async () => {
    const { simulator, backend } = setup(() => {}, {
      systemPrompt: "Be brief.",
    });
    await screen.findByRole("heading", { name: "Instructions" });
    await userEvent.clear(field());
    await userEvent.type(field(), "   ");
    await userEvent.tab();
    await waitFor(() =>
      expect(patches(simulator)).toEqual([{ systemPrompt: "" }]),
    );
    const clear = screen.getByRole<HTMLButtonElement>("button", {
      name: "Clear",
    });
    await waitFor(() => expect(clear.disabled).toBe(true));

    backend.settings.systemPrompt = "Be brief.";
    await userEvent.type(field(), "x");
    await userEvent.tab();
    await waitFor(() => expect(field().value).toBe("x"));
    await userEvent.click(clear);
    await waitFor(() =>
      expect(patches(simulator).at(-1)).toEqual({
        systemPrompt: "",
      }),
    );
    expect(field().value).toBe("");
    expect(backend.settings.systemPrompt).toBe("");
  });

  it("keeps a refused prompt in the field, under its error", async () => {
    const { backend } = setup();
    await screen.findByRole("heading", { name: "Instructions" });
    backend.settingsErrors.update = { kind: "internal", message: "disk full" };
    await userEvent.type(field(), "Answer in French.");
    await userEvent.tab();
    const card = screen.getByTestId("system-prompt-settings");
    expect((await within(card).findByRole("alert")).textContent).toContain(
      "disk full",
    );
    expect(field().value).toBe("Answer in French.");
  });
});

describe("Settings: Models, VRAM gauge", () => {
  const GIB = 1024 ** 3;

  it("shows each loaded model, the other uses and the free VRAM", async () => {
    setup((backend) => {
      backend.running = [
        runningModel("qwen3:0.6b", { size: 2 * GIB, sizeVram: 2 * GIB }),
        runningModel("gemma3:270m", { size: GIB, sizeVram: GIB / 2 }),
      ];
      backend.machine = {
        gpus: [
          { name: "Radeon RX 7600", vramTotal: 8 * GIB, vramFree: 4 * GIB },
        ],
        ramTotal: 32 * GIB,
        ramAvailable: 16 * GIB,
      };
    });
    const gauge = await screen.findByTestId("vram-gauge");
    expect(within(gauge).getByTestId("vram-gpus").textContent).toBe(
      "Radeon RX 7600",
    );
    // 4 Gio occupés : 2,5 pour les modèles, 1,5 pour le reste.
    expect(within(gauge).getByTestId("vram-usage").textContent).toBe(
      "4.0 GB of 8.0 GB used",
    );
    const meter = within(gauge).getByRole("meter", { name: "VRAM" });
    expect(meter.getAttribute("aria-valuenow")).toBe(String(4 * GIB));
    expect(meter.getAttribute("aria-valuemax")).toBe(String(8 * GIB));
    expect(meter.getAttribute("aria-valuetext")).toBe("4.0 GB of 8.0 GB used");
    const parts = within(gauge).getAllByTestId("vram-model");
    expect(parts.map((part) => [part.dataset.model, part.style.width])).toEqual(
      [
        ["qwen3:0.6b", "25%"],
        ["gemma3:270m", "6.25%"],
      ],
    );
    expect(within(gauge).getByTestId("vram-other").style.width).toBe("18.75%");
    expect(within(gauge).getByText("Other uses: 1.5 GB")).toBeTruthy();
    expect(within(gauge).getByTestId("vram-free").textContent).toBe(
      "Free: 4.0 GB",
    );
    expect(within(gauge).getByTestId("vram-spilled").textContent).toBe(
      "512 MB doesn't fit in VRAM: that part runs from system memory, more slowly.",
    );
    // La pastille de chaque ligne reprend la teinte de sa part.
    const swatches = screen
      .getAllByTestId("loaded-model")
      .map((row) => row.querySelector("[data-testid=vram-swatch]")?.className);
    expect(swatches.map((name) => name?.match(/bg-accent\S*/)?.[0])).toEqual([
      "bg-accent",
      "bg-accent/60",
    ]);
    expect(
      parts.map((part) => part.className.match(/bg-accent\S*/)?.[0]),
    ).toEqual(["bg-accent", "bg-accent/60"]);
  });

  it("without the free memory of a card, shows only the models", async () => {
    setup((backend) => {
      backend.running = [runningModel("qwen3:0.6b")];
    });
    const gauge = await screen.findByTestId("vram-gauge");
    expect(within(gauge).getByTestId("vram-usage").textContent).toBe(
      "1.0 GB of 24 GB used",
    );
    expect(within(gauge).queryByTestId("vram-other")).toBeNull();
    expect(within(gauge).queryByText(/Other uses/)).toBeNull();
    expect(within(gauge).getByTestId("vram-free").textContent).toBe(
      "Free: 23 GB",
    );
    expect(within(gauge).queryByTestId("vram-spilled")).toBeNull();
  });

  it("says so for a remote server and for a computer without a graphics card", async () => {
    const { app, backend } = setup((backend) => {
      backend.machine = null;
    });
    expect((await screen.findByTestId("vram-unknown")).textContent).toBe(
      "VRAM usage can't be read for this Ollama server.",
    );
    expect(screen.queryByRole("meter")).toBeNull();
    backend.machine = { gpus: [], ramTotal: 16 * GIB, ramAvailable: 8 * GIB };
    await act(() =>
      app.queryClient.invalidateQueries({ queryKey: machineMemoryKey }),
    );
    expect((await screen.findByTestId("vram-no-gpu")).textContent).toBe(
      "No graphics card detected: models run on the processor, in system memory.",
    );
    expect(screen.queryByRole("meter")).toBeNull();
  });

  it("reads the memory again with the list, and after an unload", async () => {
    const { simulator } = setup((backend) => {
      backend.running = [runningModel("qwen3:0.6b")];
    });
    await screen.findByTestId("vram-gauge");
    await waitFor(() =>
      expect(simulator.callsTo("machine_memory").length).toBeGreaterThan(0),
    );
    const before = simulator.callsTo("machine_memory").length;
    await userEvent.click(screen.getByRole("button", { name: "Unload" }));
    await waitFor(() =>
      expect(screen.queryAllByTestId("vram-model")).toEqual([]),
    );
    expect(simulator.callsTo("machine_memory").length).toBeGreaterThan(before);
    expect(screen.getByTestId("vram-usage").textContent).toBe(
      "0 B of 24 GB used",
    );
  });
});

describe("Settings: Models in other languages", () => {
  /** Noms des modèles (et leurs morceaux, montrés par ModelLabel) : des données. */
  const NAMES = [
    "qwen3:0.6b",
    "qwen3.5:0.8b",
    "gemma3:270m",
    "big-split:8b",
    "qwen3",
    "qwen3.5",
    "gemma3",
    "big-split",
    "0.6b",
    "0.8b",
    "270m",
    "8b",
    // Carte graphique du simulateur (jauge de la VRAM).
    "NVIDIA GeForce RTX 3090",
  ];
  const tab = () => screen.getByTestId("models-settings");

  /**
   * Langue réglée et affichée (chaque lecture des réglages rend celle du
   * moteur).
   */
  function speak(backend: FakeBackend, lang: "fr" | "es") {
    Object.assign(backend.settings, {
      language: lang,
      effectiveLanguage: lang,
    });
    act(() => setLanguage(lang));
  }

  it("writes every text of the tab from the catalogs: loaded models, errors", async () => {
    const now = Date.now();
    const { backend } = setup((backend) => {
      backend.running = [
        runningModel("qwen3:0.6b", { expiresAt: now + 4.5 * MINUTE }),
        runningModel("big-split:8b", {
          size: 4 * RUNNING_SIZE,
          sizeVram: 3 * RUNNING_SIZE,
          contextLength: 32768,
          kept: true,
        }),
        runningModel("gemma3:270m", { sizeVram: 0, expiresAt: now - MINUTE }),
      ];
      backend.runningErrors.load.set("qwen3.5:0.8b", {
        kind: "ollama",
        message: "out of memory",
      });
    });
    await loadedCard();
    usePseudoLanguage();
    await waitFor(() =>
      expect(screen.getByTestId("loaded-models-summary").textContent).toMatch(
        /^⟦/,
      ),
    );
    // Échec d'un enregistrement (ligne), puis d'un chargement.
    backend.settingsErrors.update = { kind: "database", message: "disk full" };
    await userEvent.click(
      screen.getByRole("switch", { name: "⟦Preload the chosen model⟧" }),
    );
    await screen.findByText("⟦Couldn't save this setting: disk full⟧");
    await userEvent.click(screen.getByTestId("load-model-button"));
    await screen.findByTestId("load-error");
    const allow = [...NAMES, "disk full", "out of memory"];
    expect(untranslatedTexts(tab(), allow)).toEqual([]);

    // Échec d'un déchargement, puis Unloading… pendant Unload all.
    backend.runningErrors.unload = { kind: "ollama", message: "busy" };
    await userEvent.click(screen.getAllByTestId("unload-model")[0]);
    await screen.findByTestId("unload-error");
    expect(untranslatedTexts(tab(), [...allow, "busy"])).toEqual([]);
  });

  it("writes the empty and unreachable states from the catalogs", async () => {
    const { backend } = setup((backend) => {
      backend.models = [];
    });
    usePseudoLanguage();
    const card = await loadedCard();
    await within(card).findByText("⟦No other model installed⟧");
    await screen.findByTestId("vram-gauge");
    expect(untranslatedTexts(tab(), NAMES)).toEqual([]);

    const message = "Could not connect to Ollama at http://127.0.0.1:11434";
    backend.runningErrors.list = { kind: "ollama", message };
    await within(card).findByRole("alert", undefined, { timeout: 4000 });
    expect(untranslatedTexts(tab(), [message])).toEqual([]);
  });

  it("speaks French and Spanish: steps, plural, sizes, details", async () => {
    const now = Date.now();
    const { backend } = setup((backend) => {
      backend.running = [
        runningModel("qwen3:0.6b", { expiresAt: now + 4.5 * MINUTE }),
        runningModel("gemma3:270m", { kept: true }),
      ];
    });
    await loadedCard();
    speak(backend, "fr");
    const slider = screen.getByRole("slider", {
      name: "Garder les modèles chargés",
    });
    expect(slider.getAttribute("aria-valuetext")).toBe("Par défaut");
    expect(
      [
        ...slider
          .closest("[class*=justify-between]")!
          .querySelectorAll(".text-xs"),
      ].map((label) => label.textContent),
    ).toEqual([
      "Par défaut",
      "1 min",
      "5 min",
      "15 min",
      "30 min",
      "1 heure",
      "4 heures",
      "Toujours",
    ]);
    expect(
      screen.getByRole("switch", { name: "Précharger le modèle choisi" }),
    ).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Modèles chargés" }));
    expect(screen.getByTestId("loaded-models-summary").textContent).toBe(
      "2 modèles chargés · 2,0 Go en mémoire",
    );
    expect(rows()).toEqual([
      [
        "qwen3:0.6b",
        "1,0 Go · 100 % GPU · contexte de 4k · Déchargement dans 4 minutes",
      ],
      ["gemma3:270m", "1,0 Go · 100 % GPU · contexte de 4k · Reste chargé"],
    ]);
    expect(screen.getByRole("button", { name: "Tout décharger" })).toBeTruthy();

    speak(backend, "es");
    await userEvent.click(
      within(
        screen
          .getAllByTestId("loaded-model")
          .find((row) => row.dataset.model === "gemma3:270m")!,
      ).getByRole("button", { name: "Liberar" }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("loaded-models-summary").textContent).toBe(
        "1 modelo cargado · 1,0 GB en memoria",
      ),
    );
    expect(screen.getByLabelText("Cargar un modelo")).toBeTruthy();
  });
});
