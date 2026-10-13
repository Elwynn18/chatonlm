// Écriture d'un cours face au simulateur : ce qui s'écrit dit en texte
// (plan, palier n sur m et son étape, nouvel essai, bilan), paliers prêts,
// « on peut commencer dès que le premier palier est prêt », durées dites
// « d'étude » ; « Pause » / « Resume » (page de préparation, plan, lecteur,
// attente d'un palier, nouvel essai), demandes qui reprennent un cours en
// pause, tour d'un autre cours, partie en échec relancée, liste de
// l'accueil ; textes traduits (pseudo-langue, français, espagnol).

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Course } from "@/bindings/Course";
import { resetStudyStreamsForTests } from "@/hooks/useStudy";
import { setLanguage } from "@/i18n";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import type { FakeStudy, SeedOptions } from "@/test/study";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

beforeEach(() => resetStudyStreamsForTests());

/**
 * Cours « c1 » dont rien ne s'écrit tout seul (le test fait avancer
 * l'écriture), préparé par `before` avant l'ouverture de sa page.
 */
function setup(
  options: SeedOptions = {},
  before: (study: FakeStudy, course: Course) => Promise<void> | void = () => {},
) {
  const simulator = installSimulator();
  const backend: FakeBackend = fakeBackend(simulator);
  const study = backend.study;
  study.auto = false;
  const course = study.seed({ id: "c1", ...options });
  return {
    simulator,
    backend,
    study,
    course,
    async open(path = "/study/c1") {
      await before(study, course);
      return renderApp(path);
    },
  };
}

/** Les `right` premières réponses de l'essai sont justes. */
async function answerAttempt(
  study: FakeStudy,
  course: Course,
  level: number,
  attempt: number,
  right: number,
) {
  const questions = course.levels[level].attempts[attempt].questions;
  for (const [n, question] of questions.entries()) {
    await study.answer(course.id, question.id, n < right);
  }
}

const status = () => screen.getByTestId("study-generation");
const statusText = () => status().textContent;
const stage = () => screen.queryByTestId("study-generation-stage")?.textContent;
const hint = () => screen.queryByTestId("study-generation-hint")?.textContent;
const pause = () => screen.getByTestId<HTMLButtonElement>("study-pause");

