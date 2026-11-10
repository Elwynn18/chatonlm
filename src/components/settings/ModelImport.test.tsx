// Carte « Models from Ollama » de Settings › Models face au faux moteur de
// la reprise : recherche à la demande des deux origines, cases cochées par
// défaut, reprise des noms cochés, progression par l'événement, « Stop »,
// bilan, retéléchargement de l'équivalent sur Hugging Face (et son échec
// dit), « Try again »
// d'un modèle abandonné, raisons traduites par code, place des copies,
// dossier partagé (toujours des copies), ménage de l'ancien dossier (modèles nommés avant la
// confirmation) et son bilan, langues.

import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import type { HubDownload } from "@/bindings/HubDownload";
import type { HubDownloadRequest } from "@/bindings/HubDownloadRequest";
import type { ImportOrigin } from "@/bindings/ImportOrigin";
import type { ImportProblem } from "@/bindings/ImportProblem";
import { setLanguage } from "@/i18n";
import { hubDownloadsKey } from "@/hooks/useDownloads";
import { useModelImportEvents } from "@/hooks/useModelImport";
import { createQueryClient } from "@/lib/queryClient";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import {
  BUILTIN_OLLAMA_DIR,
  BUSY,
  candidate,
  type FakeModelImport,
  fakeModelImport,
  MACHINE_OLLAMA_DIR,
  source,
} from "@/test/modelImport";
import { installSimulator, type Simulator } from "@/test/simulator";
import { ModelImport } from "./ModelImport";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const GIB = 1024 ** 3;
const QWEN_HF = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
const GEMMA_HF = "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M";

/** Modèles de l'intégré : prêts, déjà là, à retélécharger, laissé. */
function builtinModels() {
  return [
    candidate("qwen3:0.6b", { aliases: ["qwen3:latest"], noTemplate: true }),
    candidate("llava:7b", { vision: true, sizeBytes: 4 * GIB }),
    candidate("mistral:7b", { status: "installed", id: "@ollama/mistral:7b" }),
    candidate("gemma3:4b", {
      status: "redownload",
      problem: "ollamaFormat",
      suggestion: GEMMA_HF,
    }),
    candidate("kimi-k2:cloud", { status: "skipped", problem: "cloud" }),
  ];
}

/** Téléchargements du ModelHub, en mémoire. */
function fakeDownloads(simulator: Simulator) {
  const downloads: HubDownload[] = [];
  // Copies, comme le moteur : un changement sur place se voit à la relecture.
  simulator.handle("hub_downloads", () =>
    downloads.map((download) => ({ ...download })),
  );
  simulator.handle("hub_download", (args) => {
    const request = args.request as HubDownloadRequest;
    // Comme la file du ModelHub : une nouvelle demande remplace l'ancienne.
    const index = downloads.findIndex((d) => d.model === request.model);
    if (index !== -1) downloads.splice(index, 1);
    downloads.push({
      model: request.model,
      repo: request.repo,
      label: request.label,
      state: "downloading",
      completed: GIB,
      total: 4 * GIB,
      bytesPerSecond: null,
      error: null,
      startedAt: Date.now(),
      finishedAt: null,
    });
    return null;
  });
  return downloads;
}

/** Écouteur de `model-import` de la route racine. */
function ImportEvents() {
  useModelImportEvents();
  return null;
}

/**
 * La carte seule, avec l'écouteur de la route racine ; `prepare` règle le
 * faux moteur avant le rendu.
 */
function renderCard(prepare: (fake: FakeModelImport) => void = () => {}) {
  const simulator = installSimulator();
  const fake = fakeModelImport(simulator);
  fake.sources.builtin = [source("builtin", builtinModels())];
  fake.sources.machine = [
    source("machine", [
      candidate("phi4:14b", { sizeBytes: 9 * GIB }),
      candidate("smollm2:135m", { sizeBytes: GIB / 4 }),
    ]),
  ];
  prepare(fake);
  const downloads = fakeDownloads(simulator);
  const queryClient = createQueryClient();
  const view = render(
    <QueryClientProvider client={queryClient}>
      <ImportEvents />
      <ModelImport />
    </QueryClientProvider>,
  );
  onTestFinished(() => {
    cleanup();
    queryClient.clear();
  });
  return { simulator, fake, downloads, queryClient, ...view };
}

