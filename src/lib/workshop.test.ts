// Fonctions pures de l'Atelier : curseur d'autonomie (Maj+Tab), titre et
// dossier tirés de la tâche comme le moteur, pastille de la carte, codes du
// moteur traduits (erreurs, avertissements, vérifications, mesures), frise
// des phases, temps de travail, phrases de l'activité et du journal.

import { describe, expect, it } from "vitest";
import type { ProjectSummary } from "@/bindings/ProjectSummary";
import type { WorkshopEvent } from "@/bindings/WorkshopEvent";
import type { WorkshopEventKind } from "@/bindings/WorkshopEventKind";
import { translator } from "@/i18n";
import {
  activityLine,
  badgeOf,
  checkDetail,
  checkSubject,
  durationText,
  elapsedMs,
  projectIdOf,
  errorKey,
  eventText,
  folderNameOf,
  isDoubtful,
  isSignal,
  isToReview,
  moveBrick,
  nextAutonomy,
  phaseMarks,
  planChanged,
  proposedFolderName,
  readMeasure,
  titleOf,
  warningKey,
} from "@/lib/workshop";
import { check, DEMO_PLAN } from "@/test/workshop";

const t = translator("en");

function summary(patch: Partial<ProjectSummary> = {}): ProjectSummary {
  return {
    id: 1,
    title: "Guide",
    strategy: null,
    strategyForced: false,
    autonomy: "ask",
    state: "running",
    phase: null,
    model: "",
    modelUsed: "",
    position: null,
    error: null,
    folder: "/p",
    createdAt: 0,
    updatedAt: 0,
    finishedAt: null,
    ...patch,
  };
}

function events(...kinds: WorkshopEventKind[]): WorkshopEvent[] {
  return kinds.map((kind, index) => ({
    seq: index + 1,
    atMs: 1_000 + index * 60_000,
    kind,
  }));
}

const name = (id: number) => (id === 4 ? "Compost" : `part ${id}`);

