import { afterEach, describe, expect, it, vi } from "vitest";
import { applyTheme, isDark } from "./useTheme";

function stubSystem(dark: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: dark && query === "(prefers-color-scheme: dark)",
    media: query,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.documentElement.className = "";
  document.documentElement.style.colorScheme = "";
});

describe("theme", () => {
  it("forced themes ignore the system", () => {
    expect(isDark("dark", false)).toBe(true);
    expect(isDark("light", true)).toBe(false);
  });

  it("system theme follows the system", () => {
    expect(isDark("system", true)).toBe(true);
    expect(isDark("system", false)).toBe(false);
  });

  it("applies the class and the color scheme", () => {
    const root = document.documentElement;
    stubSystem(true);
    applyTheme("light");
    expect(root.classList.contains("dark")).toBe(false);
    expect(root.style.colorScheme).toBe("light");

    applyTheme("system");
    expect(root.classList.contains("dark")).toBe(true);
    expect(root.style.colorScheme).toBe("light dark");

    stubSystem(false);
    applyTheme("dark");
    expect(root.classList.contains("dark")).toBe(true);
    expect(root.style.colorScheme).toBe("dark");
  });
});
