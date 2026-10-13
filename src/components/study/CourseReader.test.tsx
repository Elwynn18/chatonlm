// Lecteur de cours face au simulateur : cartes d'un palier (accroche du
// tuteur, sections, exemple résolu révélé pas à pas, « À retenir » révélés
// un à un), questions des cinq types (clic, chiffres, Entrée, champ dans la
// phrase, flèches, listes), retours (juste, faux, orthographe), essai manqué
// → « Explain differently » → nouvel essai → « Continue anyway », palier
// réussi (étoiles, confettis, mouvement réduit), série, parcours des
// paliers, attente et échec d'un palier, bilan et fin (fiche, Copy, Save,
// Go further), flux du tuteur (« I don't understand », « Ask a question »,
// Stop, Échap), menu (Report a mistake, Save notes, Delete), position et
// temps enregistrés, clavier et focus ; textes traduits (pseudo-langue,
// français, espagnol).

import {
  act,
  configure,
  createEvent,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnswerValue } from "@/bindings/AnswerValue";
import type { Course } from "@/bindings/Course";
import { resetStudyStreamsForTests } from "@/hooks/useStudy";
import { setLanguage, tNow } from "@/i18n";
import { solutionSteps, variantOf } from "@/lib/study";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import type { FakeStudy, SeedOptions } from "@/test/study";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

beforeEach(() => resetStudyStreamsForTests());
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/**
 * Cours « c1 » commencé (10 min : deux paliers, bilan de trois questions),
 * préparé par `before` (réponses, position) avant l'ouverture de sa page.
 */
function setup(
  options: SeedOptions = {},
  before: (study: FakeStudy, course: Course) => Promise<void> | void = () => {},
  prepare: (backend: FakeBackend) => void = () => {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  const study = backend.study;
  const course = study.seed({ id: "c1", minutes: 10, ...options });
  return {
    simulator,
    backend,
    study,
    course,
    async open() {
      await before(study, course);
      const app = renderApp("/study/c1");
      await screen.findByTestId("study-path");
      return app;
    },
  };
}

/** Réponses d'un essai (les `right` premières justes), sans la page. */
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

const heading = () =>
  screen.getByRole("heading", { level: 3 }).textContent ?? "";
const button = (name: string | RegExp) =>
  screen.getByRole<HTMLButtonElement>("button", { name });
const queryButton = (name: string | RegExp) =>
  screen.queryByRole<HTMLButtonElement>("button", { name });
const positions = (simulator: ReturnType<typeof installSimulator>) =>
  simulator.callsTo("study_progress").map((args) => args.position);
const options = () => screen.getAllByTestId("study-option");
const feedback = () => screen.getByTestId("study-feedback");

/** Tous les textes d'un cours (contenus donnés par l'essai). */
function contentOf(value: unknown, out: string[] = []): string[] {
  if (typeof value === "string") {
    if (value) out.push(value);
  } else if (Array.isArray(value)) {
    for (const item of value) contentOf(item, out);
  } else if (value && typeof value === "object") {
    for (const item of Object.values(value)) contentOf(item, out);
  }
  return out;
}

describe("Lesson cards", () => {
  it("opens at the intro, then each card of the level with Continue or Enter; the title takes the focus", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup();
    await open();
    expect(screen.getByTestId("study-intro")).toBeTruthy();
    expect(heading()).toBe("Course on Photosynthesis");
    // Une durée d'étude, pas une attente.
    expect(screen.getByTestId("study-intro").textContent).toContain(
      "About 7 minutes of study to go.",
    );
    await user.click(button("Let's start"));
    expect(heading()).toBe("Heading 1.1");
    expect(document.activeElement).toBe(
      screen.getByRole("heading", { level: 3 }),
    );
    // L'accroche du tuteur, en bulle, en tête de la première section.
    const hook = screen.getByTestId("study-hook");
    expect(hook.dataset.speaker).toBe("tutor");
    expect(hook.textContent).toBe("Did you know this about level 1?");
    expect(screen.getByTestId("study-section").textContent).toContain(
      "Level 1 · Level title 1",
    );
    expect(screen.getByTestId("study-section").textContent).toContain(
      "Part 1 of 3",
    );
    await user.keyboard("{Enter}");
    expect(heading()).toBe("Heading 1.2");
    expect(screen.queryByTestId("study-hook")).toBeNull();
    await user.keyboard("{Enter}");
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("study-example")).toBeTruthy();
    expect(positions(simulator)).toEqual([
      "intro",
      "L0:s0",
      "L0:s1",
      "L0:s2",
      "L0:example",
    ]);
  });

  it("worked example: the problem first, then the solution step by step, or all at once", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, (_, course) => {
      course.position = "L0:example";
    });
    await open();
    const card = screen.getByTestId("study-example");
    expect(card.textContent).toContain("Try it yourself first");
    expect(card.textContent).toContain("Problem of level 1.");
    expect(screen.queryByTestId("study-solution")).toBeNull();
    await user.click(button("Show the solution"));
    expect(
      screen.getAllByTestId("study-solution-step").map((s) => s.textContent),
    ).toEqual(["First step 1."]);
    expect(screen.getByTestId("study-solution").textContent).toContain(
      "Step 1 of 2",
    );
    // Entrée : l'étape suivante, puis la carte suivante.
    await user.keyboard("{Enter}");
    expect(screen.getAllByTestId("study-solution-step")).toHaveLength(2);
    expect(queryButton("Show the next step")).toBeNull();
    expect(queryButton("Show everything")).toBeNull();
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("study-recap")).toBeTruthy();
  });

  it("worked example: Show everything reveals the whole solution", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, (_, course) => {
      course.position = "L0:example";
    });
    await open();
    await user.click(button("Show the solution"));
    await user.click(button("Show everything"));
    expect(screen.getAllByTestId("study-solution-step")).toHaveLength(2);
  });

  it("key points are revealed one by one (click or Enter), then the common mistake", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, (_, course) => {
      course.position = "L0:recap";
    });
    await open();
    expect(
      screen.getAllByTestId("study-key-point").map((p) => p.textContent),
    ).toEqual(["1Key point 1a"]);
    expect(screen.queryByTestId("study-pitfall")).toBeNull();
    await user.keyboard("{Enter}");
    expect(screen.getAllByTestId("study-key-point")).toHaveLength(2);
    expect(screen.getByTestId("study-pitfall").textContent).toBe(
      "Common mistakePitfall of level 1.",
    );
    await user.click(button("Continue"));
    expect(screen.getByText("Practice — not counted")).toBeTruthy();
  });

  it("Enter on a focused button only activates that button", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    await open();
    button("Continue").focus();
    await user.keyboard("{Enter}");
    expect(heading()).toBe("Heading 1.2");
    expect(positions(simulator).slice(-1)).toEqual(["L0:s1"]);
  });

  it("a held Enter moves one card only", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    await open();
    // Un appui, trois répétitions de la touche tenue.
    await user.keyboard("{Enter>4}");
    expect(heading()).toBe("Heading 1.2");
    expect(positions(simulator)).toEqual(["L0:s0", "L0:s1"]);
  });

  it("a held Enter never presses the button that just took the focus", () => {
    // Répétition sur « Continue » (focalisé après « Check ») : le navigateur
    // l'activerait ; la page l'en empêche.
    const { open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    return open().then(() => {
      const focused = button("Continue");
      focused.focus();
      const repeat = createEvent.keyDown(focused, {
        key: "Enter",
        repeat: true,
      });
      fireEvent(focused, repeat);
      expect(repeat.defaultPrevented).toBe(true);
      const first = createEvent.keyDown(focused, { key: "Enter" });
      fireEvent(focused, first);
      expect(first.defaultPrevented).toBe(false);
    });
  });

  it("Enter on a button other than the card's main action activates that button only", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 0);
      course.position = "L0:a0:result";
    });
    await open();
    // Entrée hors d'un bouton : « Explain differently » ; sur « Review the
    // lesson » : ce bouton seulement.
    button("Review the lesson").focus();
    await user.keyboard("{Enter}");
    expect(heading()).toBe("Heading 1.1");
    expect(simulator.callsTo("study_remedy")).toEqual([]);
  });
});