describe("Writing of a course, on its plan", () => {
  it("says what is being written, how many levels are ready, and when to start", async () => {
    const { study, open } = setup({ status: "planned", readyLevels: 0 });
    await open();
    const plan = await screen.findByTestId("study-plan");
    // Une durée d'étude : l'écriture est dite à part.
    expect(plan.textContent).toContain("About 20 min of study");
    expect(status().getAttribute("role")).toBe("status");
    expect(status().dataset.state).toBe("writing");
    expect(statusText()).toBe("Writing level 1 of 4…·0 of 4 levels ready");
    expect(stage()).toBe("Step 1 of 2: the lesson");
    expect(hint()).toBe(
      "You can start as soon as the first level is ready. The next levels are written while you study.",
    );
    // L'étape n'est pas dans la région annoncée (pas d'annonce à chaque
    // étape d'une partie).
    expect(
      status().contains(screen.getByTestId("study-generation-stage")),
    ).toBe(false);
    // La roue n'est pas le seul signe, et ne tourne pas sous mouvement réduit.
    expect(status().querySelector("svg")?.getAttribute("class")).toContain(
      "motion-safe:animate-spin",
    );

    await study.writeLevel("c1", 0, "questions");
    await waitFor(() => expect(stage()).toBe("Step 2 of 2: the questions"));
    expect(statusText()).toBe("Writing level 1 of 4…·0 of 4 levels ready");

    await study.readyLevel("c1", 0);
    await waitFor(() => expect(statusText()).toBe("1 of 4 levels ready"));
    expect(status().dataset.state).toBe("idle");
    expect(stage()).toBeUndefined();
    expect(hint()).toBe(
      "The first level is ready: you can start. The next levels are written while you study.",
    );
    // Rien à mettre en pause avant « Start ».
    expect(screen.queryByTestId("study-pause")).toBeNull();
  });

  it("Pause stops the writing of this course, Resume takes it up again", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup(
      { status: "planned", readyLevels: 0 },
      (s) => s.writeLevel("c1", 0, "questions"),
    );
    await open();
    await screen.findByTestId("study-plan");
    expect(pause().textContent).toBe("Pause");
    expect(pause().getAttribute("aria-label")).toBe(
      "Pause the writing of this course",
    );
    await user.click(pause());
    expect(simulator.callsTo("study_pause")).toEqual([
      { courseId: "c1", paused: true },
    ]);
    await waitFor(() => expect(status().dataset.state).toBe("paused"));
    expect(statusText()).toBe("Writing paused·0 of 4 levels ready");
    expect(stage()).toBeUndefined();
    expect(hint()).toBe(
      "Nothing more is written for this course until you resume.",
    );
    // La partie arrêtée attend la reprise.
    expect(study.course("c1").levels[0].state).toBe("pending");
    expect(status().querySelector("svg")?.getAttribute("class")).not.toContain(
      "animate-spin",
    );
    expect(pause().textContent).toBe("Resume");
    expect(pause().getAttribute("aria-label")).toBe(
      "Resume the writing of this course",
    );
    await user.click(pause());
    expect(simulator.callsTo("study_pause")[1]).toEqual({
      courseId: "c1",
      paused: false,
    });
    await waitFor(() => expect(status().dataset.state).toBe("writing"));
    expect(statusText()).toBe("Writing level 1 of 4…·0 of 4 levels ready");
  });

  it("a refused pause is shown", async () => {
    const user = userEvent.setup();
    const { study, open } = setup({ status: "planned", readyLevels: 0 });
    study.failures.set("study_pause", {
      kind: "invalid",
      message: "This action is not available for this course right now",
    });
    await open();
    await screen.findByTestId("study-plan");
    await user.click(pause());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "This action is not available for this course right now",
    );
    expect(status().dataset.state).toBe("writing");
  });
});

describe("Writing of a course, while its plan is prepared", () => {
  it("tells when to start, and pauses the plan", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({ status: "planning" });
    await open();
    await screen.findByTestId("study-planning");
    const steps = () =>
      within(screen.getByRole("list", { name: "Steps" }))
        .getAllByRole("listitem")
        .map((item) => [item.textContent, item.getAttribute("aria-busy")]);
    // L'écriture du plan est dite par les étapes : pas deux fois.
    expect(steps()).toEqual([["Writing the plan", "true"]]);
    expect(statusText()).toBe("");
    expect(hint()).toBe(
      "You can start as soon as the first level is ready. The next levels are written while you study.",
    );
    await user.click(pause());
    expect(simulator.callsTo("study_pause")).toEqual([
      { courseId: "c1", paused: true },
    ]);
    await waitFor(() => expect(statusText()).toBe("Writing paused"));
    // Plus rien n'avance.
    expect(steps()).toEqual([["Writing the plan", null]]);
    expect(
      screen
        .getByRole("list", { name: "Steps" })
        .querySelector(".animate-spin, [class*=animate-spin]"),
    ).toBeNull();
    await user.click(pause());
    await waitFor(() => expect(statusText()).toBe(""));
    expect(steps()).toEqual([["Writing the plan", "true"]]);
  });

  it("another course written first: waiting, said in words", async () => {
    const { open } = setup({ status: "planning" }, (_, course) => {
      course.queueBusy = true;
    });
    await open();
    await screen.findByTestId("study-planning");
    expect(status().dataset.state).toBe("waiting");
    expect(statusText()).toBe(
      "Waiting for another course to be written first…",
    );
  });
});