const section = (origin: ImportOrigin) =>
  screen.getByTestId(`model-import-${origin}`);

/** « Look for models » d'une origine, puis ses dossiers affichés. */
async function look(origin: ImportOrigin) {
  await waitFor(() =>
    expect(
      within(section(origin))
        .getAllByRole("button")[0]
        .hasAttribute("disabled"),
    ).toBe(false),
  );
  await userEvent.click(within(section(origin)).getAllByRole("button")[0]);
  return (
    await within(section(origin)).findAllByTestId("model-import-source")
  )[0];
}

/** Lignes d'un dossier : nom, statut. */
function rows(container: HTMLElement) {
  return within(container)
    .getAllByTestId("model-import-candidate")
    .map((row) => [
      row.querySelector(".font-mono")?.textContent,
      row.dataset.status,
    ]);
}

const checkbox = (container: HTMLElement, name: string) =>
  within(container).getByRole<HTMLInputElement>("checkbox", { name });

describe("ModelImport", () => {
  it("looks for each Ollama's models only on demand", async () => {
    const { simulator } = renderCard();
    expect(
      await screen.findByRole("heading", { name: "Models from Ollama" }),
    ).toBeTruthy();
    expect(within(section("builtin")).getByRole("heading").textContent).toBe(
      "Built-in Ollama",
    );
    expect(within(section("machine")).getByRole("heading").textContent).toBe(
      "Ollama on this computer",
    );
    expect(simulator.callsTo("model_import_scan")).toHaveLength(0);

    const builtin = await look("builtin");
    expect(simulator.callsTo("model_import_scan")).toEqual([
      { origin: "builtin" },
    ]);
    expect(within(builtin).getByText(BUILTIN_OLLAMA_DIR)).toBeTruthy();
    expect(rows(builtin)).toEqual([
      ["qwen3:0.6b", "ready"],
      ["llava:7b", "ready"],
      ["mistral:7b", "installed"],
      ["gemma3:4b", "redownload"],
      ["kimi-k2:cloud", "skipped"],
    ]);
    // Prêts : cochés par défaut ; les autres sans case.
    expect(checkbox(builtin, "qwen3:0.6b").checked).toBe(true);
    expect(checkbox(builtin, "llava:7b").checked).toBe(true);
    expect(within(builtin).getAllByRole("checkbox")).toHaveLength(2);
    expect(within(builtin).getByText("Also named qwen3:latest")).toBeTruthy();
    expect(within(builtin).getByText("Vision")).toBeTruthy();
    expect(
      within(builtin).getByText(
        "No chat template: llama.cpp will use a default one.",
      ),
    ).toBeTruthy();
    expect(within(builtin).getByText("Already in ChatonLM")).toBeTruthy();
    expect(
      within(builtin).getByText(
        "Stored in a format of Ollama's own: download it again from Hugging Face.",
      ),
    ).toBeTruthy();
    expect(
      within(builtin).getByText("Cloud model: there is nothing to import."),
    ).toBeTruthy();
    // Même disque, non partagé : liens physiques, pas d'avis de copie.
    expect(within(builtin).queryByTestId("model-import-space")).toBeNull();
    expect(within(builtin).queryByTestId("model-import-shared")).toBeNull();
    // La machine n'a pas été cherchée.
    expect(
      within(section("machine")).queryByTestId("model-import-source"),
    ).toBeNull();

    const machine = await look("machine");
    expect(simulator.callsTo("model_import_scan")).toEqual([
      { origin: "builtin" },
      { origin: "machine" },
    ]);
    expect(within(machine).getByText(MACHINE_OLLAMA_DIR)).toBeTruthy();
    expect(rows(machine)).toEqual([
      ["phi4:14b", "ready"],
      ["smollm2:135m", "ready"],
    ]);
  });

  it("says how much the copies need, from the checked models", async () => {
    renderCard((fake) => {
      fake.sources.machine[0].freeBytes = 5 * GIB;
    });
    const machine = await look("machine");
    const space = () => within(machine).getByTestId("model-import-space");
    expect(space().textContent).toBe(
      "Not enough free space: 9.3 GB needed, 5.0 GB free.",
    );
    expect(space().className).toContain("text-red-600");
    await userEvent.click(checkbox(machine, "phi4:14b"));
    expect(space().textContent).toBe(
      "These models will be copied: 256 MB needed, 5.0 GB free.",
    );
    expect(space().className).not.toContain("text-red-600");
    expect(
      within(machine).getByRole("button", { name: "Import 1 model" }),
    ).toBeTruthy();
    // Rien de coché : ni avis ni reprise possible.
    await userEvent.click(checkbox(machine, "smollm2:135m"));
    expect(within(machine).queryByTestId("model-import-space")).toBeNull();
    expect(
      within(machine).getByRole<HTMLButtonElement>("button", {
        name: "Import 0 models",
      }).disabled,
    ).toBe(true);
  });

  it("imports the checked models, shows the progress, and stops on demand", async () => {
    const { simulator, fake } = renderCard();
    const builtin = await look("builtin");
    await userEvent.click(checkbox(builtin, "llava:7b"));
    await userEvent.click(
      within(builtin).getByRole("button", { name: "Import 1 model" }),
    );
    await waitFor(() => expect(fake.starts).toHaveLength(1));
    expect(simulator.callsTo("model_import_start")).toEqual([
      { origin: "builtin", dir: BUILTIN_OLLAMA_DIR, names: ["qwen3:0.6b"] },
    ]);

    await fake.progress({
      model: "qwen3:0.6b",
      doneBytes: GIB / 4,
    });
    const running = await screen.findByTestId("model-import-running");
    expect(within(running).getByText("Importing qwen3:0.6b…")).toBeTruthy();
    const bar = within(running).getByRole("progressbar", {
      name: "Importing models",
    });
    expect(bar.getAttribute("aria-valuenow")).toBe("25");
    expect(bar.getAttribute("aria-valuetext")).toBe(
      "0 of 1 model · 256 MB of 1.0 GB copied · 25%",
    );
    // Une seule reprise à la fois.
    expect(
      within(builtin).getByRole<HTMLButtonElement>("button", {
        name: "Import 1 model",
      }).disabled,
    ).toBe(true);

    await userEvent.click(
      within(running).getByRole("button", { name: "Stop" }),
    );
    await waitFor(() => expect(fake.cancels).toBe(1));
    await fake.finish({ imported: [], cancelled: true });
    const report = await screen.findByTestId("model-import-report");
    expect(screen.queryByTestId("model-import-running")).toBeNull();
    expect(within(report).getByText("Import stopped.")).toBeTruthy();
    expect(
      within(report).getByText("Last import from the built-in Ollama"),
    ).toBeTruthy();
  });

  it("looks again at the folders once an import is done", async () => {
    const { simulator, fake } = renderCard();
    const builtin = await look("builtin");
    await userEvent.click(
      within(builtin).getByRole("button", { name: "Import 2 models" }),
    );
    await waitFor(() => expect(fake.starts).toHaveLength(1));
    await fake.progress({ doneModels: 1 });
    // Les deux modèles sont maintenant dans ChatonLM.
    fake.sources.builtin[0].models = fake.sources.builtin[0].models.map(
      (model) =>
        model.status === "ready" ? { ...model, status: "installed" } : model,
    );
    const leftovers = simulator.callsTo("model_import_leftovers").length;
    await fake.finish();
    await waitFor(() =>
      expect(simulator.callsTo("model_import_scan")).toHaveLength(2),
    );
    await waitFor(() =>
      expect(within(section("builtin")).queryAllByRole("checkbox")).toEqual([]),
    );
    expect(simulator.callsTo("model_import_leftovers").length).toBeGreaterThan(
      leftovers,
    );
    const report = screen.getByTestId("model-import-report");
    expect(within(report).getByText("2 models imported:")).toBeTruthy();
    expect(within(report).getByText("qwen3:0.6b")).toBeTruthy();
    expect(within(report).getByText("llava:7b")).toBeTruthy();
  });

  it("reports the models to download again and those that failed", async () => {
    const { simulator, downloads } = renderCard((fake) => {
      fake.state.last = {
        origin: "machine",
        dir: MACHINE_OLLAMA_DIR,
        imported: [{ name: "phi4:14b", id: "@ollama/phi4:14b" }],
        redownload: [
          { name: "qwen3:0.6b", suggestion: QWEN_HF },
          { name: "odd:1b", suggestion: null },
        ],
        failed: [{ name: "smollm2:135m", error: "Disk full." }],
        cancelled: false,
        error: null,
      };
    });
    const report = await screen.findByTestId("model-import-report");
    expect(
      within(report).getByText("Last import from Ollama on this computer"),
    ).toBeTruthy();
    expect(within(report).getByText("1 model imported:")).toBeTruthy();
    expect(within(report).getByText("smollm2:135m: Disk full.")).toBeTruthy();
    expect(
      within(report).getByText("No equivalent was found on Hugging Face."),
    ).toBeTruthy();
    expect(
      within(report).getByText(`On Hugging Face: ${QWEN_HF}`),
    ).toBeTruthy();

    await userEvent.click(
      within(report).getByRole("button", { name: "Download qwen3:0.6b again" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("hub_download")).toEqual([
        { request: { model: QWEN_HF, repo: null, label: null } },
      ]),
    );
    // Suivi comme au ModelHub.
    expect(await within(report).findByRole("progressbar")).toBeTruthy();
    expect(
      within(report).queryByRole("button", {
        name: "Download qwen3:0.6b again",
      }),
    ).toBeNull();
    expect(downloads).toHaveLength(1);
  });

  it("shows an import that could not start at all", async () => {
    renderCard((fake) => {
      fake.state.last = {
        origin: "builtin",
        dir: BUILTIN_OLLAMA_DIR,
        imported: [],
        redownload: [],
        failed: [],
        cancelled: false,
        error: "Not enough disk space: 4.0 GB needed, 1.0 GB free.",
      };
    });
    const report = await screen.findByTestId("model-import-report");
    expect(within(report).getByRole("alert").textContent).toBe(
      "Not enough disk space: 4.0 GB needed, 1.0 GB free.",
    );
    expect(within(report).queryByText("No model was imported.")).toBeNull();
  });

  it("downloads the Hugging Face equivalent of a model in Ollama's format", async () => {
    const { simulator } = renderCard();
    const builtin = await look("builtin");
    expect(
      within(builtin).getByText(`On Hugging Face: ${GEMMA_HF}`),
    ).toBeTruthy();
    await userEvent.click(
      within(builtin).getByRole("button", { name: "Download gemma3:4b again" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("hub_download")).toEqual([
        { request: { model: GEMMA_HF, repo: null, label: null } },
      ]),
    );
    expect(await within(builtin).findByRole("progressbar")).toBeTruthy();
  });

  it("says why the download of the equivalent failed, and tries again", async () => {
    const { simulator, downloads, queryClient } = renderCard();
    const builtin = await look("builtin");
    const button = () =>
      within(builtin).getByRole("button", { name: "Download gemma3:4b again" });
    await userEvent.click(button());
    expect(await within(builtin).findByRole("progressbar")).toBeTruthy();
    // Disque plein pendant le téléchargement.
    Object.assign(downloads[0], {
      state: "failed",
      error: "No space left on device",
    });
    await act(() =>
      queryClient.invalidateQueries({ queryKey: hubDownloadsKey }),
    );
    expect((await within(builtin).findByRole("alert")).textContent).toBe(
      "The download failed: No space left on device",
    );
    expect(button().textContent).toBe("Try again");
    await userEvent.click(button());
    await waitFor(() =>
      expect(simulator.callsTo("hub_download")).toHaveLength(2),
    );
    expect(await within(builtin).findByRole("progressbar")).toBeTruthy();
    expect(within(builtin).queryByRole("alert")).toBeNull();
  });

  it("tries a model given up after two failures again", async () => {
    const { fake } = renderCard((fake) => {
      fake.sources.builtin[0].models.push(
        candidate("phi3:mini", { status: "failed" }),
      );
    });
    const builtin = await look("builtin");
    expect(
      within(builtin).getByText("Import failed twice in a row."),
    ).toBeTruthy();
    await userEvent.click(
      within(builtin).getByRole("button", {
        name: "Try importing phi3:mini again",
      }),
    );
    await waitFor(() =>
      expect(fake.starts).toEqual([
        { origin: "builtin", dir: BUILTIN_OLLAMA_DIR, names: ["phi3:mini"] },
      ]),
    );
  });

  it("explains each reason a model is not imported", async () => {
    const problems: [ImportProblem, string][] = [
      [
        "ollamaFormat",
        "Stored in a format of Ollama's own: download it again from Hugging Face.",
      ],
      ["notGguf", "Not a GGUF model file: ChatonLM can't use it."],
      ["blobMissing", "Some of its files are missing."],
      ["cloud", "Cloud model: there is nothing to import."],
      ["adapter", "Adapter (LoRA): not supported."],
      ["invalidName", "Its name can't be saved on this computer."],
      ["unreadable", "Its files can't be read."],
      ["embedding", "Embedding model: not used for chats."],
      ["unsupported", "llama.cpp can't run this kind of model."],
    ];
    renderCard((fake) => {
      fake.sources.machine = [
        source(
          "machine",
          problems.map(([problem]) =>
            candidate(`m-${problem}`, { status: "skipped", problem }),
          ),
        ),
      ];
    });
    const machine = await look("machine");
    for (const [problem, text] of problems) {
      const row = within(machine)
        .getByText(`m-${problem}`)
        .closest("li") as HTMLElement;
      expect(within(row).getByText(text)).toBeTruthy();
    }
    // Rien de prêt : pas de bouton d'import.
    expect(
      within(machine).queryByRole("button", { name: /Import/ }),
    ).toBeNull();
  });

  it("says when the engine refuses to start an import", async () => {
    const { fake } = renderCard((fake) => {
      fake.state.running = {
        origin: "machine",
        dir: MACHINE_OLLAMA_DIR,
        model: null,
        doneModels: 0,
        totalModels: 1,
        doneBytes: 0,
        totalBytes: 0,
      };
    });
    // Reprise lancée ailleurs : préparation, sans octets.
    const running = await screen.findByTestId("model-import-running");
    expect(within(running).getByText("Preparing the import…")).toBeTruthy();
    fake.state.running = null;
    fake.failures.set("model_import_start", { kind: "invalid", message: BUSY });
    await fake.emit();
    const builtin = await look("builtin");
    await userEvent.click(
      within(builtin).getByRole("button", { name: "Import 2 models" }),
    );
    expect((await within(builtin).findByRole("alert")).textContent).toBe(
      `Could not start the import: ${BUSY}`,
    );
  });

  it("says when a folder can't be read, or nothing was found", async () => {
    renderCard((fake) => {
      fake.sources.builtin = [source("builtin", [], { readable: false })];
      fake.sources.machine = [];
    });
    const builtin = await look("builtin");
    expect(within(builtin).getByRole("alert").textContent).toBe(
      "ChatonLM can't read this folder.",
    );
    await userEvent.click(within(section("machine")).getByRole("button"));
    expect(
      await within(section("machine")).findByText(
        "No Ollama folder was found.",
      ),
    ).toBeTruthy();
  });

  it("copies from a built-in folder shared with this computer's Ollama, even on the same drive", async () => {
    // Même disque : sans le partage, des liens physiques (rien à copier,
    // aucune place dite). Le ménage, lui, suit ce que le moteur rend
    // (`service::cleanable` refuse un dossier partagé, essayé côté Rust).
    renderCard((fake) => {
      fake.sources.builtin[0] = {
        ...fake.sources.builtin[0],
        shared: true,
        sameDrive: true,
      };
    });
    const builtin = await look("builtin");
    expect(within(builtin).getByTestId("model-import-shared").textContent).toBe(
      "This folder also belongs to the Ollama installed on this computer: its models are copied, and nothing in it is ever deleted.",
    );
    expect(within(builtin).getByTestId("model-import-space").textContent).toBe(
      "These models will be copied: 5.0 GB needed, 120 GB free.",
    );
  });

  it("says nothing about copies for a built-in folder on the same drive, not shared", async () => {
    renderCard((fake) => {
      fake.sources.builtin[0] = {
        ...fake.sources.builtin[0],
        shared: false,
        sameDrive: true,
      };
    });
    const builtin = await look("builtin");
    expect(within(builtin).queryByTestId("model-import-shared")).toBeNull();
    expect(within(builtin).queryByTestId("model-import-space")).toBeNull();
  });

  it("names the models that will disappear before deleting the old folder", async () => {
    const { simulator, fake } = renderCard((fake) => {
      fake.leftovers = {
        dir: BUILTIN_OLLAMA_DIR,
        files: 14,
        bytes: 6 * GIB,
        models: [
          candidate("gemma3:4b", {
            status: "redownload",
            problem: "ollamaFormat",
            suggestion: GEMMA_HF,
          }),
          candidate("nomic-embed-text:v1.5", {
            status: "skipped",
            problem: "embedding",
          }),
        ],
      };
      fake.cleanup = { removedFiles: 12, removedBytes: 5 * GIB, failed: 2 };
    });
    const notice = await screen.findByTestId("model-import-leftovers");
    expect(notice.querySelector("p")?.textContent).toBe(
      "The old folder of the built-in Ollama still takes 6.0 GB (14 files). Its models have been imported: delete it to free up space.",
    );
    expect(within(notice).getByText(BUILTIN_OLLAMA_DIR)).toBeTruthy();
    expect(
      within(notice).getByText(
        "These models were not imported and will be deleted with it:",
      ),
    ).toBeTruthy();
    expect(
      [...within(notice).getByTestId("model-import-lost").children].map(
        (item) => item.textContent,
      ),
    ).toEqual(["gemma3:4b", "nomic-embed-text:v1.5"]);

    // Annulé : rien n'est effacé.
    await userEvent.click(
      within(notice).getByRole("button", { name: "Delete" }),
    );
    let confirm = await screen.findByRole("alertdialog", {
      name: "Delete the old folder of the built-in Ollama?",
    });
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("model_import_cleanup")).toHaveLength(0);

    await userEvent.click(
      within(notice).getByRole("button", { name: "Delete" }),
    );
    confirm = await screen.findByRole("alertdialog");
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual([
      `6.0 GB will be freed in ${BUILTIN_OLLAMA_DIR}. This can't be undone.`,
      "These 2 models will be deleted: gemma3:4b, nomic-embed-text:v1.5.",
    ]);
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Delete" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("model_import_cleanup")).toHaveLength(1),
    );
    expect(await screen.findByText("12 files deleted (5.0 GB).")).toBeTruthy();
    expect(
      screen.getByText(
        "2 files could not be deleted: close the programs that use them, then try again.",
      ),
    ).toBeTruthy();
    // Relu : plus rien à effacer.
    await waitFor(() =>
      expect(screen.queryByTestId("model-import-leftovers")).toBeNull(),
    );
    expect(fake.leftovers).toBeNull();
  });

  it("says why the cleanup failed", async () => {
    renderCard((fake) => {
      fake.leftovers = {
        dir: BUILTIN_OLLAMA_DIR,
        files: 1,
        bytes: GIB,
        models: [],
      };
      fake.failures.set("model_import_cleanup", {
        kind: "invalid",
        message: "Not yet.",
      });
    });
    const notice = await screen.findByTestId("model-import-leftovers");
    expect(within(notice).queryByTestId("model-import-lost")).toBeNull();
    await userEvent.click(
      within(notice).getByRole("button", { name: "Delete" }),
    );
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not delete the old folder: Not yet.",
    );
  });

  it("keeps an event newer than a state read still on its way", async () => {
    const { simulator, fake } = renderCard();
    const builtin = await look("builtin");
    // L'état relu après « Import » est retenu : l'événement de progression
    // arrive avant sa réponse, plus ancienne que lui.
    let answer: (state: unknown) => void = () => {};
    let asked = false;
    simulator.handle("model_import_state", () => {
      asked = true;
      return new Promise((resolve) => (answer = resolve));
    });
    await userEvent.click(
      within(builtin).getByRole("button", { name: "Import 2 models" }),
    );
    await waitFor(() => expect(asked).toBe(true));
    const stale = structuredClone(fake.state);
    await fake.progress({ model: "llava:7b", doneModels: 1, doneBytes: GIB });
    expect(await screen.findByText("Importing llava:7b…")).toBeTruthy();
    await act(async () => answer(stale));
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(screen.getByText("Importing llava:7b…")).toBeTruthy();
    expect(screen.queryByText("Preparing the import…")).toBeNull();
  });

  it("stops listening once the page is gone", async () => {
    const { simulator, unmount } = renderCard();
    await waitFor(() => expect(simulator.listening("model-import")).toBe(1));
    unmount();
    await waitFor(() => expect(simulator.listening("model-import")).toBe(0));
  });

  it("speaks French and Spanish", async () => {
    const { fake } = renderCard((fake) => {
      fake.leftovers = {
        dir: BUILTIN_OLLAMA_DIR,
        files: 1,
        bytes: GIB,
        models: [],
      };
    });
    act(() => setLanguage("fr"));
    expect(
      await screen.findByRole("heading", { name: "Modèles d'Ollama" }),
    ).toBeTruthy();
    const notice = await screen.findByTestId("model-import-leftovers");
    expect(notice.querySelector("p")?.textContent).toContain(
      "occupe encore 1,0 Go (1 fichier).",
    );
    const builtin = await look("builtin");
    expect(
      within(builtin).getByRole("button", { name: "Reprendre 2 modèles" }),
    ).toBeTruthy();
    await userEvent.click(
      within(builtin).getByRole("button", { name: "Reprendre 2 modèles" }),
    );
    await waitFor(() => expect(fake.starts).toHaveLength(1));
    await fake.progress({ model: "llava:7b", doneModels: 1, doneBytes: GIB });
    await waitFor(() =>
      expect(
        screen.getByRole("progressbar").getAttribute("aria-valuetext"),
      ).toBe("Modèles repris\u00a0: 1 sur 2 · 1,0 Go sur 5,0 Go copiés · 20 %"),
    );
    act(() => setLanguage("es"));
    expect(screen.getByText("Importando llava:7b…")).toBeTruthy();
  });

  it("has every text translated", async () => {
    usePseudoLanguage();
    const { fake, downloads, queryClient } = renderCard((fake) => {
      fake.sources.builtin[0].models.push(
        candidate("phi3:mini", { status: "failed" }),
        candidate("odd:1b", { status: "redownload", problem: "unsupported" }),
      );
      fake.sources.machine[0].freeBytes = GIB;
      fake.sources.builtin.push(
        source("builtin", [], {
          dir: "/mnt/old",
          readable: false,
          shared: true,
        }),
      );
      fake.leftovers = {
        dir: BUILTIN_OLLAMA_DIR,
        files: 3,
        bytes: GIB,
        models: [candidate("gemma3:4b")],
      };
      fake.state.last = {
        origin: "machine",
        dir: MACHINE_OLLAMA_DIR,
        imported: [{ name: "phi4:14b", id: "@ollama/phi4:14b" }],
        redownload: [{ name: "qwen3:0.6b", suggestion: QWEN_HF }],
        failed: [{ name: "smollm2:135m", error: "Disk full." }],
        cancelled: true,
        error: null,
      };
    });
    await screen.findByTestId("model-import-report");
    await screen.findByTestId("model-import-leftovers");
    const builtinSource = await look("builtin");
    await look("machine");
    // Téléchargement de l'équivalent en échec : sa raison et « Try again ».
    downloads.push({
      model: GEMMA_HF,
      repo: null,
      label: null,
      state: "failed",
      completed: 0,
      total: GIB,
      bytesPerSecond: null,
      error: "No space left on device",
      startedAt: 1,
      finishedAt: 2,
    });
    await act(() =>
      queryClient.invalidateQueries({ queryKey: hubDownloadsKey }),
    );
    await within(builtinSource).findByText(/No space left on device/);
    const data = [
      BUILTIN_OLLAMA_DIR,
      MACHINE_OLLAMA_DIR,
      "/mnt/old",
      QWEN_HF,
      GEMMA_HF,
      "qwen3:0.6b",
      "qwen3:latest",
      "llava:7b",
      "mistral:7b",
      "gemma3:4b",
      "kimi-k2:cloud",
      "phi3:mini",
      "odd:1b",
      "phi4:14b",
      "smollm2:135m",
      "Disk full.",
      "No space left on device",
    ];
    expect(untranslatedTexts(document.body, data)).toEqual([]);

    // Confirmation du ménage, puis une reprise en cours.
    await userEvent.click(
      within(screen.getByTestId("model-import-leftovers")).getByRole("button"),
    );
    const confirm = await screen.findByRole("alertdialog");
    expect(untranslatedTexts(document.body, data)).toEqual([]);
    await userEvent.click(within(confirm).getAllByRole("button")[0]);
    const builtin = within(section("builtin")).getAllByTestId(
      "model-import-source",
    )[0];
    await userEvent.click(
      within(builtin).getAllByRole("button").at(-1) as HTMLElement,
    );
    await waitFor(() => expect(fake.starts).toHaveLength(1));
    await fake.progress({ model: "qwen3:0.6b", doneBytes: GIB });
    await screen.findByTestId("model-import-running");
    expect(untranslatedTexts(document.body, data)).toEqual([]);
  });
});
