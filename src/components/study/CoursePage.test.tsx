// Page d'un cours face au simulateur, avant le lecteur : préparation du plan
// (étapes, Cancel), sujet ambigu (sens proposé, sens libre, sujet tel quel),
// plan (contenu, Start, New plan, Delete), échec (compte relié ou cours sans
// le web, modèle, Retry), cours introuvable, relecture à chaque
// « study-updated » ; textes traduits (pseudo-langue, français).

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { setLanguage } from "@/i18n";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import type { SeedOptions } from "@/test/study";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(
  options: SeedOptions = {},
  prepare: (backend: FakeBackend) => void = () => {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  const course = backend.study.seed({ id: "c1", ...options });
  const app = renderApp(`/study/${course.id}`);
  return { simulator, backend, study: backend.study, app, course };
}

const button = (name: string) =>
  screen.getByRole<HTMLButtonElement>("button", { name });

describe("Course being prepared", () => {
  it("shows the steps (documents, web search, plan) and Cancel deletes the course", async () => {
    const user = userEvent.setup();
    const { simulator, app } = setup({
      status: "planning",
      web: true,
      sources: [{ kind: "document", title: "notes.md", url: "" }],
    });
    await screen.findByTestId("study-planning");
    expect(screen.getByTestId("study-planning").textContent).toContain(
      "Preparing your course…",
    );
    const steps = within(screen.getByRole("list", { name: "Steps" }))
      .getAllByRole("listitem")
      .map((item) => [item.textContent, item.getAttribute("aria-busy")]);
    expect(steps).toEqual([
      ["Reading 1 documentdone", null],
      ["Searching the web", "true"],
      ["Writing the plan", "true"],
    ]);
    await user.click(button("Cancel"));
    expect(simulator.callsTo("study_delete")).toEqual([{ courseId: "c1" }]);
    await screen.findByTestId("study-home");
    expect(app.path()).toBe("/study");
  });

  it("in French and Spanish: a finished step reads as done (feminine in French)", async () => {
    setup({
      status: "planning",
      sources: [{ kind: "document", title: "notes.md", url: "" }],
    });
    await screen.findByTestId("study-planning");
    const first = () =>
      within(screen.getByRole("list")).getAllByRole("listitem")[0].textContent;
    act(() => setLanguage("fr"));
    expect(first()).toBe("Lecture de 1 documentterminée");
    act(() => setLanguage("es"));
    expect(first()).toBe("Leyendo 1 documentolisto");
  });

  it("shows the plan as soon as it is written (study-updated)", async () => {
    const { study } = setup({ status: "planning" }, (b) => {
      b.study.auto = false;
    });
    await screen.findByTestId("study-planning");
    await study.finishPlan("c1");
    await screen.findByTestId("study-plan");
  });
});

describe("Unclear topic", () => {
  it("offers each meaning, then the plan", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({ status: "unclear", topic: "Mercury" });
    await screen.findByTestId("study-unclear");
    expect(screen.getByTestId("study-unclear").textContent).toBe(
      "Which meaning do you want?",
    );
    await user.click(button("The planet"));
    expect(simulator.callsTo("study_clarify")).toEqual([
      { courseId: "c1", choice: "The planet" },
    ]);
    await screen.findByTestId("study-plan");
    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
      "Course on Mercury (The planet)",
    );
  });

  it("takes a meaning typed by the learner, or the topic as it is", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({ status: "unclear", topic: "Mercury" });
    await screen.findByTestId("study-unclear");
    const use = button("Use this meaning");
    expect(use.disabled).toBe(true);
    await user.type(
      screen.getByRole("textbox", {
        name: "Something else? Say what you mean:",
      }),
      "The Roman god{Enter}",
    );
    expect(simulator.callsTo("study_clarify")).toEqual([
      { courseId: "c1", choice: "The Roman god" },
    ]);
    await screen.findByTestId("study-plan");
  });

  it("Use the topic as it is sends no meaning", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({ status: "unclear" });
    await screen.findByTestId("study-unclear");
    await user.click(button("Use the topic as it is"));
    expect(simulator.callsTo("study_clarify")).toEqual([
      { courseId: "c1", choice: null },
    ]);
  });
});