describe("Questions", () => {
  it("multiple choice: a digit chooses, Enter checks; right answer praised, then read only", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:check";
    });
    await open();
    expect(screen.getByRole("radiogroup").getAttribute("aria-labelledby")).toBe(
      screen.getByRole("heading", { level: 3 }).firstElementChild?.id,
    );
    expect(button("Check").disabled).toBe(true);
    await user.keyboard("2");
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: /Right answer/ })
        .checked,
    ).toBe(true);
    await user.keyboard("{Enter}");
    expect(simulator.callsTo("study_answer")).toEqual([
      {
        courseId: "c1",
        questionId: "L0-check",
        answer: { type: "choice", index: 1 },
      },
    ]);
    await screen.findByTestId("study-feedback");
    expect(feedback().dataset.correct).toBe("true");
    expect(feedback().textContent).toMatch(
      /^Correct!(Great job\.|Exactly\.|Well done\.|Spot on\.|Nice work\.|Perfect\.)Because of L0-check\.$/,
    );
    // Retour annoncé, option attendue marquée (icône et texte).
    expect(
      screen.getByTestId("study-feedback-region").getAttribute("aria-live"),
    ).toBe("polite");
    expect(options()[1].dataset.state).toBe("expected");
    expect(options()[1].textContent).toContain("(right answer)");
    expect(
      screen
        .getAllByRole<HTMLInputElement>("radio")
        .every((radio) => radio.disabled),
    ).toBe(true);
    expect(document.activeElement).toBe(button("Continue"));
    await user.keyboard("{Enter}");
    expect(screen.getByText("Checkpoint · 1 of 3")).toBeTruthy();
  });

  it("the same question always gets the same encouragement", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, (_, course) => {
      course.position = "L0:check";
    });
    await open();
    await user.click(options()[1]);
    await user.click(button("Check"));
    const first = (await screen.findByTestId("study-feedback")).textContent;
    // Variante tirée de l'identifiant de la question (ordre du catalogue).
    const praise = [
      "Great job.",
      "Exactly.",
      "Well done.",
      "Spot on.",
      "Nice work.",
      "Perfect.",
    ][variantOf("L0-check", 6)];
    expect(first).toBe(`Correct!${praise}Because of L0-check.`);
    await user.click(button("Continue"));
    await user.click(
      screen.getByRole("button", { name: /^Level 1: Level title 1/ }),
    );
    // Retour à la question (lecture seule) : même texte.
    for (let i = 0; i < 5; i += 1) await user.click(button("Continue"));
    expect(screen.getByTestId("study-feedback").textContent).toBe(first);
  });

  it("wrong choice: a shake, the right answer and the explanation", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, (_, course) => {
      course.position = "L0:check";
    });
    await open();
    await user.click(screen.getByText("Wrong first"));
    await user.click(button("Check"));
    await screen.findByTestId("study-feedback");
    expect(feedback().dataset.correct).toBe("false");
    expect(feedback().textContent).toContain("Not quite.");
    expect(feedback().textContent).toContain("Right answer: Right answer");
    expect(feedback().textContent).toContain("Because of L0-check.");
    expect(options().map((option) => option.dataset.state)).toEqual([
      "wrong",
      "expected",
      undefined,
    ]);
    expect(options()[0].textContent).toContain("(your answer)");
    expect(
      screen
        .getByTestId("study-question")
        .querySelector("[data-question-input]")?.className,
    ).toContain("motion-safe:animate-study-shake");
  });

  it("true or false: 1 is True, 2 is False", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:a0:q1";
    });
    await open();
    expect(screen.getByText("Checkpoint · 2 of 3")).toBeTruthy();
    await user.keyboard("2");
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: /False/ }).checked,
    ).toBe(true);
    await user.keyboard("1{Enter}");
    expect(simulator.callsTo("study_answer")[0].answer).toEqual({
      type: "trueFalse",
      value: true,
    });
    await screen.findByTestId("study-feedback");
    expect(feedback().dataset.correct).toBe("true");
  });

  it("fill in the blank in a language course: wrong but close shows the spelling once", async () => {
    // Cours de langue : un accent manquant est faux, mais `close` (moteur,
    // correction stricte de la sorte `language`) ; la réponse attendue
    // n'est montrée qu'une fois, dans la remarque sur l'orthographe.
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:a0:q2";
    });
    await open();
    simulator.handle("study_answer", (args) => ({
      given: (args as { answer: AnswerValue }).answer,
      correct: false,
      close: true,
      expected: { type: "blank", text: "rïght" },
      detail: [],
      explanation: "Because of L0-A0-Q2.",
      at: 1,
    }));
    await user.click(
      screen.getByRole<HTMLInputElement>("textbox", { name: "Your answer" }),
    );
    await user.keyboard("right{Enter}");
    await screen.findByTestId("study-feedback");
    expect(feedback().dataset.correct).toBe("false");
    expect(feedback().textContent).toBe(
      "Not quite.Almost — watch the spelling: rïghtBecause of L0-A0-Q2.",
    );
  });

  it("fill in the blank: the field in the sentence, Enter checks, a typo is accepted with the spelling", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:a0:q2";
    });
    await open();
    const field = screen.getByRole<HTMLInputElement>("textbox", {
      name: "Your answer",
    });
    // Dans la phrase : le texte avant et après autour du champ.
    expect(field.parentElement?.textContent).toBe(
      "The answer of L0-A0-Q2 is  here.",
    );
    await user.click(field);
    // Un chiffre va au champ, pas à une option.
    await user.keyboard("rihgt{Enter}");
    expect(simulator.callsTo("study_answer")[0].answer).toEqual({
      type: "blank",
      text: "rihgt",
    });
    await screen.findByTestId("study-feedback");
    // Acceptée à une faute près : pas d'éloge (« Perfect. »), la remarque.
    expect(feedback().textContent).toBe(
      "Correct!Almost — watch the spelling: rightBecause of L0-A0-Q2.",
    );
    expect(field.readOnly).toBe(true);
    // Entrée dans le champ : Continue (« See my result »).
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("study-result")).toBeTruthy();
  });

  it("fill in the blank: Enter held in the field checks once and keeps the feedback", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:a0:q2";
    });
    await open();
    await user.click(screen.getByRole("textbox", { name: "Your answer" }));
    await user.keyboard("right{Enter>4}");
    await screen.findByTestId("study-feedback");
    expect(simulator.callsTo("study_answer")).toHaveLength(1);
    // Le retour reste montré : la touche tenue n'a pas continué.
    expect(screen.getByTestId("study-question")).toBeTruthy();
    expect(screen.queryByTestId("study-result")).toBeNull();
  });

  it("ordering: arrows move the items, then each place is marked right or wrong", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, async (study, course) => {
      await answerAttempt(study, course, 0, 0, 3);
      course.position = "L1:a0:q2";
    });
    await open();
    const items = () =>
      screen
        .getAllByTestId("study-order-item")
        .map((item) => item.textContent?.replace(/^\d/, ""));
    expect(items()).toEqual(["Step two", "Step one", "Step three"]);
    expect(button("Move “Step two” up").disabled).toBe(true);
    await user.click(button("Move “Step two” down"));
    expect(items()).toEqual(["Step one", "Step two", "Step three"]);
    // Au clavier : le bouton garde le focus et peut resservir.
    button("Move “Step three” up").focus();
    await user.keyboard("{Enter}");
    expect(items()).toEqual(["Step one", "Step three", "Step two"]);
    await user.click(button("Check"));
    expect(simulator.callsTo("study_answer")[0].answer).toEqual({
      type: "order",
      order: [1, 2, 0],
    });
    await screen.findByTestId("study-feedback");
    expect(
      screen.getAllByTestId("study-order-item").map((item) => item.textContent),
    ).toEqual(["1Step oneright", "2Step threewrong", "3Step twowrong"]);
    expect(feedback().textContent).toContain(
      "Right order:Step oneStep twoStep three",
    );
  });

  it("matching: a list per item, Check once all are chosen", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup(
      { minutes: 20 },
      async (study, course) => {
        await answerAttempt(study, course, 0, 0, 3);
        await answerAttempt(study, course, 1, 0, 3);
        course.position = "L2:a0:q2";
      },
    );
    await open();
    const rows = screen.getAllByTestId("study-match-row");
    expect(rows).toHaveLength(3);
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Cat" }),
      "Meow",
    );
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Dog" }),
      "Woof",
    );
    expect(button("Check").disabled).toBe(true);
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Cow" }),
      "Woof",
    );
    await user.click(button("Check"));
    expect(simulator.callsTo("study_answer")[0].answer).toEqual({
      type: "match",
      pairs: [2, 0, 0],
    });
    await screen.findByTestId("study-feedback");
    expect(rows.map((row) => row.textContent?.endsWith("right"))).toEqual([
      true,
      true,
      false,
    ]);
    expect(feedback().textContent).toContain("Cow → Moo");
  });

  it("a blank inside a formula: the whole prompt as the title (formula rendered), the field alone below", async () => {
    const { open } = setup({}, (_, course) => {
      const question = course.levels[0].attempts[0].questions[2];
      if (question.body.type !== "blank") throw new Error("not a blank");
      question.body.prompt = String.raw`Complete: \(\frac{1}{4} + \frac{2}{4} = \frac{___}{4}\).`;
      course.position = "L0:a0:q2";
    });
    await open();
    const card = screen.getByTestId("study-question");
    const title = screen.getByRole("heading", { level: 3 });
    // Jamais coupé dans la formule : rendue par KaTeX (le trou en blanc
    // souligné), pas en LaTeX brut ni en erreur.
    expect(title.querySelector(".katex")).not.toBeNull();
    expect(card.querySelector(".katex-error")).toBeNull();
    // Hors des formules rendues (leur source reste dans le MathML).
    const outside = card.cloneNode(true) as HTMLElement;
    for (const math of outside.querySelectorAll(".katex")) math.remove();
    expect(outside.textContent).not.toContain(String.raw`\frac`);
    // Énoncé montré une seule fois (en titre), le champ sous lui.
    expect(card.querySelectorAll(".katex")).toHaveLength(1);
    const field = screen.getByRole("textbox", { name: "Your answer" });
    expect(title.contains(field)).toBe(false);
  });

  it("a blank that cannot be cut is shown once, not repeated above the field", async () => {
    const { open } = setup({}, (_, course) => {
      const question = course.levels[0].attempts[0].questions[2];
      if (question.body.type !== "blank") throw new Error("not a blank");
      question.body.prompt = "It costs $5, so the total is ___ dollars.";
      course.position = "L0:a0:q2";
    });
    await open();
    const text = screen.getByTestId("study-question").textContent ?? "";
    expect(text.split("It costs").length - 1).toBe(1);
    expect(heading()).toBe("It costs $5, so the total is ___ dollars.");
    expect(screen.getByRole("textbox", { name: "Your answer" })).toBeTruthy();
  });

  it("Markdown and formulas in ordering and matching items: plain text in the lists and names, rendered in the feedback", async () => {
    const user = userEvent.setup();
    const { open } = setup({ minutes: 20 }, async (study, course) => {
      await answerAttempt(study, course, 0, 0, 3);
      await answerAttempt(study, course, 1, 0, 3);
      const match = course.levels[2].attempts[0].questions[2];
      if (match.body.type !== "match") throw new Error("not a match");
      match.body.right = ["**Woof**", String.raw`\(M_2\)`, "`Meow`"];
      course.position = "L2:a0:q2";
    });
    await open();
    // Listes déroulantes natives : texte seul, formule lue (« M₂ »).
    const cat = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Cat",
    });
    expect([...cat.options].map((option) => option.textContent)).toEqual([
      "Choose…",
      "Woof",
      "M₂",
      "Meow",
    ]);
    // Toutes fausses : les paires attendues rendues en Markdown.
    await user.selectOptions(cat, "Woof");
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Dog" }),
      "Woof",
    );
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Cow" }),
      "Woof",
    );
    await user.click(button("Check"));
    await screen.findByTestId("study-feedback");
    const pairs = within(feedback()).getAllByRole("listitem");
    expect(pairs.map((pair) => pair.textContent?.split(" → ")[0])).toEqual([
      "Cat",
      "Dog",
      "Cow",
    ]);
    expect(
      pairs[1].querySelector("[data-streamdown=strong]")?.textContent,
    ).toBe("Woof");
    expect(pairs[2].querySelector(".katex")).not.toBeNull();
    expect(pairs[0].querySelector("code")?.textContent).toBe("Meow");
    expect(feedback().textContent).not.toMatch(/\*\*|\\\(/);
  });

  it("an ordering item in Markdown is named in plain text by its arrows", async () => {
    const { open } = setup({}, async (study, course) => {
      await answerAttempt(study, course, 0, 0, 3);
      const order = course.levels[1].attempts[0].questions[2];
      if (order.body.type !== "order") throw new Error("not an order");
      order.body.items = ["Step **two**", String.raw`Step \(1\)`, "Step three"];
      course.position = "L1:a0:q2";
    });
    await open();
    expect(button("Move “Step two” down")).toBeTruthy();
    expect(button("Move “Step 1” up")).toBeTruthy();
  });

  it("a refused answer is shown", async () => {
    const user = userEvent.setup();
    const { study, open } = setup({}, (_, course) => {
      course.position = "L0:check";
    });
    study.failures.set("study_answer", {
      kind: "invalid",
      message: "This level is locked",
    });
    await open();
    await user.click(options()[0]);
    await user.click(button("Check"));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "This level is locked",
    );
  });
});

