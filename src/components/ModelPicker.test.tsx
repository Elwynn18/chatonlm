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
import type { ChatModel } from "@/bindings/ChatModel";
import type { HubHardware } from "@/bindings/HubHardware";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import type { RemoteProvider } from "@/bindings/RemoteProvider";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { ModelPicker } from "./ModelPicker";

afterEach(() => {
  cleanup();
  clearMocks();
});

function info(id: string, fields: Partial<ChatModel> = {}): ChatModel {
  const remote = id.startsWith("@remote/");
  return {
    id,
    displayName: id,
    origin: remote ? "remote" : "hf",
    provider: remote ? id.split("/")[1] : null,
    sizeBytes: remote ? null : 1,
    paramsLabel: "1B",
    quant: "Q4_K_M",
    contextMax: null,
    capabilities: ["completion"],
    installedAt: remote ? null : 0,
    ...fields,
  };
}

const INSTALLED = [info("qwen3.5:0.8b"), info("qwen3:0.6b"), info("llama3")];

/** Fiche d'un dépôt de Hugging Face (hub_repo) : ses versions. */
function repoCard(repo: string, quants: string[], recommended: string) {
  return {
    repo: {
      quants: quants.map((label) => ({
        label,
        model: `${repo}:${label}`,
        recommended: label === recommended,
      })),
    },
    status: { fetchedAt: null, problem: null },
  };
}

/**
 * Backend simulé : réglages en mémoire, liste de modèles donnée (le moteur
 * n'y met les modèles distants que le réglage activé : les essais les
 * donnent en conséquence) ; `recommended` : réponse de recommended_models
 * (Error : échec) ; `repos` : fiches de Hugging Face par dépôt (absent :
 * aucune donnée, Error : hors ligne) ; `remoteModels` : réglage des modèles
 * distants ; `providers` : fournisseurs distants réglés (Error : illisibles).
 */
