/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/utils/clipboard.test.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: copyText rejects instead of returning a boolean,
 * globals restored after each test, rich copy (HTML + plain text) tests,
 * focus given back after the execCommand fallback.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { copyRich, copyText } from "./clipboard";

beforeEach(() => {
  vi.restoreAllMocks();
});

// restoreAllMocks ne rend pas les globales remplacées par stubGlobal.
afterEach(() => {
  vi.unstubAllGlobals();
});

/** Document réduit à ce que le repli execCommand utilise. */
function stubDocument(copied: boolean) {
  const textarea = {
    value: "",
    setAttribute: vi.fn(),
    style: {} as Record<string, string>,
    select: vi.fn(),
    remove: vi.fn(),
  };
  const append = vi.fn();
  const execCommand = vi.fn().mockReturnValue(copied);
  vi.stubGlobal("document", {
    createElement: vi.fn().mockReturnValue(textarea),
    body: { append },
    execCommand,
  });
  return { textarea, append, execCommand };
}

/** ClipboardItem réduit : garde les données reçues. */
class FakeClipboardItem {
  constructor(readonly items: Record<string, Blob>) {}
}

describe("copyText", () => {
  it("copies via Clipboard API when available", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", {
      clipboard: {
        writeText,
      },
    });

    await expect(copyText("ollama launch claude")).resolves.toBeUndefined();

    expect(writeText).toHaveBeenCalledWith("ollama launch claude");
  });

  it("falls back to execCommand when Clipboard API fails", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("not allowed"));
    vi.stubGlobal("navigator", {
      clipboard: {
        writeText,
      },
    });
    const { textarea, append, execCommand } = stubDocument(true);

    await expect(copyText("ollama launch openclaw")).resolves.toBeUndefined();

    expect(textarea.value).toBe("ollama launch openclaw");
    expect(execCommand).toHaveBeenCalledWith("copy");
    expect(append).toHaveBeenCalledWith(textarea);
    expect(textarea.remove).toHaveBeenCalled();
  });

  it("gives the focus back to its element after the execCommand fallback", async () => {
    vi.stubGlobal("navigator", {});
    const button = document.createElement("button");
    document.body.append(button);
    button.focus();
    // Comme WebKit (pas jsdom) : select() donne le focus à la zone de texte.
    const select = HTMLTextAreaElement.prototype.select;
    vi.spyOn(HTMLTextAreaElement.prototype, "select").mockImplementation(
      function (this: HTMLTextAreaElement) {
        this.focus();
        select.call(this);
      },
    );
    // jsdom n'a pas execCommand.
    let focusedDuringCopy: Element | null = null;
    const execCommand = vi.fn(() => {
      focusedDuringCopy = document.activeElement;
      return true;
    });
    Object.defineProperty(document, "execCommand", {
      value: execCommand,
      configurable: true,
    });
    try {
      await expect(copyText("ollama pull qwen3")).resolves.toBeUndefined();
      expect(focusedDuringCopy).toBeInstanceOf(HTMLTextAreaElement);
      expect(document.activeElement).toBe(button);
      expect(document.querySelector("textarea")).toBeNull();
    } finally {
      delete (document as { execCommand?: unknown }).execCommand;
      button.remove();
    }
  });

  it("rejects when both ways fail, and still removes the text area", async () => {
    vi.stubGlobal("navigator", {});
    const { textarea, execCommand } = stubDocument(false);

    await expect(copyText("ollama ps")).rejects.toThrow("copy refused");

    expect(execCommand).toHaveBeenCalledWith("copy");
    expect(textarea.remove).toHaveBeenCalled();
  });
});

describe("copyRich", () => {
  it("writes HTML and plain text in one clipboard item", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    const writeText = vi.fn();
    vi.stubGlobal("navigator", { clipboard: { write, writeText } });
    vi.stubGlobal("ClipboardItem", FakeClipboardItem);

    await copyRich("<p><strong>Hi</strong></p>", "**Hi**");

    expect(write).toHaveBeenCalledOnce();
    const [items] = write.mock.calls[0] as [FakeClipboardItem[]];
    expect(items).toHaveLength(1);
    const { "text/html": html, "text/plain": text } = items[0].items;
    expect(html.type).toBe("text/html");
    expect(await html.text()).toBe("<p><strong>Hi</strong></p>");
    expect(text.type).toBe("text/plain");
    expect(await text.text()).toBe("**Hi**");
    expect(writeText).not.toHaveBeenCalled();
  });

  it("writes before awaiting anything, while the click still counts", () => {
    const write = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { write } });
    vi.stubGlobal("ClipboardItem", FakeClipboardItem);

    void copyRich("<p>Hi</p>", "Hi");

    expect(write).toHaveBeenCalledOnce();
  });

  it("falls back to plain text when the rich write is refused", async () => {
    const write = vi.fn().mockRejectedValue(new Error("not allowed"));
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { write, writeText } });
    vi.stubGlobal("ClipboardItem", FakeClipboardItem);

    await expect(copyRich("<p>Hi</p>", "Hi")).resolves.toBeUndefined();

    expect(writeText).toHaveBeenCalledWith("Hi");
  });

  it("falls back to plain text without ClipboardItem", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { write, writeText } });
    vi.stubGlobal("ClipboardItem", undefined);

    await copyRich("<p>Hi</p>", "Hi");

    expect(write).not.toHaveBeenCalled();
    expect(writeText).toHaveBeenCalledWith("Hi");
  });

  it("falls back to execCommand, then rejects when everything fails", async () => {
    vi.stubGlobal("navigator", {});
    const { execCommand } = stubDocument(false);

    await expect(copyRich("<p>Hi</p>", "Hi")).rejects.toThrow("copy refused");

    expect(execCommand).toHaveBeenCalledWith("copy");
  });
});