describe("Writing of a course, in the reader", () => {
  it("the header tells the level being written and the levels ready, then nothing once all is written", async () => {
    const { study, open } = setup({ minutes: 10, readyLevels: 1 });
    await open();
    await screen.findByTestId("study-path");
    // Temps d'étude restant, et à part l'écriture en cours.
    expect(screen.getByText("About 8 min of study left")).toBeTruthy();
    expect(statusText()).toBe("Writing level 2 of 2…·1 of 2 levels ready");
    // Une ligne : l'étape est laissée aux pages d'attente.
    expect(stage()).toBeUndefined();
    await study.readyLevel("c1", 1);
    await waitFor(() =>
      expect(statusText()).toBe(
        "Writing the final review…·2 of 2 levels ready",
      ),
    );
    await study.readyFinal("c1");
    await waitFor(() => expect(status().dataset.state).toBe("idle"));
    // Plus rien de visible ; dit une fois aux lecteurs d'écran.
    expect(statusText()).toBe("The whole course is written.");
    expect(status().querySelector(".sr-only")?.textContent).toBe(
      "The whole course is written.",
    );
    expect(screen.queryByTestId("study-pause")).toBeNull();
  });

  it("Pause in the header: the path no longer says a level is being written", async () => {
    const user = userEvent.setup();
    const { study, open } = setup({ minutes: 10, readyLevels: 1 }, (s) =>
      s.writeLevel("c1", 1, "lesson"),
    );
    await open();
    await screen.findByTestId("study-path");
    expect(screen.getByTestId("study-path").textContent).toContain(
      "Being written…",
    );
    await user.click(pause());
    await waitFor(() =>
      expect(statusText()).toBe("Writing paused·1 of 2 levels ready"),
    );
    expect(screen.getByTestId("study-path").textContent).not.toContain(
      "Being written…",
    );
    expect(study.course("c1").paused).toBe(true);
  });

  it("a level reached before it is written: what is written first, its step, the levels ready", async () => {
    const { study, open } = setup(
      { minutes: 20, readyLevels: 1 },
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 3);
        course.position = "L1:s0";
        await s.writeLevel("c1", 1, "lesson");
      },
    );
    await open();
    const wait = await screen.findByTestId("study-level-wait");
    expect(wait.textContent).toContain("This level isn't written yet.");
    const inCard = within(wait);
    // Une seule région d'annonces : celle de la carte remplace l'en-tête.
    expect(screen.getAllByRole("status")).toEqual([
      inCard.getByTestId("study-generation"),
    ]);
    expect(inCard.getByTestId("study-generation").textContent).toBe(
      "Writing level 2 of 4…·1 of 4 levels ready",
    );
    expect(inCard.getByTestId("study-generation-stage").textContent).toBe(
      "Step 1 of 2: the lesson",
    );
    await study.writeLevel("c1", 1, "questions");
    await waitFor(() =>
      expect(inCard.getByTestId("study-generation-stage").textContent).toBe(
        "Step 2 of 2: the questions",
      ),
    );
    await study.readyLevel("c1", 1);
    await waitFor(() =>
      expect(screen.queryByTestId("study-level-wait")).toBeNull(),
    );
    expect(screen.getByRole("heading", { level: 3 }).textContent).toBe(
      "Heading 2.1",
    );
  });

  it("a level reached while the course is paused: the wait says so and offers Resume", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup(
      { minutes: 10, readyLevels: 1, paused: true },
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 3);
        course.position = "L1:s0";
      },
    );
    await open();
    const wait = await screen.findByTestId("study-level-wait");
    const inCard = within(wait);
    expect(inCard.getByTestId("study-generation").textContent).toBe(
      "Writing paused·1 of 2 levels ready",
    );
    expect(inCard.getByTestId("study-generation-hint").textContent).toBe(
      "Nothing more is written for this course until you resume.",
    );
    const resume = inCard.getByRole("button", {
      name: "Resume the writing of this course",
    });
    expect(resume.textContent).toBe("Resume");
    await user.click(resume);
    expect(simulator.callsTo("study_pause")).toEqual([
      { courseId: "c1", paused: false },
    ]);
    await waitFor(() =>
      expect(inCard.getByTestId("study-generation").textContent).toBe(
        "Writing level 2 of 2…·1 of 2 levels ready",
      ),
    );
  });

  it("the final review reached before the last level is written, then while paused", async () => {
    const user = userEvent.setup();
    const { study, open } = setup(
      { minutes: 10, finalReady: false },
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 3);
        await answerAttempt(s, course, 1, 0, 3);
        course.position = "final:q0";
      },
    );
    await open();
    const wait = await screen.findByTestId("study-final-wait");
    expect(wait.textContent).toContain("The final review isn't written yet.");
    expect(wait.textContent).not.toContain("after the last level");
    expect(within(wait).getByTestId("study-generation").textContent).toBe(
      "Writing the final review…·2 of 2 levels ready",
    );
    await user.click(within(wait).getByTestId("study-pause"));
    await waitFor(() =>
      expect(within(wait).getByTestId("study-generation").textContent).toBe(
        "Writing paused·2 of 2 levels ready",
      ),
    );
    // Un palier refait entre-temps : le bilan vient après lui.
    await study.update("c1", (course) => {
      course.levels[1].state = "pending";
    });
    await waitFor(() =>
      expect(wait.textContent).toContain("It's written after the last level."),
    );
  });

  it("a request that needs writing resumes a paused course: Explain differently", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup(
      { minutes: 10, paused: true, finalReady: false },
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 0);
        course.position = "L0:a0:result";
      },
    );
    await open();
    await screen.findByTestId("study-result");
    expect(statusText()).toBe("Writing paused·2 of 2 levels ready");
    // Relectures du cours retenues : ce qui suit vient de la réponse de la
    // commande, pas du cours relu.
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    simulator.handle("study_get", async (args) => {
      await held;
      return study.course(args.courseId as string);
    });
    await user.click(
      screen.getByRole("button", { name: "Explain differently" }),
    );
    expect(simulator.callsTo("study_remedy")).toEqual([
      { courseId: "c1", level: 0 },
    ]);
    // Le moteur reprend le cours : l'attente du nouvel essai tout de suite,
    // pas la pause.
    await screen.findByText("Preparing a new explanation…");
    expect(statusText()).toBe(
      "Writing a new explanation for level 1…·2 of 2 levels ready",
    );
    expect(screen.getAllByTestId("study-generation")).toHaveLength(1);
    release();
    await waitFor(() => expect(study.course("c1").paused).toBe(false));
    expect(statusText()).toBe(
      "Writing a new explanation for level 1…·2 of 2 levels ready",
    );
  });

  it("a new attempt asked for, then the course paused: the result says so and offers Resume", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({ minutes: 10 }, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 0);
      course.levels[0].remedy = "pending";
      course.paused = true;
      course.position = "L0:a0:result";
    });
    await open();
    const result = await screen.findByTestId("study-result");
    expect(result.textContent).not.toContain("Preparing a new explanation…");
    expect(within(result).getByTestId("study-generation").textContent).toBe(
      "Writing paused·2 of 2 levels ready",
    );
    // Dit aussi par l'en-tête, seule région d'annonces.
    expect(screen.getAllByRole("status")).toEqual([
      within(screen.getByRole("banner")).getByTestId("study-generation"),
    ]);
    // Le parcours ne dit pas « en cours d'écriture » un nouvel essai que la
    // pause retient.
    const path = screen.getByTestId("study-path");
    expect(path.textContent).not.toContain("Being written…");
    await user.click(within(result).getByTestId("study-pause"));
    expect(simulator.callsTo("study_pause")).toEqual([
      { courseId: "c1", paused: false },
    ]);
    await screen.findByText("Preparing a new explanation…");
    expect(path.textContent).toContain("Being written…");
  });

  it("a level that could not be written, further on: said in the header, with Retry", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup({ minutes: 20, readyLevels: 2 });
    await open();
    await screen.findByTestId("study-path");
    await study.failLevel("c1", 2, {
      code: "failed",
      message: "The level could not be written.",
    });
    // Un palier reste à écrire : lui d'abord, l'échec ensuite.
    await waitFor(() =>
      expect(statusText()).toBe("Writing level 4 of 4…·2 of 4 levels ready"),
    );
    await study.readyLevel("c1", 3);
    await waitFor(() => expect(status().dataset.state).toBe("failed"));
    expect(statusText()).toBe(
      "Level 3 couldn't be written.·3 of 4 levels ready",
    );
    expect(screen.queryByTestId("study-pause")).toBeNull();
    await user.click(
      within(screen.getByRole("banner")).getByRole("button", { name: "Retry" }),
    );
    expect(simulator.callsTo("study_retry")).toEqual([
      { courseId: "c1", withoutWeb: false },
    ]);
    await waitFor(() =>
      expect(statusText()).toBe("Writing level 3 of 4…·3 of 4 levels ready"),
    );
  });
});