describe("Attempts", () => {
  it("a failed attempt explains the mistakes; Explain differently prepares a new attempt", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup(
      {},
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 1);
        course.position = "L0:a0:result";
      },
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    expect(heading()).toBe("Not passed yet");
    expect(screen.getByTestId("study-score").textContent).toBe("1 / 3");
    expect(screen.getByTestId("study-result").textContent).toContain(
      "You need 2 right answers to pass this level.",
    );
    const missed = within(screen.getByTestId("study-missed")).getAllByRole(
      "listitem",
    );
    expect(missed.map((item) => item.textContent)).toEqual([
      "Statement L0-A0-Q1 holds.Your answer: FalseRight answer: TrueBecause of L0-A0-Q1.",
      "The answer of L0-A0-Q2 is ___ here.Your answer: nothing like itRight answer: rightBecause of L0-A0-Q2.",
    ]);
    expect(queryButton("Continue anyway")).toBeNull();
    expect(queryButton("Next level")).toBeNull();
    await user.click(button("Explain differently"));
    expect(simulator.callsTo("study_remedy")).toEqual([
      { courseId: "c1", level: 0 },
    ]);
    // En file dans le moteur (« pending ») : déjà une attente, dans la carte
    // et dans le parcours.
    expect(study.course("c1").levels[0].remedy).toBe("pending");
    await screen.findByText("Preparing a new explanation…");
    expect(queryButton("Explain differently")).toBeNull();
    expect(screen.getByTestId("study-path").textContent).toContain(
      "Being written…",
    );
    await study.finishRemedy("c1", 0);
    await user.click(
      await screen.findByRole("button", { name: "See the new explanation" }),
    );
    expect(heading()).toBe("Another way to see it");
    const bubble = screen.getByTestId("study-remedy-text");
    expect(bubble.textContent).toBe("Another way to see level 1.");
    expect(bubble.closest("[data-speaker]")?.getAttribute("data-speaker")).toBe(
      "tutor",
    );
    await user.click(button("Try new questions"));
    expect(screen.getByText("Checkpoint · 1 of 3")).toBeTruthy();
  });

  it("a new attempt that failed to be written: the error and Retry", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup(
      {},
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 0);
        course.position = "L0:a0:result";
      },
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    await user.click(button("Explain differently"));
    await screen.findByText("Preparing a new explanation…");
    await study.failRemedy("c1", 0, {
      code: "invalid",
      message: "The new questions were not usable.",
    });
    const alert = await screen.findByTestId("study-error");
    expect(alert.textContent).toContain("The new questions were not usable.");
    expect(queryButton("Explain differently")).toBeNull();
    await user.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(simulator.callsTo("study_retry")).toEqual([
      { courseId: "c1", withoutWeb: false },
    ]);
  });

  it("Review the lesson goes back to the first section", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 0);
      course.position = "L0:a0:result";
    });
    await open();
    await user.click(button("Review the lesson"));
    expect(heading()).toBe("Heading 1.1");
  });

  it("after two failed attempts, Continue anyway marks the level to review and opens the next one", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 0);
      await s.finishRemedy(course.id, 0);
      await answerAttempt(s, course, 0, 1, 1);
      course.position = "L0:a1:result";
    });
    await open();
    expect(screen.getByTestId("study-result").textContent).toContain(
      "“Continue anyway” moves on and marks this level to review.",
    );
    await user.click(button("Continue anyway"));
    expect(simulator.callsTo("study_skip")).toEqual([
      { courseId: "c1", level: 0 },
    ]);
    await waitFor(() => expect(heading()).toBe("Heading 2.1"));
    const node = screen.getAllByTestId("study-path-node")[0];
    expect(node.dataset.status).toBe("review");
    expect(node.getAttribute("aria-label")).toBe(
      "Level 1: Level title 1, to review, 0 stars out of 3",
    );
  });

  it("Enter twice on a failed result asks for one new attempt only", async () => {
    const { simulator, open } = setup(
      {},
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 0);
        course.position = "L0:a0:result";
      },
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    // Deux appuis avant toute réponse du moteur.
    fireEvent.keyDown(document.body, { key: "Enter" });
    fireEvent.keyDown(document.body, { key: "Enter" });
    await screen.findByText("Preparing a new explanation…");
    expect(simulator.callsTo("study_remedy")).toHaveLength(1);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("Explain differently: the wait shows as soon as the engine takes the request, before the course is read again", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup(
      {},
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 0);
        course.position = "L0:a0:result";
      },
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    // Relectures du cours retenues jusqu'à la fin de l'essai.
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    simulator.handle("study_get", async (args) => {
      await held;
      return study.course(args.courseId as string);
    });
    await user.click(button("Explain differently"));
    await screen.findByText("Preparing a new explanation…");
    expect(queryButton("Explain differently")).toBeNull();
    await user.keyboard("{Enter}");
    expect(simulator.callsTo("study_remedy")).toHaveLength(1);
    expect(screen.queryByRole("alert")).toBeNull();
    release();
  });

  it("Continue anyway opens the next level directly, never the intro in between", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 0);
      await s.finishRemedy(course.id, 0);
      await answerAttempt(s, course, 0, 1, 1);
      course.position = "L0:a1:result";
    });
    await open();
    // Titres de carte montrés, dans l'ordre.
    const seen: string[] = [];
    const observer = new MutationObserver(() => {
      const title = document.querySelector("[data-card-heading]")?.textContent;
      if (title && seen.at(-1) !== title) seen.push(title);
    });
    observer.observe(document.body, {
      subtree: true,
      childList: true,
      characterData: true,
    });
    await user.click(button("Continue anyway"));
    await waitFor(() => expect(heading()).toBe("Heading 2.1"));
    observer.disconnect();
    expect(seen).not.toContain("Course on Photosynthesis");
    expect(seen.at(-1)).toBe("Heading 2.1");
  });

  it("a passed level celebrates: three stars one after the other, confetti, Next level", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 3);
      course.position = "L0:a0:result";
    });
    await open();
    expect(heading()).toBe("Level complete!");
    const stars = screen
      .getByTestId("study-result")
      .querySelector("[data-stars]") as HTMLElement;
    expect(stars.getAttribute("aria-label")).toBe("3 stars out of 3");
    const delays = [...stars.querySelectorAll("svg")].map(
      (star) => (star as SVGElement).style.animationDelay,
    );
    expect(delays).toEqual(["200ms", "450ms", "700ms"]);
    expect(screen.getByTestId("study-confetti")).toBeTruthy();
    await user.click(button("Next level"));
    expect(heading()).toBe("Heading 2.1");
  });

  it("no confetti under reduced motion; the stars stay", async () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query.includes("prefers-reduced-motion"),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 2);
      course.position = "L0:a0:result";
    });
    await open();
    expect(screen.queryByTestId("study-confetti")).toBeNull();
    expect(
      screen
        .getByTestId("study-result")
        .querySelector("[data-stars]")
        ?.getAttribute("aria-label"),
    ).toBe("2 stars out of 3");
  });

  it("the streak flame shows three right answers in a row, and goes out on a mistake", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 3);
      course.position = "L1:check";
    });
    await open();
    expect(screen.getByTestId("study-streak").textContent).toBe(
      "33 right answers in a row",
    );
    await user.click(screen.getByText("Wrong first"));
    await user.click(button("Check"));
    await waitFor(() =>
      expect(screen.queryByTestId("study-streak")).toBeNull(),
    );
  });
});

