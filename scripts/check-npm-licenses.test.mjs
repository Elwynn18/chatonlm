import { describe, expect, it } from "vitest";
import { effectiveLicense } from "./check-npm-licenses.mjs";
import { isAllowed } from "./lib/spdx.mjs";

describe("effectiveLicense", () => {
  it("accepts khroma 2.1.0, whose license file was checked by hand", () => {
    expect(effectiveLicense("khroma", "2.1.0", "")).toBe("MIT");
    expect(isAllowed(effectiveLicense("khroma", "2.1.0", ""))).toBe(true);
  });

  it("pins the exception to the exact version", () => {
    expect(effectiveLicense("khroma", "2.1.1", "")).toBe("");
    expect(isAllowed(effectiveLicense("khroma", "2.1.1", ""))).toBe(false);
  });

  it("keeps the declared license when there is one", () => {
    expect(effectiveLicense("khroma", "2.1.0", "GPL-3.0")).toBe("GPL-3.0");
    expect(effectiveLicense("katex", "0.16.47", "MIT")).toBe("MIT");
    expect(effectiveLicense("other", "1.0.0", "")).toBe("");
  });
});