describe("workshop logic", () => {
  it("cycles the autonomy levels in order, back to the first", () => {
    expect(nextAutonomy("plan")).toBe("ask");
    expect(nextAutonomy("ask")).toBe("acceptActions");
    expect(nextAutonomy("acceptActions")).toBe("autonomous");
    expect(nextAutonomy("autonomous")).toBe("plan");
  });

  it("takes the title and folder from the task like the engine", () => {
    expect(titleOf("\n\n  Guide :  la   permaculture ?  \nsuite")).toBe(
      "Guide : la permaculture ?",
    );
    expect(proposedFolderName("  Guide : la  permaculture ?  ")).toBe(
      "Guide - la permaculture",
    );
    const long = `${"word ".repeat(30)}end`;
    expect(titleOf(long).endsWith("…")).toBe(true);
    expect([...titleOf(long)].length).toBeLessThanOrEqual(81);
    expect(folderNameOf("..")).toBe("");
    expect(folderNameOf("CON")).toBe("");
    expect(folderNameOf("a/b\\c")).toBe("a-b-c");
    expect([...folderNameOf("é".repeat(100))].length).toBe(60);
  });

  it("marks the card when a project works, and first when one waits", () => {
    expect(badgeOf([])).toBeNull();
    expect(badgeOf([summary({ state: "queued" })])).toBeNull();
    expect(badgeOf([summary({ state: "done" })])).toBeNull();
    expect(badgeOf([summary({ state: "running" })])).toBe("working");
    expect(badgeOf([summary({ state: "yielding" })])).toBe("working");
    expect(
      badgeOf([summary({ state: "running" }), summary({ state: "waiting" })]),
    ).toBe("needsYou");
  });

  it("translates the engine codes, never showing them", () => {
    expect(errorKey("planUnusable")).toBe("workshop.errors.planUnusable");
    expect(errorKey("crashed")).toBe("workshop.errors.engine");
    expect(errorKey("timeout")).toBe("workshop.errors.connection");
    expect(errorKey("remoteKey")).toBe("workshop.errors.account");
    expect(errorKey("somethingNew")).toBe("workshop.errors.unknown");
    expect(errorKey(null)).toBe("workshop.errors.unknown");
    expect(warningKey("lengthCapped")).toBe("workshop.warnings.lengthCapped");
    expect(warningKey("planDefect.overlap")).toBe(
      "workshop.warnings.planDefect",
    );
    expect(warningKey("specDefect.undecided")).toBe(
      "workshop.warnings.specDefect",
    );
    expect(warningKey("future")).toBe("workshop.warnings.generic");
  });

  it("reads the program's measures and says them in words", () => {
    expect(readMeasure("3239/3300-3700 words")).toEqual({
      value: 3239,
      min: 3300,
      max: 3700,
      unit: "words",
    });
    expect(readMeasure("4/8- headings")).toEqual({
      value: 4,
      min: 8,
      max: null,
      unit: "headings",
    });
    expect(readMeasure("950/1000")).toEqual({
      value: 950,
      min: 1000,
      max: 1000,
      unit: "words",
    });
    expect(readMeasure("unavailable")).toBeNull();
    expect(
      checkDetail(t, check("C1", false, { evidence: "3239/3300-3700 words" })),
    ).toBe("3239 words (target: 3300 to 3700)");
    expect(
      checkDetail(t, check("C2", false, { evidence: "5/-10 headings@2" })),
    ).toBe("5 level-2 headings (target: at most 10)");
    expect(
      checkDetail(t, check("length", true, { evidence: "950/1000" })),
    ).toBe("950 words (target: 1000)");
    // Preuve d'un programme illisible : code interne, rien n'est montré.
    expect(checkDetail(t, check("empty", false, { evidence: "x" }))).toBeNull();
    // Preuve d'un juge : citation du texte, montrée telle quelle.
    expect(
      checkDetail(
        t,
        check("C3", true, { method: "formJudge", evidence: "« … »" }),
      ),
    ).toBe("« … »");
    expect(checkSubject(t, check("C3", true))).toBe("Criterion C3");
    const criteria = new Map([["C3", "Cites 2 sources"]]);
    expect(checkSubject(t, check("C3", true), criteria)).toBe(
      "C3: Cites 2 sources",
    );
    expect(checkSubject(t, check("C4", true), criteria)).toBe("Criterion C4");
    expect(checkSubject(t, check("internalWord", true))).toBe(
      "No Workshop wording left in the text",
    );
    expect(checkSubject(t, check("newCode", true))).toBe(
      "Check by the program",
    );
    expect(
      checkSubject(
        t,
        check("Basil likes shade", false, { method: "factJudge" }),
      ),
    ).toBe("Basil likes shade");
  });

  it("sorts the checks of the report", () => {
    const wrong = check("200 g", false, {
      method: "factJudge",
      blocking: true,
    });
    const doubtful = check("coffee", false, {
      method: "factJudge",
      blocking: false,
    });
    const signal = check("engineWord", false, { blocking: false });
    expect([wrong, doubtful, signal].filter(isToReview)).toEqual([wrong]);
    expect([wrong, doubtful, signal].filter(isDoubtful)).toEqual([doubtful]);
    expect([wrong, doubtful, signal].filter(isSignal)).toEqual([signal]);
  });

  it("marks the phases from the events, skipped ones counting as done", () => {
    const marks = phaseMarks(
      events(
        { type: "phaseStarted", phase: "collect" },
        { type: "phaseDone", phase: "collect" },
        { type: "phaseStarted", phase: "strategy" },
        { type: "phaseDone", phase: "strategy" },
        // Cadrage et spécification sautés (autonomie) : faits quand même.
        { type: "phaseStarted", phase: "plan" },
      ),
      null,
      false,
    );
    expect(marks.collect).toBe("done");
    expect(marks.framing).toBe("done");
    expect(marks.spec).toBe("done");
    expect(marks.plan).toBe("current");
    expect(marks.build).toBe("pending");
    expect(marks.deliver).toBe("pending");
    // Sans journal : la phase du projet.
    expect(phaseMarks([], "build", false).build).toBe("current");
    expect(phaseMarks([], null, false).collect).toBe("pending");
    // Livré : tout est fait.
    expect(phaseMarks([], "deliver", true).deliver).toBe("done");
  });

  it("counts the working time from the first event, never below the last", () => {
    const list = events(
      { type: "phaseStarted", phase: "collect" },
      { type: "phaseDone", phase: "collect" },
    );
    expect(elapsedMs([], true, 5_000)).toBe(0);
    expect(elapsedMs(list, false, 999_999)).toBe(60_000);
    expect(elapsedMs(list, true, 1_000 + 300_000)).toBe(300_000);
    expect(elapsedMs(list, true, 0)).toBe(60_000);
    expect(durationText(t, 30_000)).toBe("under a minute");
    expect(projectIdOf("12")).toBe(12);
    for (const bad of [
      "abc",
      "",
      "0",
      "-1",
      "1.5",
      "1e3",
      " 2",
      "99999999999999999",
    ]) {
      expect(projectIdOf(bad)).toBeNull();
    }
    expect(durationText(t, 14 * 60_000)).toBe("14 min");
    expect(durationText(t, (2 * 60 + 14) * 60_000)).toBe("2 h 14 min");
  });

  it("counts only the working periods: not the wait for an answer, nor a pause", () => {
    const at = (minute: number, kind: WorkshopEventKind): WorkshopEvent => ({
      seq: minute,
      atMs: minute * 60_000,
      kind,
    });
    const run: WorkshopEventKind = {
      type: "modelWarnings",
      model: "m",
      warnings: [],
    };
    const list = [
      // Travail de la minute 1 à 3, question à 3, réponse à 10 (attente
      // non comptée), travail jusqu'à 12, pause (rien n'est noté), relance
      // à 30, travail jusqu'à 35, fin.
      at(1, run),
      at(2, { type: "phaseStarted", phase: "plan" }),
      at(3, {
        type: "questionAsked",
        question: { type: "framing", value: ["Who?"] },
      }),
      at(10, {
        type: "answerReceived",
        answer: { type: "framing", value: [] },
      }),
      at(11, { type: "yielding" }),
      at(12, { type: "resumed" }),
      at(30, run),
      at(35, { type: "finished", state: "done" }),
    ];
    // 2 + 2 + 5 minutes.
    expect(elapsedMs(list, false, 99 * 60_000)).toBe(9 * 60_000);
    // Encore au travail après la relance : jusqu'à maintenant.
    expect(elapsedMs(list.slice(0, 7), true, 34 * 60_000)).toBe(8 * 60_000);
    // Question en attente : le temps ne court pas.
    expect(elapsedMs(list.slice(0, 3), false, 50 * 60_000)).toBe(2 * 60_000);
  });

  it("says the activity from the state and the last event", () => {
    const list = events({
      type: "activity",
      activity: { code: "fixingBrick", id: 4 },
    });
    expect(activityLine(t, summary(), list, name)).toBe("Fixing “Compost”…");
    expect(activityLine(t, summary(), [], name)).toBe("Getting started…");
    expect(
      activityLine(t, summary({ state: "queued", position: 2 }), [], name),
    ).toBe("Waiting for its turn: position 2 in the queue.");
    expect(
      activityLine(
        t,
        summary({ state: "failed", error: "planUnusable" }),
        [],
        name,
      ),
    ).toBe("No usable plan could be drafted.");
    expect(activityLine(t, summary({ state: "yielding" }), list, name)).toBe(
      "You're using the app: the project waits until you're done.",
    );
    const lent = events(
      { type: "activity", activity: { code: "writingBrick", id: 4 } },
      { type: "gpuLent", to: "image" },
    );
    expect(activityLine(t, summary(), lent, name)).toBe(
      "The text model lends the graphics card to the image engine…",
    );
    const back = [
      ...lent,
      ...events({ type: "gpuBack" }).map((e) => ({ ...e, seq: 3 })),
    ];
    expect(activityLine(t, summary(), back, name)).toBe("Writing “Compost”…");
  });

  it("writes each event of the log in words", () => {
    const say = (kind: WorkshopEventKind) =>
      eventText(t, { seq: 1, atMs: 0, kind }, name);
    expect(say({ type: "phaseStarted", phase: "build" })).toBe(
      "Phase “Writing” started.",
    );
    expect(
      say({ type: "strategyChosen", strategy: "longForm", unanimous: true }),
    ).toBe("Strategy: Long-form writing (three votes out of three).");
    expect(say({ type: "planReady", plan: DEMO_PLAN })).toBe(
      "Plan ready: 5 parts.",
    );
    expect(say({ type: "brick", id: 4, state: "toReview", attempt: 1 })).toBe(
      "“Compost”: To review.",
    );
    expect(
      say({
        type: "brickChecks",
        id: 4,
        attempt: 0,
        checks: [check("C1", true), check("C2", false)],
      }),
    ).toBe("“Compost”: 1 check of 2 not passed.");
    expect(say({ type: "warning", code: "attachmentsCut" })).toBe(
      "The attached documents were too long and were cut.",
    );
    expect(say({ type: "error", code: "deliverWriteFailed" })).toBe(
      "The files couldn't be written to the project folder.",
    );
    expect(
      say({ type: "modelWarnings", model: "qwen", warnings: ["small", "x"] }),
    ).toBe("Model: qwen. This model is small: expect a modest result.");
    expect(say({ type: "finished", state: "failed" })).toBe("Project failed.");
  });

  it("knows whether the plan was changed, and moves a part", () => {
    const same = { title: DEMO_PLAN.title, bricks: [...DEMO_PLAN.bricks] };
    expect(planChanged(DEMO_PLAN, same)).toBe(false);
    const moved = { ...same, bricks: moveBrick(same.bricks, 1, 1) };
    expect(moved.bricks.map((brick) => brick.id)).toEqual([1, 3, 2, 4, 5]);
    expect(planChanged(DEMO_PLAN, moved)).toBe(true);
    expect(moveBrick([1, 2], 0, -1)).toEqual([1, 2]);
    const renamed = {
      ...same,
      bricks: same.bricks.map((brick) =>
        brick.id === 3 ? { ...brick, words: 900 } : brick,
      ),
    };
    expect(planChanged(DEMO_PLAN, renamed)).toBe(true);
  });

  it("follows the language", () => {
    const fr = translator("fr");
    expect(durationText(fr, (60 + 5) * 60_000)).toBe("1 h 5 min");
    // Groupement des chiffres de l'app : à partir de cinq chiffres.
    expect(
      checkDetail(
        fr,
        check("C1", false, { evidence: "12000/10000-20000 words" }),
      ),
    ).toBe("12\u202f000 mots (visé\u00a0: 10\u202f000 à 20\u202f000)");
    expect(
      checkDetail(fr, check("C1", false, { evidence: "3239/3300-3700 words" })),
    ).toBe("3239 mots (visé : 3300 à 3700)");
  });
});
