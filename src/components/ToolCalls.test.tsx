// Affichage de l'appel `generate_image` (propre à ChatonLM) : avancement lu
// dans le flux vivant (src/lib/streams.ts), conseil après 20 s de
// préparation, cadre réservé à la taille de l'image, arrêt.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ImageProgress } from "@/bindings/ImageProgress";
import type { ToolCall } from "@/bindings/ToolCall";
import * as streams from "@/lib/streams";
import { SLOW_PREPARATION_MS, ToolCallDisplay } from "./ToolCalls";

afterEach(() => {
  cleanup();
  streams.resetStreams();
  vi.useRealTimers();
});

const HINT = "The first image takes longer: the model is being prepared.";

const CALL: ToolCall = {
  name: "generate_image",
  arguments: JSON.stringify({ prompt: "A red kitten" }),
  result: null,
  error: null,
};

/** Flux vivant dont la réponse 2 a l'appel CALL. */
function liveStream() {
  streams.start({
    streamId: "s",
    chatId: "c",
    content: "A red kitten",
    model: "m",
  });
  streams.apply("s", { type: "replyStarted", messageId: 2, createdAt: 1 });
  streams.apply("s", { type: "toolCalls", messageId: 2, toolCalls: [CALL] });
}

function advance(fields: Partial<ImageProgress>) {
  act(() =>
    streams.apply("s", {
      type: "imageProgress",
      messageId: 2,
      index: 0,
      progress: {
        stage: "starting",
        step: null,
        total: null,
        preview: null,
        cpu: false,
        width: 512,
        height: 512,
        ...fields,
      },
    }),
  );
  // Fragments regroupés (16 ms).
  act(() => vi.advanceTimersByTime(20));
}

function renderCall(call = CALL, isStreaming = true, isRemoteRunning = false) {
  return render(
    <ToolCallDisplay
      call={call}
      messageId={2}
      index={0}
      isStreaming={isStreaming}
      isRemoteRunning={isRemoteRunning}
    />,
  );
}

describe("generate_image call", () => {
  it("gives a hint after 20 s of preparation, gone once it generates", () => {
    vi.useFakeTimers();
    liveStream();
    renderCall();
    // Avant le premier avancement : démarrage déjà compté.
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS - 1000));
    advance({ stage: "starting" });
    expect(screen.queryByText(HINT)).toBeNull();
    act(() => vi.advanceTimersByTime(1000));
    expect(screen.getByText(HINT)).toBeTruthy();
    advance({ stage: "generating", step: 1, total: 4 });
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("counts the 20 s again from each preparation stage", () => {
    vi.useFakeTimers();
    liveStream();
    renderCall();
    advance({ stage: "starting" });
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS - 1000));
    // Chargement : nouveau compte, le démarrage ne compte plus.
    advance({ stage: "loading" });
    act(() => vi.advanceTimersByTime(1000));
    expect(screen.queryByText(HINT)).toBeNull();
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS - 1000));
    expect(screen.getByText(HINT)).toBeTruthy();
  });

  it("the hint shown before a wait does not come back at once after it", () => {
    vi.useFakeTimers();
    liveStream();
    renderCall();
    advance({ stage: "starting" });
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS));
    expect(screen.getByText(HINT)).toBeTruthy();
    advance({ stage: "queued" });
    expect(screen.queryByText(HINT)).toBeNull();
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS));
    expect(screen.queryByText(HINT)).toBeNull();
    advance({ stage: "loading" });
    expect(screen.queryByText(HINT)).toBeNull();
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS - 1000));
    expect(screen.queryByText(HINT)).toBeNull();
    act(() => vi.advanceTimersByTime(1000));
    expect(screen.getByText(HINT)).toBeTruthy();
  });

  it("no hint for a short preparation", () => {
    vi.useFakeTimers();
    liveStream();
    renderCall();
    advance({ stage: "loading" });
    act(() => vi.advanceTimersByTime(5000));
    advance({ stage: "generating", step: 1, total: 4 });
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS));
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("reserves a frame with the proportions of the image, at most 512 px", () => {
    vi.useFakeTimers();
    liveStream();
    renderCall();
    const frame = () => screen.getByTestId("image-frame");
    // Avant l'annonce de la taille : carré.
    expect(frame().style.aspectRatio).toBe("512 / 512");
    advance({ stage: "starting", width: 1536, height: 768 });
    expect(frame().style.aspectRatio).toBe("1536 / 768");
    expect(frame().style.width).toBe("512px");
    expect(frame().style.maxWidth).toBe("100%");
    advance({ stage: "starting", width: 256, height: 384 });
    expect(frame().style.width).toBe("256px");
  });

  it("before any progress, the frame takes the size asked by the call", () => {
    const sized = (args: Record<string, unknown>) => {
      cleanup();
      renderCall({
        ...CALL,
        arguments: JSON.stringify({ prompt: "A red kitten", ...args }),
      });
      return screen.getByTestId("image-frame").style.aspectRatio;
    };
    expect(sized({ width: 1024, height: 768 })).toBe("1024 / 768");
    // Comme le moteur : multiple de 16 le plus proche, borné à [256, 1536].
    expect(sized({ width: 500, height: "777" })).toBe("496 / 784");
    expect(sized({ width: 10, height: 5000 })).toBe("256 / 1536");
    // Côté absent ou illisible : 512.
    expect(sized({ width: 768 })).toBe("768 / 512");
    expect(sized({ width: "big", height: "" })).toBe("512 / 512");
  });

  it("a call without result outside the reply being written is stopped", () => {
    renderCall(CALL, false);
    expect(screen.getByText("Image stopped")).toBeTruthy();
    expect(screen.queryByText("Creating an image…")).toBeNull();
  });

  it("an Easy Link bot's reply still running keeps its image in progress", () => {
    vi.useFakeTimers();
    // Aucun flux dans la page : préparation sans avancement.
    renderCall(CALL, false, true);
    expect(screen.getByText("Creating an image…")).toBeTruthy();
    expect(screen.getByText("A red kitten")).toBeTruthy();
    expect(screen.getByTestId("image-frame")).toBeTruthy();
    expect(screen.queryByText("Image stopped")).toBeNull();
    expect(screen.queryByRole("progressbar")).toBeNull();
    act(() => vi.advanceTimersByTime(SLOW_PREPARATION_MS));
    expect(screen.getByText(HINT)).toBeTruthy();
  });

  it("other tools keep their line", () => {
    renderCall({ ...CALL, name: "web_search", arguments: '{"query":"cats"}' });
    expect(document.body.textContent).toBe("Searching for cats…");
  });
});
