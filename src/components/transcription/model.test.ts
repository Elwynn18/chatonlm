// Fonctions pures de l'outil Transcription : paragraphes (règle du moteur),
// horodatage, texte copié, passages en delta, avancement, temps restant,
// conseils du choix du modèle, adresse.

import { describe, expect, it } from "vitest";
import type { TranscriptionJob } from "@/bindings/TranscriptionJob";
import {
  clock,
  copyText,
  engineAdvice,
  isWebAddress,
  mergeStatus,
  paragraphs,
  remainingMinutes,
  stagePercent,
  whisperMinutes,
} from "@/components/transcription/model";

const s = (start: number, end: number, text: string) => ({ start, end, text });

function job(patch: Partial<TranscriptionJob> = {}): TranscriptionJob {
  return {
    id: "a",
    source: "file",
    origin: "/a.mp3",
    stage: "transcribing",
    model: "m",
    engine: "parakeet",
    language: null,
    title: null,
    channel: null,
    url: null,
    site: null,
    durationS: 600,
    receivedBytes: null,
    totalBytes: null,
    doneS: 0,
    speed: null,
    segments: [],
    segmentsFrom: 0,
    error: null,
    ...patch,
  };
}

describe("transcription model", () => {
  it("groups passages into paragraphs like the engine: pause over 1.5 s, or 60 s covered", () => {
    expect(
      paragraphs([
        s(0, 2, " Hello. "),
        s(3, 5, "World."),
        s(6.6, 8, "After a pause."),
        s(8.5, 9, "  "),
        s(9, 60, "Long."),
        s(61, 70, "Still long."),
        s(70.5, 72, "New paragraph after 60 s."),
      ]),
    ).toEqual([
      { start: 0, end: 5, text: "Hello. World." },
      { start: 6.6, end: 70, text: "After a pause. Long. Still long." },
      { start: 70.5, end: 72, text: "New paragraph after 60 s." },
    ]);
  });

  it("writes positions as m:ss, or h:mm:ss for an hour or more", () => {
    expect(clock(0)).toBe("0:00");
    expect(clock(65.9)).toBe("1:05");
    expect(clock(754)).toBe("12:34");
    expect(clock(3723)).toBe("1:02:03");
    expect(clock(65, true)).toBe("0:01:05");
    expect(clock(Number.NaN)).toBe("0:00");
  });

  it("copies paragraphs, with timestamps on demand", () => {
    const segments = [s(0, 2, "One."), s(5, 6, "Two.")];
    expect(copyText(segments, false)).toBe("One.\n\nTwo.");
    expect(copyText(segments, true)).toBe("[0:00] One.\n\n[0:05] Two.");
    expect(copyText([s(3700, 3702, "Late.")], true)).toBe("[1:01:40] Late.");
  });

  it("joins passages received in delta, and asks for a reread when some are missing", () => {
    const known = { job: job({ segments: [s(0, 1, "a")] }) };
    expect(
      mergeStatus(known, {
        job: job({ segments: [s(1, 2, "b")], segmentsFrom: 1, doneS: 2 }),
      }),
    ).toEqual({
      job: job({ segments: [s(0, 1, "a"), s(1, 2, "b")], doneS: 2 }),
    });
    // Un passage perdu, un autre travail, rien de connu : relire.
    expect(
      mergeStatus(known, { job: job({ segments: [], segmentsFrom: 2 }) }),
    ).toBeNull();
    expect(
      mergeStatus(known, { job: job({ id: "b", segmentsFrom: 1 }) }),
    ).toBeNull();
    expect(
      mergeStatus(undefined, { job: job({ segmentsFrom: 1 }) }),
    ).toBeNull();
    // État complet (segmentsFrom 0) ou vide : tel quel.
    const full = { job: job({ segments: [s(0, 1, "x")] }) };
    expect(mergeStatus(known, full)).toBe(full);
    expect(mergeStatus(known, { job: null })).toEqual({ job: null });
  });

  it("measures the current stage and the time left", () => {
    expect(
      stagePercent(
        job({ stage: "downloading", receivedBytes: 50, totalBytes: 200 }),
      ),
    ).toBe(25);
    expect(
      stagePercent(job({ stage: "downloading", receivedBytes: 5 })),
    ).toBeNull();
    expect(stagePercent(job({ stage: "decoding", doneS: 150 }))).toBe(25);
    expect(stagePercent(job({ doneS: 900 }))).toBe(100);
    expect(stagePercent(job({ durationS: null }))).toBeNull();
    expect(stagePercent(job({ stage: "fetching" }))).toBeNull();
    expect(remainingMinutes(job({ doneS: 0 }))).toBeNull();
    expect(remainingMinutes(job({ doneS: 300, speed: 10 }))).toBe(1);
    expect(remainingMinutes(job({ doneS: 0, speed: 2 }))).toBe(5);
    expect(remainingMinutes(job({ stage: "decoding", speed: 2 }))).toBeNull();
    expect(whisperMinutes(3600)).toBe(55);
    expect(whisperMinutes(10)).toBe(1);
  });

  it("advises against Whisper on a long sound and Parakeet on two languages", () => {
    const base = {
      durationS: null,
      language: "auto",
      parakeetLanguages: ["en", "fr"],
    };
    expect(
      engineAdvice({
        ...base,
        engine: "whisper",
        source: "file",
        durationS: 3600,
      }),
    ).toEqual([{ kind: "whisperSlowFile", minutes: 55 }]);
    expect(
      engineAdvice({
        ...base,
        engine: "whisper",
        source: "file",
        durationS: 120,
      }),
    ).toEqual([]);
    expect(engineAdvice({ ...base, engine: "whisper", source: "url" })).toEqual(
      [{ kind: "whisperSlow" }],
    );
    expect(
      engineAdvice({ ...base, engine: "parakeet", source: "url" }),
    ).toEqual([{ kind: "twoLanguages" }]);
    // Langue choisie que Parakeet connaît : il ne l'impose pas (il devine
    // celle de chaque passage), Whisper est proposé.
    expect(
      engineAdvice({
        ...base,
        engine: "parakeet",
        source: "url",
        language: "fr",
        parakeetLanguages: ["fr", "en"],
      }),
    ).toEqual([{ kind: "parakeetGuesses" }]);
    expect(
      engineAdvice({
        ...base,
        engine: "parakeet",
        source: "file",
        language: "ja",
      }),
    ).toEqual([{ kind: "parakeetLanguage" }, { kind: "twoLanguages" }]);
    expect(engineAdvice({ ...base, engine: null, source: null })).toEqual([]);
  });

  it("accepts only web addresses", () => {
    expect(isWebAddress(" https://video.test/a?b=1 ")).toBe(true);
    expect(isWebAddress("http://x.org")).toBe(true);
    expect(isWebAddress("video.test/a")).toBe(false);
    expect(isWebAddress("file:///home/a.mp4")).toBe(false);
    expect(isWebAddress("https://")).toBe(false);
  });
});