describe("Level path and progress", () => {
  it("shows each level's status and stars; a click opens an open level, never a locked one", async () => {
    const user = userEvent.setup();
    const { open } = setup({ minutes: 20 }, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 2);
      course.position = "L1:s1";
    });
    await open();
    const nodes = screen.getAllByTestId("study-path-node");
    expect(nodes.map((node) => node.dataset.status)).toEqual([
      "passed",
      "current",
      "locked",
      "locked",
    ]);
    expect(nodes[0].getAttribute("aria-label")).toBe(
      "Level 1: Level title 1, passed, 2 stars out of 3",
    );
    expect(nodes[1].getAttribute("aria-current")).toBe("step");
    expect(nodes[1].className).toContain("motion-safe:animate-study-pulse");
    expect(nodes[2]).toHaveProperty("disabled", true);
    expect(screen.getByTestId("study-path-final")).toHaveProperty(
      "disabled",
      true,
    );
    await user.click(nodes[0]);
    expect(heading()).toBe("Heading 1.1");
    expect(nodes[0].getAttribute("aria-current")).toBe("step");
    const bar = screen.getByRole("progressbar", { name: "Course progress" });
    expect(bar.getAttribute("aria-valuetext")).toBe("10 of 45 steps");
    expect(screen.getByText("About 11 min of study left")).toBeTruthy();
  });

  it("a level not written yet: said so, then its first card; a failed one: the error and Retry", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup(
      { readyLevels: 1 },
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 3);
        course.position = "L1:s0";
      },
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    expect(screen.getByTestId("study-level-wait").textContent).toContain(
      "This level isn't written yet.",
    );
    act(() => setLanguage("fr"));
    expect(screen.getByTestId("study-level-wait").textContent).toContain(
      "Ce palier n'est pas encore écrit.",
    );
    act(() => setLanguage("en"));
    await study.failLevel("c1", 1, {
      code: "failed",
      message: "The level could not be written.",
    });
    const alert = await screen.findByTestId("study-error");
    expect(alert.textContent).toContain("The level could not be written.");
    await user.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(simulator.callsTo("study_retry")).toEqual([
      { courseId: "c1", withoutWeb: false },
    ]);
    await study.readyLevel("c1", 1);
    await waitFor(() => expect(heading()).toBe("Heading 2.1"));
    // La carte prête remplace l'attente : son titre prend le focus.
    expect(document.activeElement).toBe(
      screen.getByRole("heading", { level: 3 }),
    );
  });

  it("marks a level being written in the path", async () => {
    const { study, open } = setup(
      { readyLevels: 1 },
      () => {},
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    expect(screen.getByTestId("study-path").textContent).not.toContain(
      "Being written…",
    );
    await study.update("c1", (course) => {
      course.levels[1].state = "generating";
    });
    await waitFor(() =>
      expect(screen.getByTestId("study-path").textContent).toContain(
        "Being written…",
      ),
    );
    act(() => setLanguage("es"));
    expect(screen.getByTestId("study-path").textContent).toContain(
      "En preparación…",
    );
  });

  it("saves the position on every card and every 15 s, and resumes there", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const { simulator, open } = setup({}, (_, course) => {
      course.position = "L0:recap";
      course.elapsedMs = 120_000;
    });
    await open();
    expect(screen.getByTestId("study-recap")).toBeTruthy();
    const calls = () => simulator.callsTo("study_progress");
    expect(calls()).toEqual([
      { courseId: "c1", position: "L0:recap", elapsedMs: expect.any(Number) },
    ]);
    expect(calls()[0].elapsedMs).toBeGreaterThanOrEqual(120_000);
    act(() => vi.advanceTimersByTime(15_000));
    expect(calls()).toHaveLength(2);
    act(() => vi.advanceTimersByTime(15_000));
    expect(calls()).toHaveLength(3);
  });
});

