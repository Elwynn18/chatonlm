// Jauge de la VRAM : parts des modèles, autres usages, libre, débordement en
// mémoire vive, lectures incohérentes.

import { describe, expect, it } from "vitest";
import type { MachineMemory } from "@/bindings/MachineMemory";
import { runningModel } from "@/test/backend";
import { modelShade, share, vramView } from "./vram";

const GIB = 1024 ** 3;

function machine(...gpus: [number, number | null][]): MachineMemory {
  return {
    gpus: gpus.map(([total, free], index) => ({
      name: `GPU ${index}`,
      vramTotal: total * GIB,
      vramFree: free === null ? null : free * GIB,
    })),
    ramTotal: 32 * GIB,
    ramAvailable: 16 * GIB,
  };
}

describe("vramView", () => {
  it("is unknown without a reading, and says when there is no card", () => {
    expect(vramView(null, [])).toEqual({ kind: "unknown" });
    expect(vramView(undefined, [])).toEqual({ kind: "unknown" });
    expect(vramView(machine(), [])).toEqual({ kind: "noGpu" });
    expect(vramView(machine([0, null]), [])).toEqual({ kind: "noGpu" });
    // Carte non reconnue alors qu'Ollama a mis un modèle en VRAM.
    expect(vramView(machine(), [runningModel("a", { sizeVram: GIB })])).toEqual(
      { kind: "unknown" },
    );
    expect(vramView(machine(), [runningModel("a", { sizeVram: 0 })])).toEqual({
      kind: "noGpu",
    });
  });

  it("adds up the cards, and deduces the other uses from the free memory", () => {
    const view = vramView(machine([8, 3], [8, 7]), [
      runningModel("a", { size: 4 * GIB, sizeVram: 4 * GIB }),
    ]);
    expect(view).toEqual({
      kind: "gauge",
      gpus: ["GPU 0", "GPU 1"],
      total: 16 * GIB,
      models: [{ name: "a", index: 0, bytes: 4 * GIB }],
      other: 2 * GIB,
      used: 6 * GIB,
      free: 10 * GIB,
      spilled: 0,
    });
  });

  it("leaves out the other uses when a card does not tell its free memory", () => {
    const view = vramView(machine([8, 3], [8, null]), [
      runningModel("a", { size: 4 * GIB, sizeVram: 4 * GIB }),
    ]);
    expect(view).toMatchObject({ other: 0, used: 4 * GIB, free: 12 * GIB });
  });

  it("keeps the rank of each model and skips those without VRAM", () => {
    const view = vramView(machine([8, null]), [
      runningModel("cpu", { size: 2 * GIB, sizeVram: 0 }),
      runningModel("gpu", { size: GIB, sizeVram: GIB }),
      runningModel("split", { size: 3 * GIB, sizeVram: 2 * GIB }),
    ]);
    expect(view).toMatchObject({
      models: [
        { name: "gpu", index: 1, bytes: GIB },
        { name: "split", index: 2, bytes: 2 * GIB },
      ],
      // Le modèle sur le processeur et la part qui ne tient pas.
      spilled: 3 * GIB,
    });
  });

  it("never goes negative nor past the total with readings taken apart", () => {
    // Mémoire libre lue avant le chargement : plus de modèles que d'occupé.
    expect(
      vramView(machine([8, 8]), [runningModel("a", { sizeVram: 2 * GIB })]),
    ).toMatchObject({ other: 0, used: 2 * GIB, free: 6 * GIB });
    // Modèles annoncés au-delà du total.
    expect(
      vramView(machine([4, 0]), [
        runningModel("a", { size: 6 * GIB, sizeVram: 6 * GIB }),
      ]),
    ).toMatchObject({ other: 0, used: 4 * GIB, free: 0 });
  });
});

describe("share", () => {
  it("is a percentage of the total, at most 100", () => {
    expect(share(GIB, 4 * GIB)).toBe(25);
    expect(share(8 * GIB, 4 * GIB)).toBe(100);
    expect(share(0, 4 * GIB)).toBe(0);
    expect(share(GIB, 0)).toBe(0);
  });
});

describe("modelShade", () => {
  it("cycles through the shades", () => {
    expect([0, 1, 2, 3].map(modelShade)).toEqual([
      "bg-accent",
      "bg-accent/60",
      "bg-accent/30",
      "bg-accent",
    ]);
  });
});
