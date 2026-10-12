// Fonctions pures du lecteur de cours (onglet Study) : paramètres d'adresse,
// suite des cartes et carte suivante, reprise d'une position, progression,
// temps restant, étoiles, série, variantes, étapes d'une solution, phrase à
// trou, taille d'un modèle, temps passé.

import { describe, expect, it } from "vitest";
import type { Answered } from "@/bindings/Answered";
import type { Course } from "@/bindings/Course";
import type { ModelInfo } from "@/bindings/ModelInfo";
import type { Question } from "@/bindings/Question";
import { standaloneStudy } from "@/test/study";
import {
  bestScore,
  cardsOf,
  courseProgress,
  currentStreak,
  finalScore,
  isSmallModel,
  levelStars,
  missedQuestions,
  nextCard,
  parameterBillions,
  resolvePosition,
  secondsLeft,
  solutionSteps,
  plainInline,
  questionPrompt,
  splitBlank,
  StudyClock,
  UNGENERATED_FINAL_QUESTION_SECONDS,
  UNGENERATED_LEVEL_CARDS,
  UNGENERATED_LEVEL_SECONDS,
  validateStudySearch,
  variantOf,
} from "./study";

/** Réponses de toutes les questions d'un essai (`right` bonnes d'abord). */
async function answerAttempt(
  fake: ReturnType<typeof standaloneStudy>,
  course: Course,
  level: number,
  attempt: number,
  right: number,
) {
  const questions = course.levels[level].attempts[attempt].questions;
  for (const [n, question] of questions.entries()) {
    await fake.answer(course.id, question.id, n < right);
  }
}

describe("validateStudySearch", () => {
  it("keeps a topic, a known level and a known duration", () => {
    expect(
      validateStudySearch({
        topic: "  Stars  ",
        level: "advanced",
        minutes: 30,
      }),
    ).toEqual({ topic: "Stars", level: "advanced", minutes: 30 });
    expect(validateStudySearch({ minutes: "10" })).toEqual({ minutes: 10 });
    // Nombre lu par le routeur : gardé en texte.
    expect(validateStudySearch({ topic: 1789 })).toEqual({ topic: "1789" });
  });

  it("ignores unknown values and cuts a long topic", () => {
    expect(
      validateStudySearch({ topic: "", level: "expert", minutes: 15 }),
    ).toEqual({});
    expect(validateStudySearch({ topic: "é".repeat(400) }).topic).toHaveLength(
      300,
    );
  });
});

describe("cardsOf", () => {
  it("lists the intro, the cards of ready levels, a waiting card per other level, the final review and the end", () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", readyLevels: 1 });
    expect(cardsOf(course).map((card) => `${card.kind}@${card.key}`)).toEqual([
      "intro@intro",
      "section@L0:s0",
      "section@L0:s1",
      "section@L0:s2",
      "example@L0:example",
      "recap@L0:recap",
      "check@L0:check",
      "question@L0:a0:q0",
      "question@L0:a0:q1",
      "question@L0:a0:q2",
      "result@L0:a0:result",
      "levelWait@L1:s0",
      "levelWait@L2:s0",
      "levelWait@L3:s0",
      "finalWait@final:q0",
      "end@end",
    ]);
  });

  it("adds the explanation and the questions of a new attempt, then the final questions", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    await answerAttempt(fake, course, 0, 0, 1);
    await fake.finishRemedy(course.id, 0);
    const keys = cardsOf(course).map((card) => card.key);
    expect(
      keys.slice(keys.indexOf("L0:a0:result"), keys.indexOf("L1:s0")),
    ).toEqual([
      "L0:a0:result",
      "L0:a1:explain",
      "L0:a1:q0",
      "L0:a1:q1",
      "L0:a1:q2",
      "L0:a1:result",
    ]);
    expect(keys.slice(-4)).toEqual(["final:q0", "final:q1", "final:q2", "end"]);
  });
});