describe("Writing of the courses, in the list", () => {
  it("tells which courses are still being written or paused, and their levels ready", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const study = backend.study;
    study.auto = false;
    study.seed({ id: "done", topic: "Done", minutes: 10, updatedAt: 5 });
    study.seed({
      id: "writing",
      topic: "Writing",
      readyLevels: 1,
      updatedAt: 4,
    });
    study.seed({
      id: "paused",
      topic: "Paused",
      readyLevels: 2,
      paused: true,
      updatedAt: 3,
    });
    study.seed({
      id: "planning",
      topic: "Planning",
      status: "planning",
      paused: true,
      updatedAt: 2,
    });
    study.seed({ id: "new", topic: "New", status: "planning", updatedAt: 1 });
    renderApp("/study");
    const rows = await screen.findAllByTestId("study-course");
    const writing = (row: HTMLElement) =>
      within(row).queryByTestId("study-course-writing")?.textContent;
    expect(rows.map(writing)).toEqual([
      undefined,
      "Being written… 1 of 4 levels ready",
      "Writing paused 2 of 4 levels ready",
      "Writing paused",
      undefined,
    ]);
    expect(rows[4].textContent).toContain("Preparing…");
    // La liste suit l'écriture (study-updated).
    await study.readyLevel("writing", 1);
    await waitFor(() =>
      expect(writing(screen.getAllByTestId("study-course")[0])).toBe(
        "Being written… 2 of 4 levels ready",
      ),
    );
  });
});

