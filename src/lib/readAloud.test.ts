// Fonctions pures de la lecture à voix haute : réponse à qui revient l'état
// et l'erreur, titre d'une voix, conversation tirée du chemin et changement
// de conversation.

import { describe, expect, it } from "vitest";
import type { ReadAloudState } from "@/bindings/ReadAloudState";
import { translator } from "@/i18n";
import {
  chatOfPath,
  errorFor,
  IDLE,
  leavesChat,
  needsVoiceText,
  phaseFor,
  voiceTitle,
} from "@/lib/readAloud";

const state = (patch: Partial<ReadAloudState>): ReadAloudState => ({
  ...IDLE,
  ...patch,
});

describe("reading state of each response", () => {
  it("the phase belongs to the response being read only", () => {
    const playing = state({ messageId: 4, phase: "playing" });
    expect(phaseFor(playing, 4)).toBe("playing");
    expect(phaseFor(playing, 2)).toBe("idle");
    expect(phaseFor(IDLE, 4)).toBe("idle");
  });

  it("an error at rest belongs to the response the engine names with it", () => {
    const failed = state({ messageId: 4, error: "noOutputDevice" });
    expect(errorFor(failed, 4)).toBe("noOutputDevice");
    expect(errorFor(failed, 2)).toBeNull();
    // Une autre lecture commence : plus d'erreur nulle part.
    const next = state({ messageId: 2, phase: "preparing" });
    expect(errorFor(next, 4)).toBeNull();
    expect(errorFor(next, 2)).toBeNull();
  });

  it("no error while a reading runs", () => {
    expect(
      errorFor(
        state({ messageId: 4, phase: "playing", error: "engineFailed" }),
        4,
      ),
    ).toBeNull();
  });
});

describe("texts", () => {
  it("a voice is titled by its language and its name", () => {
    const t = translator("en");
    expect(voiceTitle(t, { language: "fr", voice: "estelle" })).toBe(
      "French · Estelle",
    );
    expect(
      voiceTitle(translator("es"), { language: "en", voice: "alba" }),
    ).toBe("Inglés · Alba");
  });

  it("the dialog names the voice of the language, with its size", () => {
    expect(needsVoiceText(translator("en"), "es", "175 MB")).toBe(
      "Reading aloud in Spanish needs the Spanish voice (175 MB). It's downloaded once from the Internet (files from GitHub and Hugging Face) and stays on this computer.",
    );
    expect(needsVoiceText(translator("fr"), "fr", "175 Mo")).toBe(
      "La lecture en français nécessite la voix française (175 Mo). Elle est téléchargée une fois depuis Internet (fichiers de GitHub et de Hugging Face) et reste sur cet ordinateur.",
    );
    expect(needsVoiceText(translator("es"), "en", "175 MB")).toBe(
      "Leer en inglés requiere la voz inglesa (175 MB). Se descarga una sola vez desde Internet (archivos de GitHub y Hugging Face) y se queda en este equipo.",
    );
  });
});

describe("conversation shown", () => {
  it("is read from the path", () => {
    expect(chatOfPath("/c/chat-1")).toBe("chat-1");
    expect(chatOfPath("/c/new")).toBe("new");
    expect(chatOfPath("/c/a%20b")).toBe("a b");
    expect(chatOfPath("/settings")).toBeNull();
    expect(chatOfPath("/hub/voices")).toBeNull();
    expect(chatOfPath("/c")).toBeNull();
  });

  it("changes when another one opens, not when a new chat gets its id", () => {
    expect(leavesChat("chat-1", "chat-2")).toBe(true);
    expect(leavesChat("chat-1", "new")).toBe(true);
    expect(leavesChat("chat-1", "chat-1")).toBe(false);
    expect(leavesChat("new", "chat-3")).toBe(false);
    expect(leavesChat(null, "chat-1")).toBe(false);
  });
});