describe("nextCard", () => {
  it("waits for an answer, then goes on", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    expect(nextCard(course, "intro")?.key).toBe("L0:s0");
    expect(nextCard(course, "L0:recap")?.key).toBe("L0:check");
    expect(nextCard(course, "L0:check")).toBeNull();
    await fake.answer(course.id, "L0-check", false);
    expect(nextCard(course, "L0:check")?.key).toBe("L0:a0:q0");
  });

  it("stops after a failed attempt until a new attempt or Continue anyway", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    await answerAttempt(fake, course, 0, 0, 1);
    // Palier suivant verrouillé : pas de suite.
    expect(nextCard(course, "L0:a0:result")).toBeNull();
    await fake.finishRemedy(course.id, 0);
    expect(nextCard(course, "L0:a0:result")?.key).toBe("L0:a1:explain");
  });

  it("opens the next level after a passed attempt, and the final review after the last level", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    await answerAttempt(fake, course, 0, 0, 2);
    expect(nextCard(course, "L0:a0:result")?.key).toBe("L1:s0");
    expect(nextCard(course, "L1:a0:result")).toBeNull();
    await answerAttempt(fake, course, 1, 0, 3);
    expect(nextCard(course, "L1:a0:result")?.key).toBe("final:q0");
  });

  it("never leaves a waiting card", () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", readyLevels: 0 });
    expect(nextCard(course, "L0:s0")).toBeNull();
  });
});

describe("resolvePosition", () => {
  it("keeps a reachable card, falls back to the intro (the end once finished)", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    expect(resolvePosition(course, "L0:example")).toBe("L0:example");
    expect(resolvePosition(course, "nonsense")).toBe("intro");
    // Palier verrouillé.
    expect(resolvePosition(course, "L1:s1")).toBe("intro");
    // Carte disparue (essai effacé) : début de son palier.
    expect(resolvePosition(course, "L0:a3:q0")).toBe("L0:s0");
    const done = fake.seed({ status: "finished", minutes: 10 });
    expect(resolvePosition(done, "")).toBe("end");
  });
});

describe("courseProgress and secondsLeft", () => {
  it("counts the cards before the shown one in the current level, all the cards of finished levels", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    // Palier prêt : 3 sections + exemple + à retenir + entraînement + 3
    // questions + résultat = 10 cartes ; bilan : 3 questions.
    expect(courseProgress(course, "intro")).toEqual({ done: 0, total: 23 });
    expect(courseProgress(course, "L0:a0:q1")).toEqual({ done: 7, total: 23 });
    await answerAttempt(fake, course, 0, 0, 3);
    expect(courseProgress(course, "L1:s0")).toEqual({ done: 10, total: 23 });
    await fake.answer(course.id, "F-Q0", true);
    expect(courseProgress(course, "final:q1").done).toBe(11);
  });

  it("estimates levels not written yet", () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", readyLevels: 1 });
    expect(courseProgress(course, "intro").total).toBe(
      10 + 3 * UNGENERATED_LEVEL_CARDS + 5,
    );
    expect(secondsLeft(course)).toBe(
      180 +
        3 * UNGENERATED_LEVEL_SECONDS +
        5 * UNGENERATED_FINAL_QUESTION_SECONDS,
    );
  });

  it("leaves out finished levels and an answered final review", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    expect(secondsLeft(course)).toBe(2 * 180 + 3 * 25);
    await answerAttempt(fake, course, 0, 0, 3);
    expect(secondsLeft(course)).toBe(180 + 3 * 25);
    const done = fake.seed({ status: "finished", minutes: 10 });
    expect(secondsLeft(done)).toBe(0);
    expect(courseProgress(done, "end")).toEqual({ done: 23, total: 23 });
  });
});

describe("scores", () => {
  it("gives stars: 3 without a mistake, 2 with one, 1 after a new attempt, 0 to review", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 30 });
    expect(levelStars(course, 0)).toBeNull();
    await answerAttempt(fake, course, 0, 0, 3);
    expect(levelStars(course, 0)).toBe(3);
    await answerAttempt(fake, course, 1, 0, 2);
    expect(levelStars(course, 1)).toBe(2);
    await answerAttempt(fake, course, 2, 0, 1);
    await fake.finishRemedy(course.id, 2);
    await answerAttempt(fake, course, 2, 1, 3);
    expect(levelStars(course, 2)).toBe(1);
    expect(bestScore(course, 2)).toEqual({ correct: 3, total: 3 });
    // « Continue anyway » (statut du moteur).
    course.levels[3].status = "review";
    expect(levelStars(course, 3)).toBe(0);
  });

  it("lists the missed questions of the attempts and the final review, not the practice", async () => {
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    await fake.answer(course.id, "L0-check", false);
    await answerAttempt(fake, course, 0, 0, 2);
    await answerAttempt(fake, course, 1, 0, 3);
    await fake.answer(course.id, "F-Q0", false);
    expect(
      missedQuestions(course).map((m) => [m.question.id, m.level]),
    ).toEqual([
      ["L0-A0-Q2", 0],
      ["F-Q0", null],
    ]);
    expect(finalScore(course)).toEqual({ correct: 0, total: 3, answered: 1 });
  });

  it("counts the right answers in a row, the last one included", () => {
    const answer = (correct: boolean, at: number): Answered => ({
      given: { type: "trueFalse", value: true },
      expected: { type: "trueFalse", value: true },
      correct,
      close: false,
      detail: [],
      explanation: "",
      at,
    });
    const fake = standaloneStudy();
    const course = fake.seed({ status: "active", minutes: 10 });
    expect(currentStreak(course)).toBe(0);
    course.answers = {
      b: answer(true, 3),
      a: answer(false, 1),
      c: answer(true, 2),
      d: answer(true, 4),
    };
    expect(currentStreak(course)).toBe(3);
    course.answers.e = answer(false, 5);
    expect(currentStreak(course)).toBe(0);
  });
});

