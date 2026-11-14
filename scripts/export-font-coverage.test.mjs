// La table des caractères des polices du PDF (src/lib/export/coverage.ts)
// suit les fichiers de Fontsource : relevée par fontkit (dépendance de
// pdfkit), elle doit être identique. En cas d'écart (paquet de polices mis à
// jour), l'essai donne les plages attendues.
import { createRequire } from "node:module";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { COVERAGE } from "../src/lib/export/coverage.ts";

const require = createRequire(import.meta.url);
const fontkit = require("fontkit");
const root = path.resolve(import.meta.dirname, "..");
const files = (pkg, names) =>
  names.map((name) =>
    path.join(root, "node_modules/@fontsource", pkg, "files", name),
  );
const STYLES = ["400-normal", "400-italic", "700-normal", "700-italic"];
const FONTS = {
  sans: files(
    "figtree",
    STYLES.map((s) => `figtree-latin-${s}.woff`),
  ),
  sansExt: files(
    "figtree",
    STYLES.map((s) => `figtree-latin-ext-${s}.woff`),
  ),
  mono: files("jetbrains-mono", ["jetbrains-mono-latin-400-normal.woff"]),
  monoExt: files("jetbrains-mono", [
    "jetbrains-mono-latin-ext-400-normal.woff",
  ]),
};

/** Plages d'un ensemble de points de code, au format de coverage.ts. */
function ranges(codes) {
  const out = [];
  for (const code of [...codes].sort((a, b) => a - b)) {
    const last = out.at(-1);
    if (last && last[1] === code - 1) last[1] = code;
    else out.push([code, code]);
  }
  return out.map(([start, end]) => (start === end ? start : [start, end]));
}

describe("export font coverage", () => {
  for (const [family, list] of Object.entries(FONTS)) {
    it(`matches the ${family} font files, the same in every style`, () => {
      const sets = list.map(
        (file) => new Set(fontkit.openSync(file).characterSet),
      );
      for (const set of sets) expect(ranges(set)).toEqual(ranges(sets[0]));
      expect(COVERAGE[family]).toEqual(ranges(sets[0]));
    });
  }
});
