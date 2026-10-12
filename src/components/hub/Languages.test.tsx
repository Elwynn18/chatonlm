// Langues du ModelHub : chaque écran sous la pseudo-langue (aucun texte
// écrit en dur : tout ce qui reste hors des marques est une donnée de
// Hugging Face, d'Ollama ou du catalogue de la dictée), puis quelques écrans
// en français et en espagnol (pluriels, tailles, dates, temps restant) et un
// changement de langue sans rechargement.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { HubDownload } from "@/bindings/HubDownload";
import type { HubProblem } from "@/bindings/HubProblem";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { setLanguage, translator } from "@/i18n";
import { formatTimeAgo } from "@/i18n/format";
import { initials } from "@/lib/hub";
import { POLISH_MODELS } from "@/lib/dictation";
import { parseModelName } from "@/lib/modelNames";
import { PARAKEET, speechCatalog, WHISPER_TINY } from "@/test/dictation";
import { demoCatalog, FAMILY_LABELS, type FakeHub, GIB } from "@/test/hub";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 ** 2;
const NBSP = " ";

/** Onglets et page du ModelHub (sans la barre latérale du chat). */
const hubRoot = () => screen.getByTestId("hub-scroll").parentElement!;

/** Noms, auteurs, initiales et familles du catalogue de démonstration. */
function catalogData(): string[] {
  return [
    ...demoCatalog().flatMap(({ model }) => [
      model.id,
      model.name,
      model.author,
      initials(model.name),
      model.license ?? "",
      model.architecture ?? "",
    ]),
    ...Object.values(FAMILY_LABELS),
    "RTX 3090",
  ];
}

/** Adresses des liens (infobulles des liens externes). */
function links(root: Element): string[] {
  return [...root.querySelectorAll("a")].map(
    (link) => link.getAttribute("href") ?? "",
  );
}

/** Textes non traduits de l'écran du ModelHub, données permises ôtées. */
function leftovers(allow: string[] = []): string[] {
  const root = hubRoot();
  return untranslatedTexts(root, [...catalogData(), ...links(root), ...allow]);
}

function download(model: string, fields: Partial<HubDownload>): HubDownload {
  return {
    model,
    repo: /^hf\.co\/([^:]+)/.exec(model)?.[1] ?? null,
    label: model.split(":").at(-1) ?? null,
    state: "downloading",
    completed: 0,
    total: 0,
    bytesPerSecond: null,
    error: null,
    startedAt: 1,
    finishedAt: null,
    ...fields,
  };
}

const QWEN = "hf.co/unsloth/Qwen3.5-9B-GGUF:Q4_K_M";
const PHI = "hf.co/unsloth/Phi-4-mini-reasoning-GGUF:Q8_0";
const SMOL = "hf.co/ggml-org/SmolLM3-3B-GGUF:Q4_K_M";
const GEMMA = "hf.co/google/gemma-4-12B-it-qat-q4_0-gguf:Q4_0";
const LFM = "hf.co/LiquidAI/LFM2-1.2B-GGUF:Q4_K_M";
const TIMEOUT = "Hugging Face didn't answer in time. Try again.";

/** Un téléchargement de chaque état, modèles et transcription. */
function variety({
  backend,
  hub,
}: {
  backend: ReturnType<typeof renderHub>["backend"];
  hub: FakeHub;
}) {
  backend.models.unshift(PHI);
  hub.downloads.push(
    download(PHI, {
      state: "done",
      completed: 4 * GIB,
      total: 4 * GIB,
      finishedAt: 100,
    }),
    download(QWEN, {
      completed: 2 * GIB,
      total: 5 * GIB,
      bytesPerSecond: 50 * MIB,
    }),
    download(SMOL, { state: "failed", error: TIMEOUT, finishedAt: 200 }),
    download(GEMMA, { state: "queued" }),
    download(LFM, { state: "cancelled", finishedAt: 50 }),
  );
  const speech = (
    id: string,
    name: string,
    fields: Partial<SpeechDownload>,
  ): SpeechDownload => ({
    id,
    name,
    state: "downloading",
    completedBytes: 0,
    totalBytes: 512 * MIB,
    error: null,
    ...fields,
  });
  backend.dictation.install(PARAKEET);
  backend.dictation.downloads.push(
    speech(PARAKEET, "Parakeet TDT 0.6B v3", { state: "done" }),
    speech(WHISPER_TINY, "Whisper Tiny", { completedBytes: 128 * MIB }),
    speech("whisper-base", "Whisper Base", { state: "cancelled" }),
  );
}

