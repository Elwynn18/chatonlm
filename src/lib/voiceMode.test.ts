import { describe, expect, it } from "vitest";
import type { Message } from "@/bindings/Message";
import type { VoiceModePhase } from "@/bindings/VoiceModePhase";
import { translator } from "@/i18n";
import type { LiveStream } from "@/lib/streams";
import {
  activityPercent,
  handsFreeAction,
  helpKey,
  levelSource,
  noticeText,
  phaseLabel,
  replyActivity,
} from "./voiceMode";

const en = translator("en");
const fr = translator("fr");

function reply(extra: Partial<Message> = {}): Message {
  return {
    id: 2,
    role: "assistant",
    content: "",
    thinking: "",
    model: "m",
    createdAt: 1,
    updatedAt: 1,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
    ...extra,
  };
}

function stream(extra: Partial<LiveStream> = {}): LiveStream {
  return {
    streamId: "s",
    chatId: "chat-1",
    content: "Hi",
    model: "m",
    editOf: null,
    startedAt: 1,
    pending: { ...reply(), id: -1, role: "user" },
    user: null,
    replies: [],
    download: null,
    loading: false,
    compacting: false,
    context: undefined,
    compactions: [],
    finished: false,
    ...extra,
  };
}

describe("replyActivity", () => {
  it("download, loading, web search, otherwise thinking", () => {
    expect(replyActivity(null)).toEqual({ kind: "thinking" });
    expect(
      replyActivity(
        stream({ download: { total: 200, completed: 50, done: false } }),
      ),
    ).toEqual({ kind: "downloadingModel", percent: 25 });
    expect(
      replyActivity(
        stream({ download: { total: 0, completed: 0, done: false } }),
      ),
    ).toEqual({ kind: "downloadingModel", percent: null });
    // Téléchargement fini : la suite (chargement).
    expect(
      replyActivity(
        stream({
          download: { total: 1, completed: 1, done: true },
          loading: true,
        }),
      ),
    ).toEqual({ kind: "loadingModel" });
    const call = { name: "web_search", arguments: "{}", error: null };
    expect(
      replyActivity(
        stream({
          replies: [reply({ toolCalls: [{ ...call, result: null }] })],
        }),
      ),
    ).toEqual({ kind: "searching" });
    // Résultat reçu : le modèle reprend (réflexion).
    expect(
      replyActivity(
        stream({
          replies: [reply({ toolCalls: [{ ...call, result: "[]" }] })],
        }),
      ),
    ).toEqual({ kind: "thinking" });
    expect(
      replyActivity(stream({ replies: [reply({ thinkingStartedAt: 5 })] })),
    ).toEqual({ kind: "thinking" });
  });
});

describe("phaseLabel", () => {
  const label = (
    phase: VoiceModePhase,
    muted = false,
    activity = replyActivity(null),
  ) => phaseLabel(en, { phase, muted }, activity);

  it("one label per phase, the reply's activity while responding", () => {
    expect(label("starting")).toBe("Getting ready…");
    expect(label("listening")).toBe("Listening…");
    expect(label("hearing")).toBe("Listening…");
    expect(label("transcribing")).toBe("Transcribing…");
    expect(label("responding")).toBe("Thinking…");
    expect(label("responding", false, { kind: "loadingModel" })).toBe(
      "Loading model…",
    );
    expect(label("responding", false, { kind: "searching" })).toBe(
      "Searching the web…",
    );
    // Sans pourcentage : il est montré à part (activityPercent).
    expect(
      label("responding", false, { kind: "downloadingModel", percent: 42 }),
    ).toBe("Downloading model…");
    expect(label("speaking")).toBe("Speaking…");
    expect(label("paused")).toBe("Paused — click to resume");
  });

  it("microphone off while it would listen; the reply's phases stay", () => {
    expect(label("listening", true)).toBe("Microphone off");
    expect(label("paused", true)).toBe("Microphone off");
    expect(label("speaking", true)).toBe("Speaking…");
    expect(label("responding", true)).toBe("Thinking…");
  });

  it("in French, with the percent written the French way", () => {
    expect(
      phaseLabel(
        fr,
        { phase: "listening", muted: false },
        { kind: "thinking" },
      ),
    ).toBe("À l'écoute…");
    expect(
      phaseLabel(
        fr,
        { phase: "responding", muted: false },
        { kind: "downloadingModel", percent: 42 },
      ),
    ).toBe("Téléchargement du modèle…");
    expect(
      activityPercent(
        fr,
        { phase: "responding" },
        { kind: "downloadingModel", percent: 42 },
      ),
    ).toBe("42 %");
  });

  it("the model's download percent only while responding, when known", () => {
    const downloading = { kind: "downloadingModel", percent: 7 } as const;
    expect(activityPercent(en, { phase: "responding" }, downloading)).toBe(
      "7%",
    );
    expect(activityPercent(en, { phase: "speaking" }, downloading)).toBeNull();
    expect(
      activityPercent(
        en,
        { phase: "responding" },
        { kind: "downloadingModel", percent: null },
      ),
    ).toBeNull();
    expect(
      activityPercent(en, { phase: "responding" }, { kind: "thinking" }),
    ).toBeNull();
  });
});

