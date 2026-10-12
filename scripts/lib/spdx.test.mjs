import { describe, expect, it } from "vitest";
import { isAllowed } from "./spdx.mjs";

describe("isAllowed", () => {
  it("accepts allowed identifiers and expressions", () => {
    expect(isAllowed("MIT")).toBe(true);
    expect(isAllowed("MIT OR Apache-2.0")).toBe(true);
    expect(isAllowed("(MIT OR GPL-3.0) AND ISC")).toBe(true);
    expect(isAllowed("Apache-2.0 AND (MIT OR ((ISC)))")).toBe(true);
  });

  it("needs one allowed branch for OR and all branches for AND", () => {
    expect(isAllowed("GPL-3.0 OR MIT")).toBe(true);
    expect(isAllowed("MIT AND GPL-3.0")).toBe(false);
    expect(isAllowed("GPL-3.0")).toBe(false);
  });

  it("gives AND precedence over OR", () => {
    expect(isAllowed("GPL-3.0 AND MIT OR ISC")).toBe(true);
    expect(isAllowed("MIT OR ISC AND GPL-3.0")).toBe(true);
    expect(isAllowed("(MIT OR ISC) AND GPL-3.0")).toBe(false);
  });

  it("refuses malformed expressions", () => {
    for (const expression of [
      "",
      "MIT OR",
      "(MIT",
      "(MIT GPL-3.0",
      "(Apache-2.0 SSPL-1.0",
      "(MIT OR ISC GPL-3.0",
      "MIT )",
      "AND",
      "MIT OR OR ISC",
    ]) {
      expect(isAllowed(expression), expression).toBe(false);
    }
  });

  it("refuses what it cannot evaluate", () => {
    expect(isAllowed("GPL-2.0 WITH Classpath-exception-2.0")).toBe(false);
    expect(isAllowed("LicenseRef-Proprietary")).toBe(false);
    expect(isAllowed("SEE LICENSE IN LICENSE.txt")).toBe(false);
    expect(isAllowed("mit")).toBe(false);
  });
});
