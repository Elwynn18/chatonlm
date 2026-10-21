import { describe, expect, it } from "vitest";
import type { Message } from "@/bindings/Message";
import type { ModelInfo } from "@/bindings/ModelInfo";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import {
  chatModel,
  defaultModel,
  paidPlan,
  pickerEntries,
  recommendedNames,
  selectedEntry,
  upstreamCandidate,
} from "./models";

function info(model: string): ModelInfo {
  return {
    model,
    digest: `digest-${model}`,
    size: 1,
    parameterSize: "1B",
    quantizationLevel: "Q4_K_M",
    contextLength: null,
    capabilities: ["completion"],
  };
}

function message(model: string | null, role: "user" | "assistant"): Message {
  return {
    id: 1,
    role,
    content: "",
    thinking: "",
    model,
    createdAt: 0,
    updatedAt: 0,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

const installed = [info("qwen3.5:0.8b"), info("qwen3:0.6b"), info("llama3")];

describe("model selection", () => {
  it("finds the selected model, installed or not", () => {
    expect(selectedEntry(installed, "qwen3:0.6b")?.info?.model).toBe(
      "qwen3:0.6b",
    );
    expect(selectedEntry(installed, "gemma4")).toEqual({
      model: "gemma4",
      info: null,
    });
    expect(selectedEntry(installed, "")).toBeNull();
  });

  it("defaults to the most recently installed model", () => {
    expect(defaultModel(installed)).toBe("qwen3.5:0.8b");
    expect(defaultModel([])).toBeNull();
  });

  it("restores the last model used in a chat", () => {
    expect(
      chatModel([
        message(null, "user"),
        message("qwen3:0.6b", "assistant"),
        message(null, "user"),
        message("llama3", "assistant"),
        message(null, "user"),
      ]),
    ).toBe("llama3");
    expect(chatModel([message(null, "user")])).toBeNull();
  });

  it("filters by substring and lists a missing selected model last", () => {
    const missing = selectedEntry(installed, "gemma4");
    expect(pickerEntries(installed, missing, "").map((e) => e.model)).toEqual([
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
      "gemma4",
    ]);
    expect(
      pickerEntries(installed, missing, "QWEN").map((e) => e.model),
    ).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
    expect(
      pickerEntries(installed, missing, "gem").map((e) => e.model),
    ).toEqual(["gemma4"]);
    // Un modèle installé reste installé pendant une recherche.
    const chosen = selectedEntry(installed, "llama3");
    expect(
      pickerEntries(installed, chosen, "qwen").map((e) => e.model),
    ).toEqual(["qwen3.5:0.8b", "qwen3:0.6b"]);
  });
  it("lists the recommended models first, with their installed copy", () => {
    const tags = [info("llama3"), info("qwen3.5"), info("qwen3:0.6b")];
    const entries = pickerEntries(tags, null, "", ["gemma4", "qwen3.5"]);
    expect(entries).toEqual([
      { model: "gemma4", info: null },
      { model: "qwen3.5", info: tags[1] },
      { model: "llama3", info: tags[0] },
      { model: "qwen3:0.6b", info: tags[2] },
    ]);
    // Une autre étiquette n'est pas la copie installée.
    expect(
      pickerEntries(installed, null, "", ["qwen3.5"]).map((e) => [
        e.model,
        e.info !== null,
      ]),
    ).toEqual([
      ["qwen3.5", false],
      ["qwen3.5:0.8b", true],
      ["qwen3:0.6b", true],
      ["llama3", true],
    ]);
  });

  it("lists each name once, the missing selected model last", () => {
    const recommended = ["gemma4", "qwen3.5"];
    // Choisi et recommandé : à sa place de recommandé, sans doublon.
    expect(
      pickerEntries(
        installed,
        selectedEntry(installed, "gemma4"),
        "",
        recommended,
      ).map((e) => e.model),
    ).toEqual(["gemma4", "qwen3.5", "qwen3.5:0.8b", "qwen3:0.6b", "llama3"]);
    expect(
      pickerEntries(
        installed,
        selectedEntry(installed, "mistral"),
        "",
        recommended,
      ).map((e) => e.model),
    ).toEqual([
      "gemma4",
      "qwen3.5",
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
      "mistral",
    ]);
  });

  it("filters every entry by substring, the name found upstream last", () => {
    const missing = selectedEntry(installed, "mistral");
    const names = (search: string, upstream: string | null = null) =>
      pickerEntries(installed, missing, search, ["gemma4"], upstream).map(
        (e) => e.model,
      );
    expect(names("EMMA")).toEqual(["gemma4"]);
    expect(names("3")).toEqual(["qwen3.5:0.8b", "qwen3:0.6b", "llama3"]);
    expect(names("llama3.2", "llama3.2")).toEqual(["llama3.2"]);
    expect(names("3", "llama3.2")).toEqual([
      "qwen3.5:0.8b",
      "qwen3:0.6b",
      "llama3",
      "llama3.2",
    ]);
    // Déjà listé : pas de doublon.
    expect(names("llama3", "llama3")).toEqual(["llama3"]);
  });

  it("looks a typed name up only when no entry has exactly that name", () => {
    const entries = pickerEntries(installed, null, "", ["gemma4"]);
    expect(upstreamCandidate(entries, "")).toBeNull();
    expect(upstreamCandidate(entries, "   ")).toBeNull();
    expect(upstreamCandidate(entries, " llama3.2 ")).toBe("llama3.2");
    expect(upstreamCandidate(entries, "llama")).toBe("llama");
    // Correspondance exacte, rognée, sans casse : installé ou recommandé.
    expect(upstreamCandidate(entries, "LLAMA3")).toBeNull();
    expect(upstreamCandidate(entries, " qwen3:0.6b ")).toBeNull();
    expect(upstreamCandidate(entries, "Gemma4")).toBeNull();
  });
});

describe("names without case (Ollama resolves them so)", () => {
  const SMOL = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M";
  const smol = "hf.co/bartowski/smollm2-135m-instruct-gguf:q4_k_m";

  it("the selected model is the installed one, with its spelling", () => {
    const models = [info(SMOL), info("qwen3:0.6b")];
    expect(selectedEntry(models, smol)).toEqual({
      model: SMOL,
      info: models[0],
    });
  });

  it("lists a name once, the installed spelling first", () => {
    const models = [info(SMOL)];
    const selected = selectedEntry(models, smol);
    expect(
      pickerEntries(models, selected, "", [smol.toUpperCase()]).map((e) => [
        e.model,
        e.info !== null,
      ]),
    ).toEqual([[SMOL, true]]);
    // Choisi sous une autre casse, non installé : une seule ligne.
    const other = selectedEntry([], "Gemma4");
    expect(
      pickerEntries([], other, "", ["gemma4"]).map((e) => e.model),
    ).toEqual(["gemma4"]);
  });
});

describe("cloud models", () => {
  const RECOMMENDED: RecommendedModel[] = [
    { model: "kimi-k2.6:cloud", requiredPlan: "pro" },
    { model: "gpt-oss:120b-cloud", requiredPlan: "max" },
    { model: "qwen3.5:cloud", requiredPlan: "free" },
    { model: "glm-5.3:cloud", requiredPlan: null },
    { model: "gemma4", requiredPlan: "pro" },
  ];

  it("are recommended only once allowed", () => {
    expect(recommendedNames(RECOMMENDED, false)).toEqual(["gemma4"]);
    expect(recommendedNames(RECOMMENDED, true)).toEqual([
      "kimi-k2.6:cloud",
      "gpt-oss:120b-cloud",
      "qwen3.5:cloud",
      "glm-5.3:cloud",
      "gemma4",
    ]);
  });

  it("show the paid plan they need, never for a local model", () => {
    expect(paidPlan(RECOMMENDED, "kimi-k2.6:cloud")).toBe("pro");
    expect(paidPlan(RECOMMENDED, "KIMI-K2.6:cloud")).toBe("pro");
    expect(paidPlan(RECOMMENDED, "gpt-oss:120b-cloud")).toBe("max");
    expect(paidPlan(RECOMMENDED, "qwen3.5:cloud")).toBeNull();
    expect(paidPlan(RECOMMENDED, "glm-5.3:cloud")).toBeNull();
    expect(paidPlan(RECOMMENDED, "unknown:cloud")).toBeNull();
    expect(paidPlan(RECOMMENDED, "gemma4")).toBeNull();
  });

  it("are never the default model while not allowed", () => {
    const models = [info("mine:my-cloud"), info("qwen3:0.6b")];
    expect(defaultModel(models)).toBe("mine:my-cloud");
    expect(defaultModel(models, false)).toBe("qwen3:0.6b");
    expect(defaultModel([info("mine:my-cloud")], false)).toBeNull();
  });

  it("are not looked up in the registry while not allowed", () => {
    expect(upstreamCandidate([], " kimi-k3:cloud ", false)).toBeNull();
    expect(upstreamCandidate([], " kimi-k3:cloud ")).toBe("kimi-k3:cloud");
    expect(upstreamCandidate([], "llama3.2", false)).toBe("llama3.2");
  });
});
