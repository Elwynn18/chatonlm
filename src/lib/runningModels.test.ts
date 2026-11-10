import { describe, expect, it } from "vitest";
import type { EngineRunningModel } from "@/bindings/EngineRunningModel";
import { translator } from "@/i18n";
import {
  contextText,
  expiryText as expiryIn,
  KEEP_ALIVE_STEPS,
  keepAliveLabel,
  loadedSummary as loadedSummaryIn,
  processorText as processorTextIn,
  runningDetails as runningDetailsIn,
  runningOf,
  slowRun,
} from "./runningModels";

const GB = 1024 ** 3;
const NOW = 1_800_000_000_000;
const QWEN = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";

// Textes en anglais (langue source) ; le français et l'espagnol à part.
const en = translator("en");
const fr = translator("fr");
const es = translator("es");
const processorText = (m: EngineRunningModel) => processorTextIn(en, m);
const expiryText = (m: EngineRunningModel, now: number) => expiryIn(en, m, now);
const runningDetails = (m: EngineRunningModel, now: number) =>
  runningDetailsIn(en, m, now);
const loadedSummary = (models: EngineRunningModel[]) =>
  loadedSummaryIn(en, models);

function model(fields: Partial<EngineRunningModel> = {}): EngineRunningModel {
  return {
    id: QWEN,
    state: "ready",
    sizeBytes: GB,
    vramBytes: GB,
    contextLength: 4096,
    expiresAt: null,
    kept: false,
    inUse: false,
    gpuLayers: 29,
    totalLayers: 29,
    cpuFallback: false,
    notice: null,
    cpuOnly: false,
    paramsLabel: "0.6B",
    ...fields,
  };
}

