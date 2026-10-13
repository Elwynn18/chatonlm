// Fonctions pures de la lecture à voix haute : réponse ou texte d'un cours à
// qui revient l'état et l'erreur, titre d'une voix, conversation tirée du
// chemin et changement de conversation.

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
  readAloudBlockedText,
  readAloudErrorText,
  sameSource,
  sourceOf,
  voiceTitle,
} from "@/lib/readAloud";

const state = (patch: Partial<ReadAloudState>): ReadAloudState => ({
  ...IDLE,
  ...patch,
});

const answer = (messageId: number) => ({ kind: "message", messageId }) as const;
const passage = (courseId: string, key: string) =>
  ({ kind: "study", courseId, passage: key }) as const;

describe("reading state of each response", () => {
  it("the phase belongs to the response being read only", () => {
    const playing = state({ source: answer(4), phase: "playing" });
    expect(phaseFor(playing, answer(4))).toBe("playing");
    expect(phaseFor(playing, answer(2))).toBe("idle");
    expect(phaseFor(IDLE, answer(4))).toBe("idle");
  });

  it("an error at rest belongs to the response the engine names with it", () => {
    const failed = state({ source: answer(4), error: "noOutputDevice" });
    expect(errorFor(failed, answer(4))).toBe("noOutputDevice");
    expect(errorFor(failed, answer(2))).toBeNull();
    // Une autre lecture commence : plus d'erreur nulle part.
    const next = state({ source: answer(2), phase: "preparing" });
    expect(errorFor(next, answer(4))).toBeNull();
    expect(errorFor(next, answer(2))).toBeNull();
  });

  it("no error while a reading runs", () => {
    expect(
      errorFor(
        state({ source: answer(4), phase: "playing", error: "engineFailed" }),
        answer(4),
      ),
    ).toBeNull();
  });
});

describe("reading state of a course text", () => {
  it("the phase and the error belong to that passage of that course only", () => {
    const lesson = passage("course-1", "L0:s1");
    const playing = state({ source: lesson, phase: "playing" });
    expect(phaseFor(playing, passage("course-1", "L0:s1"))).toBe("playing");
    expect(phaseFor(playing, passage("course-1", "L0:s2"))).toBe("idle");
    expect(phaseFor(playing, passage("course-2", "L0:s1"))).toBe("idle");
    // Ni une réponse, quel que soit son numéro, ni l'inverse.
    expect(phaseFor(playing, answer(0))).toBe("idle");
    expect(
      phaseFor(state({ source: answer(4), phase: "playing" }), lesson),
    ).toBe("idle");
    const failed = state({ source: lesson, error: "engineFailed" });
    expect(errorFor(failed, lesson)).toBe("engineFailed");
    expect(errorFor(failed, passage("course-1", "L0:recap"))).toBeNull();
    expect(errorFor(failed, answer(4))).toBeNull();
  });

  it("a target names its text the way the engine does", () => {
    expect(
      sourceOf({ kind: "message", chatId: "chat-1", messageId: 4 }),
    ).toEqual({ kind: "message", messageId: 4 });
    expect(
      sourceOf({ kind: "study", courseId: "course-1", passage: "L0:s1" }),
    ).toEqual({ kind: "study", courseId: "course-1", passage: "L0:s1" });
    expect(sameSource(null, null)).toBe(false);
  });

  it("a course text is not called a response", () => {
    const t = translator("en");
    expect(readAloudErrorText(t, "engineFailed")).toBe(
      "Couldn't read this response aloud.",
    );
    expect(readAloudErrorText(t, "engineFailed", "study")).toBe(
      "Couldn't read this text aloud.",
    );
    // Les autres erreurs ne parlent pas du texte : les mêmes.
    expect(readAloudErrorText(t, "noOutputDevice", "study")).toBe(
      readAloudErrorText(t, "noOutputDevice"),
    );
    expect(readAloudBlockedText(t, "nothingToRead")).toBe(
      "There's nothing to read aloud in this response.",
    );
    expect(readAloudBlockedText(t, "nothingToRead", "study")).toBe(
      "There's nothing to read aloud in this text.",
    );
    expect(readAloudBlockedText(t, "unsupportedLanguage", "study")).toBe(
      "Reading aloud is available in English, French and Spanish.",
    );
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
