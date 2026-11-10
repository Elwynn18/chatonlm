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
    // Carte non reconnue alors que le moteur a mis un modèle en VRAM.
    expect(
      vramView(machine(), [runningModel("a", { vramBytes: GIB })]),
    ).toEqual({ kind: "unknown" });
    expect(vramView(machine(), [runningModel("a", { vramBytes: 0 })])).toEqual({
      kind: "noGpu",
    });
  });

  it("adds up the cards, and deduces the other uses from the free memory", () => {
    const view = vramView(machine([8, 3], [8, 7]), [
      runningModel("a", { sizeBytes: 4 * GIB, vramBytes: 4 * GIB }),
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
      runningModel("a", { sizeBytes: 4 * GIB, vramBytes: 4 * GIB }),
    ]);
    expect(view).toMatchObject({ other: 0, used: 4 * GIB, free: 12 * GIB });
  });

  it("keeps the rank of each model and skips those without VRAM", () => {
    const view = vramView(machine([8, null]), [
      runningModel("cpu", {
        sizeBytes: 2 * GIB,
        vramBytes: 0,
        cpuOnly: true,
        gpuLayers: 0,
      }),
      runningModel("gpu", { sizeBytes: GIB, vramBytes: GIB }),
      runningModel("split", {
        sizeBytes: 3 * GIB,
        vramBytes: 2 * GIB,
        gpuLayers: 20,
        totalLayers: 28,
      }),
    ]);
    expect(view).toMatchObject({
      models: [
        { name: "gpu", index: 1, bytes: GIB },
        { name: "split", index: 2, bytes: 2 * GIB },
      ],
      // Seule la part du modèle partagé : le processeur seul est choisi (ou
      // un repli, dit par l'escargot), pas un manque de VRAM.
      spilled: GIB,
    });
  });

  it("only counts as spilled the layers left on the processor", () => {
    const MIB = 1024 ** 2;
    // Tout sur la carte (29/29) : llama.cpp garde pourtant la table des
    // jetons et des tampons en mémoire vive (journal réel de Qwen3-0.6B).
    const allOnCard = runningModel("all", {
      sizeBytes: GIB + 134 * MIB,
      vramBytes: GIB,
      gpuLayers: 29,
      totalLayers: 29,
    });
    expect(vramView(machine([8, null]), [allOnCard])).toMatchObject({
      spilled: 0,
    });
    // En chargement : estimation, rien encore sur la carte.
    const loading = runningModel("loading", {
      state: "loading",
      sizeBytes: 5 * GIB,
      vramBytes: 0,
      gpuLayers: null,
      totalLayers: null,
    });
    expect(vramView(machine([8, null]), [loading])).toMatchObject({
      spilled: 0,
    });
    // 20 couches sur 29 : la part restée en mémoire vive.
    const split = runningModel("split", {
      sizeBytes: 6 * GIB,
      vramBytes: 4 * GIB,
      gpuLayers: 20,
      totalLayers: 29,
    });
    expect(
      vramView(machine([8, null]), [allOnCard, loading, split]),
    ).toMatchObject({ spilled: 2 * GIB });
    // Réservé au-delà de la carte (pilote qui déborde en mémoire partagée).
    expect(
      vramView(machine([4, null]), [
        runningModel("big", { sizeBytes: 6 * GIB, vramBytes: 6 * GIB }),
      ]),
    ).toMatchObject({ spilled: 2 * GIB });
  });

  it("never goes negative nor past the total with readings taken apart", () => {
    // Mémoire libre lue avant le chargement : plus de modèles que d'occupé.
    expect(
      vramView(machine([8, 8]), [runningModel("a", { vramBytes: 2 * GIB })]),
    ).toMatchObject({ other: 0, used: 2 * GIB, free: 6 * GIB });
    // Modèles annoncés au-delà du total.
    expect(
      vramView(machine([4, 0]), [
        runningModel("a", { sizeBytes: 6 * GIB, vramBytes: 6 * GIB }),
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
