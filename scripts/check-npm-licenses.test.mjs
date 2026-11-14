import { describe, expect, it } from "vitest";
import {
  checkPrebuilt,
  effectiveLicense,
  PREBUILT,
  regionPackages,
  sourceMapPackages,
} from "./check-npm-licenses.mjs";
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

describe("checkPrebuilt", () => {
  it("knows every package compiled into the pdfmake and docx builds", () => {
    const { errors, notes } = checkPrebuilt();
    expect(errors).toEqual([]);
    expect(notes).toEqual([]);
    const pdfmake = sourceMapPackages(
      "node_modules/pdfmake/build/pdfmake.js.map",
    );
    expect(pdfmake).toContain("pdfkit");
    expect(pdfmake).toContain("fontkit");
    expect(pdfmake).toContain("core-js");
    expect([...pdfmake].filter((name) => name.startsWith("?"))).toEqual([]);
    expect(regionPackages("node_modules/docx/dist/index.mjs")).toEqual(
      new Set(["docx", "@oxc-project/runtime", "fflate", "nanoid"]),
    );
  });

  it("refuses an unknown package, a refused license or another version", () => {
    const [key, entry] = Object.entries(PREBUILT)[0];
    const withoutTslib = Object.fromEntries(
      Object.entries(entry.packages).filter(([name]) => name !== "tslib"),
    );
    const unknown = checkPrebuilt({
      [key]: { ...entry, packages: withoutTslib },
    });
    expect(unknown.errors).toEqual([
      `Paquet compilé dans ${key} non relu : tslib (PREBUILT de scripts/check-npm-licenses.mjs)`,
    ]);
    const refused = checkPrebuilt({
      [key]: { ...entry, packages: { ...entry.packages, tslib: "GPL-3.0" } },
    });
    expect(refused.errors).toEqual([
      `Licence refusée (compilé dans ${key}) : tslib (GPL-3.0)`,
    ]);
    const name = key.slice(0, key.lastIndexOf("@"));
    const other = checkPrebuilt({ [`${name}@0.0.1`]: entry });
    expect(other.errors).toHaveLength(1);
    expect(other.errors[0]).toMatch(/à relire/);
    const extra = checkPrebuilt({
      [key]: { ...entry, packages: { ...entry.packages, "left-pad": "MIT" } },
    });
    expect(extra.notes).toEqual([
      `Paquet absent de ${key}, à retirer de PREBUILT : left-pad`,
    ]);
  });
});