describe("running models", () => {
  it("has the engine's keep-alive steps, in slider order", () => {
    expect(KEEP_ALIVE_STEPS.map((step) => step.seconds)).toEqual([
      0, 60, 300, 900, 1800, 3600, 14400, -1,
    ]);
  });

  it("names each keep-alive step in the language", () => {
    const labels = (t: typeof en) =>
      KEEP_ALIVE_STEPS.map((step) => keepAliveLabel(t, step.seconds));
    expect(labels(en)).toEqual([
      "Default",
      "1 min",
      "5 min",
      "15 min",
      "30 min",
      "1 hour",
      "4 hours",
      "Always",
    ]);
    expect(labels(fr)).toEqual([
      "Par défaut",
      "1 min",
      "5 min",
      "15 min",
      "30 min",
      "1 heure",
      "4 heures",
      "Toujours",
    ]);
    expect(labels(es).slice(5)).toEqual(["1 hora", "4 horas", "Siempre"]);
  });

  it("writes the details and the summary in French and in Spanish", () => {
    const split = model({
      sizeBytes: 4 * GB,
      gpuLayers: 21,
      totalLayers: 28,
      contextLength: 32768,
      expiresAt: NOW + 4.5 * 60_000,
    });
    expect(runningDetailsIn(fr, split, NOW)).toBe(
      "4,0 Go · 25 %/75 % CPU/GPU · contexte de 32k · Déchargement dans 4 minutes",
    );
    expect(expiryIn(es, model({ expiresAt: NOW + 1000 }), NOW)).toBe(
      "Se libera en 1 segundo",
    );
    expect(expiryIn(fr, model({ kept: true }), NOW)).toBe("Reste chargé");
    // Pluriel : « one » et « other », tailles à la façon de la langue.
    expect(loadedSummaryIn(fr, [model()])).toBe(
      "1 modèle chargé · 1,0 Go en mémoire",
    );
    expect(loadedSummaryIn(es, [model(), model({ sizeBytes: 3 * GB })])).toBe(
      "2 modelos cargados · 4,0 GB en memoria",
    );
  });

  it("splits the processor by layers on the graphics card", () => {
    expect(processorText(model())).toBe("100% GPU");
    expect(processorText(model({ gpuLayers: 0 }))).toBe("100% CPU");
    expect(processorText(model({ cpuOnly: true }))).toBe("100% CPU");
    expect(processorText(model({ gpuLayers: 21, totalLayers: 28 }))).toBe(
      "25%/75% CPU/GPU",
    );
    // Jamais 0 ni 100 tant qu'une couche reste de chaque côté.
    expect(processorText(model({ gpuLayers: 199, totalLayers: 200 }))).toBe(
      "1%/99% CPU/GPU",
    );
    expect(processorText(model({ gpuLayers: 1, totalLayers: 200 }))).toBe(
      "99%/1% CPU/GPU",
    );
    // Inconnu (pas de carte, journal illisible) : rien n'est montré.
    expect(processorText(model({ gpuLayers: null }))).toBeNull();
    expect(processorText(model({ totalLayers: 0 }))).toBeNull();
  });

  it("tells why the chosen model is slow, never for a normal run", () => {
    const split = model({ gpuLayers: 21, totalLayers: 28 });
    expect(slowRun([split], QWEN)).toEqual({ kind: "split", cpu: 25 });
    // Quantification sans la casse, comme le moteur.
    expect(slowRun([split], QWEN.replace("Q4_K_M", "q4_k_m"))).toEqual({
      kind: "split",
      cpu: 25,
    });
    // Carte en échec : tout sur le processeur, dit à part.
    expect(
      slowRun(
        [model({ cpuFallback: true, cpuOnly: true, gpuLayers: null })],
        QWEN,
      ),
    ).toEqual({ kind: "fallback" });
    // Tout sur la carte, sur le processeur par choix ou sans carte,
    // en chargement, absent.
    expect(slowRun([model()], QWEN)).toBeNull();
    expect(
      slowRun([model({ cpuOnly: true, gpuLayers: null })], QWEN),
    ).toBeNull();
    expect(
      slowRun([model({ gpuLayers: null, totalLayers: null })], QWEN),
    ).toBeNull();
    expect(slowRun([{ ...split, state: "loading" }], QWEN)).toBeNull();
    expect(slowRun([split], "ggml-org/gemma-3-1b-it-GGUF:Q4_K_M")).toBeNull();
    expect(slowRun([], QWEN)).toBeNull();
  });

  it("writes the context in k when it is a multiple of 1024", () => {
    expect(contextText(32768)).toBe("32k");
    expect(contextText(4096)).toBe("4k");
    expect(contextText(262144)).toBe("256k");
    expect(contextText(8000)).toBe("8000");
    expect(contextText(1536)).toBe("1536");
    expect(contextText(null)).toBeNull();
    expect(contextText(0)).toBeNull();
  });

  it("says when the model will be unloaded", () => {
    const at = (ms: number) => expiryText(model({ expiresAt: NOW + ms }), NOW);
    expect(expiryText(model({ kept: true }), NOW)).toBe("Stays loaded");
    expect(expiryText(model(), NOW)).toBeNull();
    expect(at(-60_000)).toBe("Unloads soon");
    expect(at(0)).toBe("Unloads soon");
    expect(at(999)).toBe("Unloads soon");
    expect(at(1000)).toBe("Unloads in 1 second");
    expect(at(59_999)).toBe("Unloads in 59 seconds");
    expect(at(60_000)).toBe("Unloads in 1 minute");
    expect(at(4 * 60_000 + 59_000)).toBe("Unloads in 4 minutes");
    expect(at(3_599_000)).toBe("Unloads in 59 minutes");
    expect(at(3_600_000)).toBe("Unloads in 1 hour");
    expect(at(5_399_000)).toBe("Unloads in 1 hour");
    expect(at(5_400_000)).toBe("Unloads in 2 hours");
    expect(at(14_399_000)).toBe("Unloads in 4 hours");
    // Gardé : l'échéance éventuelle ne compte pas.
    expect(expiryText(model({ kept: true, expiresAt: NOW - 1 }), NOW)).toBe(
      "Stays loaded",
    );
  });

  it("joins the details, leaving out what is unknown", () => {
    expect(
      runningDetails(
        model({
          sizeBytes: 4 * GB,
          gpuLayers: 21,
          totalLayers: 28,
          contextLength: 32768,
          expiresAt: NOW + 4.5 * 60_000,
        }),
        NOW,
      ),
    ).toBe("4.0 GB · 25%/75% CPU/GPU · 32k context · Unloads in 4 minutes");
    expect(
      runningDetails(
        model({ gpuLayers: null, contextLength: 0, expiresAt: null }),
        NOW,
      ),
    ).toBe("1.0 GB");
    // En chargement : seulement ce qu'il fait.
    expect(runningDetails(model({ state: "loading" }), NOW)).toBe(
      "Loading into memory…",
    );
  });

  it("sums up the loaded models", () => {
    expect(loadedSummary([model()])).toBe("1 model loaded · 1.0 GB in memory");
    expect(loadedSummary([model(), model({ sizeBytes: 3 * GB })])).toBe(
      "2 models loaded · 4.0 GB in memory",
    );
  });

  it("finds a model of the engine by its id", () => {
    const loaded = model();
    expect(runningOf([loaded], QWEN)).toBe(loaded);
    expect(runningOf([loaded], "unsloth/Qwen3-0.6B-GGUF:q4_k_m")).toBe(loaded);
    expect(runningOf([loaded], "unsloth/Qwen3-0.6B-GGUF:Q8_0")).toBeNull();
  });
});
