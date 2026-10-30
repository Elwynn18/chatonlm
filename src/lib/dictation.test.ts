import { describe, expect, it } from "vitest";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { translator } from "@/i18n";
import {
  addWord,
  isActiveSpeechDownload,
  isCloudModel,
  isLocalAddress,
  navigablePath,
  phaseText,
  POLISH_MODELS,
  polishModelNote,
  polishModelSize,
  speechDownloadPercent,
  speechDownloadText,
} from "./dictation";

const MIB = 1024 ** 2;
const en = translator("en");
const fr = translator("fr");
const es = translator("es");

function download(fields: Partial<SpeechDownload>): SpeechDownload {
  return {
    id: "whisper-tiny",
    name: "Whisper Tiny",
    state: "downloading",
    completedBytes: 0,
    totalBytes: 0,
    error: null,
    ...fields,
  };
}

describe("navigablePath", () => {
  it("keeps the pages of the settings, of the ModelHub and Proofread", () => {
    for (const path of [
      "/settings",
      "/settings?section=dictation",
      "/settings?section=shortcuts",
      "/proofread",
      "/proofread?selection=1",
      "/hub",
      "/hub/",
      "/hub/transcription",
      "/hub/model/unsloth/Qwen3.5-9B-GGUF",
      "/hub?q=qwen#top",
      "/study/019a3b2c-1d2e-7f30-8a4b-5c6d7e8f9a0b",
      "/c/019a3b2c-1d2e-7f30-8a4b-5c6d7e8f9a0b",
    ]) {
      expect(navigablePath(path), path).toBe(path);
    }
  });

  it("refuses anything else", () => {
    for (const path of [
      "",
      "/",
      "/c/new",
      "/c/019a3b2c-1d2e-7f30-8a4b-5c6d7e8f9a0b/x",
      "/study/../settings",
      "/study/019a3b2c",
      "/study",
      "/settingsX",
      "/hubs",
      "/proofreading",
      "proofread",
      "/proofread?selection=a b",
      "settings",
      "//example.com/settings",
      "https://example.com/settings",
      "javascript:alert(1)",
      "/settings\\..",
      "/hub/a b",
      "/settings?section=a b",
      42,
      null,
    ]) {
      expect(navigablePath(path), String(path)).toBeNull();
    }
  });
});

describe("addWord", () => {
  it("adds a trimmed word at the end", () => {
    expect(addWord(["Ollama"], "  ChatonLM ")).toEqual(["Ollama", "ChatonLM"]);
  });

  it("refuses an empty word, or one already there whatever its case", () => {
    expect(addWord(["ChatonLM"], "   ")).toBeNull();
    expect(addWord(["ChatonLM"], "chatonlm")).toBeNull();
  });
});

describe("speech downloads", () => {
  it("active while queued, downloading or verifying", () => {
    const states: SpeechDownload["state"][] = [
      "queued",
      "downloading",
      "verifying",
      "done",
      "failed",
      "cancelled",
    ];
    expect(
      states.map((state) => isActiveSpeechDownload(download({ state }))),
    ).toEqual([true, true, true, false, false, false]);
  });

  it("says where it is", () => {
    expect(speechDownloadText(en, download({ state: "queued" }))).toBe(
      "Waiting…",
    );
    expect(speechDownloadText(en, download({ state: "verifying" }))).toBe(
      "Verifying…",
    );
    expect(
      speechDownloadText(
        en,
        download({ completedBytes: 120 * MIB, totalBytes: 640 * MIB }),
      ),
    ).toBe("120 MB of 640 MB");
    expect(speechDownloadText(en, download({ completedBytes: 3 * MIB }))).toBe(
      "3.0 MB",
    );
    expect(
      speechDownloadPercent(
        download({ completedBytes: 160 * MIB, totalBytes: 640 * MIB }),
      ),
    ).toBe(25);
    expect(speechDownloadPercent(download({ completedBytes: 5 }))).toBe(0);
  });

  it("says it in the language of the interface, with its sizes", () => {
    const progress = download({
      completedBytes: 120 * MIB,
      totalBytes: 2 * 1024 * MIB,
    });
    expect(speechDownloadText(fr, progress)).toBe("120 Mo sur 2,0 Go");
    expect(speechDownloadText(es, progress)).toBe("120 MB de 2,0 GB");
    expect(speechDownloadText(fr, download({ state: "queued" }))).toBe(
      "En attente…",
    );
    expect(speechDownloadText(es, download({ state: "verifying" }))).toBe(
      "Verificando…",
    );
  });
});

describe("phaseText", () => {
  it("announces each phase but rest", () => {
    expect(phaseText(en, "idle")).toBeNull();
    expect(phaseText(en, "recording")).toBe("Listening…");
    expect(phaseText(en, "transcribing")).toBe("Transcribing…");
    expect(phaseText(en, "polishing")).toBe("Polishing…");
    expect(phaseText(fr, "recording")).toBe("À l'écoute…");
    expect(phaseText(es, "polishing")).toBe("Puliendo…");
  });
});

describe("polish models", () => {
  const model = (name: string) =>
    POLISH_MODELS.find((entry) => entry.model === name)!;

  it("sizes and notes in the language of the interface", () => {
    expect(polishModelSize(en, model("gemma3:4b"))).toBe("3.3 GB");
    expect(polishModelSize(fr, model("gemma3:4b"))).toBe("3,3 Go");
    expect(polishModelNote(en, model("gemma3:4b"))).toBeNull();
    expect(polishModelNote(en, model("gemma3:12b"))).toBe(
      "Best quality, needs about 12 GB of GPU memory",
    );
    expect(polishModelNote(fr, model("gemma3:12b"))).toBe(
      "Meilleure qualité, demande environ 12 Go de mémoire GPU",
    );
    expect(polishModelNote(es, model("gemma3:1b"))).toBe(
      "Para equipos modestos, menos fiable",
    );
  });
});

describe("isCloudModel", () => {
  it("names ending in cloud, like the engine", () => {
    expect(isCloudModel("gpt-oss:120b-cloud")).toBe(true);
    expect(isCloudModel("kimi-k2:cloud")).toBe(true);
    expect(isCloudModel("gemma3:4b")).toBe(false);
    expect(isCloudModel("cloudy-model:1b")).toBe(false);
    // Comme le serveur : étiquette sans casse ; un nom local en « cloud »
    // reste local.
    expect(isCloudModel("kimi-k2.6:Cloud")).toBe(true);
    expect(isCloudModel("pointcloud")).toBe(false);
    expect(isCloudModel("registry.example.com:5000/cloud")).toBe(false);
  });
});

describe("isLocalAddress", () => {
  it("this machine", () => {
    for (const address of [
      "",
      "http://127.0.0.1:11434",
      "http://localhost:11434",
      "localhost:8080/ollama",
      "http://[::1]:11434",
      "[::1]:11434",
      "http://127.0.0.2:11434",
      "http://0.0.0.0:11434",
      "http://ollama.localhost",
    ]) {
      expect(isLocalAddress(address), address).toBe(true);
    }
  });

  it("another machine, or an address that cannot be read", () => {
    for (const address of [
      "http://192.168.1.20:11434",
      "https://ollama.example.com",
      "10.0.0.2:11434",
      "http://[fe80::1]:11434",
      "http://localhost.example.com",
      "http://127.0.0.1.example.com",
      "http://exa mple",
    ]) {
      expect(isLocalAddress(address), address).toBe(false);
    }
  });
});