describe("Position kept", () => {
  it("leaving the course and coming back keeps the position and the time spent", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup({}, (_, course) => {
      course.elapsedMs = 60_000;
    });
    const app = await open();
    await user.click(button("Let's start"));
    await user.keyboard("{Enter}{Enter}");
    expect(heading()).toBe("Heading 1.3");
    await act(() => app.router.navigate({ to: "/study" }));
    await screen.findByTestId("study-home");
    expect(simulator.callsTo("study_progress").at(-1)).toMatchObject({
      position: "L0:s2",
    });
    // Temps enregistré entre-temps (dans la base, pas dans le cache).
    study.course("c1").elapsedMs = 240_000;
    const before = simulator.callsTo("study_progress").length;
    await act(() =>
      app.router.navigate({
        to: "/study/$courseId",
        params: { courseId: "c1" },
      }),
    );
    await waitFor(() => expect(heading()).toBe("Heading 1.3"));
    const back = simulator.callsTo("study_progress").slice(before);
    expect(back[0]).toMatchObject({ courseId: "c1", position: "L0:s2" });
    expect(back[0].elapsedMs).toBeGreaterThanOrEqual(240_000);
  });
});

describe("Final review and end", () => {
  it("the final review, then the end: score, stars, time, key points, missed questions, Go further", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const { simulator, open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 3);
      await answerAttempt(s, course, 1, 0, 3);
      course.position = "final:q0";
    });
    const app = await open();
    expect(screen.getByText("Final review · 1 of 3")).toBeTruthy();
    expect(
      screen.getByTestId("study-path-final").getAttribute("aria-current"),
    ).toBe("step");
    await user.keyboard("2{Enter}");
    await screen.findByTestId("study-feedback");
    await user.keyboard("{Enter}");
    // Vrai ou faux : faux, une erreur.
    await user.keyboard("2{Enter}");
    await screen.findByTestId("study-feedback");
    await user.keyboard("{Enter}");
    await user.type(
      screen.getByRole("textbox", { name: "Your answer" }),
      "right{Enter}",
    );
    await screen.findByTestId("study-feedback");
    // Fin : une fois le cours relu (fini à la dernière réponse).
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    const end = await screen.findByTestId("study-end");
    expect(heading()).toBe("Course completed!");
    expect(screen.getByTestId("study-final-score").textContent).toBe("2 / 3");
    // Une phrase : le score en grand, puis sa légende accordée.
    expect(
      screen.getByTestId("study-final-score").parentElement?.textContent,
    ).toBe("2 / 3 right answers in the final review");
    expect(screen.getByTestId("study-total-stars").textContent).toBe(
      "6 stars out of 6",
    );
    expect(end.textContent).toContain("1 minute spent");
    expect(end.textContent).toContain("Key point 1a");
    expect(end.textContent).toContain("Key point 2b");
    expect(
      within(screen.getByTestId("study-missed")).getAllByRole("listitem"),
    ).toHaveLength(1);
    expect(screen.getByTestId("study-confetti")).toBeTruthy();
    expect(screen.getByText("Completed")).toBeTruthy();

    await user.click(await screen.findByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(
      "# Course on Photosynthesis\n\n- Key point 1a\n- Key point 1b\n- Key point 2a\n- Key point 2b\n",
    );
    await user.click(button("Save as Markdown"));
    expect(simulator.callsTo("study_save_notes")).toEqual([{ courseId: "c1" }]);
    await screen.findByText("Notes saved");

    const further = screen.getAllByTestId("study-further");
    expect(further.map((link) => link.textContent)).toEqual([
      "Next topic A",
      "Next topic B",
      "Next topic C",
    ]);
    await user.click(further[0]);
    const topic = await screen.findByRole<HTMLTextAreaElement>("textbox", {
      name: "What do you want to learn?",
    });
    expect(topic.value).toBe("Next topic A");
    expect(app.router.state.location.search).toEqual({
      topic: "Next topic A",
      level: "beginner",
      minutes: 10,
    });
  });

  it("the final score is one sentence, singular or plural in each language", async () => {
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 3);
      await answerAttempt(s, course, 1, 0, 3);
      const final = course.finalQuiz!.questions;
      for (const [n, question] of final.entries()) {
        await s.answer(course.id, question.id, n === 0);
      }
      course.position = "end";
    });
    await open();
    const sentence = () =>
      screen.getByTestId("study-final-score").parentElement?.textContent;
    expect(sentence()).toBe("1 / 3 right answer in the final review");
    act(() => setLanguage("fr"));
    expect(sentence()).toBe("1 / 3 bonne réponse au bilan final");
    act(() => setLanguage("es"));
    expect(sentence()).toBe("1 / 3 respuesta correcta en el repaso final");
  });

  it("the final review not written yet: said so", async () => {
    const { open } = setup(
      { finalReady: false },
      async (s, course) => {
        await answerAttempt(s, course, 0, 0, 3);
        await answerAttempt(s, course, 1, 0, 3);
        course.position = "final:q0";
      },
      (backend) => {
        backend.study.auto = false;
      },
    );
    await open();
    expect(screen.getByTestId("study-final-wait").textContent).toContain(
      "The final review isn't written yet.",
    );
  });
});