describe("Writing of a course in other languages", () => {
  it("has no hard-coded text on a wait card: writing, pause, failure", async () => {
    const user = userEvent.setup();
    const { study, course, open } = setup(
      { minutes: 10, readyLevels: 1 },
      async (s, c) => {
        await answerAttempt(s, c, 0, 0, 3);
        c.position = "L1:s0";
        await s.writeLevel("c1", 1, "lesson");
      },
    );
    await open();
    await screen.findByTestId("study-level-wait");
    usePseudoLanguage();
    const texts = (value: unknown, out: string[] = []): string[] => {
      if (typeof value === "string") {
        if (value) out.push(value);
      } else if (value && typeof value === "object") {
        for (const item of Object.values(value)) texts(item, out);
      }
      return out;
    };
    const allow = () => [...texts(course), "qwen3:0.6b", "?", "·"];
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    // En pause.
    await user.click(screen.getAllByTestId("study-pause")[0]);
    await waitFor(() => expect(study.course("c1").paused).toBe(true));
    await waitFor(() =>
      expect(screen.getAllByTestId("study-generation")[0].dataset.state).toBe(
        "paused",
      ),
    );
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    // Repris, puis en échec : l'erreur de la carte d'attente.
    await user.click(screen.getAllByTestId("study-pause")[0]);
    await study.failLevel("c1", 1, { code: "failed", message: "Refused." });
    await screen.findByTestId("study-error");
    expect(untranslatedTexts(document.body, [...allow(), "Refused."])).toEqual(
      [],
    );
  });

  it("has no hard-coded text in the header of the reader: writing, failure", async () => {
    const { study, course, open } = setup({ minutes: 10, readyLevels: 1 });
    await open();
    await screen.findByTestId("study-path");
    usePseudoLanguage();
    const texts = (value: unknown, out: string[] = []): string[] => {
      if (typeof value === "string") {
        if (value) out.push(value);
      } else if (value && typeof value === "object") {
        for (const item of Object.values(value)) texts(item, out);
      }
      return out;
    };
    const allow = () => [...texts(course), "qwen3:0.6b", "?", "·"];
    expect(status().dataset.state).toBe("writing");
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    await study.failLevel("c1", 1, { code: "failed", message: "Refused." });
    await waitFor(() => expect(status().dataset.state).toBe("failed"));
    expect(statusText()).toBe(
      "⟦Level 2 couldn't be written.⟧·⟦1 of 2 levels ready⟧",
    );
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
  });

  it("has no hard-coded text on the plan", async () => {
    const { study, course, open } = setup({
      status: "planned",
      readyLevels: 0,
    });
    await open();
    await screen.findByTestId("study-plan");
    usePseudoLanguage();
    const texts = (value: unknown, out: string[] = []): string[] => {
      if (typeof value === "string") {
        if (value) out.push(value);
      } else if (value && typeof value === "object") {
        for (const item of Object.values(value)) texts(item, out);
      }
      return out;
    };
    const allow = () => [...texts(course), "qwen3:0.6b", "?", "·"];
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    await study.readyLevel("c1", 0);
    await waitFor(() =>
      expect(screen.getByTestId("study-generation").dataset.state).toBe("idle"),
    );
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
  });

  it("has no hard-coded text in the list", async () => {
    const { study, open } = setup({ readyLevels: 1 }, (s) => {
      s.seed({ id: "c2", topic: "Rome", readyLevels: 2, paused: true });
    });
    await open("/study");
    await screen.findAllByTestId("study-course-writing");
    usePseudoLanguage();
    const allowed = [
      ...[...study.courses.values()].map((course) => course.plan!.title),
      "qwen3:0.6b",
      "qwen3.5:0.8b",
      "?",
    ];
    expect(
      screen
        .getAllByTestId("study-course-writing")
        .map((element) => element.textContent),
    ).toEqual([
      "⟦Writing paused⟧ ⟦2 of 4 levels ready⟧",
      "⟦Being written…⟧ ⟦1 of 4 levels ready⟧",
    ]);
    expect(
      untranslatedTexts(screen.getByTestId("study-in-progress"), allowed),
    ).toEqual([]);
  });

  it("in French and Spanish: study time, writing, plurals, typography", async () => {
    const { study, open } = setup({ status: "planned", readyLevels: 0 });
    await open();
    const plan = await screen.findByTestId("study-plan");
    act(() => setLanguage("fr"));
    expect(plan.textContent).toContain("Environ 20 min d'étude");
    expect(statusText()).toBe(
      "Écriture du palier 1 sur 4…·0 palier prêt sur 4",
    );
    expect(stage()).toBe("Étape 1 sur 2 : la leçon");
    expect(pause().textContent).toBe("Pause");
    expect(pause().getAttribute("aria-label")).toBe(
      "Mettre en pause l'écriture de ce cours",
    );
    await study.readyLevel("c1", 0);
    await waitFor(() => expect(statusText()).toBe("1 palier prêt sur 4"));
    expect(hint()).toBe(
      "Le premier palier est prêt : vous pouvez commencer. Les suivants sont écrits pendant que vous étudiez.",
    );
    act(() => setLanguage("es"));
    expect(plan.textContent).toContain("Unos 20 min de estudio");
    expect(statusText()).toBe("1 nivel listo de 4");
    expect(hint()).toBe(
      "El primer nivel está listo: ya puedes empezar. Los siguientes se escriben mientras estudias.",
    );
    act(() => setLanguage("en"));
  });
});
