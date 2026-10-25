// Accueil de l'onglet Study face au simulateur : entrée de la barre
// latérale, formulaire (sujet, Entrée, niveau, durée, préremplissage par
// l'adresse), documents (sélecteur natif, retrait, image et excès refusés,
// fichiers repris par la création), recherche web, modèle (sélecteur,
// aucun modèle, petit modèle), listes des cours (en cours, terminés,
// suppression confirmée), erreurs ; textes traduits (pseudo-langue,
// français, espagnol).

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { setLanguage } from "@/i18n";
import {
  type FakeBackend,
  fakeBackend,
  modelInfo,
  runningModel,
} from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(
  prepare: (backend: FakeBackend) => void = () => {},
  path = "/study",
  options: Parameters<typeof fakeBackend>[1] = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    files: {
      "/docs/notes.md": {},
      "/docs/paper.pdf": {},
      "/docs/extra.txt": {},
      "/docs/more.txt": {},
      "/docs/photo.png": {},
    },
    ...options,
  });
  prepare(backend);
  const app = renderApp(path);
  return { simulator, backend, study: backend.study, app };
}

const topic = () =>
  screen.getByRole<HTMLTextAreaElement>("textbox", {
    name: "What do you want to learn?",
  });
const createButton = () =>
  screen.getByRole<HTMLButtonElement>("button", { name: "Create course" });

async function home() {
  await screen.findByTestId("study-home");
  // Liste lue.
  await waitFor(() =>
    expect(
      screen.queryByTestId("study-empty") ??
        screen.queryByTestId("study-in-progress") ??
        screen.queryByTestId("study-completed"),
    ).not.toBeNull(),
  );
}