describe("Tutor", () => {
  it("I don't understand: a simpler explanation streamed in a tutor bubble, kept once done", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup({}, (_, course) => {
      course.position = "L0:s1";
    });
    await open();
    await user.click(button("I don't understand"));
    expect(simulator.callsTo("study_explain")).toEqual([
      {
        courseId: "c1",
        level: 0,
        section: 1,
        streamId: expect.any(String),
        onEvent: expect.stringMatching(/^__CHANNEL__:/),
      },
    ]);
    const stream = study.lastStream();
    await stream.step();
    const live = screen.getByTestId("study-explain-live");
    expect(live.dataset.speaker).toBe("tutor");
    expect(live.textContent).toContain("Simpler words");
    expect(button("Stop")).toBeTruthy();
    await stream.finish();
    await waitFor(() =>
      expect(screen.getByTestId("study-explain").textContent).toBe(
        "In simpler wordsSimpler words for section 2.",
      ),
    );
    expect(screen.queryByTestId("study-explain-live")).toBeNull();
    expect(button("I don't understand")).toBeTruthy();
  });

  it("the tutor's typing dots stay still under reduced motion (motion-safe only)", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    await open();
    await user.click(button("I don't understand"));
    const dots = [
      ...screen.getByTestId("study-typing").querySelectorAll("[aria-hidden]"),
    ] as HTMLElement[];
    expect(dots).toHaveLength(3);
    for (const dot of dots) {
      expect(dot.style.animationName).toBe("");
      expect(dot.style.animation).toBe("");
      expect(dot.className).toContain(
        "motion-safe:animate-[typing_1.4s_infinite]",
      );
    }
  });

  it("Escape stops an explanation: what came is shown, not kept", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    await open();
    await user.click(button("I don't understand"));
    const stream = study.lastStream();
    await stream.step();
    await user.keyboard("{Escape}");
    expect(simulator.callsTo("study_stream_cancel")).toEqual([
      { streamId: stream.streamId },
    ]);
    await stream.finish();
    const live = screen.getByTestId("study-explain-live");
    expect(live.textContent).toContain("Stopped.");
    expect(study.course("c1").levels[0].explains).toEqual([]);
  });

  it("Ask a question: a mini-chat (learner on the right, tutor on the left), Escape closes it", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup({}, (_, course) => {
      course.position = "L0:check";
    });
    await open();
    await user.click(button("Ask a question"));
    const panel = screen.getByTestId("study-ask");
    const field = within(panel).getByRole("textbox", { name: "Your question" });
    expect(document.activeElement).toBe(field);
    // Chiffres et Entrée vont à la question, pas aux options.
    await user.keyboard("Why 2?{Enter}");
    expect(
      screen
        .getAllByRole<HTMLInputElement>("radio")
        .some((radio) => radio.checked),
    ).toBe(false);
    expect(simulator.callsTo("study_answer")).toEqual([]);
    expect(simulator.callsTo("study_ask")).toEqual([
      {
        courseId: "c1",
        level: 0,
        question: "Why 2?",
        streamId: expect.any(String),
        onEvent: expect.stringMatching(/^__CHANNEL__:/),
      },
    ]);
    expect(
      within(panel)
        .getByText("Why 2?")
        .closest("[data-speaker]")
        ?.getAttribute("data-speaker"),
    ).toBe("learner");
    await study.lastStream().finish();
    await waitFor(() =>
      expect(within(panel).getByTestId("study-ask-answer").textContent).toBe(
        "Answer to: Why 2?",
      ),
    );
    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("study-ask")).toBeNull();
    expect(document.activeElement).toBe(button("Ask a question (1 asked)"));
  });

  it("Stop ends an answer being written", async () => {
    const user = userEvent.setup();
    const { study, open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    await open();
    await user.click(button("Ask a question"));
    await user.keyboard("What?{Enter}");
    const stream = study.lastStream();
    await stream.step();
    await user.click(
      within(screen.getByTestId("study-ask")).getByRole("button", {
        name: "Stop",
      }),
    );
    await stream.finish();
    expect(screen.getByTestId("study-ask-live").textContent).toContain(
      "Stopped.",
    );
    expect(study.course("c1").levels[0].asks).toEqual([]);
  });
});

