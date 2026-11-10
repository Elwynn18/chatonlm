import { describe, expect, it } from "vitest";
import {
  displayTag,
  isRemoteModel,
  parseModelName,
  sameModel,
  shortModelName,
  withoutGgufSuffix,
} from "./modelNames";

describe("model names", () => {
  it("splits a Hugging Face id into title, tag, publisher and repository", () => {
    expect(
      parseModelName("bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M"),
    ).toEqual({
      title: "SmolLM2-135M-Instruct",
      tag: "Q4_K_M",
      publisher: "bartowski",
      repo: "bartowski/SmolLM2-135M-Instruct-GGUF",
      kind: "hf",
    });
    expect(parseModelName("unsloth/Qwen3.5-9B-GGUF:UD-Q4_K_XL")).toMatchObject({
      title: "Qwen3.5-9B",
      tag: "UD-Q4_K_XL",
      kind: "hf",
    });
  });

  it("still reads the Hugging Face names of Ollama kept in the history", () => {
    expect(parseModelName("HF.CO/Qwen/Qwen3-8B-GGUF")).toEqual({
      title: "Qwen3-8B",
      tag: "",
      publisher: "Qwen",
      repo: "Qwen/Qwen3-8B-GGUF",
      kind: "hf",
    });
    expect(
      parseModelName("huggingface.co/unsloth/Qwen3.5-9B-GGUF:UD-Q4_K_XL").repo,
    ).toBe("unsloth/Qwen3.5-9B-GGUF");
  });

  it("splits a model taken from Ollama, its fallback id included", () => {
    expect(parseModelName("@ollama/qwen3:0.6b")).toEqual({
      title: "qwen3",
      tag: "0.6b",
      publisher: null,
      repo: null,
      kind: "ollama",
    });
    expect(
      parseModelName("@ollama/ollama.com/someone/mybot:latest"),
    ).toMatchObject({ title: "mybot", tag: "latest", kind: "ollama" });
  });

  it("keeps the name of a remote model whole, its provider apart", () => {
    expect(
      parseModelName("@remote/openrouter/meta-llama/llama-3.3-70b:free"),
    ).toEqual({
      title: "meta-llama/llama-3.3-70b:free",
      tag: "",
      publisher: "openrouter",
      repo: null,
      kind: "remote",
    });
    expect(parseModelName("@remote/broken")).toMatchObject({
      title: "broken",
      publisher: null,
      kind: "remote",
    });
    expect(isRemoteModel(" @remote/mistral/mistral-small-latest")).toBe(true);
    expect(isRemoteModel("@ollama/qwen3:0.6b")).toBe(false);
  });

  it("splits an old Ollama name into title and tag", () => {
    expect(parseModelName("qwen3:0.6b")).toEqual({
      title: "qwen3",
      tag: "0.6b",
      publisher: null,
      repo: null,
      kind: "other",
    });
    expect(parseModelName("library/gemma3")).toMatchObject({
      title: "gemma3",
      tag: "",
      publisher: null,
      kind: "other",
    });
    // Le port d'un hôte n'est pas une étiquette.
    expect(parseModelName("localhost:5000/team/model")).toMatchObject({
      title: "model",
      tag: "",
      publisher: "team",
    });
  });

  it("strips the GGUF suffix and the .gguf extension of a file tag", () => {
    expect(withoutGgufSuffix("gemma-4-12B-it-qat-q4_0-gguf")).toBe(
      "gemma-4-12B-it-qat-q4_0",
    );
    expect(withoutGgufSuffix("Model_GGUF")).toBe("Model");
    expect(displayTag("Qwen3.8-27B-Q4_K_L.gguf")).toBe("Qwen3.8-27B-Q4_K_L");
    expect(displayTag("Q4_K_M")).toBe("Q4_K_M");
  });

  it("compares ids like the engine: the quantization without case", () => {
    expect(
      sameModel(
        "bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
        " bartowski/SmolLM2-135M-Instruct-GGUF:q4_k_m",
      ),
    ).toBe(true);
    // Le dépôt, lui, se compare exactement.
    expect(
      sameModel(
        "bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
        "bartowski/smollm2-135m-instruct-gguf:Q4_K_M",
      ),
    ).toBe(false);
    expect(sameModel("@ollama/qwen3:0.6b", "@ollama/qwen3:1.7b")).toBe(false);
    expect(sameModel("a/B-GGUF:Q8_0", "a/B-GGUF")).toBe(false);
    // Distant : exactement.
    expect(sameModel("@remote/p/Model:Free", "@remote/p/Model:free")).toBe(
      false,
    );
    expect(sameModel("@remote/p/m", "@remote/p/m")).toBe(true);
  });

  it("shortens a name to quote it", () => {
    expect(shortModelName("bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M")).toBe(
      "SmolLM2-135M-Instruct:Q4_K_M",
    );
    expect(shortModelName("@ollama/qwen3:0.6b")).toBe("qwen3:0.6b");
    expect(shortModelName("@remote/mistral/mistral-small-latest")).toBe(
      "mistral-small-latest",
    );
    expect(shortModelName("hf.co/a/Model-GGUF")).toBe("Model");
    expect(shortModelName(" qwen3:0.6b ")).toBe("qwen3:0.6b");
    expect(shortModelName("someone/model")).toBe("someone/model");
  });
});
