// @vitest-environment node
// Liste des modèles de l'app vue par l'extension : `ChatModel` de l'app 0.10
// (identifiants entiers, distant d'après `origin`), ancien `ModelInfo` d'une
// app 0.9.x (noms d'Ollama, `:latest` retiré, cloud d'après le nom).
import { describe, expect, it } from "vitest";
import { listedModels, looksRemote, shortName } from "./models";

describe("models of the app", () => {
  it("reads the ChatModel list of the app", () => {
    expect(
      listedModels([
        {
          id: "Qwen/Qwen3-0.6B-GGUF:Q8_0",
          displayName: "Qwen3 0.6B",
          origin: "hf",
          provider: null,
          sizeBytes: 639_000_000,
          paramsLabel: "0.6B",
          quant: "Q8_0",
          contextMax: 40960,
          capabilities: ["completion", "thinking", 3],
          installedAt: 1,
          // Champs de l'ancien `ModelInfo`, ajoutés par l'app pour 0.1.x :
          // sans effet ici.
          model: "autre",
          contextLength: 1,
        },
        {
          id: "@ollama/llama3.2:latest",
          displayName: "llama3.2",
          origin: "ollama",
          contextMax: null,
        },
        {
          // Distant d'après `origin`, quel que soit son nom.
          id: "@remote/mistral/mistral-small-latest",
          displayName: "Mistral Small",
          origin: "remote",
          provider: "mistral",
          contextMax: 0,
        },
      ]),
    ).toEqual([
      {
        id: "Qwen/Qwen3-0.6B-GGUF:Q8_0",
        name: "Qwen3 0.6B",
        remote: false,
        contextMax: 40960,
        parameters: "0.6B",
        quantization: "Q8_0",
        capabilities: ["completion", "thinking"],
      },
      {
        // `:latest` gardé : l'app ne reconnaîtrait pas `@ollama/llama3.2`.
        id: "@ollama/llama3.2:latest",
        name: "llama3.2",
        remote: false,
        contextMax: null,
        parameters: null,
        quantization: null,
        capabilities: [],
      },
      {
        id: "@remote/mistral/mistral-small-latest",
        name: "Mistral Small",
        remote: true,
        contextMax: null,
        parameters: null,
        quantization: null,
        capabilities: [],
      },
    ]);
  });

  it("still reads the ModelInfo list of an app 0.9.x", () => {
    expect(
      listedModels([
        {
          model: "qwen3:latest",
          digest: "abc",
          parameterSize: "8.2B",
          quantizationLevel: "Q4_K_M",
          contextLength: 40960,
          capabilities: ["completion"],
        },
        { model: "gpt-oss:120b-cloud", contextLength: 131072 },
      ]),
    ).toEqual([
      {
        id: "qwen3",
        name: "qwen3",
        remote: false,
        contextMax: 40960,
        parameters: "8.2B",
        quantization: "Q4_K_M",
        capabilities: ["completion"],
      },
      {
        id: "gpt-oss:120b-cloud",
        name: "gpt-oss:120b-cloud",
        remote: true,
        contextMax: 131072,
        parameters: null,
        quantization: null,
        capabilities: [],
      },
    ]);
  });

  it("skips what it cannot read", () => {
    expect(listedModels(null)).toEqual([]);
    expect(listedModels({ id: "x" })).toEqual([]);
    expect(listedModels([null, 3, {}, { id: " " }, { model: 7 }])).toEqual([]);
  });

  it("shortens only an old Ollama name", () => {
    expect(shortName("qwen3:latest")).toBe("qwen3");
    expect(shortName("qwen3:0.6b")).toBe("qwen3:0.6b");
    expect(shortName("@ollama/qwen3:latest")).toBe("@ollama/qwen3:latest");
    expect(shortName("hf.co/a/b:latest")).toBe("hf.co/a/b:latest");
    expect(shortName("Qwen/Qwen3-0.6B-GGUF:Q8_0")).toBe(
      "Qwen/Qwen3-0.6B-GGUF:Q8_0",
    );
  });

  it("guesses a remote model from its id", () => {
    expect(looksRemote("@remote/openrouter/x")).toBe(true);
    expect(looksRemote("kimi-k2:1t-cloud")).toBe(true);
    expect(looksRemote("Qwen/Qwen3-0.6B-GGUF:Q8_0")).toBe(false);
    // Un « @remote » qui n'est pas en tête n'est pas un préfixe.
    expect(looksRemote("a/@remote/b:Q4_K_M")).toBe(false);
  });
});