describe("Course menu", () => {
  it("Report a mistake rewrites the level after confirmation", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup({}, async (s, course) => {
      await s.answer(course.id, "L0-check", true);
      course.position = "L0:check";
    });
    await open();
    await user.click(button("Course options"));
    await user.click(
      screen.getByRole("menuitem", { name: "Report a mistake in this level" }),
    );
    const dialog = screen.getByRole("alertdialog");
    expect(dialog.textContent).toContain(
      "This level will be written again, and your answers in it will be erased.",
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Rewrite the level" }),
    );
    expect(simulator.callsTo("study_regenerate")).toEqual([
      { courseId: "c1", level: 0 },
    ]);
    await waitFor(() => expect(heading()).toBe("Heading 1.1"));
  });

  it("Save notes, then Delete course after confirmation", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup();
    const app = await open();
    await user.click(button("Course options"));
    expect(
      screen.getAllByRole("menuitem").map((item) => item.textContent),
    ).toEqual(["Save notes", "Delete course"]);
    await user.click(screen.getByRole("menuitem", { name: "Save notes" }));
    expect(simulator.callsTo("study_save_notes")).toHaveLength(1);
    await screen.findByText("Notes saved");
    await user.click(button("Course options"));
    await user.click(screen.getByRole("menuitem", { name: "Delete course" }));
    await user.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(simulator.callsTo("study_delete")).toEqual([{ courseId: "c1" }]);
    await screen.findByTestId("study-home");
    expect(app.path()).toBe("/study");
  });

  it("deleting the course from its page saves no progress afterwards (no error)", async () => {
    const user = userEvent.setup();
    const error = vi.spyOn(console, "error");
    const { simulator, open } = setup();
    await open();
    const saves = simulator.callsTo("study_progress").length;
    await user.click(button("Course options"));
    await user.click(screen.getByRole("menuitem", { name: "Delete course" }));
    await user.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    await screen.findByTestId("study-home");
    // Enregistrement au départ de la page, refusé : cours supprimé.
    await waitFor(() =>
      expect(simulator.callsTo("study_progress").length).toBeGreaterThan(saves),
    );
    await act(async () => {});
    expect(
      error.mock.calls.filter(
        ([message]) => message === "Failed to save the progress:",
      ),
    ).toEqual([]);
    error.mockRestore();
  });

  it("Report a mistake forgets the explanations and answers of the old lesson", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup({}, (_, course) => {
      course.position = "L0:s0";
    });
    await open();
    await user.click(button("I don't understand"));
    await study.lastStream().finish();
    await screen.findByTestId("study-explain");
    // Une question en cours de réponse.
    await user.click(button("Ask a question"));
    await user.keyboard("Why?{Enter}");
    const ask = study.lastStream();
    await ask.step();
    expect(screen.getByTestId("study-ask-live")).toBeTruthy();
    await user.click(button("Course options"));
    await user.click(
      screen.getByRole("menuitem", { name: "Report a mistake in this level" }),
    );
    await user.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Rewrite the level",
      }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("study-explain")).toBeNull(),
    );
    expect(document.querySelector("[data-card-heading]")?.textContent).toBe(
      "Heading 1.1",
    );
    expect(screen.queryByTestId("study-explain-live")).toBeNull();
    expect(screen.queryByTestId("study-ask-live")).toBeNull();
    // La réponse en cours est arrêtée.
    expect(simulator.callsTo("study_stream_cancel")).toEqual([
      { streamId: ask.streamId },
    ]);
  });

  it("Generated by the model of the course", async () => {
    const { open } = setup({ model: "gemma3:4b" });
    await open();
    expect(
      screen.getByText("Generated by gemma3:4b. It can make mistakes."),
    ).toBeTruthy();
  });
});

