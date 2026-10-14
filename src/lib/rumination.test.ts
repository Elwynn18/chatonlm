import { describe, expect, it } from "vitest";
import { isRuminating } from "@/lib/rumination";

const SUBJECTS = ["test0", "test1", "test2", "test3", "the link"];
const CHECKS = [
  "the permissions are right",
  "the size matches",
  "the date can be set",
  "the link count is two",
];

/** Comme Qwen le 14 oct. 2026 : même ouverture, phrases toutes différentes. */
function ruminating(paragraphs: number): string {
  return Array.from(
    { length: paragraphs },
    (_, i) =>
      `Need maybe if ${SUBJECTS[i % SUBJECTS.length]} after step ${i}: check that ${CHECKS[i % CHECKS.length]} once more.\n\n`,
  ).join("");
}

/** Longue réflexion légitime : ouvertures variées, « Wait » fréquent. */
function reasoning(paragraphs: number): string {
  const openers = [
    "Wait, let me",
    "Hmm, so the",
    "Alternatively we could",
    "Now consider the",
    "Wait, maybe the",
    "So far we have",
    "Let me compute",
    "But then the",
    "Okay, next the",
    "Check: does the",
    "Another idea is",
    "First, recall that",
  ];
  return Array.from(
    { length: paragraphs },
    (_, i) =>
      `${openers[(i * 7) % openers.length]} value ${i * 13} gives remainder ${i % 11} for modulus ${i + 2}, which we keep.\n\n`,
  ).join("");
}

describe("isRuminating", () => {
  it("flags a long thinking that keeps opening with the same words", () => {
    const text = ruminating(400);
    expect(text.length).toBeGreaterThan(20_000);
    expect(isRuminating(text)).toBe(true);
  });

  it("never flags a short thinking", () => {
    const text = ruminating(200);
    expect(text.length).toBeLessThan(20_000);
    expect(isRuminating(text)).toBe(false);
  });

  it("does not flag a long, varied reasoning", () => {
    const text = reasoning(600);
    expect(text.length).toBeGreaterThan(30_000);
    expect(isRuminating(text)).toBe(false);
  });

  it("flags the same line written again and again", () => {
    const script = "mkdir test0 && chmod 715 test0\n";
    const text = Array.from(
      { length: 7 },
      (_, i) => `${reasoning(40)}Draft ${i}:\n${script}`,
    ).join("");
    expect(text.length).toBeGreaterThan(20_000);
    expect(isRuminating(text)).toBe(true);
    // Une ligne réécrite cinq fois : un brouillon, pas un ressassement.
    const fewer = Array.from(
      { length: 5 },
      (_, i) => `${reasoning(60)}Draft ${i}:\n${script}`,
    ).join("");
    expect(isRuminating(fewer)).toBe(false);
  });

  it("ignores code fences and short lines", () => {
    const block = "```\n}\n```\n";
    const text = reasoning(300) + block.repeat(50);
    expect(isRuminating(text)).toBe(false);
  });

  it("only looks at the end of the thinking", () => {
    // Ressassement au début, puis une longue suite variée : plus signalé.
    const text = ruminating(400) + reasoning(600);
    expect(isRuminating(text)).toBe(false);
  });
});
