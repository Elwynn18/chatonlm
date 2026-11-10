// Modèles distants, fonctions pures : nom montré d'un fournisseur, ancien
// Ollama d'une autre machine (réglage « Server address » des versions 0.9).

import { describe, expect, it } from "vitest";
import type { RemoteProvider } from "@/bindings/RemoteProvider";
import { formerServer, providerName } from "./remoteModels";

function provider(id: string, name: string): RemoteProvider {
  return {
    id,
    kind: "custom",
    name,
    baseUrl: "http://192.168.1.20:11434",
    enabled: true,
    hasKey: false,
    keyHint: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

describe("providerName", () => {
  it("prefers the name given in the settings, then the list's, then the id", () => {
    const model = "@remote/custom-2/llama-3.3";
    expect(providerName(model, [provider("custom-2", "Salon")], "Old")).toBe(
      "Salon",
    );
    expect(providerName(model, [provider("custom", "Other")], "Old")).toBe(
      "Old",
    );
    expect(providerName(model, undefined)).toBe("custom-2");
    expect(providerName("@ollama/qwen3:0.6b", [])).toBeNull();
  });
});

describe("formerServer", () => {
  it("gives the address of an Ollama on another computer, with its OpenAI API", () => {
    expect(formerServer("http://192.168.1.20:11434")).toEqual({
      address: "http://192.168.1.20:11434",
      baseUrl: "http://192.168.1.20:11434/v1",
    });
    // Sans port en http : celui d'Ollama ; sans schéma : http.
    expect(formerServer("nas.local")?.baseUrl).toBe(
      "http://nas.local:11434/v1",
    );
    expect(formerServer("https://ollama.example.com/")?.baseUrl).toBe(
      "https://ollama.example.com/v1",
    );
  });

  it("is null for nothing, this computer or an unreadable address", () => {
    for (const address of [
      "",
      "  ",
      null,
      undefined,
      "http://localhost:11434",
      "http://127.0.0.1:11434",
      "127.0.0.2",
      "http://[::1]:11434",
      "http://0.0.0.0:11434",
      "ftp://192.168.1.20",
      "http://",
    ]) {
      expect(formerServer(address)).toBeNull();
    }
  });
});