describe("Reader in other languages", () => {
  it("has no hard-coded text: lesson cards, tutor, questions, path", async () => {
    const user = userEvent.setup();
    const { study, open, course } = setup({}, async (s, c) => {
      await answerAttempt(s, c, 0, 0, 3);
      c.position = "L1:s0";
    });
    await open();
    usePseudoLanguage();
    const allow = (...more: string[]) => [
      ...contentOf(course),
      // Étapes de la solution, montrées une à une.
      ...course.levels.flatMap((level) =>
        solutionSteps(level.lesson?.example.solution ?? ""),
      ),
      "qwen3:0.6b",
      "qwen3.5:0.8b",
      "?",
      ...more,
    ];
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    // Explication en flux, puis arrêtée.
    await user.click(button("⟦I don't understand⟧"));
    await study.lastStream().step();
    expect(untranslatedTexts(document.body, allow("Simpler words"))).toEqual(
      [],
    );
    await user.keyboard("{Escape}");
    await study.lastStream().finish();
    expect(untranslatedTexts(document.body, allow("Simpler words"))).toEqual(
      [],
    );
    // Exemple révélé, puis À retenir.
    for (let i = 0; i < 3; i += 1) {
      await user.click(button("⟦Continue⟧"));
    }
    await user.click(button("⟦Show the solution⟧"));
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    await user.click(button("⟦Show everything⟧"));
    await user.keyboard("{Enter}");
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("study-pitfall")).toBeTruthy();
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    // Question manquée.
    await user.keyboard("{Enter}");
    await user.click(screen.getByText("Wrong first"));
    await user.click(button("⟦Check⟧"));
    await screen.findByTestId("study-feedback");
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    // Question posée au tuteur.
    await user.click(button("⟦Ask a question⟧"));
    await user.keyboard("Why?{Enter}");
    await study.lastStream().finish();
    await screen.findByTestId("study-ask-answer");
    expect(
      untranslatedTexts(document.body, allow("Why?", "Answer to: Why?")),
    ).toEqual([]);
  });

  it("has no hard-coded text: failed attempt, remedy, end", async () => {
    const user = userEvent.setup();
    const first = setup({}, async (s, c) => {
      await answerAttempt(s, c, 0, 0, 0);
      await s.finishRemedy(c.id, 0);
      await answerAttempt(s, c, 0, 1, 0);
      c.position = "L0:a1:result";
    });
    await first.open();
    usePseudoLanguage();
    const allow = () => [
      ...contentOf(first.course),
      "qwen3:0.6b",
      "qwen3.5:0.8b",
      "?",
    ];
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    await user.click(button("⟦Continue anyway⟧"));
    await waitFor(() =>
      expect(screen.getByTestId("study-section")).toBeTruthy(),
    );
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
    await answerAttempt(first.study, first.study.course("c1"), 1, 0, 3);
    for (const q of first.study.course("c1").finalQuiz!.questions) {
      await first.study.answer("c1", q.id, true);
    }
    await user.click(screen.getByTestId("study-path-final"));
    await screen.findByTestId("study-end");
    expect(untranslatedTexts(document.body, allow())).toEqual([]);
  });

  it("in French and Spanish: eyebrow, parts, feedback typography, plurals", async () => {
    const user = userEvent.setup();
    const { open } = setup({}, async (s, course) => {
      await answerAttempt(s, course, 0, 0, 3);
      course.position = "L1:check";
    });
    await open();
    act(() => setLanguage("fr"));
    expect(screen.getByText("Entraînement — non compté")).toBeTruthy();
    expect(screen.getByTestId("study-streak").textContent).toBe(
      "33 bonnes réponses d'affilée",
    );
    expect(screen.getByText("Encore 4 min d'étude environ")).toBeTruthy();
    // Le nom suit le total (« 0 étapes » au début serait faux).
    expect(
      screen.getByRole("progressbar").getAttribute("aria-valuetext"),
    ).toMatch(/^\d+ sur \d+ étapes$/);
    await user.click(screen.getByText("Right answer"));
    await user.click(button("Vérifier"));
    await screen.findByTestId("study-feedback");
    expect(feedback().textContent).toMatch(/^Juste\u00a0!/);
    act(() => setLanguage("es"));
    expect(feedback().textContent).toMatch(/^¡Correcto!/);
    // Espagnol neutre (passé simple), comme le reste du catalogue.
    expect(tNow("study.feedback.praise.spotOn")).toBe("Diste en el clavo.");
    expect(button("Continuar")).toBeTruthy();
    expect(
      screen.getAllByTestId("study-path-node")[0].getAttribute("aria-label"),
    ).toBe("Nivel 1: Level title 1, superado, 3 estrellas de 3");
  });
});
