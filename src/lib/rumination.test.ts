import { describe, expect, it } from "vitest";
import { isRuminating, looksLikeCode } from "@/lib/rumination";

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
      { length: 10 },
      (_, i) => `${reasoning(30)}Draft ${i}:\n${script}`,
    ).join("");
    expect(text.length).toBeGreaterThan(20_000);
    expect(isRuminating(text)).toBe(true);
    // Une ligne réécrite neuf fois : des brouillons, pas un ressassement.
    const fewer = Array.from(
      { length: 9 },
      (_, i) => `${reasoning(40)}Draft ${i}:\n${script}`,
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

  // Longues réflexions légitimes qui ressemblaient à un ressassement
  // (relecture du 14 oct. 2026).
  describe("does not flag a legitimate long thinking", () => {
    it("enumerating cases: « For n = k, … » 40 times", () => {
      let cases = "";
      for (let n = 1; n <= 40; n++) {
        cases += `For n = ${n}, n^2 + 1 = ${n * n + 1}, which factors as something we check, so it works.\n\n`;
      }
      const text = reasoning(300) + cases;
      expect(text.length).toBeGreaterThan(20_000);
      expect(isRuminating(text)).toBe(false);
      // Que des cas : toujours rien.
      let only = "";
      for (let n = 1; n <= 300; n++) {
        only += `For n = ${n}, n^2 + 1 = ${n * n + 1}, which factors as something we check, so it works.\n\n`;
      }
      expect(only.length).toBeGreaterThan(20_000);
      expect(isRuminating(only)).toBe(false);
    });

    it("six drafts of the same function", () => {
      const draft =
        "def solve(nums, target):\n    seen = {}\n    for i, x in enumerate(nums):\n        if target - x in seen:\n            return [seen[target - x], i]\n        seen[x] = i\n    return []\n\n";
      const text = Array.from(
        { length: 6 },
        (_, d) =>
          `${`Thinking about the edge cases of draft ${d}, maybe the input can be empty or very large and the recursion depth matters here. `.repeat(28)}\n\n${draft}`,
      ).join("");
      expect(text.length).toBeGreaterThan(20_000);
      expect(isRuminating(text)).toBe(false);
    });

    it("twelve Markdown tables", () => {
      const text = Array.from(
        { length: 12 },
        (_, k) =>
          `${`Comparing option ${k} on cost and latency and memory before drafting the table for it. `.repeat(20)}\n| Option | Cost | Latency |\n|---|---|---|\n| A | 1 | 2 |\n\n`,
      ).join("");
      expect(text.length).toBeGreaterThan(20_000);
      expect(isRuminating(text)).toBe(false);
    });

    it("six Markdown tables", () => {
      const text = Array.from(
        { length: 6 },
        (_, k) =>
          `${`Comparing option ${k} on cost and latency and memory before drafting the table for it. `.repeat(40)}\n| Option | Cost | Latency |\n|---|---|---|\n| A | 1 | 2 |\n\n`,
      ).join("");
      expect(text.length).toBeGreaterThan(20_000);
      expect(isRuminating(text)).toBe(false);
    });

    it("the same short conclusion eight times", () => {
      const text = Array.from(
        { length: 8 },
        (_, k) =>
          `${`Try placing ${k} in row ${k} column ${(k * 3) % 9}; then the box constraint for box ${k % 3} forces another value elsewhere and we propagate. `.repeat(23)}\nThat doesn't work.\n\n`,
      ).join("");
      expect(text.length).toBeGreaterThan(20_000);
      expect(isRuminating(text)).toBe(false);
    });

    it("R1 style: « Wait, let me » opens about 20% of the paragraphs", () => {
      const openers = [
        "Wait, let me",
        "Hmm, so the",
        "Alternatively we could",
        "Now consider the",
        "Wait, but the",
      ];
      const text = Array.from(
        { length: 600 },
        (_, i) =>
          `${i % 5 === 0 ? "Wait, let me" : openers[1 + (i % 4)]} value ${i * 13} gives remainder ${i % 11} for modulus ${i + 2}, which we keep.\n\n`,
      ).join("");
      expect(text.length).toBeGreaterThan(20_000);
      expect(isRuminating(text)).toBe(false);
    });
  });

  it("tells code and tables from sentences", () => {
    expect(looksLikeCode("|---|---|---|")).toBe(true);
    expect(looksLikeCode("| Option | Cost | Latency |")).toBe(true);
    expect(looksLikeCode("seen[x] = i")).toBe(true);
    expect(looksLikeCode("return solve(nums, target) + 1")).toBe(true);
    expect(looksLikeCode("for i, x in enumerate nums:")).toBe(true);
    expect(looksLikeCode("0x00, 0x00, 0x00, 0x00")).toBe(true);
    expect(looksLikeCode("```python")).toBe(true);
    expect(looksLikeCode("That doesn't work, so try again.")).toBe(false);
    expect(looksLikeCode("mkdir test0 && chmod 715 test0")).toBe(false);
  });
});
