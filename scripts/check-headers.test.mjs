import { describe, expect, it } from "vitest";
import { headerErrors, MARKER, parseListed } from "./check-headers.mjs";

const A = "app/ui/app/src/components/A.tsx";
const B = "app/ui/app/src/components/B.tsx";
const C = "app/ui/app/src/components/C.tsx";

const NOTICES = `== Part 1. Ollama ==

Files (ChatonLM path <- Ollama path : copied | adapted | translated to Rust):
  src/a.tsx <- ${A} : adapted
  src/b.tsx <- ${B} : adapted
  src/b.tsx <- ${C} : adapted
  src/c.ts <- app/c.ts : rewritten

== Part 2. Other third-party material ==

  src/d.ts <- app/d.ts : copied
`;

/** En-tête d'attribution qui nomme les fichiers d'Ollama donnés. */
function header(...origins) {
  return `/*\n * SPDX-License-Identifier: MIT\n * ${MARKER}\n *\n * Adapted from Ollama (https://github.com/ollama/ollama),\n * files ${origins.join(" and\n * ")}, commit 42e911bc3d05.\n */\n`;
}

describe("parseListed", () => {
  it("keeps every Ollama file of a file listed on several lines", () => {
    const { listed } = parseListed(NOTICES);
    expect(listed.get("src/b.tsx")).toEqual([B, C]);
    expect(listed.get("src/a.tsx")).toEqual([A]);
  });

  it("reports malformed lines and reads part 1 only", () => {
    const { listed, errors } = parseListed(NOTICES);
    expect(errors).toEqual([
      "THIRD_PARTY_LICENSES : ligne mal formée « src/c.ts <- app/c.ts : rewritten »",
    ]);
    expect([...listed.keys()]).toEqual(["src/a.tsx", "src/b.tsx"]);
  });
});

describe("headerErrors", () => {
  it("accepts a header that names every Ollama file", () => {
    expect(headerErrors("src/b.tsx", [B, C], header(B, C))).toEqual([]);
  });

  it("reports each Ollama file missing from the header", () => {
    expect(headerErrors("src/b.tsx", [B, C], header(C))).toEqual([
      `src/b.tsx : en-tête d'attribution manquant ou sans « ${B} »`,
    ]);
  });

  it("reports a header without the Ollama copyright line", () => {
    const content = header(A).replace(MARKER, "");
    expect(headerErrors("src/a.tsx", [A], content)).toEqual([
      `src/a.tsx : en-tête d'attribution manquant ou sans « ${A} »`,
    ]);
  });
});
