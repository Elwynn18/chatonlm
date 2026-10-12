import { describe, expect, it } from "vitest";
import { fakeCorrection, fakeSplit } from "./proofread";

describe("fake proofread engine", () => {
  it("splits at blank lines, blanks around each passage, and joins back", () => {
    // Texte vide ou blanc : aucun passage (comme le moteur).
    expect(fakeSplit("")).toEqual([]);
    expect(fakeSplit(" \n\n ")).toEqual([]);
    for (const text of [
      "One.",
      "  One.\n\nTwo.\r\n\r\n  Three.  \n",
      "\n\nA\n \nB\n\n\n",
      "- a\n- b\n\n```\ncode\n```",
    ]) {
      const parts = fakeSplit(text);
      expect(
        parts.map((p) => p.before + p.text + p.after).join(""),
        JSON.stringify(text),
      ).toBe(text);
      for (const part of parts) {
        expect(part.text).not.toBe("");
        expect(part.text.trim()).toBe(part.text);
      }
    }
    expect(fakeSplit("One.\n\nTwo.").map((p) => p.text)).toEqual([
      "One.",
      "Two.",
    ]);
  });

  it("corrects like the fake server, rephrases a few phrases more", () => {
    expect(fakeCorrection("Je suis allez au cinema, teh end", "correct")).toBe(
      "Je suis allé au cinéma, the end",
    );
    expect(fakeCorrection("teh film was very good", "rephrase")).toBe(
      "the film was excellent",
    );
  });
});
