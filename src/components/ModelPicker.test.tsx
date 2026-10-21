import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import type { ModelInfo } from "@/bindings/ModelInfo";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { ModelPicker } from "./ModelPicker";

afterEach(() => {
  cleanup();
  clearMocks();
});

function info(model: string): ModelInfo {
  return {
    model,
    digest: `digest-${model}`,
    size: 1,
    parameterSize: "1B",
    quantizationLevel: "Q4_K_M",
    contextLength: null,
    capabilities: ["completion"],
  };
}

const INSTALLED = [info("qwen3.5:0.8b"), info("qwen3:0.6b"), info("llama3")];

/**
 * Backend simulé : réglages en mémoire, liste de modèles donnée ;
 * `recommended` : réponse de recommended_models (Error : échec) ; `upstream` :
 * réponse du registre par nom (absent : false, Error : registre injoignable) ;
 * `cloudModels` : réglage des modèles cloud ; `cloudDisabled` : cloud coupé
 * sur le serveur (cloud_status).
 */
function backend(
  models: ModelInfo[] | Promise<never>,
  selectedModel = "",
  {
    recommended = [] as (string | RecommendedModel)[] | Error,
    upstream = {} as Record<string, boolean | Error>,
    cloudModels = false,
    cloudDisabled = false,
  } = {},
) {
  const settings = {
    serverUrl: "",
    selectedModel,
    theme: "system",
    cloudModels,
  };
  const patches: SettingsPatch[] = [];
  /** Noms demandés au registre, avec l'heure de la demande. */
  const lookups: { model: string; at: number }[] = [];
  mockIPC((command, args) => {
    switch (command) {
      case "get_settings":
        return { ...settings };
      case "update_settings": {
        const { patch } = args as { patch: SettingsPatch };
        patches.push(patch);
        Object.assign(settings, patch);
        return { ...settings };
      }
      case "list_models":
        return models;
      case "cloud_status":
        return { disabled: cloudDisabled, source: "none" };
      case "recommended_models":
        if (recommended instanceof Error) throw recommended;
        return recommended.map((entry) =>
          typeof entry === "string"
            ? { model: entry, requiredPlan: null }
            : entry,
        );
      case "model_exists_upstream": {
        const { model } = args as { model: string };
        lookups.push({ model, at: Date.now() });
        const found = upstream[model] ?? false;
        if (found instanceof Error) throw found;
        return found;
      }
      default:
        throw new Error(`unexpected command ${command}`);
    }
  });
  return { patches, lookups };
}

function renderPicker() {
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ModelPicker chatId="new" />
    </QueryClientProvider>,
  );
}

const button = () => screen.getByTitle("Select model");
const options = () =>
  screen.getAllByRole("option").map((option) => option.textContent);
/** Pour chaque ligne : porte-t-elle l'icône « non téléchargé » ? */
const missing = () =>
  screen
    .getAllByRole("option")
    .map((option) => option.querySelector("[aria-label='Not downloaded']"))
    .map((icon) => icon !== null);

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Ouvre le menu une fois les modèles recommandés affichés. */
async function openWith(first: string) {
  await userEvent.click(button());
  await waitFor(() => expect(options()[0]).toBe(first));
}

