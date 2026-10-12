import { beforeEach, describe, expect, it, vi } from "vitest";

const setZoom = vi.fn<(level: number) => Promise<void>>();
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ setZoom }),
}));

const { setZoomLevel, zoomIn, zoomLevel, zoomOut, zoomReset } =
  await import("./zoom");

beforeEach(async () => {
  setZoom.mockResolvedValue(undefined);
  await zoomReset();
  setZoom.mockClear();
});

describe("zoom", () => {
  it("steps by 0.1 between 90 % and 120 %", async () => {
    for (let i = 0; i < 4; i++) await zoomIn();
    expect(zoomLevel()).toBe(1.2);
    expect(setZoom.mock.calls.map(([level]) => level)).toEqual([1.1, 1.2]);
    for (let i = 0; i < 5; i++) await zoomOut();
    expect(zoomLevel()).toBe(0.9);
    expect(setZoom.mock.calls.map(([level]) => level)).toEqual([
      1.1, 1.2, 1.1, 1, 0.9,
    ]);
    await zoomReset();
    expect(zoomLevel()).toBe(1);
    expect(setZoom).toHaveBeenLastCalledWith(1);
  });

  it("does nothing when the level doesn't change", async () => {
    await zoomReset();
    expect(setZoom).not.toHaveBeenCalled();
  });

  it("goes on from the level asked for, before setZoom answers", async () => {
    let answer: () => void = () => {};
    setZoom.mockImplementationOnce(
      () => new Promise<void>((resolve) => (answer = resolve)),
    );
    const first = zoomIn();
    const second = zoomIn();
    answer();
    await Promise.all([first, second]);
    expect(setZoom.mock.calls.map(([level]) => level)).toEqual([1.1, 1.2]);
    expect(zoomLevel()).toBe(1.2);
  });

  it("keeps the previous level when setZoom fails", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    setZoom.mockRejectedValueOnce(new Error("no"));
    await zoomIn();
    expect(zoomLevel()).toBe(1);
    expect(consoleError).toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it("rounds to a tenth", async () => {
    await setZoomLevel(1.0999999);
    expect(zoomLevel()).toBe(1.1);
    await setZoomLevel(5);
    expect(zoomLevel()).toBe(1.2);
  });
});