describe("Course plan", () => {
  it("shows title, summary, objectives, levels and their notions, duration and sources", async () => {
    setup({
      status: "planned",
      topic: "Rome",
      web: true,
      sources: [{ kind: "document", title: "notes.md", url: "" }],
    });
    const plan = await screen.findByTestId("study-plan");
    expect(within(plan).getByRole("heading", { level: 2 }).textContent).toBe(
      "Course on Rome",
    );
    expect(plan.textContent).toContain("A short course about Rome.");
    expect(plan.textContent).toContain("Discover");
    expect(plan.textContent).toContain("About 20 min");
    expect(plan.textContent).toContain("4 levels");
    expect(
      within(plan)
        .getAllByRole("heading", { level: 3 })
        .map((h) => h.textContent),
    ).toEqual(["Objectives", "Levels", "Sources"]);
    expect(plan.textContent).toContain("Know the basics");
    expect(plan.textContent).toContain("Level 1 Level title 1");
    expect(
      within(plan).getAllByRole("list", { name: "Key ideas" })[0].textContent,
    ).toBe("Point 1aPoint 1b");
    expect(within(plan).getByText("notes.md")).toBeTruthy();
    const link = within(plan).getByRole("link", { name: "A web page" });
    expect(link.getAttribute("href")).toBe("https://example.com/page");
  });

  it("Start opens the first level (the plan has just been read)", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({ status: "planned" });
    await screen.findByTestId("study-plan");
    expect(document.activeElement).toBe(button("Start"));
    await user.click(button("Start"));
    expect(simulator.callsTo("study_start")).toEqual([{ courseId: "c1" }]);
    const section = await screen.findByTestId("study-section");
    expect(within(section).getByRole("heading", { level: 3 }).textContent).toBe(
      "Heading 1.1",
    );
    // Position enregistrée : la première carte.
    await waitFor(() =>
      expect(simulator.callsTo("study_progress").at(-1)).toMatchObject({
        courseId: "c1",
        position: "L0:s0",
      }),
    );
  });

  it("New plan asks for another plan", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({ status: "planned", topic: "Rome" });
    await screen.findByTestId("study-plan");
    await user.click(button("New plan"));
    expect(simulator.callsTo("study_replan")).toEqual([{ courseId: "c1" }]);
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
        "Course on Rome (plan 2)",
      ),
    );
  });

  it("Delete asks first, then goes back to Study", async () => {
    const user = userEvent.setup();
    const { simulator, app } = setup({ status: "planned" });
    await screen.findByTestId("study-plan");
    await user.click(button("Delete"));
    await user.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(simulator.callsTo("study_delete")).toEqual([{ courseId: "c1" }]);
    await screen.findByTestId("study-home");
    expect(app.path()).toBe("/study");
  });
});

describe("Failed course", () => {
  it("signed out: Sign In opens the sign-in page, or the course goes on without web search", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({
      status: "failed",
      web: true,
      error: { code: "signedOut", message: "Sign in to use web search." },
    });
    const alert = await screen.findByTestId("study-error");
    expect(alert.textContent).toContain("Sign in to use web search.");
    await user.click(within(alert).getByRole("button", { name: "Sign In" }));
    expect(simulator.callsTo("account_sign_in")).toHaveLength(1);
    await user.click(
      within(alert).getByRole("button", {
        name: "Continue without web search",
      }),
    );
    expect(simulator.callsTo("study_retry")).toEqual([
      { courseId: "c1", withoutWeb: true },
    ]);
    await screen.findByTestId("study-plan");
  });

  it("signed out, then signed in: Retry, with web search", async () => {
    const user = userEvent.setup();
    const { simulator } = setup(
      {
        status: "failed",
        web: true,
        error: { code: "signedOut", message: "Sign in to use web search." },
      },
      (b) => {
        b.account = {
          state: "signedIn",
          name: "ada",
          email: "ada@example.com",
          plan: "free",
          avatar: null,
        };
      },
    );
    const alert = await screen.findByTestId("study-error");
    await within(alert).findByText("Signed in.");
    expect(
      within(alert)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Retry", "Continue without web search"]);
    await user.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(simulator.callsTo("study_retry")).toEqual([
      { courseId: "c1", withoutWeb: false },
    ]);
  });

  it("no model: the model picker, ModelHub and Retry", async () => {
    const user = userEvent.setup();
    const { simulator } = setup({
      status: "failed",
      error: { code: "modelNotFound", message: "Model “x” not found" },
    });
    const alert = await screen.findByTestId("study-error");
    expect(within(alert).getByRole("combobox", { name: "Model" })).toBeTruthy();
    expect(
      within(alert)
        .getByRole("link", { name: "Open ModelHub" })
        .getAttribute("href"),
    ).toBe("/hub");
    await user.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(simulator.callsTo("study_retry")).toEqual([
      { courseId: "c1", withoutWeb: false },
    ]);
  });

  it("other failures: Retry only", async () => {
    setup({
      status: "failed",
      error: { code: "unreachable", message: "Ollama is not reachable" },
    });
    const alert = await screen.findByTestId("study-error");
    expect(
      within(alert)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Retry"]);
  });

  it("a course deleted elsewhere is not found", async () => {
    const { study } = setup({ status: "planned" });
    await screen.findByTestId("study-plan");
    study.courses.delete("c1");
    await study.emit("c1", true);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "This course no longer exists.",
    );
    expect(screen.getByRole("link", { name: "Back to courses" })).toBeTruthy();
    act(() => setLanguage("fr"));
    expect(screen.getByRole("link", { name: "Retour aux cours" })).toBeTruthy();
    act(() => setLanguage("es"));
    expect(
      screen.getByRole("link", { name: "Volver a los cursos" }),
    ).toBeTruthy();
  });
});

