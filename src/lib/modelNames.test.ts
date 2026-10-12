import { describe, expect, it } from "vitest";
import {
  displayTag,
  isHuggingFaceName,
  parseModelName,
  sameModel,
  withoutGgufSuffix,
} from "./modelNames";

describe("model names", () => {
  it("splits a Hugging Face name into title, tag, publisher and repository", () => {
    expect(
      parseModelName("hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M"),
    ).toEqual({
      title: "SmolLM2-135M-Instruct",
      tag: "Q4_K_M",
      publisher: "bartowski",
      repo: "bartowski/SmolLM2-135M-Instruct-GGUF",
    });
    // Hôte sans tenir compte de la casse, huggingface.co aussi, sans étiquette.
    expect(parseModelName("HF.CO/Qwen/Qwen3-8B-GGUF")).toEqual({
      title: "Qwen3-8B",
      tag: "",
      publisher: "Qwen",
      repo: "Qwen/Qwen3-8B-GGUF",
    });
    expect(
      parseModelName("huggingface.co/unsloth/Qwen3.5-9B-GGUF:UD-Q4_K_XL").repo,
    ).toBe("unsloth/Qwen3.5-9B-GGUF");
  });

  it("splits an Ollama name into title and tag", () => {
    expect(parseModelName("qwen3:0.6b")).toEqual({
      title: "qwen3",
      tag: "0.6b",
      publisher: null,
      repo: null,
    });
    expect(parseModelName("library/gemma3")).toMatchObject({
      title: "gemma3",
      tag: "",
      publisher: null,
    });
    expect(parseModelName("someone/custom:latest")).toMatchObject({
      title: "custom",
      tag: "latest",
      publisher: "someone",
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

  it("compares names without case, as Ollama resolves them", () => {
    expect(
      sameModel(
        "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
        "hf.co/bartowski/smollm2-135m-instruct-gguf:q4_k_m",
      ),
    ).toBe(true);
    expect(sameModel("qwen3:0.6b", "qwen3:1.7b")).toBe(false);
    expect(isHuggingFaceName("hf.co/a/b:c")).toBe(true);
    expect(isHuggingFaceName("qwen3:0.6b")).toBe(false);
  });
});
