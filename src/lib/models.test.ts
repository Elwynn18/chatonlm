import { describe, expect, it } from "vitest";
import type { ChatModel } from "@/bindings/ChatModel";
import type { HubQuant } from "@/bindings/HubQuant";
import type { HubRepo } from "@/bindings/HubRepo";
import type { Message } from "@/bindings/Message";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import {
  chatModel,
  defaultModel,
  pickerEntries,
  recommendedNames,
  restoredModel,
  selectedEntry,
  typedRepo,
  upstreamModel,
} from "./models";

function info(id: string, fields: Partial<ChatModel> = {}): ChatModel {
  return {
    id,
    displayName: id,
    origin: id.startsWith("@remote/") ? "remote" : "hf",
    provider: null,
    sizeBytes: 1,
    paramsLabel: "1B",
    quant: "Q4_K_M",
    contextMax: null,
    capabilities: ["completion"],
    installedAt: 0,
    ...fields,
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

const QWEN35 = "unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M";
const QWEN3 = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
const LLAMA = "@ollama/llama3:latest";
const GEMMA = "ggml-org/gemma-4-E4B-it-GGUF:Q4_0";
const installed = [
  info(QWEN35, { displayName: "Qwen3.5-0.8B" }),
  info(QWEN3, { displayName: "Qwen3-0.6B" }),
  info(LLAMA, { displayName: "llama3", origin: "ollama" }),
];

describe("model selection", () => {
  it("finds the selected model, installed or not", () => {
    expect(selectedEntry(installed, QWEN3)?.info?.id).toBe(QWEN3);
    expect(selectedEntry(installed, GEMMA)).toEqual({
      model: GEMMA,
      info: null,
    });
    expect(selectedEntry(installed, "")).toBeNull();
  });

  it("takes the id of the base for another case of the quantization", () => {
    expect(selectedEntry(installed, QWEN3.replace("Q4_K_M", "q4_k_m"))).toEqual(
      { model: QWEN3, info: installed[1] },
    );
  });

  it("defaults to the most recently installed local model, never a remote one", () => {
    expect(defaultModel(installed)).toBe(QWEN35);
    expect(defaultModel([])).toBeNull();
    const remote = info("@remote/mistral/mistral-small-latest");
    expect(defaultModel([remote, ...installed])).toBe(QWEN35);
    expect(defaultModel([remote])).toBeNull();
  });

  it("restores the last model used in a chat", () => {
    expect(
      chatModel([
        message(null, "user"),
        message(QWEN3, "assistant"),
        message(null, "user"),
        message(LLAMA, "assistant"),
        message(null, "user"),
      ]),
    ).toBe(LLAMA);
    expect(chatModel([message(null, "user")])).toBeNull();
  });

  it("filters by substring of the id or the shown name, the missing selected model last", () => {
    const missing = selectedEntry(installed, GEMMA);
    const ids = (search: string, selected = missing) =>
      pickerEntries(installed, selected, search).map((e) => e.model);
    expect(ids("")).toEqual([QWEN35, QWEN3, LLAMA, GEMMA]);
    expect(ids("QWEN")).toEqual([QWEN35, QWEN3]);
    expect(ids("gem")).toEqual([GEMMA]);
    // Nom montré seulement (« Qwen3-0.6B » est aussi dans l'identifiant,
    // « llama3 » aussi : on cherche un nom montré différent).
    const renamed = [info("a/b-GGUF:Q8_0", { displayName: "Friendly" })];
    expect(pickerEntries(renamed, null, "friend").map((e) => e.model)).toEqual([
      "a/b-GGUF:Q8_0",
    ]);
    // Un modèle installé reste installé pendant une recherche.
    expect(ids("qwen", selectedEntry(installed, LLAMA))).toEqual([
      QWEN35,
      QWEN3,
    ]);
  });

  it("lists the recommended models first, with their installed copy, each once", () => {
    const entries = pickerEntries(installed, null, "", [GEMMA, QWEN3]);
    expect(entries).toEqual([
      { model: GEMMA, info: null },
      { model: QWEN3, info: installed[1] },
      { model: QWEN35, info: installed[0] },
      { model: LLAMA, info: installed[2] },
    ]);
    // Choisi et recommandé : à sa place de recommandé, sans doublon.
    expect(
      pickerEntries(installed, selectedEntry(installed, GEMMA), "", [
        GEMMA,
      ]).map((e) => e.model),
    ).toEqual([GEMMA, QWEN35, QWEN3, LLAMA]);
  });

  it("lists the model found on Hugging Face last, never twice", () => {
    const names = (search: string, upstream: string | null) =>
      pickerEntries(installed, null, search, [], upstream).map((e) => e.model);
    expect(
      names("unsloth/Qwen3-8B-GGUF", "unsloth/Qwen3-8B-GGUF:Q4_K_M"),
    ).toEqual(["unsloth/Qwen3-8B-GGUF:Q4_K_M"]);
    expect(names("unsloth", "unsloth/Qwen3-8B-GGUF:Q4_K_M")).toEqual([
      QWEN35,
      QWEN3,
      "unsloth/Qwen3-8B-GGUF:Q4_K_M",
    ]);
    expect(names(QWEN3, QWEN3)).toEqual([QWEN3]);
    // Tapé sous la forme d'une adresse : montré quand même.
    expect(
      names(
        "https://huggingface.co/unsloth/Qwen3-8B-GGUF:q8_0",
        "unsloth/Qwen3-8B-GGUF:Q8_0",
      ),
    ).toEqual(["unsloth/Qwen3-8B-GGUF:Q8_0"]);
  });

  it("recommends the ids in the order received", () => {
    const recommended: RecommendedModel[] = [
      {
        model: GEMMA,
        displayName: "gemma-4-E4B-it",
        sizeBytes: null,
        vramBytes: null,
        fit: null,
      },
      {
        model: QWEN35,
        displayName: "Qwen3.5-0.8B",
        sizeBytes: null,
        vramBytes: null,
        fit: null,
      },
    ];
    expect(recommendedNames(recommended)).toEqual([GEMMA, QWEN35]);
  });
});

describe("a typed Hugging Face id", () => {
  const entries = pickerEntries(installed, null, "");

  it("is read with or without its quantization, from an address too", () => {
    expect(typedRepo(entries, " unsloth/Qwen3-8B-GGUF ")).toEqual({
      repo: "unsloth/Qwen3-8B-GGUF",
      quant: null,
    });
    expect(typedRepo(entries, "unsloth/Qwen3-8B-GGUF:UD-Q4_K_XL")).toEqual({
      repo: "unsloth/Qwen3-8B-GGUF",
      quant: "UD-Q4_K_XL",
    });
    expect(
      typedRepo(entries, "https://huggingface.co/unsloth/Qwen3-8B-GGUF/"),
    ).toEqual({ repo: "unsloth/Qwen3-8B-GGUF", quant: null });
    expect(typedRepo(entries, "hf.co/a/b:Q8_0")).toEqual({
      repo: "a/b",
      quant: "Q8_0",
    });
  });

  it("is not looked up when it is not an id or is already listed", () => {
    expect(typedRepo(entries, "")).toBeNull();
    expect(typedRepo(entries, "qwen")).toBeNull();
    expect(typedRepo(entries, "a/b/c")).toBeNull();
    expect(typedRepo(entries, "@ollama/llama3:latest")).toBeNull();
    expect(typedRepo(entries, QWEN3)).toBeNull();
    expect(typedRepo(entries, "unsloth/Qwen3-0.6B-GGUF:q4_k_m")).toBeNull();
  });

  it("gives the version asked for, else the one advised for the machine", () => {
    const quant = (
      label: string,
      fields: Partial<HubQuant> = {},
    ): HubQuant => ({
      label,
      file: `${label}.gguf`,
      model: `u/R-GGUF:${label}`,
      issue: null,
      size: 1,
      downloadSize: 1,
      tier: "balanced",
      fit: null,
      memory: null,
      recommended: false,
      ...fields,
    });
    const repo = {
      quants: [
        quant("Q8_0"),
        quant("Q4_K_M", { recommended: true }),
        quant("BF16", { model: null }),
      ],
    } as unknown as HubRepo;
    expect(upstreamModel(repo, "q8_0")).toBe("u/R-GGUF:Q8_0");
    expect(upstreamModel(repo, null)).toBe("u/R-GGUF:Q4_K_M");
    // Absente, ou non téléchargeable (dépôt restreint).
    expect(upstreamModel(repo, "Q2_K")).toBeNull();
    expect(upstreamModel(repo, "BF16")).toBeNull();
    expect(upstreamModel(null, null)).toBeNull();
  });
});

describe("restoredModel", () => {
  const OLLAMA = "@ollama/qwen3:0.6b";
  const HF = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
  const models = [info(OLLAMA), info(HF), info("@ollama/llama3:latest")];

  it("adopts an installed model under the id of the list", () => {
    expect(
      restoredModel(models, HF.replace("Q4_K_M", "q4_k_m"), false),
    ).toEqual({ kind: "adopt", model: HF });
    expect(restoredModel(models, OLLAMA, false)).toEqual({
      kind: "adopt",
      model: OLLAMA,
    });
  });

  it("finds an old Ollama name under its new id", () => {
    expect(restoredModel(models, "qwen3:0.6b", false)).toEqual({
      kind: "adopt",
      model: OLLAMA,
    });
    // Sans étiquette : `latest`.
    expect(restoredModel(models, "llama3", false)).toEqual({
      kind: "adopt",
      model: "@ollama/llama3:latest",
    });
    // Ancien nom de Hugging Face d'Ollama : son identifiant.
    expect(
      restoredModel(models, "hf.co/unsloth/Qwen3-0.6B-GGUF:Q4_K_M", false),
    ).toEqual({ kind: "adopt", model: HF });
  });

  it("leaves to the engine an old name it does not find", () => {
    expect(restoredModel(models, "phi4:14b", false)).toEqual({
      kind: "legacy",
      model: "phi4:14b",
    });
    expect(
      restoredModel(models, "hf.co/bartowski/Phi-4-GGUF:Q8_0", false),
    ).toEqual({ kind: "legacy", model: "hf.co/bartowski/Phi-4-GGUF:Q8_0" });
  });

  it("adopts a Hugging Face id not installed (downloaded at the next send)", () => {
    expect(restoredModel(models, "bartowski/Phi-4-GGUF:Q8_0", false)).toEqual({
      kind: "adopt",
      model: "bartowski/Phi-4-GGUF:Q8_0",
    });
  });

  it("keeps the selection for a model taken from Ollama then deleted, or a remote model not allowed", () => {
    expect(restoredModel(models, "@ollama/gemma3:4b", true)).toBeNull();
    expect(restoredModel(models, "@remote/mistral/small", false)).toBeNull();
    expect(restoredModel(models, "@remote/mistral/small", true)).toEqual({
      kind: "adopt",
      model: "@remote/mistral/small",
    });
    expect(restoredModel(models, "  ", true)).toBeNull();
  });
});
