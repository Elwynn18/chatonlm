import { describe, expect, it } from "vitest";
import { changeCount, diffWords, sideOf, tokenize } from "./textDiff";

/** Générateur pseudo-aléatoire reproductible (mulberry32). */
function random(seed: number) {
  let state = seed;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let value = Math.imul(state ^ (state >>> 15), 1 | state);
    value ^= value + Math.imul(value ^ (value >>> 7), 61 | value);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

const WORDS = [
  "the",
  "teh",
  "cinéma",
  "cinema",
  "allé",
  "allez",
  "l'homme",
  "año",
  "😀",
  "👩‍💻",
  "42",
  ",",
  ".",
  "?",
  "\n",
  "\n\n",
  " ",
  "  ",
  " ",
  "\u202f",
  "👍🏽",
  "👍🏻",
  "🇫🇷",
  "\r\n",
  "- ",
  "`code`",
];

function randomText(next: () => number, words: number) {
  let text = "";
  for (let i = 0; i < words; i += 1) {
    text += WORDS[Math.floor(next() * WORDS.length)];
    if (next() < 0.6) text += " ";
  }
  return text;
}

/** Modifie quelques mots d'un texte (corrections plausibles). */
function mutate(next: () => number, text: string) {
  return tokenize(text)
    .map((token) => {
      const roll = next();
      if (roll < 0.1) return "";
      if (roll < 0.2) return WORDS[Math.floor(next() * WORDS.length)];
      if (roll < 0.25)
        return `${token}${WORDS[Math.floor(next() * WORDS.length)]}`;
      return token;
    })
    .join("");
}

describe("tokenize", () => {
  it("splits words, blanks and other characters, and joins back", () => {
    expect(tokenize("Je suis allez, au cinema !")).toEqual([
      "Je",
      " ",
      "suis",
      " ",
      "allez",
      ",",
      " ",
      "au",
      " ",
      "cinema",
      " ",
      "!",
    ]);
    const text = "l'año 😀\r\n\n- `x` ?";
    expect(tokenize(text).join("")).toBe(text);
    expect(tokenize("")).toEqual([]);
  });
});

describe("tokenize by graphemes", () => {
  it("keeps a grapheme whole: skin tone, flag, ZWJ sequence", () => {
    expect(tokenize("Bravo 👍🏽 🇫🇷👩‍💻!")).toEqual([
      "Bravo",
      " ",
      "👍🏽",
      " ",
      "🇫🇷",
      "👩‍💻",
      "!",
    ]);
    // Lettre suivie d'une marque combinante : un seul mot.
    expect(tokenize("cafe\u0301 ok")).toEqual(["cafe\u0301", " ", "ok"]);
  });
});

describe("diffWords", () => {
  it("marks replaced words, deletion before insertion", () => {
    const segments = diffWords(
      "Je suis allez au cinema hier.",
      "Je suis allé au cinéma hier.",
    );
    expect(segments).toEqual([
      { kind: "same", text: "Je suis " },
      { kind: "del", text: "allez" },
      { kind: "ins", text: "allé" },
      { kind: "same", text: " au " },
      { kind: "del", text: "cinema" },
      { kind: "ins", text: "cinéma" },
      { kind: "same", text: " hier." },
    ]);
    expect(changeCount(segments)).toBe(2);
  });

  it("marks added and removed punctuation and words", () => {
    const segments = diffWords(
      "Hello world how are you",
      "Hello, world, are you?",
    );
    expect(sideOf(segments, "del")).toBe("Hello world how are you");
    expect(sideOf(segments, "ins")).toBe("Hello, world, are you?");
    expect(segments.filter((s) => s.kind === "ins").map((s) => s.text)).toEqual(
      [",", ",", "?"],
    );
    expect(segments.filter((s) => s.kind === "del").map((s) => s.text)).toEqual(
      [" how"],
    );
    expect(changeCount(segments)).toBe(3);
  });

  it("gives one same segment for an unchanged text, nothing for an empty one", () => {
    expect(diffWords("Already fine.\n\nYes.", "Already fine.\n\nYes.")).toEqual(
      [{ kind: "same", text: "Already fine.\n\nYes." }],
    );
    expect(diffWords("", "")).toEqual([]);
    expect(changeCount([])).toBe(0);
  });

  it("handles a text emptied or written from nothing", () => {
    expect(diffWords("abc def", "")).toEqual([
      { kind: "del", text: "abc def" },
    ]);
    expect(diffWords("", "abc")).toEqual([{ kind: "ins", text: "abc" }]);
  });

  it("never splits an emoji between a deletion and an insertion", () => {
    expect(diffWords("Bravo 👍🏽 !", "Bravo 👍🏻 !")).toEqual([
      { kind: "same", text: "Bravo " },
      { kind: "del", text: "👍🏽" },
      { kind: "ins", text: "👍🏻" },
      { kind: "same", text: " !" },
    ]);
    expect(diffWords("Go 🇫🇷", "Go 🇪🇸")).toEqual([
      { kind: "same", text: "Go " },
      { kind: "del", text: "🇫🇷" },
      { kind: "ins", text: "🇪🇸" },
    ]);
  });

  it("a space replaced by another space is neither marked nor counted", () => {
    // U+202F (typographie française) réécrite en espace ordinaire.
    const segments = diffWords(
      "Bonjour\u202f! Ça va\u00a0?",
      "Bonjour ! Ça va ?",
    );
    expect(segments).toEqual([
      { kind: "same", text: "Bonjour" },
      { kind: "space", text: " ", before: "\u202f" },
      { kind: "same", text: "! Ça va" },
      { kind: "space", text: " ", before: "\u00a0" },
      { kind: "same", text: "?" },
    ]);
    expect(changeCount(segments)).toBe(0);
    expect(sideOf(segments, "del")).toBe("Bonjour\u202f! Ça va\u00a0?");
    expect(sideOf(segments, "ins")).toBe("Bonjour ! Ça va ?");
    // Espace en moins ou saut de ligne : un vrai changement.
    expect(changeCount(diffWords("a  b", "a b"))).toBe(1);
    expect(changeCount(diffWords("a\nb", "a b"))).toBe(1);
  });

  it("property: unchanged text is all same; both texts are rebuilt", () => {
    const next = random(1234);
    for (let round = 0; round < 300; round += 1) {
      const before = randomText(next, Math.floor(next() * 60));
      const same = diffWords(before, before);
      expect(same.every((segment) => segment.kind === "same")).toBe(true);
      expect(sideOf(same, "del")).toBe(before);

      const after =
        next() < 0.2
          ? randomText(next, Math.floor(next() * 60))
          : mutate(next, before);
      const segments = diffWords(before, after);
      expect(sideOf(segments, "del"), JSON.stringify([before, after])).toBe(
        before,
      );
      expect(sideOf(segments, "ins"), JSON.stringify([before, after])).toBe(
        after,
      );
      // Jamais deux segments voisins de même nature, jamais de segment vide.
      for (let i = 0; i < segments.length; i += 1) {
        expect(segments[i].text).not.toBe("");
        // Aucun graphème coupé (teinte ou ZWJ détachés de leur emoji).
        expect(/^[\p{Emoji_Modifier}\u200d]/u.test(segments[i].text)).toBe(
          false,
        );
        if (i > 0) expect(segments[i].kind).not.toBe(segments[i - 1].kind);
      }
    }
  });

  it("stays fast on a long passage", () => {
    const next = random(99);
    const before = randomText(next, 600);
    const after = mutate(next, before);
    const start = performance.now();
    const segments = diffWords(before, after);
    expect(performance.now() - start).toBeLessThan(1000);
    expect(sideOf(segments, "ins")).toBe(after);
  });
});
