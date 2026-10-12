import { describe, expect, it } from "vitest";
import type { ProofreadPart } from "@/bindings/ProofreadPart";
import {
  allDone,
  assembleText,
  correctionCount,
  partViews,
  someKept,
} from "./proofread";

const PARTS: ProofreadPart[] = [
  { before: "\n", text: "Teh cat.", after: "\n\n" },
  { before: "", text: "I recieve.", after: " " },
  { before: "", text: "End", after: "" },
];

describe("assembleText", () => {
  it("rebuilds the text: results of finished passages, originals of the others", () => {
    expect(assembleText(PARTS, [])).toBe("\nTeh cat.\n\nI recieve. End");
    expect(
      assembleText(PARTS, [
        { text: "The cat.", kept: false },
        null,
        { text: "End.", kept: false },
      ]),
    ).toBe("\nThe cat.\n\nI recieve. End.");
  });
});

describe("partViews", () => {
  it("done, streaming and pending passages; marks after Correct only", () => {
    const results = [{ text: "The cat.", kept: false }, null, null];
    const views = partViews("correct", PARTS, results, {
      index: 1,
      text: "I rec",
    });
    expect(views.map((view) => view.state)).toEqual([
      "done",
      "streaming",
      "pending",
    ]);
    expect(views[0]).toMatchObject({
      before: "\n",
      after: "\n\n",
      segments: [
        { kind: "del", text: "Teh" },
        { kind: "ins", text: "The" },
        { kind: "same", text: " cat." },
      ],
    });
    expect(correctionCount(views)).toBe(1);

    const rephrased = partViews("rephrase", PARTS, results, null);
    expect(rephrased[0]).toMatchObject({
      segments: [{ kind: "same", text: "The cat." }],
    });
    expect(correctionCount(rephrased)).toBe(0);
    // Pas encore de fragment : texte d'origine atténué.
    expect(
      partViews("correct", PARTS, results, { index: 1, text: "" })[1],
    ).toMatchObject({ state: "pending", text: "I recieve." });
  });

  it("a kept passage has no marks", () => {
    const views = partViews(
      "correct",
      PARTS,
      [{ text: "Teh cat.", kept: true }, null, null],
      null,
    );
    expect(views[0]).toMatchObject({
      kept: true,
      segments: [{ kind: "same", text: "Teh cat." }],
    });
    expect(someKept([{ text: "x", kept: true }, null])).toBe(true);
    expect(someKept([{ text: "x", kept: false }, null])).toBe(false);
    expect(allDone([{ text: "x", kept: false }, null])).toBe(false);
    expect(allDone([{ text: "x", kept: false }])).toBe(true);
  });
});