/** Données des téléchargements de `variety` : titres, étiquettes, erreur. */
function downloadData(): string[] {
  return [
    ...[PHI, QWEN, SMOL, GEMMA, LFM].flatMap((model) => {
      const name = parseModelName(model);
      return [model, name.title, name.tag ?? "", initials(name.title)];
    }),
    TIMEOUT,
    "Parakeet TDT 0.6B v3",
    "Whisper Tiny",
    "Whisper Base",
    "PT",
    "WT",
    "WB",
  ];
}

describe("ModelHub in the pseudo-language: no text left in English", () => {
  it("Discover: home, machine, sections, cards and families", async () => {
    renderHub("/hub");
    await screen.findByRole("heading", { name: "Trending" });
    await screen.findByTestId("machine");
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  it("Discover: filter chips set, a section listed in full", async () => {
    renderHub("/hub?section=coding&uses=code&license=open");
    await screen.findAllByTestId("model-card");
    // Filtres posés : Reset filters visible.
    screen.getByRole("button", { name: "Reset filters" });
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  it("Discover: no model matches the filters", async () => {
    renderHub("/hub?uses=vision&size=large&license=open");
    await screen.findByTestId("hub-empty");
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  it("Discover: search results, Verified only and an unverified publisher", async () => {
    renderHub("/hub?q=qwen&verified=false");
    await screen.findByRole("heading", { name: "Results for “qwen”" });
    usePseudoLanguage();
    expect(leftovers(["qwen"])).toEqual([]);
  });

  it("Discover: a family, its count and the search on Hugging Face", async () => {
    renderHub("/hub?family=qwen");
    await screen.findByRole("link", {
      name: "Search Hugging Face for “Qwen”",
    });
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  const PROBLEMS: HubProblem[] = [
    { kind: "offline" },
    { kind: "rateLimited", retryAfter: 30 },
    { kind: "unavailable", incident: true },
    { kind: "unavailable", incident: false },
    { kind: "failed", message: "HTTP 502" },
  ];
  it.each(
    PROBLEMS.flatMap((problem) =>
      [true, false].map((cached) => ({ problem, cached })),
    ),
  )(
    "Discover: Hugging Face $problem.kind, saved data: $cached",
    async ({ problem, cached }) => {
      renderHub("/hub", ({ hub }) => {
        hub.problem = problem;
        hub.cached = cached;
      });
      await screen.findByTestId(cached ? "hub-banner" : "hub-empty");
      usePseudoLanguage();
      expect(leftovers(["HTTP 502"])).toEqual([]);
    },
  );

  it("Model page: not found", async () => {
    renderHub("/hub/model/nobody/nothing-GGUF");
    await screen.findByTestId("hub-empty");
    usePseudoLanguage();
    expect(leftovers(["nobody/nothing-GGUF"])).toEqual([]);
  });

  it("Model page: facts, capabilities, versions, download and README", async () => {
    renderHub("/hub/model/unsloth/Qwen3.5-9B-GGUF", ({ hub }) => {
      hub.readmes.set(
        "unsloth/Qwen3.5-9B-GGUF",
        "Weights. ![Benchmarks](https://example.com/bench.png)",
      );
    });
    await screen.findByRole("heading", { name: "Choose a version" });
    await userEvent.click(
      await screen.findByRole("button", { name: /^Show all \d+ versions$/ }),
    );
    await screen.findByText(/^Image: Benchmarks/);
    const versions = screen.getAllByTestId("version");
    const versionData = versions.flatMap((version) => {
      const radio = version.querySelector("input")!;
      return [version.getAttribute("title") ?? "", radio.value];
    });
    usePseudoLanguage();
    expect(
      leftovers([
        ...versionData,
        screen.getByTestId("ollama-name").textContent!,
        "Weights.",
        "Benchmarks",
        "example.com",
      ]),
    ).toEqual([]);
  });

  it("Model page: a restricted model and a model card that failed", async () => {
    renderHub("/hub/model/google/gemma-3-27b-it-qat-q4_0-gguf", ({ hub }) => {
      hub.failures.set("hub_readme", { kind: "system", message: "boom" });
    });
    await screen.findByRole("heading", { name: "Choose a version" });
    await screen.findByText(/couldn't be loaded/);
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  it("Downloads: active, queued, finished, failed, cancelled and speech models", async () => {
    renderHub("/hub/downloads", variety);
    await screen.findAllByTestId("download");
    await screen.findAllByTestId("speech-download");
    usePseudoLanguage();
    expect(leftovers(downloadData())).toEqual([]);
  });

  it("Downloads: none yet", async () => {
    renderHub("/hub/downloads");
    await screen.findByText("No downloads yet.");
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  it("Installed: rows, details and the delete confirmation", async () => {
    renderHub("/hub/installed", ({ backend }) => {
      backend.models.unshift(QWEN);
    });
    await screen.findAllByTestId("installed-model");
    const data = screen
      .getAllByTestId("installed-model")
      .flatMap((row) => [
        row.querySelector("[title]")?.getAttribute("title") ?? "",
      ]);
    usePseudoLanguage();
    expect(
      leftovers([
        ...data,
        ...["qwen3", "0.6b", "qwen3.5", "0.8b", "Qwen3.5-9B", "Q4_K_M"],
        ...["751.63M", "unsloth", "Q3", "Q3.5"],
      ]),
    ).toEqual([]);
  });

  it("Installed: no models yet", async () => {
    renderHub("/hub/installed", ({ backend }) => {
      backend.models.length = 0;
    });
    await screen.findByText("No models yet");
    usePseudoLanguage();
    expect(leftovers()).toEqual([]);
  });

  it("Transcription: cards, license, attribution, progress, In use", async () => {
    renderHub("/hub/transcription", ({ backend }) => {
      backend.dictation.install(PARAKEET);
      backend.dictation.downloads.push({
        id: WHISPER_TINY,
        name: "Whisper Tiny",
        state: "downloading",
        completedBytes: 37 * MIB,
        totalBytes: 74 * MIB,
        error: null,
      });
    });
    await screen.findAllByTestId("speech-model");
    await screen.findByRole("progressbar");
    // Description et langues : traduites par identifiant, pas permises.
    const catalog = speechCatalog().flatMap((model) => [
      model.name,
      model.license,
      initials(model.name),
    ]);
    usePseudoLanguage();
    expect(
      leftovers([
        ...catalog,
        "Parakeet",
        "Whisper",
        "nvidia/parakeet-tdt-0.6b-v3",
        "istupakov/parakeet-tdt-0.6b-v3-onnx",
        "ggerganov/whisper.cpp",
      ]),
    ).toEqual([]);
  });

  it("Transcription in French and Spanish: catalog texts by model id", async () => {
    renderHub("/hub/transcription");
    await screen.findAllByTestId("speech-model");
    act(() => setLanguage("fr"));
    const parakeet = screen.getAllByTestId("speech-model")[0];
    expect(parakeet.textContent).toContain("25 langues européennes");
    expect(parakeet.textContent).toContain(
      "Détecte la langue automatiquement.",
    );
    act(() => setLanguage("es"));
    expect(parakeet.textContent).toContain("25 idiomas europeos");
  });

  it("Polish: recommended models, sizes, notes, Ollama unreachable", async () => {
    renderHub("/hub/polish");
    await screen.findAllByTestId("polish-model");
    const names = POLISH_MODELS.flatMap((entry) => [
      entry.model,
      initials(parseModelName(entry.model).title),
    ]);
    usePseudoLanguage();
    expect(leftovers(names)).toEqual([]);
  });
});

describe("ModelHub in French and Spanish", () => {
  it("Discover in French: titles, plurals, fit chips with sizes", async () => {
    renderHub("/hub");
    await screen.findByRole("heading", { name: "Trending" });
    await screen.findByTestId("machine");
    act(() => setLanguage("fr"));
    expect(
      screen.getByRole("heading", { name: "Découvrir des modèles" }),
    ).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Tendances" })).toBeTruthy();
    expect(screen.getByTestId("machine").textContent).toBe(
      `RTX 3090 · 24${NBSP}Go VRAM · 32${NBSP}Go RAM`,
    );
    const families = screen.getByRole("region", { name: "Familles" });
    expect(
      within(families).getByRole("link", { name: /Qwen/ }).textContent,
    ).toContain("4 modèles");
    expect(
      within(families).getByRole("link", { name: /gpt-oss/ }).textContent,
    ).toContain("1 modèle");
    // Texte des éléments normalisé par testing-library (espaces insécables
    // comprises) : le texte cherché s'écrit avec des espaces simples.
    expect(
      screen.getAllByText("Tient sur votre GPU · 7,5 Go").length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByText("En partie sur le CPU · 16 Go").length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByRole("textbox", {
        name: "Rechercher des modèles sur Hugging Face",
      }),
    ).toBeTruthy();
  });

  it("Model page in Spanish: facts, relative date, languages, download size", async () => {
    renderHub("/hub/model/unsloth/Qwen3.5-9B-GGUF");
    await screen.findByRole("heading", { name: "Choose a version" });
    act(() => setLanguage("es"));
    await screen.findByRole("heading", { name: "Elige una versión" });
    const facts = Object.fromEntries(
      [...document.querySelectorAll("dl > div")].map((item) => [
        item.querySelector("dt")?.textContent,
        item.querySelector("dd")?.textContent,
      ]),
    );
    expect(facts).toMatchObject({
      Parámetros: "9,7B",
      Contexto: "256K tokens",
      Licencia: "apache-2.0",
      Idiomas: "Inglés, Chino, Francés +3",
    });
    const qwen = demoCatalog().find(
      ({ model }) => model.id === "unsloth/Qwen3.5-9B-GGUF",
    )!.model;
    expect(facts.Actualizado).toBe(
      formatTimeAgo(translator("es"), qwen.lastModified!),
    );
    // Mots de l'espagnol (« hace 3 semanas », « la semana pasada »).
    expect(facts.Actualizado).toMatch(/hace|pasad|ayer/);
    expect(
      screen.getByRole("button", { name: /^Descargar · / }).textContent,
    ).toMatch(new RegExp(`^Descargar · \\d+(,\\d)?${NBSP}GB$`));
  });

  it("Downloads in French: progress line with time left, states and buttons", async () => {
    renderHub("/hub/downloads", variety);
    await screen.findAllByTestId("download");
    act(() => setLanguage("fr"));
    const rows = screen.getAllByTestId("download");
    expect(rows[0].textContent).toContain(
      `2,0${NBSP}Go sur 5,0${NBSP}Go · 50${NBSP}Mo/s · 1${NBSP}min restante`,
    );
    expect(
      within(rows[0]).getByRole("progressbar").getAttribute("aria-label"),
    ).toBe("Téléchargement de Qwen3.5-9B");
    expect(within(rows[1]).getByText("En attente…")).toBeTruthy();
    expect(rows[3].textContent).toContain(`Installé · 4,0${NBSP}Go`);
    expect(
      within(rows[3])
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Discuter", "Retirer"]);
    expect(rows[4].textContent).toContain("Annulé");
    expect(
      screen.getByRole("heading", { name: "Modèles de transcription" }),
    ).toBeTruthy();
  });

  it("Installed in Spanish: count, total size and modified date", async () => {
    renderHub("/hub/installed");
    await screen.findAllByTestId("installed-model");
    act(() => setLanguage("es"));
    expect(screen.getByRole("heading", { level: 2 }).textContent).toMatch(
      /^2 modelos · /,
    );
    expect(screen.getAllByTestId("installed-model")[0].textContent).toMatch(
      /Modificado (hace|ayer|justo ahora)/,
    );
    expect(
      screen.getByRole("button", { name: "Eliminar qwen3:0.6b" }),
    ).toBeTruthy();
  });

  it("changes language in place: the typed search stays", async () => {
    renderHub("/hub");
    await screen.findByRole("heading", { name: "Trending" });
    const field = screen.getByRole("textbox", {
      name: "Search Hugging Face models",
    });
    await userEvent.type(field, "q");
    act(() => setLanguage("es"));
    const same = screen.getByRole("textbox", {
      name: "Buscar modelos en Hugging Face",
    }) as HTMLInputElement;
    expect(same).toBe(field);
    expect(same.value).toBe("q");
    expect(screen.getByRole("link", { name: "Descubrir" })).toBeTruthy();
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Tendencias" })).toBeTruthy(),
    );
  });
});