describe("Course page in other languages", () => {
  it("has no hard-coded text: planning, unclear, plan, failure", async () => {
    const user = userEvent.setup();
    const { study, backend, app } = setup(
      {
        status: "planning",
        web: true,
        sources: [{ kind: "document", title: "notes.md", url: "" }],
      },
      (b) => {
        b.study.auto = false;
      },
    );
    await screen.findByTestId("study-planning");
    usePseudoLanguage();
    const allow = [
      "Photosynthesis",
      "notes.md",
      "Which meaning do you want?",
      "The planet",
      "The metal",
      "Course on Photosynthesis",
      "A short course about Photosynthesis.",
      "Know the basics",
      "Use the main idea",
      "Avoid the trap",
      "Level title 1",
      "Level title 2",
      "Level title 3",
      "Level title 4",
      "Goal of level 1",
      "Goal of level 2",
      "Goal of level 3",
      "Goal of level 4",
      "Point 1a",
      "Point 1b",
      "Point 2a",
      "Point 2b",
      "Point 3a",
      "Point 3b",
      "Point 4a",
      "Point 4b",
      "A web page",
      "https://example.com/page",
      "Calvin cycle details",
      "Boom",
      "qwen3:0.6b",
      "qwen3.5:0.8b",
    ];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await study.update("c1", (course) => {
      course.status = "unclear";
      course.clarify = {
        question: "Which meaning do you want?",
        options: ["The planet", "The metal"],
      };
    });
    await screen.findByTestId("study-unclear");
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await study.finishPlan("c1");
    await study.update("c1", (course) => {
      course.plan!.scope = "Calvin cycle details";
    });
    await screen.findByText(/Calvin cycle details/);
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await study.failPlan("c1", { code: "noModel", message: "Boom" });
    await screen.findByTestId("study-error");
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await study.failPlan("c1", { code: "modelNotFound", message: "Boom" });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    // Recherche web sans compte, puis compte relié.
    await study.failPlan("c1", { code: "signedOut", message: "Boom" });
    await screen.findByRole("button", { name: "⟦Sign In⟧" });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    backend.account = {
      state: "signedIn",
      name: "ada",
      email: "ada@example.com",
      plan: "free",
      avatar: null,
    };
    await act(() =>
      app.queryClient.invalidateQueries({ queryKey: ["account"] }),
    );
    await screen.findByRole("button", { name: "⟦Retry⟧" });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await user.keyboard("{Tab}");
  });

  it("in French: plan with plurals, typography", async () => {
    setup({ status: "planned", minutes: 10 });
    await screen.findByTestId("study-plan");
    act(() => setLanguage("fr"));
    const plan = screen.getByTestId("study-plan");
    expect(plan.textContent).toContain("Le plan de votre cours");
    expect(plan.textContent).toContain("2 paliers");
    expect(plan.textContent).toContain("Environ 10 min");
    expect(plan.textContent).toContain("Objectifs");
    expect(within(plan).getAllByRole("list", { name: "Notions" })).toHaveLength(
      2,
    );
    expect(button("Commencer")).toBeTruthy();
  });
});