describe("variantOf", () => {
  it("is stable and within range, and differs between questions", () => {
    expect(variantOf("L0-A0-Q0", 6)).toBe(variantOf("L0-A0-Q0", 6));
    const variants = new Set(
      ["L0-A0-Q0", "L0-A0-Q1", "L0-A0-Q2", "L1-A0-Q0", "F-Q0", "F-Q1"].map(
        (id) => variantOf(id, 6),
      ),
    );
    expect(variants.size).toBeGreaterThan(1);
    for (const variant of variants) {
      expect(variant).toBeGreaterThanOrEqual(0);
      expect(variant).toBeLessThan(6);
    }
  });
});

describe("solutionSteps", () => {
  it("splits a list into its items, an intro paragraph first", () => {
    expect(
      solutionSteps(
        "We compute it:\n1. Add $a$.\n2. Then\n   divide.\n3. Done.",
      ),
    ).toEqual([
      "We compute it:",
      "1. Add $a$.",
      "2. Then\n   divide.",
      "3. Done.",
    ]);
  });

  it("splits paragraphs, never inside a code or formula block", () => {
    expect(
      solutionSteps(
        "First.\n\n```py\nx = 1\n\ny = 2\n```\n\n$$\na\n\nb\n$$\n\nLast.",
      ),
    ).toEqual([
      "First.",
      "```py\nx = 1\n\ny = 2\n```",
      "$$\na\n\nb\n$$",
      "Last.",
    ]);
  });

  it("never takes a list inside a code block for steps; a block glued to the text is a step", () => {
    expect(
      solutionSteps("Run:\n```\n- a\n- b\n```\nThen $$\n$$\nx\n$$"),
    ).toEqual(["Run:", "```\n- a\n- b\n```", "Then $$", "$$\nx\n$$"]);
  });

  it("keeps a single step and an indented continuation", () => {
    expect(solutionSteps("Only one step.")).toEqual(["Only one step."]);
    expect(solutionSteps("- One\n\n  more of one\n- Two")).toEqual([
      "- One\n\n  more of one",
      "- Two",
    ]);
  });
});

describe("splitBlank", () => {
  it("cuts the prompt at its blank, whatever its length", () => {
    expect(splitBlank("The sky is ___ today.")).toEqual([
      "The sky is ",
      " today.",
    ]);
    expect(splitBlank("A _____ B")).toEqual(["A ", " B"]);
  });

  it("refuses a blank inside a formula, code or bold text", () => {
    expect(splitBlank("Solve $x = ___$ now")).toBeNull();
    expect(splitBlank("Call `f(___)`")).toBeNull();
    expect(splitBlank("**The ___ word**")).toBeNull();
    expect(splitBlank("*The ___ word*")).toBeNull();
    expect(splitBlank("No blank")).toBeNull();
  });

  it("never cuts inside \\(…\\), \\[…\\] or $$…$$ (the forms the engine sends)", () => {
    expect(
      splitBlank(
        String.raw`Complete: \(\frac{1}{4} + \frac{2}{4} = \frac{___}{4}\).`,
      ),
    ).toBeNull();
    expect(splitBlank(String.raw`Then \[ y = 2x + ___ \] holds.`)).toBeNull();
    expect(splitBlank("So $$x^2 = ___$$ here.")).toBeNull();
    expect(splitBlank("A lone $ before ___ stays whole.")).toBeNull();
    expect(splitBlank("Use ``a`b`` then ``f(___)``")).toBeNull();
  });

  it("cuts after closed formulas and code, escaped dollars included", () => {
    expect(
      splitBlank(String.raw`If \(x^2 = 9\) and \(x > 0\), x = ___.`),
    ).toEqual([String.raw`If \(x^2 = 9\) and \(x > 0\), x = `, "."]);
    expect(splitBlank("Since $$a = b$$, `f(x)` gives ___ now.")).toEqual([
      "Since $$a = b$$, `f(x)` gives ",
      " now.",
    ]);
    expect(splitBlank(String.raw`It costs \$5, the total is ___.`)).toEqual([
      String.raw`It costs \$5, the total is `,
      ".",
    ]);
  });
});