describe("ModelPicker", () => {
  it("shows the selected model and lists the installed ones", async () => {
    backend(INSTALLED, "qwen3:0.6b");
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("qwen3:0.6b"));
    await userEvent.click(button());
    expect(document.activeElement).toBe(screen.getByLabelText("Find model"));
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b", "llama3"]);
    expect(
      screen.getByRole("option", { name: "qwen3:0.6b" }).ariaSelected,
    ).toBe("true");
  });

  it("filters with the search, then selects with the keyboard", async () => {
    const { patches } = backend(INSTALLED, "llama3");
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard("QWEN");
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(patches).toEqual([{ selectedModel: "qwen3:0.6b" }]);
    await waitFor(() => expect(button().textContent).toBe("qwen3:0.6b"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("lists a selected model that is not installed last, with an icon", async () => {
    backend(INSTALLED, "gemma4");
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("gemma4"));
    await userEvent.click(button());
    expect(options()).toEqual([
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
      "gemma4",
    ]);
    expect(screen.getAllByLabelText("Not downloaded")).toHaveLength(1);
  });

  it("chooses the most recently installed model when none is selected", async () => {
    const { patches } = backend(INSTALLED);
    renderPicker();
    await waitFor(() =>
      expect(patches).toEqual([{ selectedModel: "qwen3.5:0.8b" }]),
    );
    await waitFor(() => expect(button().textContent).toBe("qwen3.5:0.8b"));
  });

  it("without any model: Select a model, No models found", async () => {
    const { patches } = backend([]);
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("Select a model"));
    await userEvent.click(button());
    expect(screen.getByText("No models found")).toBeTruthy();
    expect(patches).toEqual([]);
  });

  it("says Loading... until the list arrives, Escape closes", async () => {
    backend(new Promise<never>(() => {}), "llama3");
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("Loading..."));
    await userEvent.click(button());
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("ModelPicker: recommended models", () => {
  it("lists them first, the installed copy or the download icon, then the other installed models", async () => {
    backend(INSTALLED, "qwen3:0.6b", { recommended: ["gemma4", "llama3"] });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("qwen3:0.6b"));
    await openWith("gemma4");
    expect(options()).toEqual([
      "gemma4",
      "llama3",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
    ]);
    expect(missing()).toEqual([true, false, false, false]);
    expect(
      screen.getByRole("option", { name: "qwen3:0.6b" }).ariaSelected,
    ).toBe("true");
  });

  it("lists a selected recommended model that is not installed once, in its place", async () => {
    backend(INSTALLED, "gemma4", { recommended: ["gemma4", "qwen3.5"] });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("gemma4"));
    await openWith("gemma4");
    expect(options()).toEqual([
      "gemma4",
      "qwen3.5",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
    ]);
    expect(missing()).toEqual([true, true, false, false, false]);
  });

  it("never makes a recommended model the default", async () => {
    const { patches } = backend(INSTALLED, "", { recommended: ["gemma4"] });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("qwen3.5:0.8b"));
    await openWith("gemma4");
    expect(patches).toEqual([{ selectedModel: "qwen3.5:0.8b" }]);
  });

  it("with no model installed, proposes them but chooses none", async () => {
    const { patches } = backend([], "", {
      recommended: ["gemma4", "qwen3.5"],
    });
    renderPicker();
    await openWith("gemma4");
    expect(options()).toEqual(["gemma4", "qwen3.5"]);
    expect(missing()).toEqual([true, true]);
    await delay(50);
    expect(button().textContent).toBe("Select a model");
    expect(patches).toEqual([]);
  });

  it("are left out when they cannot be read", async () => {
    backend(INSTALLED, "llama3", { recommended: new Error("404") });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await delay(50);
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b", "llama3"]);
  });

  it("choosing one that is not installed only selects it", async () => {
    const { patches } = backend(INSTALLED, "llama3", {
      recommended: ["gemma4"],
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await openWith("gemma4");
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(patches).toEqual([{ selectedModel: "gemma4" }]);
    await waitFor(() => expect(button().textContent).toBe("gemma4"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("ModelPicker: name looked up in the registry", () => {
  it("proposes a typed name found upstream, asked once 300 ms after the last keystroke", async () => {
    const { lookups, patches } = backend(INSTALLED, "llama3", {
      recommended: ["gemma4"],
      upstream: { "llama3.2": true },
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await openWith("gemma4");
    await userEvent.keyboard("llama3.2");
    const typedAt = Date.now();
    expect(screen.getByText("No models found")).toBeTruthy();
    await delay(150);
    expect(lookups).toEqual([]);

    const option = await screen.findByRole("option", { name: "llama3.2" });
    expect(lookups.map((l) => l.model)).toEqual(["llama3.2"]);
    expect(lookups[0].at - typedAt).toBeGreaterThanOrEqual(150);
    expect(options()).toEqual(["llama3.2"]);
    expect(missing()).toEqual([true]);

    // Choisi : simplement enregistré (téléchargé au prochain envoi).
    await userEvent.click(option);
    expect(patches).toEqual([{ selectedModel: "llama3.2" }]);
    await waitFor(() => expect(button().textContent).toBe("llama3.2"));
    // Rouvert : en fin de liste, avec l'icône.
    await openWith("gemma4");
    expect(options()).toEqual([
      "gemma4",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
      "llama3.2",
    ]);
    expect(missing()).toEqual([true, false, false, false, true]);
  });

  it("forgets the name found upstream once the search changes", async () => {
    backend(INSTALLED, "llama3", { upstream: { "qwen3:4b": true } });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard(" qwen3:4b ");
    await screen.findByRole("option", { name: "qwen3:4b" });
    await userEvent.clear(screen.getByLabelText("Find model"));
    await userEvent.keyboard("qwen3");
    // Ni tout de suite (réponse pour l'ancien nom), ni après le registre.
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
    await delay(400);
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
  });

  it.each([
    ["an installed model", " QWEN3:0.6B "],
    ["a recommended model", "Gemma4"],
    ["the selected model", "mistral"],
  ])("does not ask the registry for the exact name of %s", async (_, typed) => {
    const { lookups } = backend(INSTALLED, "mistral", {
      recommended: ["gemma4"],
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("mistral"));
    await openWith("gemma4");
    await userEvent.keyboard(typed);
    await delay(500);
    expect(lookups).toEqual([]);
    expect(options()).toHaveLength(1);
  });

  it.each([
    ["absent upstream", false],
    ["the registry is unreachable", new Error("dns error")],
  ])("adds nothing when the name is %s", async (_, answer) => {
    const { lookups } = backend(INSTALLED, "llama3", {
      upstream: { nosuchmodel: answer },
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard("nosuchmodel");
    await waitFor(() =>
      expect(lookups.map((l) => l.model)).toEqual(["nosuchmodel"]),
    );
    await delay(50);
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByText("No models found")).toBeTruthy();
  });
});

describe("ModelPicker: Hugging Face names", () => {
  const SMOL = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M";

  it("shows the title and the tag, which stays visible; full name as tooltip", async () => {
    backend([info(SMOL), info("qwen3:0.6b")], SMOL);
    renderPicker();
    await waitFor(() =>
      expect(button().textContent).toBe("SmolLM2-135M-InstructQ4_K_M"),
    );
    // Bouton à largeur maximale, titre tronqué, étiquette jamais tronquée.
    expect(button().className.split(" ")).toContain("max-w-64");
    const tag = within(button()).getByText("Q4_K_M");
    expect(tag.className).toContain("flex-none");
    expect(
      within(button()).getByText("SmolLM2-135M-Instruct").className,
    ).toContain("truncate");
    await userEvent.click(button());
    const option = screen.getByRole("option", { selected: true });
    expect(option.getAttribute("title")).toBe(SMOL);
    expect(option.textContent).toBe("SmolLM2-135M-Instruct · bartowskiQ4_K_M");
    // Les autres noms gardent leur orthographe, étiquette à part.
    const plain = screen.getByRole("option", { name: "qwen3:0.6b" });
    expect(within(plain).getByText(":0.6b").className).toContain("flex-none");
  });

  it("finds the selected model under another case: selected, not marked missing, listed once", async () => {
    backend([info(SMOL)], SMOL.toLowerCase());
    renderPicker();
    await waitFor(() =>
      expect(button().textContent).toBe("SmolLM2-135M-InstructQ4_K_M"),
    );
    await userEvent.click(button());
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(missing()).toEqual([false]);
    expect(screen.getByRole("option").getAttribute("aria-selected")).toBe(
      "true",
    );
  });
});

describe("ModelPicker: cloud models", () => {
  const RECOMMENDED: RecommendedModel[] = [
    { model: "kimi-k2.6:cloud", requiredPlan: "pro" },
    { model: "qwen3.5:cloud", requiredPlan: "free" },
    { model: "gemma4", requiredPlan: null },
  ];
  const CLOUD = "Cloud model: runs on ollama.com";
  /** Pour chaque ligne : porte-t-elle l'icône du nuage ? */
  const clouds = () =>
    screen
      .getAllByRole("option")
      .map((option) => option.querySelector(`[aria-label='${CLOUD}']`))
      .map((icon) => icon !== null);

  it("are left out while the setting is off, and never looked up", async () => {
    const { lookups } = backend(INSTALLED, "llama3", {
      recommended: RECOMMENDED,
      upstream: { "kimi-k3:cloud": true },
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await openWith("gemma4");
    expect(options()).toEqual([
      "gemma4",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
    ]);
    await userEvent.keyboard("kimi-k3:cloud");
    await delay(500);
    expect(lookups).toEqual([]);
    expect(screen.getByText("No models found")).toBeTruthy();
  });

  it("are listed once turned on, with a cloud and the paid plan they need", async () => {
    backend(INSTALLED, "llama3", {
      recommended: RECOMMENDED,
      cloudModels: true,
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await openWith("kimi-k2.6:cloudPro");
    expect(options()).toEqual([
      "kimi-k2.6:cloudPro",
      "qwen3.5:cloud",
      "gemma4",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
    ]);
    expect(clouds()).toEqual([true, true, false, false, false, false]);
    // Rien à télécharger pour un modèle cloud.
    expect(missing()).toEqual([false, false, true, false, false, false]);
    expect(screen.getByText("Pro").title).toBe("Needs the Ollama Pro plan");
  });

  it("stay hidden when the server has its cloud turned off", async () => {
    backend(INSTALLED, "llama3", {
      recommended: RECOMMENDED,
      cloudModels: true,
      cloudDisabled: true,
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await delay(50);
    expect(options()).toEqual([
      "gemma4",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
    ]);
  });

  it("a selected one shows its cloud on the button", async () => {
    const { patches } = backend(INSTALLED, "kimi-k2.6:cloud", {
      recommended: RECOMMENDED,
      cloudModels: true,
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("kimi-k2.6:cloud"));
    expect(within(button()).getByLabelText(CLOUD)).toBeTruthy();
    await delay(50);
    expect(patches).toEqual([]);
  });

  it.each([
    ["the setting is off", false, false],
    ["the server has its cloud turned off", true, true],
  ])(
    "a selected one gives way to the most recently installed model when %s",
    async (_, cloudModels, cloudDisabled) => {
      const { patches } = backend(INSTALLED, "kimi-k2.6:cloud", {
        recommended: RECOMMENDED,
        cloudModels,
        cloudDisabled,
      });
      renderPicker();
      await waitFor(() =>
        expect(patches).toEqual([{ selectedModel: "qwen3.5:0.8b" }]),
      );
      await waitFor(() => expect(button().textContent).toBe("qwen3.5:0.8b"));
    },
  );

  it("a selected one never gives way to a local model named like a cloud one", async () => {
    // Modèle local à l'étiquette en « -cloud » (ollama create), le plus récent : il
    // passerait pour un modèle cloud ; une seule écriture, pas de boucle.
    const { patches } = backend(
      [info("mine:my-cloud"), ...INSTALLED],
      "kimi-k2.6:cloud",
    );
    renderPicker();
    await waitFor(() =>
      expect(patches).toEqual([{ selectedModel: "qwen3.5:0.8b" }]),
    );
    await delay(100);
    expect(patches).toHaveLength(1);
  });

  it("a selected one with nothing installed leaves no model selected", async () => {
    const { patches } = backend([], "kimi-k2.6:cloud");
    renderPicker();
    await waitFor(() => expect(patches).toEqual([{ selectedModel: "" }]));
    await waitFor(() => expect(button().textContent).toBe("Select a model"));
  });
});