describe("Study home", () => {
  it("is in the sidebar between Proofread and ModelHub, highlighted on its pages", async () => {
    const user = userEvent.setup();
    setup();
    await home();
    const toggle = screen.getByRole("button", {
      name: /^(Show|Hide) sidebar$/,
    });
    if (toggle.getAttribute("aria-label") === "Show sidebar") {
      await user.click(toggle);
    }
    const link = await screen.findByRole("link", { name: "Study" });
    expect(link.getAttribute("href")).toBe("/study");
    expect(link.className.split(" ")).toContain("bg-accent");
    expect(
      within(link.closest("nav")!)
        .getAllByRole("link")
        .slice(0, 5)
        .map((element) => element.textContent),
    ).toEqual(["Chat", "Proofread", "Study", "ModelHub", "Settings"]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Study");
  });

  it("invites to a first course; Create is disabled until there is a topic", async () => {
    const user = userEvent.setup();
    setup();
    await home();
    expect(screen.getByTestId("study-empty").textContent).toBe(
      "No courses yet. What would you like to learn today?",
    );
    expect(createButton().disabled).toBe(true);
    await user.click(topic());
    await user.keyboard("   ");
    expect(createButton().disabled).toBe(true);
    await user.keyboard("x");
    expect(createButton().disabled).toBe(false);
  });

  it("creates a course with Enter (Discover, 20 min by default) and opens its plan", async () => {
    const user = userEvent.setup();
    const { simulator, app } = setup();
    await home();
    await user.click(topic());
    await user.keyboard("  Photosynthesis {Enter}");
    expect(simulator.callsTo("study_create")).toEqual([
      {
        course: {
          topic: "Photosynthesis",
          level: "beginner",
          minutes: 20,
          web: false,
          documentIds: [],
        },
      },
    ]);
    await screen.findByTestId("study-plan");
    expect(app.path()).toBe("/study/course-1");
    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
      "Course on Photosynthesis",
    );
  });

  it("sends the chosen level and duration (keyboard: arrows in each group)", async () => {
    const user = userEvent.setup();
    const { simulator } = setup();
    await home();
    await user.type(topic(), "Stars");
    await user.click(screen.getByRole("radio", { name: "Go deeper" }));
    await user.click(screen.getByRole("radio", { name: "10 min" }));
    // Flèche dans le groupe de la durée : 20 min.
    await user.keyboard("{ArrowRight}");
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: "20 min" }).checked,
    ).toBe(true);
    await user.click(createButton());
    await screen.findByTestId("study-plan");
    expect(simulator.callsTo("study_create")[0].course).toMatchObject({
      level: "advanced",
      minutes: 20,
    });
  });

  it("is filled from the address (Go further)", async () => {
    setup(() => {}, "/study?topic=Black%20holes&level=intermediate&minutes=30");
    await home();
    expect(topic().value).toBe("Black holes");
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: "I know the basics" })
        .checked,
    ).toBe(true);
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: "30 min" }).checked,
    ).toBe(true);
  });

  it("adds documents from the native picker, removes one, and hands the others to the course", async () => {
    const user = userEvent.setup();
    const { backend, simulator } = setup((b) => {
      b.picks.push(["/docs/notes.md", "/docs/paper.pdf"]);
    });
    await home();
    await user.click(screen.getByRole("button", { name: "Add a document" }));
    const chips = await screen.findAllByTestId("study-document");
    expect(chips.map((chip) => chip.textContent)).toEqual([
      "notes.md",
      "paper.pdf",
    ]);
    // Sans sujet : les documents suffisent.
    expect(createButton().disabled).toBe(false);
    await user.click(screen.getByRole("button", { name: "Remove paper.pdf" }));
    expect(simulator.callsTo("unstage")).toEqual([{ ids: ["staged-2"] }]);
    await user.click(createButton());
    await screen.findByTestId("study-plan");
    expect(simulator.callsTo("study_create")[0].course).toMatchObject({
      topic: "",
      documentIds: ["staged-1"],
    });
    // Repris par le moteur (zone vidée), jamais libérés par la page.
    expect(backend.staged.size).toBe(0);
    expect(simulator.callsTo("unstage")).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
      "Course on your documents",
    );
  });

  it("releases a refused image when the course is created with the documents", async () => {
    const user = userEvent.setup();
    const { backend, simulator } = setup((b) => {
      b.picks.push(["/docs/notes.md", "/docs/photo.png"]);
    });
    await home();
    await user.click(screen.getByRole("button", { name: "Add a document" }));
    await screen.findByText("Images can't be used for a course: photo.png");
    await user.type(topic(), "Stars");
    await user.click(createButton());
    await screen.findByTestId("study-plan");
    expect(simulator.callsTo("study_create")[0].course).toMatchObject({
      documentIds: ["staged-1"],
    });
    // Le document est parti avec le cours ; l'image, restée, est libérée.
    expect(simulator.callsTo("unstage")).toEqual([{ ids: ["staged-2"] }]);
    expect(backend.staged.size).toBe(0);
  });

  it("refuses an image and more than three documents", async () => {
    const user = userEvent.setup();
    setup((b) => {
      b.picks.push(["/docs/photo.png"]);
      b.picks.push([
        "/docs/notes.md",
        "/docs/paper.pdf",
        "/docs/extra.txt",
        "/docs/more.txt",
      ]);
    });
    await home();
    const add = screen.getByRole("button", { name: "Add a document" });
    await user.click(add);
    expect(
      (await screen.findByText("Images can't be used for a course: photo.png"))
        .textContent,
    ).toBeTruthy();
    expect(createButton().disabled).toBe(true);
    await user.click(add);
    await screen.findByText(
      "A course can use up to 3 documents. Remove some to continue.",
    );
    expect(screen.getAllByTestId("study-document")).toHaveLength(4);
    await user.type(topic(), "Stars");
    expect(createButton().disabled).toBe(true);
    expect(add).toHaveProperty("disabled", true);
    await user.click(screen.getByRole("button", { name: "Remove more.txt" }));
    expect(createButton().disabled).toBe(false);
    expect(screen.queryByText(/up to 3 documents/)).toBeNull();
  });

  it("shows an unreadable document as an error", async () => {
    const user = userEvent.setup();
    setup((b) => b.picks.push(["/docs/missing.txt"]));
    await home();
    await user.click(screen.getByRole("button", { name: "Add a document" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "missing.txt: Error reading file",
    );
  });

  it("turns web search on, unless the server's cloud is disabled", async () => {
    const user = userEvent.setup();
    const { simulator } = setup();
    await home();
    const globe = screen.getByRole("button", { name: "Web search" });
    expect(globe.getAttribute("aria-pressed")).toBe("false");
    await user.click(globe);
    expect(globe.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByText("The course will use web search")).toBeTruthy();
    await user.type(topic(), "News{Enter}");
    await screen.findByTestId("study-plan");
    expect(simulator.callsTo("study_create")[0].course).toMatchObject({
      web: true,
    });
  });

  it("has no web search button when the cloud is disabled", async () => {
    setup((b) => {
      b.cloud = { disabled: true, source: "config" };
    });
    await home();
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Web search" })).toBeNull(),
    );
  });

  it("without any model: Create disabled, a hint", async () => {
    const user = userEvent.setup();
    setup(() => {}, "/study", { selectedModel: "" });
    await home();
    await user.type(topic(), "Stars");
    await screen.findByText(
      "Choose a model for your courses first (above), or a model in the chat.",
    );
    expect(createButton().disabled).toBe(true);
  });

  it("model picker: Auto first, saves the course model", async () => {
    const user = userEvent.setup();
    const { simulator } = setup();
    await home();
    const select = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Model",
    });
    await waitFor(() =>
      expect([...select.options].map((option) => option.textContent)).toEqual([
        "Auto",
        "qwen3:0.6b",
        "qwen3.5:0.8b",
      ]),
    );
    await user.selectOptions(select, "qwen3.5:0.8b");
    expect(
      simulator
        .callsTo("update_settings")
        .map((args) => args.patch as SettingsPatch),
    ).toEqual([{ studyModel: "qwen3.5:0.8b" }]);
  });

  it("warns discreetly about a small model only", async () => {
    const { simulator, app } = setup();
    await home();
    expect((await screen.findByTestId("study-small-model")).textContent).toBe(
      "Small models make more mistakes in courses. A larger model gives better lessons.",
    );
    simulator.handle("list_models", () => [
      { ...modelInfo("qwen3:0.6b"), parameterSize: "8.2B" },
    ]);
    await act(() =>
      app.queryClient.invalidateQueries({ queryKey: ["models"] }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("study-small-model")).toBeNull(),
    );
  });

  it("Auto: a loaded model is enough without a chat model", async () => {
    const user = userEvent.setup();
    const { simulator } = setup(
      (backend) => {
        backend.running = [runningModel("qwen3.5:0.8b")];
      },
      "/study",
      { selectedModel: "" },
    );
    await home();
    await user.type(topic(), "Stars");
    await waitFor(() => expect(createButton().disabled).toBe(false));
    await user.click(createButton());
    await waitFor(() =>
      expect(simulator.callsTo("study_create")).toHaveLength(1),
    );
  });

  it("Auto: the small model note follows the best loaded model", async () => {
    const { backend, app, simulator } = setup((backend) => {
      backend.running = [runningModel("big:8b")];
    });
    backend.models.push("big:8b");
    const listed = [
      modelInfo("qwen3:0.6b"),
      { ...modelInfo("big:8b"), parameterSize: "8.2B" },
    ];
    simulator.handle("list_models", () => listed);
    await home();
    await act(() =>
      app.queryClient.invalidateQueries({ queryKey: ["models"] }),
    );
    // Le modèle du chat (qwen3:0.6b) est petit, mais « Auto » prend big:8b.
    await waitFor(() =>
      expect(simulator.callsTo("auto_model").length).toBeGreaterThan(0),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("study-small-model")).toBeNull(),
    );
    backend.running = [];
    await act(() =>
      app.queryClient.invalidateQueries({ queryKey: ["autoModel"] }),
    );
    await screen.findByTestId("study-small-model");
  });

  it("shows a refused creation", async () => {
    const user = userEvent.setup();
    const { study } = setup();
    study.failures.set("study_create", {
      kind: "invalid",
      message: "The topic is too long",
    });
    await home();
    await user.type(topic(), "Stars{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The topic is too long",
    );
    expect(createButton().disabled).toBe(false);
  });

  it("lists courses in progress and completed, with their progress and score", async () => {
    const user = userEvent.setup();
    const { study, app } = setup((b) => {
      const s = b.study;
      s.seed({ topic: "Old", status: "finished", minutes: 10, updatedAt: 1 });
      const active = s.seed({ topic: "Rome", status: "active", updatedAt: 3 });
      active.answers = {};
      s.seed({
        topic: "Mars",
        status: "planned",
        level: "advanced",
        updatedAt: 2,
      });
    });
    await home();
    const inProgress = screen.getByTestId("study-in-progress");
    const rows = within(inProgress).getAllByTestId("study-course");
    expect(
      rows.map((row) => within(row).getAllByText(/./)[0].textContent),
    ).toEqual(["Course on Rome", "Course on Mars"]);
    expect(rows[0].textContent).toContain("Discover · 20 min");
    expect(rows[0].textContent).toContain("0 of 4 levels");
    expect(rows[1].textContent).toContain("Go deeper · 20 min");
    expect(rows[1].textContent).toContain("Ready to start");
    const completed = within(screen.getByTestId("study-completed")).getByTestId(
      "study-course",
    );
    expect(completed.textContent).toContain("Final review: 3 / 3");
    expect(
      within(completed).getByRole("progressbar").getAttribute("aria-valuenow"),
    ).toBe("100");
    await user.click(within(rows[1]).getByRole("link"));
    await screen.findByTestId("study-plan");
    expect(app.path()).toBe(`/study/${[...study.courses.keys()][2]}`);
  });

  it("deletes a course after confirmation", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((b) => {
      b.study.seed({ id: "rome", topic: "Rome", status: "active" });
    });
    await home();
    await user.click(
      screen.getByRole("button", { name: "Delete Course on Rome" }),
    );
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByRole("heading").textContent).toBe(
      "Delete “Course on Rome”?",
    );
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(simulator.callsTo("study_delete")).toEqual([]);
    await user.click(
      screen.getByRole("button", { name: "Delete Course on Rome" }),
    );
    await user.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(simulator.callsTo("study_delete")).toEqual([{ courseId: "rome" }]);
    await screen.findByTestId("study-empty");
  });

  it("reads the list again when a course changes elsewhere", async () => {
    const { study } = setup((b) => {
      b.study.auto = false;
      b.study.seed({ id: "rome", topic: "Rome", status: "planning" });
    });
    await home();
    expect(screen.getByTestId("study-course").textContent).toContain(
      "Preparing…",
    );
    await study.finishPlan("rome");
    await waitFor(() =>
      expect(screen.getByTestId("study-course").textContent).toContain(
        "Ready to start",
      ),
    );
  });
});

