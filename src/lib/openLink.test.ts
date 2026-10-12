import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const openExternal = vi.hoisted(() => vi.fn());
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openExternal,
}));

import {
  dismissLinkProblem,
  linkProblem,
  openLink,
  REPEAT_MS,
  reportCopyFailure,
  resetLinks,
  subscribeLinkProblem,
  webUrl,
} from "./openLink";

const NO_BROWSER = {
  kind: "system",
  message: "no application is set to open web links (xdg-open exited with 3)",
};

beforeEach(() => {
  vi.useFakeTimers({ now: 100_000 });
  openExternal.mockReset();
  openExternal.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
  resetLinks();
});

describe("webUrl", () => {
  it("accepts http and https, local addresses included", () => {
    expect(webUrl("https://ollama.com")).toBe("https://ollama.com/");
    expect(webUrl("http://localhost:11434/api/tags")).toBe(
      "http://localhost:11434/api/tags",
    );
    expect(webUrl("HTTPS://Ollama.com/library?q=qwen#top")).toBe(
      "https://ollama.com/library?q=qwen#top",
    );
  });

  it("refuses everything else", () => {
    for (const href of [
      undefined,
      "",
      "javascript:alert(1)",
      "mailto:someone@example.com",
      "file:///etc/passwd",
      "data:text/html,hi",
      "tauri://localhost/c/new",
      "ftp://example.com",
      "/c/123",
      "#top",
      "ollama.com",
    ]) {
      expect(webUrl(href), String(href)).toBeNull();
    }
  });
});

describe("openLink", () => {
  it("opens the address through the backend", async () => {
    await openLink("https://ollama.com/");
    expect(openExternal).toHaveBeenCalledWith("https://ollama.com/");
    expect(linkProblem()).toBeNull();
  });

  it("ignores the same address clicked again within a second", async () => {
    await openLink("https://ollama.com/");
    vi.setSystemTime(100_000 + REPEAT_MS - 1);
    await openLink("https://ollama.com/");
    expect(openExternal).toHaveBeenCalledOnce();

    // Une autre adresse passe tout de suite.
    await openLink("https://ollama.com/library");
    expect(openExternal).toHaveBeenCalledTimes(2);

    vi.setSystemTime(100_000 + 2 * REPEAT_MS);
    await openLink("https://ollama.com/");
    expect(openExternal).toHaveBeenCalledTimes(3);
  });

  it("reports a failure with the backend message", async () => {
    openExternal.mockRejectedValue(NO_BROWSER);
    const listener = vi.fn();
    const unsubscribe = subscribeLinkProblem(listener);

    await expect(openLink("https://ollama.com/")).resolves.toBeUndefined();

    expect(listener).toHaveBeenCalledOnce();
    expect(linkProblem()).toEqual({
      id: expect.any(Number),
      kind: "open",
      url: "https://ollama.com/",
      detail: NO_BROWSER.message,
    });
    unsubscribe();
  });

  it("clears the notice once a link opens", async () => {
    openExternal.mockRejectedValueOnce(NO_BROWSER);
    await openLink("https://ollama.com/");
    expect(linkProblem()).not.toBeNull();

    await openLink("https://ollama.com/library");
    expect(linkProblem()).toBeNull();
  });
});

describe("notice", () => {
  it("closes only the notice it was asked to close", () => {
    reportCopyFailure("https://ollama.com/");
    const first = linkProblem();
    reportCopyFailure("https://ollama.com/library");
    const second = linkProblem();
    expect(second).toMatchObject({
      kind: "copy",
      url: "https://ollama.com/library",
    });

    dismissLinkProblem(first!.id);
    expect(linkProblem()).toBe(second);
    dismissLinkProblem(second!.id);
    expect(linkProblem()).toBeNull();
  });
});
