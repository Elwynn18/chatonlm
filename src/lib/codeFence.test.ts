import { describe, expect, it } from "vitest";
import { insideOpenFence, splitFences, splitInlineCode } from "./codeFence";

describe("insideOpenFence", () => {
  it("is open after an opening fence, closed after its closing fence", () => {
    expect(insideOpenFence("")).toBe(false);
    expect(insideOpenFence("Look:\n```")).toBe(true);
    expect(insideOpenFence("```python")).toBe(true);
    expect(insideOpenFence("```python\nprint(1)")).toBe(true);
    expect(insideOpenFence("```python\nprint(1)\n```")).toBe(false);
    expect(insideOpenFence("```\na\n```\n\n```js\nb")).toBe(true);
  });

  it("follows CommonMark: start of line, longer fences, one-line code", () => {
    // Au milieu d'une ligne, ou après quatre espaces : pas une clôture.
    expect(insideOpenFence("see ```")).toBe(false);
    expect(insideOpenFence("    ```")).toBe(false);
    expect(insideOpenFence("   ```")).toBe(true);
    // « ```code``` » sur une ligne : du code en ligne.
    expect(insideOpenFence("```code```")).toBe(false);
    // Une clôture plus courte ne referme pas, une plus longue si.
    expect(insideOpenFence("````\n```")).toBe(true);
    expect(insideOpenFence("```\n````")).toBe(false);
    // Une ligne « ```js » dans un bloc ouvert ne le referme pas.
    expect(insideOpenFence("```\n```js")).toBe(true);
  });

  it("only counts the text before the caret", () => {
    const text = "```\ncode\n```";
    expect(insideOpenFence(text, text.length)).toBe(false);
    expect(insideOpenFence(text, "```\ncode".length)).toBe(true);
    expect(insideOpenFence(text, 0)).toBe(false);
  });
});

describe("splitFences", () => {
  it("separates text and code blocks, without the fence lines", () => {
    expect(
      splitFences("Why?\n```python\nprint(1)\n  x = 2\n```\nThanks"),
    ).toEqual([
      { kind: "text", text: "Why?" },
      { kind: "code", language: "python", code: "print(1)\n  x = 2\n" },
      { kind: "text", text: "Thanks" },
    ]);
  });

  it("keeps plain text whole, and an unclosed block runs to the end", () => {
    expect(splitFences("a\n  b\n# c")).toEqual([
      { kind: "text", text: "a\n  b\n# c" },
    ]);
    expect(splitFences("```\nopen\nstill")).toEqual([
      { kind: "code", language: "", code: "open\nstill\n" },
    ]);
    expect(splitFences("```rust\n```\n")).toEqual([
      { kind: "code", language: "rust", code: "" },
    ]);
  });
});

describe("splitInlineCode", () => {
  it("finds code between matching backtick runs", () => {
    expect(splitInlineCode("run `ls -la` now")).toEqual([
      { kind: "text", text: "run " },
      { kind: "code", text: "ls -la" },
      { kind: "text", text: " now" },
    ]);
    expect(splitInlineCode("`` a ` b ``")).toEqual([
      { kind: "code", text: "a ` b" },
    ]);
    expect(splitInlineCode("```x```")).toEqual([{ kind: "code", text: "x" }]);
  });

  it("leaves an unmatched backtick as text", () => {
    expect(splitInlineCode("it`s `` fine")).toEqual([
      { kind: "text", text: "it`s `` fine" },
    ]);
    expect(splitInlineCode("``")).toEqual([{ kind: "text", text: "``" }]);
  });
});
