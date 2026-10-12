import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BEFORE_QUIT_LIMIT_MS, onBeforeQuit, quit } from "./quit";

afterEach(() => {
  clearMocks();
  vi.useRealTimers();
});

describe("quit", () => {
  it("saves what waits, then quits; a removed task is not run", async () => {
    const order: string[] = [];
    mockIPC((command) => {
      order.push(command);
      return null;
    });
    const stop = onBeforeQuit(async () => {
      await new Promise((resolve) => setTimeout(resolve, 10));
      order.push("draft saved");
    });
    const removed = vi.fn(async () => {});
    onBeforeQuit(removed)();
    // Une tâche qui échoue n'empêche pas de quitter.
    const stopFailing = onBeforeQuit(async () => {
      throw new Error("disk full");
    });
    try {
      await quit();
    } finally {
      stop();
      stopFailing();
    }
    expect(order).toEqual(["draft saved", "quit_app"]);
    expect(removed).not.toHaveBeenCalled();
  });

  it("does not wait more than the limit", async () => {
    vi.useFakeTimers();
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      return null;
    });
    const stop = onBeforeQuit(() => new Promise(() => {}));
    try {
      const done = quit();
      await vi.advanceTimersByTimeAsync(BEFORE_QUIT_LIMIT_MS - 1);
      expect(calls).toEqual([]);
      await vi.advanceTimersByTimeAsync(1);
      await done;
      expect(calls).toEqual(["quit_app"]);
    } finally {
      stop();
    }
  });
});