describe("noticeText", () => {
  it("says why the reply isn't read, or gives the engine's message", () => {
    expect(noticeText(en, { kind: "unsupportedLanguage" })).toBe(
      "This reply isn't in English, French or Spanish: it's written, not read aloud.",
    );
    expect(noticeText(en, { kind: "missingVoice", language: "fr" })).toBe(
      "The French voice isn't downloaded: this reply is written, not read aloud.",
    );
    expect(noticeText(fr, { kind: "missingVoice", language: "es" })).toBe(
      "La voix espagnole n'est pas téléchargée : cette réponse est écrite, pas lue à voix haute.",
    );
    expect(
      noticeText(en, { kind: "readFailed", error: "noOutputDevice" }),
    ).toBe("Couldn't read aloud: no audio output device was found.");
    expect(
      noticeText(en, { kind: "failed", message: "The microphone was lost" }),
    ).toBe("The microphone was lost");
  });
});

describe("handsFreeAction", () => {
  it("I'm done while speaking, interrupt the reply, resume after the pause", () => {
    const action = (phase: VoiceModePhase, muted = false) =>
      handsFreeAction({ phase, muted });
    expect(action("hearing")).toBe("endTurn");
    expect(action("responding")).toBe("interrupt");
    expect(action("speaking")).toBe("interrupt");
    expect(action("speaking", true)).toBe("interrupt");
    expect(action("paused")).toBe("resume");
    for (const phase of [
      "off",
      "starting",
      "listening",
      "transcribing",
    ] as const) {
      expect(action(phase), phase).toBeNull();
    }
    expect(action("hearing", true)).toBeNull();
  });
});

describe("helpKey and levelSource", () => {
  it("help by mode and phase", () => {
    expect(helpKey("handsFree", "listening")).toBe(
      "voiceMode.bar.help.handsFree",
    );
    expect(helpKey("handsFree", "speaking")).toBe(
      "voiceMode.bar.help.interrupt",
    );
    expect(helpKey("pushToTalk", "speaking")).toBe(
      "voiceMode.bar.help.pushToTalk",
    );
    // En pause : Espace reprend (les deux modes).
    expect(helpKey("pushToTalk", "paused")).toBe("voiceMode.bar.help.paused");
    expect(helpKey("handsFree", "paused")).toBe("voiceMode.bar.help.paused");
  });

  it("the microphone while listening, the voice while speaking", () => {
    expect(levelSource("listening", false)).toBe("input");
    expect(levelSource("hearing", false)).toBe("input");
    expect(levelSource("hearing", true)).toBeNull();
    expect(levelSource("speaking", true)).toBe("output");
    expect(levelSource("transcribing", false)).toBeNull();
    expect(levelSource("responding", false)).toBeNull();
  });
});