describe("Study home in other languages", () => {
  it("has no hard-coded text: form, documents, errors, lists, dialog", async () => {
    const user = userEvent.setup();
    setup((b) => {
      b.picks.push(["/docs/notes.md", "/docs/photo.png", "/docs/missing.txt"]);
      b.study.seed({ topic: "Rome", status: "active" });
      b.study.seed({ topic: "Mars", status: "finished" });
      b.study.seed({ topic: "Moon", status: "planning" });
    });
    await home();
    await user.click(screen.getByRole("button", { name: "Add a document" }));
    await screen.findAllByTestId("study-document");
    await user.click(screen.getByRole("button", { name: "Web search" }));
    usePseudoLanguage();
    const allow = [
      "notes.md",
      "photo.png",
      "missing.txt",
      "Course on Rome",
      "Course on Mars",
      "Moon",
      "qwen3:0.6b",
      "qwen3.5:0.8b",
    ];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await user.click(
      screen.getAllByRole("button", { name: /^⟦Delete .*⟧$/ })[0],
    );
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });

  it("in French and Spanish: levels, plural, relative date", async () => {
    setup((b) => {
      b.study.seed({ topic: "Rome", status: "active", updatedAt: Date.now() });
    });
    await home();
    act(() => setLanguage("fr"));
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Étudier",
    );
    expect(
      screen.getByText(/^Choisissez n'importe quel sujet/).textContent,
    ).toBe(
      "Choisissez n'importe quel sujet\u00a0: votre modèle rédige un petit cours avec des leçons, des exemples résolus et des questions, une étape à la fois.",
    );
    expect(screen.getByRole("radio", { name: "J'ai des bases" })).toBeTruthy();
    const row = screen.getByTestId("study-course");
    expect(row.textContent).toContain("Découverte · 20 min");
    expect(row.textContent).toContain("0 sur 4 paliers");
    expect(
      screen.getByRole("textbox", { name: "Que voulez-vous apprendre ?" }),
    ).toBeTruthy();
    act(() => setLanguage("es"));
    expect(screen.getByRole("radio", { name: "Tengo nociones" })).toBeTruthy();
    expect(screen.getByTestId("study-course").textContent).toContain(
      "0 de 4 niveles",
    );
  });
});