function backend(
  models: ChatModel[] | Promise<never>,
  selectedModel = "",
  {
    recommended = [] as (string | RecommendedModel)[] | Error,
    repos = {} as Record<string, ReturnType<typeof repoCard> | Error>,
    remoteModels = false,
    hardware = null as HubHardware | null,
    providers = [] as RemoteProvider[] | Error,
  } = {},
) {
  const settings = {
    selectedModel,
    theme: "system",
    remoteModels,
  };
  const patches: SettingsPatch[] = [];
  /** Dépôts demandés à Hugging Face, avec l'heure de la demande. */
  const lookups: { repo: string; at: number }[] = [];
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
      case "recommended_models":
        if (recommended instanceof Error) throw recommended;
        return recommended.map((entry) =>
          typeof entry === "string"
            ? {
                model: entry,
                displayName: entry,
                sizeBytes: null,
                vramBytes: null,
                fit: null,
              }
            : entry,
        );
      case "hub_hardware":
        return hardware;
      case "remote_providers":
        if (providers instanceof Error) throw providers;
        return providers;
      case "hub_repo": {
        const { id } = args as { id: string };
        lookups.push({ repo: id, at: Date.now() });
        const found = repos[id] ?? {
          repo: null,
          status: { fetchedAt: null, problem: "notFound" },
        };
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
      <ModelPicker />
    </QueryClientProvider>,
  );
  return client;
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

  it("shows the memory a recommended model needs and whether it fits", async () => {
    const GB = 1_000_000_000;
    // Tailles affichées en Gio (formatBytes) : 12 Gio s'écrivent « 12 GB ».
    const GIB = 1024 ** 3;
    backend([], "", {
      recommended: [
        {
          model: "gemma4",
          displayName: "gemma4",
          sizeBytes: null,
          vramBytes: 12 * GIB,
          fit: "fits",
        },
        {
          model: "llama4",
          displayName: "llama4",
          sizeBytes: null,
          vramBytes: 90 * GB,
          fit: "tooLarge",
        },
        // Mémoire inconnue : ni taille ni pastille.
        {
          model: "qwen3.5",
          displayName: "qwen3.5",
          sizeBytes: null,
          vramBytes: null,
          fit: null,
        },
      ],
      hardware: {
        gpus: [{ name: "GPU", vramTotal: 24 * GB, vramFree: null }],
        ramTotal: 32 * GB,
        ramAvailable: 16 * GB,
        context: 4096,
      },
    });
    renderPicker();
    await userEvent.click(button());
    const row = (name: string) =>
      screen
        .getAllByRole("option")
        .find((option) => option.dataset.modelId === name)!;
    await waitFor(() =>
      expect(within(row("gemma4")).getByText("Fits your GPU")).toBeTruthy(),
    );
    const size = within(row("gemma4")).getByTitle(
      /^Needs .* of memory to run$/,
    );
    expect(size.textContent).toMatch(/12/);
    expect(within(row("llama4")).getByText("Too large")).toBeTruthy();
    expect(within(row("qwen3.5")).queryByTitle(/of memory/)).toBeNull();
    expect(row("qwen3.5").querySelector("[data-fit]")).toBeNull();
    // Toujours l'icône « pas téléchargé ».
    expect(screen.getAllByLabelText("Not downloaded")).toHaveLength(3);
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

  it("carries the shown model's id for tests, whatever its label", async () => {
    const id = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
    backend(INSTALLED, id);
    renderPicker();
    // Texte : titre et étiquette ; identifiant entier dans l'attribut.
    await waitFor(() => expect(button().textContent).toBe("Qwen3-0.6BQ4_K_M"));
    expect(button().dataset.modelId).toBe(id);
  });

  it("carries an empty id without a model", async () => {
    backend([]);
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("Select a model"));
    expect(button().dataset.modelId).toBe("");
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

describe("ModelPicker: id looked up on Hugging Face", () => {
  const REPO = "unsloth/Qwen3-8B-GGUF";
  const card = repoCard(REPO, ["Q8_0", "Q4_K_M", "UD-Q4_K_XL"], "Q4_K_M");
  const row = (id: string) =>
    screen
      .getAllByRole("option")
      .find((option) => option.dataset.modelId === id);

  it("proposes the advised version of a typed repository, asked once 300 ms after the last keystroke", async () => {
    const { lookups, patches } = backend(INSTALLED, "llama3", {
      recommended: ["gemma4"],
      repos: { [REPO]: card },
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await openWith("gemma4");
    await userEvent.keyboard(REPO);
    const typedAt = Date.now();
    expect(screen.getByText("No models found")).toBeTruthy();
    await delay(150);
    expect(lookups).toEqual([]);

    await waitFor(() => expect(row(`${REPO}:Q4_K_M`)).toBeTruthy());
    expect(lookups.map((l) => l.repo)).toEqual([REPO]);
    expect(lookups[0].at - typedAt).toBeGreaterThanOrEqual(150);
    expect(options()).toEqual(["Qwen3-8B · unslothQ4_K_M"]);
    expect(missing()).toEqual([true]);

    // Choisi : simplement enregistré (téléchargé au prochain envoi).
    await userEvent.click(row(`${REPO}:Q4_K_M`)!);
    expect(patches).toEqual([{ selectedModel: `${REPO}:Q4_K_M` }]);
    await waitFor(() => expect(button().textContent).toBe("Qwen3-8BQ4_K_M"));
    // Rouvert : en fin de liste, avec l'icône.
    await openWith("gemma4");
    expect(missing()).toEqual([true, false, false, false, true]);
  });

  it("proposes the typed version, its quantization without case, from an address too", async () => {
    const { lookups } = backend(INSTALLED, "llama3", {
      repos: { [REPO]: card },
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard(`https://huggingface.co/${REPO}:q8_0`);
    await waitFor(() => expect(row(`${REPO}:Q8_0`)).toBeTruthy());
    expect(lookups.map((l) => l.repo)).toEqual([REPO]);
  });

  it("forgets the model found once the search changes", async () => {
    backend(INSTALLED, "llama3", { repos: { [REPO]: card } });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard(` ${REPO} `);
    await waitFor(() => expect(row(`${REPO}:Q4_K_M`)).toBeTruthy());
    await userEvent.clear(screen.getByLabelText("Find model"));
    await userEvent.keyboard("qwen3");
    // Ni tout de suite (réponse pour l'ancien dépôt), ni après.
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
    await delay(400);
    expect(options()).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
  });

  it.each([
    ["a plain name", "mistral"],
    ["the id of an installed model", "unsloth/Qwen3-0.6B-GGUF:q4_k_m"],
  ])("does not ask Hugging Face for %s", async (_, typed) => {
    const installed = [info("unsloth/Qwen3-0.6B-GGUF:Q4_K_M"), ...INSTALLED];
    const { lookups } = backend(installed, "llama3");
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard(typed);
    await delay(500);
    expect(lookups).toEqual([]);
  });

  it.each([
    ["absent from Hugging Face", undefined],
    ["Hugging Face is unreachable", new Error("dns error")],
    ["without the version typed", card],
  ])("adds nothing when the repository is %s", async (_, answer) => {
    const typed = answer === card ? `${REPO}:Q2_K` : "someone/nothing-GGUF";
    const repo = typed.split(":")[0];
    const { lookups } = backend(INSTALLED, "llama3", {
      repos: answer === undefined ? {} : { [repo]: answer },
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await userEvent.click(button());
    await userEvent.keyboard(typed);
    await waitFor(() => expect(lookups.map((l) => l.repo)).toEqual([repo]));
    await delay(50);
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByText("No models found")).toBeTruthy();
  });
});

describe("ModelPicker: Hugging Face names", () => {
  const SMOL = "bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M";

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
    // Infobulle : le nom montré en entier, jamais l'identifiant brut.
    expect(option.getAttribute("title")).toBe(
      "SmolLM2-135M-Instruct:Q4_K_M · bartowski",
    );
    expect(option.dataset.modelId).toBe(SMOL);
    expect(option.textContent).toBe("SmolLM2-135M-Instruct · bartowskiQ4_K_M");
    // Les autres noms gardent leur orthographe, étiquette à part.
    const plain = screen.getByRole("option", { name: "qwen3:0.6b" });
    expect(within(plain).getByText(":0.6b").className).toContain("flex-none");
  });

  it("finds the selected model under another case of its quantization: selected, not marked missing, listed once", async () => {
    backend([info(SMOL)], SMOL.replace("Q4_K_M", "q4_k_m"));
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

describe("ModelPicker: remote models", () => {
  const REMOTE = "@remote/mistral/mistral-small-latest";
  const MARK = "Remote model: runs at mistral";
  /** Pour chaque ligne : porte-t-elle la marque des modèles distants ? */
  const marks = () =>
    screen
      .getAllByRole("option")
      .map((option) => option.querySelector(`[aria-label='${MARK}']`))
      .map((icon) => icon !== null);

  it("are listed after the local ones, marked with their provider, nothing to download", async () => {
    backend([...INSTALLED, info(REMOTE)], "llama3", {
      recommended: ["gemma4"],
      remoteModels: true,
    });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("llama3"));
    await openWith("gemma4");
    expect(options()).toEqual([
      "gemma4",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
      "mistral-small-latest · mistral",
    ]);
    expect(marks()).toEqual([false, false, false, false, true]);
    expect(missing()).toEqual([true, false, false, false, false]);
  });

  it("a selected one shows its mark on the button", async () => {
    const { patches } = backend([...INSTALLED, info(REMOTE)], REMOTE, {
      remoteModels: true,
    });
    renderPicker();
    await waitFor(() =>
      expect(button().textContent).toBe("mistral-small-latest"),
    );
    expect(within(button()).getByLabelText(MARK)).toBeTruthy();
    await delay(50);
    expect(patches).toEqual([]);
  });

  it("name their provider by the name it was given, not by its id", async () => {
    const SALON = "@remote/custom-2/llama-3.3";
    const salon: RemoteProvider = {
      id: "custom-2",
      kind: "custom",
      name: "Serveur du salon",
      baseUrl: "http://192.168.1.20:11434/v1",
      enabled: true,
      hasKey: false,
      keyHint: null,
      createdAt: 0,
      updatedAt: 0,
    };
    backend([...INSTALLED, info(SALON)], SALON, {
      remoteModels: true,
      providers: [salon],
    });
    renderPicker();
    const named = "Remote model: runs at Serveur du salon";
    await waitFor(() =>
      expect(within(button()).getByLabelText(named)).toBeTruthy(),
    );
    await userEvent.click(button());
    const option = screen.getByRole("option", { selected: true });
    expect(option.textContent).toBe("llama-3.3 · Serveur du salon");
    expect(option.getAttribute("title")).toBe("llama-3.3 · Serveur du salon");
    expect(within(option).getByLabelText(named)).toBeTruthy();
  });

  it("fall back on the provider given by the list when the providers can't be read", async () => {
    const SALON = "@remote/custom-2/llama-3.3";
    backend([info(SALON, { provider: "Serveur du salon" })], SALON, {
      remoteModels: true,
      providers: new Error("unreadable"),
    });
    renderPicker();
    await waitFor(() =>
      expect(
        within(button()).getByLabelText(
          "Remote model: runs at Serveur du salon",
        ),
      ).toBeTruthy(),
    );
  });

  it("are never the default model", async () => {
    const { patches } = backend([info(REMOTE)], "", { remoteModels: true });
    renderPicker();
    await waitFor(() => expect(button().textContent).toBe("Select a model"));
    await delay(50);
    expect(patches).toEqual([]);
  });

  it("a selected one gives way to the most recently installed model when the setting is off", async () => {
    const { patches } = backend(INSTALLED, REMOTE);
    renderPicker();
    await waitFor(() =>
      expect(patches).toEqual([{ selectedModel: "qwen3.5:0.8b" }]),
    );
    await waitFor(() => expect(button().textContent).toBe("qwen3.5:0.8b"));
    await delay(100);
    expect(patches).toHaveLength(1);
  });

  it("a selected one with nothing installed leaves no model selected", async () => {
    const { patches } = backend([], REMOTE);
    renderPicker();
    await waitFor(() => expect(patches).toEqual([{ selectedModel: "" }]));
    await waitFor(() => expect(button().textContent).toBe("Select a model"));
  });
});
