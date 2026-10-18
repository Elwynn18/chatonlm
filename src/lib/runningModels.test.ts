import { describe, expect, it } from "vitest";
import type { RunningModel } from "@/bindings/RunningModel";
import { translator } from "@/i18n";
import {
  contextText,
  cpuShare,
  expiryText as expiryIn,
  KEEP_ALIVE_STEPS,
  keepAliveLabel,
  loadedSummary as loadedSummaryIn,
  processorText as processorTextIn,
  runningDetails as runningDetailsIn,
  sameRunningModel,
} from "./runningModels";

const GB = 1024 ** 3;
const NOW = 1_800_000_000_000;

// Textes en anglais (langue source) ; le français et l'espagnol à part.
const en = translator("en");
const fr = translator("fr");
const es = translator("es");
const processorText = (size: number, sizeVram: number) =>
  processorTextIn(en, size, sizeVram);
const expiryText = (m: RunningModel, now: number) => expiryIn(en, m, now);
const runningDetails = (m: RunningModel, now: number) =>
  runningDetailsIn(en, m, now);
const loadedSummary = (models: RunningModel[]) => loadedSummaryIn(en, models);

function model(fields: Partial<RunningModel> = {}): RunningModel {
  return {
    name: "qwen3:0.6b",
    size: GB,
    sizeVram: GB,
    contextLength: 4096,
    expiresAt: null,
    kept: false,
    parameterSize: "751.63M",
    quantizationLevel: "Q4_K_M",
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
      size: 4 * GB,
      sizeVram: 3 * GB,
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
    expect(loadedSummaryIn(es, [model(), model({ size: 3 * GB })])).toBe(
      "2 modelos cargados · 4,0 GB en memoria",
    );
  });

  it("rounds the processor split like ollama ps", () => {
    expect(processorText(GB, GB)).toBe("100% GPU");
    expect(processorText(GB, 0)).toBe("100% CPU");
    expect(processorText(4 * GB, 3 * GB)).toBe("25%/75% CPU/GPU");
    expect(processorText(3, 1)).toBe("67%/33% CPU/GPU");
    expect(processorText(1000, 995)).toBe("1%/99% CPU/GPU");
    // Incohérent : ollama ps dit « Unknown », rien n'est montré.
    expect(processorText(GB, 2 * GB)).toBeNull();
    expect(processorText(0, 0)).toBe("100% CPU");
  });

  it("gives the processor share of a model split with the graphics card", () => {
    const split = model({ size: 4 * GB, sizeVram: 3 * GB });
    expect(cpuShare([split], "qwen3:0.6b")).toBe(25);
    // Nom des réglages, sans « :latest » ni la même casse.
    expect(
      cpuShare(
        [model({ name: "llama3.2:latest", size: 3, sizeVram: 1 })],
        "Llama3.2",
      ),
    ).toBe(67);
    // Jamais 0 ni 100 : une part reste de chaque côté.
    expect(cpuShare([model({ size: 1000, sizeVram: 999 })], "qwen3:0.6b")).toBe(
      1,
    );
    expect(cpuShare([model({ size: 1000, sizeVram: 1 })], "qwen3:0.6b")).toBe(
      99,
    );
    // Tout sur la carte, tout sur le processeur, incohérent, absent.
    expect(cpuShare([model()], "qwen3:0.6b")).toBeNull();
    expect(cpuShare([model({ sizeVram: 0 })], "qwen3:0.6b")).toBeNull();
    expect(cpuShare([model({ sizeVram: 2 * GB })], "qwen3:0.6b")).toBeNull();
    expect(
      cpuShare([model({ size: 0, sizeVram: 0 })], "qwen3:0.6b"),
    ).toBeNull();
    expect(cpuShare([split], "gemma3:270m")).toBeNull();
    expect(cpuShare([], "qwen3:0.6b")).toBeNull();
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
          size: 4 * GB,
          sizeVram: 3 * GB,
          contextLength: 32768,
          expiresAt: NOW + 4.5 * 60_000,
        }),
        NOW,
      ),
    ).toBe("4.0 GB · 25%/75% CPU/GPU · 32k context · Unloads in 4 minutes");
    expect(
      runningDetails(
        model({ sizeVram: 2 * GB, contextLength: null, expiresAt: null }),
        NOW,
      ),
    ).toBe("1.0 GB");
  });

  it("sums up the loaded models", () => {
    expect(loadedSummary([model()])).toBe("1 model loaded · 1.0 GB in memory");
    expect(loadedSummary([model(), model({ size: 3 * GB })])).toBe(
      "2 models loaded · 4.0 GB in memory",
    );
  });

  it("matches the names of /api/ps with the installed ones", () => {
    expect(sameRunningModel("qwen3:latest", "qwen3")).toBe(true);
    expect(sameRunningModel("qwen3", "QWEN3:latest")).toBe(true);
    expect(sameRunningModel("qwen3:0.6b", "qwen3")).toBe(false);
    expect(
      sameRunningModel(
        "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:latest",
        "hf.co/bartowski/smollm2-135m-instruct-gguf",
      ),
    ).toBe(true);
    // Le port d'un registre n'est pas une étiquette.
    expect(
      sameRunningModel(
        "localhost:5000/library/m:latest",
        "localhost:5000/library/m",
      ),
    ).toBe(true);
    expect(sameRunningModel("localhost:5000/m", "localhost:5000/m:v1")).toBe(
      false,
    );
  });
});
