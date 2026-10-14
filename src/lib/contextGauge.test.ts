// Calculs de la jauge du contexte (propre à ChatonLM) ; son affichage et son
// panneau sont essayés dans l'app (src/test/contextCompaction.test.tsx).

import { describe, expect, it } from "vitest";
import {
  FULL_FRACTION,
  sectorPath,
  usedFraction,
  usedPercent,
} from "@/lib/contextGauge";

describe("usedFraction", () => {
  it("is used / size, bounded to [0, 1]", () => {
    expect(usedFraction({ used: 12_400, size: 32_768 })).toBeCloseTo(0.378);
    expect(usedFraction({ used: 40_000, size: 32_768 })).toBe(1);
    expect(usedFraction({ used: 0, size: 32_768 })).toBe(0);
    // Taille nulle (jamais annoncée par le moteur) : rien d'utilisé.
    expect(usedFraction({ used: 10, size: 0 })).toBe(0);
  });
});

describe("sectorPath", () => {
  it("starts at noon and turns clockwise", () => {
    // Un quart : de midi à trois heures, petit arc.
    expect(sectorPath(0.25)).toBe("M 10 10 L 10 2 A 8 8 0 0 1 18.000 10.000 Z");
    // Trois quarts : jusqu'à neuf heures, grand arc.
    expect(sectorPath(0.75)).toBe("M 10 10 L 10 2 A 8 8 0 1 1 2.000 10.000 Z");
  });
});

describe("usedPercent", () => {
  it("rounds down, so that the threshold and 100% are only shown once reached", () => {
    expect(usedPercent({ used: 795, size: 1000 })).toBe(79);
    expect(usedPercent({ used: 995, size: 1000 })).toBe(99);
    expect(usedPercent({ used: 800, size: 1000 })).toBe(80);
    expect(usedPercent({ used: 29, size: 100 })).toBe(29);
    expect(usedPercent({ used: 1200, size: 1000 })).toBe(100);
    expect(usedPercent({ used: 10, size: 0 })).toBe(0);
  });
});

describe("FULL_FRACTION", () => {
  it("is where the sector would collapse into an empty arc", () => {
    // À 99,9 % et plus, départ et arrivée de l'arc sont presque confondus.
    expect(FULL_FRACTION).toBeLessThan(1);
    expect(FULL_FRACTION).toBeGreaterThanOrEqual(0.999);
  });
});