describe("questionPrompt", () => {
  const blank = (prompt: string): Question => ({
    id: "q",
    body: { type: "blank", prompt },
  });

  it("turns a blank inside a formula into an underlined space KaTeX accepts", () => {
    expect(
      questionPrompt(
        blank(String.raw`Complete: \(\frac{___}{4}\) and $$x = ____$$.`),
      ),
    ).toBe(
      String.raw`Complete: \(\frac{\underline{\qquad}}{4}\) and $$x = \underline{\qquad}$$.`,
    );
  });

  it("leaves a blank in the text, code and other questions as they are", () => {
    expect(questionPrompt(blank("The sky is ___ and `a___b`."))).toBe(
      "The sky is ___ and `a___b`.",
    );
    expect(
      questionPrompt({
        id: "q",
        body: { type: "trueFalse", statement: String.raw`\(x_1\) holds.` },
      }),
    ).toBe(String.raw`\(x_1\) holds.`);
  });
});

describe("plainInline", () => {
  it("removes Markdown marks and keeps the words", () => {
    expect(plainInline("**Carbon** *dioxide*")).toBe("Carbon dioxide");
    expect(plainInline("The `map` function, _really_")).toBe(
      "The map function, really",
    );
    expect(plainInline("[ollama](https://ollama.com) ~~old~~")).toBe(
      "ollama old",
    );
    expect(plainInline("snake_case_name stays")).toBe("snake_case_name stays");
  });

  it("reads formulas as text", () => {
    expect(plainInline(String.raw`\(CO_2\)`)).toBe("CO₂");
    expect(plainInline(String.raw`\(\frac{1}{2}\) of \(x^{2}\)`)).toBe(
      "1/2 of x²",
    );
    expect(plainInline(String.raw`\(\frac{a+1}{2b}\)`)).toBe("(a+1)/(2b)");
    expect(plainInline(String.raw`\(\sqrt{16} = 4\), \(90^\circ\)`)).toBe(
      "√(16) = 4, 90°",
    );
    expect(plainInline(String.raw`\(2 \times \pi \leq \text{six}\)`)).toBe(
      "2 × π ≤ six",
    );
    expect(
      plainInline(String.raw`\(x_{max}\), \(y_{b}\) and $$e^{i\pi}$$`),
    ).toBe("xₘₐₓ, y_(b) and e^(iπ)");
  });

  it("leaves dollars that are not formulas (as rendered) and unclosed ones", () => {
    expect(plainInline("It costs $5 and $10")).toBe("It costs $5 and $10");
    expect(plainInline(String.raw`Open \(x + 1`)).toBe(String.raw`Open (x + 1`);
  });
});

describe("model size", () => {
  const model = (parameterSize: string): ModelInfo => ({
    model: "m",
    digest: "",
    size: 0,
    parameterSize,
    quantizationLevel: "",
    contextLength: null,
    capabilities: [],
  });

  it("reads Ollama's parameter sizes", () => {
    expect(parameterBillions("751.63M")).toBeCloseTo(0.75163);
    expect(parameterBillions("8.0B")).toBe(8);
    expect(parameterBillions("1.5T")).toBe(1500);
    expect(parameterBillions("big")).toBeNull();
  });

  it("calls a model under 3B small, never an unknown one", () => {
    expect(isSmallModel(model("751.63M"))).toBe(true);
    expect(isSmallModel(model("2.9B"))).toBe(true);
    expect(isSmallModel(model("3.2B"))).toBe(true);
    expect(isSmallModel(model("3.5B"))).toBe(false);
    expect(isSmallModel(model("3.8B"))).toBe(false);
    expect(isSmallModel(model(""))).toBe(false);
    expect(isSmallModel(undefined)).toBe(false);
  });
});

describe("StudyClock", () => {
  it("counts only the active periods, from the time already saved", () => {
    let now = 1000;
    const clock = new StudyClock(5000, () => now);
    now = 2000;
    expect(clock.elapsed()).toBe(5000);
    clock.setActive(true);
    now = 3500;
    expect(clock.elapsed()).toBe(6500);
    clock.setActive(false);
    now = 10_000;
    expect(clock.elapsed()).toBe(6500);
    clock.setActive(true);
    clock.setActive(true);
    now = 10_500;
    expect(clock.elapsed()).toBe(7000);
  });
});
