// Panneau, lot 4a : carte du cours de l'Étude (préparation, paliers, fichiers
// partis et retirés, ouverture dans l'app) et carte de l'image (avancement,
// arrêt, aperçu, enregistrement ou abandon, remplacement annoncé) ; textes
// traduits.
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import type {
  HostMessage,
  ImageView,
  PanelMessage,
  PanelState,
  StudyView,
} from "../shared/protocol";
import { Panel } from "./Panel";

const CONNECTED: PanelState = {
  connection: "connected",
  models: ["qwen3:0.6b"],
  cloudModels: [],
  model: "qwen3:0.6b",
  ephemeral: false,
  chatId: null,
  busy: false,
  generation: 0,
  notice: null,
};

const STUDY: StudyView = {
  id: 1,
  kind: "project",
  topic: "How the work project works",
  status: "planning",
  courseId: "course-1",
  levelsReady: 0,
  levelsTotal: 0,
  error: null,
  files: ["README.md", "src/index.ts"],
  blocked: [
    { label: "src/index.ts, line 2", reason: "secretValue" },
    { label: ".env", reason: "secretFile" },
  ],
};

const IMAGE: ImageView = {
  id: 1,
  target: "site/img/hero.png",
  prompt: "A red kitten",
  status: "generating",
  stage: "generating",
  step: 5,
  total: 20,
  cpu: true,
  preview: null,
  width: 1024,
  height: 576,
  exists: false,
  error: null,
};

afterEach(() => cleanup());

function fromHost(data: HostMessage) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

function renderPanel() {
  const post = vi.fn<(message: PanelMessage) => void>();
  render(<Panel post={post} />);
  fromHost({ type: "init", locale: "en", theme: "light", state: CONNECTED });
  return post;
}

const posted = (post: ReturnType<typeof renderPanel>) =>
  post.mock.calls.map(([message]) => message).filter((m) => m.type !== "ready");

describe("course card", () => {
  it("shows the preparation, the files sent and what was removed", () => {
    renderPanel();
    fromHost({ type: "study", study: STUDY });
    const card = screen.getByRole("region", { name: "Course" });
    expect(card.textContent).toContain("Course: How the work project works");
    expect(screen.getByTestId("study-progress").textContent).toBe(
      "ChatonLM is planning the course…",
    );
    expect(card.textContent).toContain("Sent: README.md, src/index.ts");
    expect(card.textContent).toContain("Secret removed: src/index.ts, line 2");
    expect(card.textContent).toContain("Not sent: .env (may hold secrets)");
  });

  it("counts the ready levels and opens the course in the app", () => {
    const post = renderPanel();
    fromHost({
      type: "study",
      study: { ...STUDY, status: "ready", levelsReady: 2, levelsTotal: 5 },
    });
    expect(screen.getByTestId("study-progress").textContent).toBe(
      "2 of 5 levels ready",
    );
    fireEvent.click(screen.getByRole("button", { name: "Open in ChatonLM" }));
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(posted(post)).toEqual([
      { type: "study", command: "open" },
      { type: "study", command: "close" },
    ]);
    fromHost({ type: "study", study: null });
    expect(screen.queryByTestId("study")).toBeNull();
  });

  it("shows the app's error", () => {
    renderPanel();
    fromHost({
      type: "study",
      study: { ...STUDY, status: "failed", courseId: null, error: "No model." },
    });
    expect(screen.getByTestId("study-progress").textContent).toBe("No model.");
    expect(
      screen.queryByRole("button", { name: "Open in ChatonLM" }),
    ).toBeNull();
  });
});

describe("image card", () => {
  it("shows the progress and stops", () => {
    const post = renderPanel();
    fromHost({ type: "image", image: IMAGE });
    expect(screen.getByTestId("image-progress").textContent).toBe(
      "Step 5 of 20",
    );
    expect(
      screen
        .getByRole("progressbar", { name: "Image progress" })
        .getAttribute("aria-valuenow"),
    ).toBe("25");
    expect(screen.getByText(/Made by the processor/)).toBeTruthy();
    // Pendant la génération, ni enregistrement ni fermeture.
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(posted(post)).toEqual([{ type: "image", command: "cancel" }]);
  });

  it("shows the preview, then saves or discards", () => {
    const post = renderPanel();
    fromHost({
      type: "image",
      image: {
        ...IMAGE,
        status: "ready",
        stage: null,
        preview: "data:image/png;base64,AAAA",
      },
    });
    expect(screen.getByRole("img", { name: "A red kitten" })).toBeTruthy();
    expect(screen.getByTestId("image-progress").textContent).toBe(
      "Not saved yet. Save it to site/img/hero.png?",
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(posted(post)).toEqual([
      { type: "image", command: "save" },
      { type: "image", command: "discard" },
    ]);
  });

  it("announces a replacement", () => {
    renderPanel();
    fromHost({
      type: "image",
      image: { ...IMAGE, status: "ready", stage: null, exists: true },
    });
    expect(screen.getByRole("button", { name: "Replace" })).toBeTruthy();
    expect(screen.getByTestId("image-progress").textContent).toContain(
      "already exists",
    );
  });
});

describe("errors and announcements", () => {
  /** Toute classe d'erreur rouge a sa variante lisible en sombre (rapport
   * 94, M4 : #b3261e sur le fond sombre de la carte ≈ 2,2:1). */
  const redWithoutDark = () =>
    [...document.querySelectorAll("[class]")]
      .map((element) => element.getAttribute("class") ?? "")
      .filter((c) => /\btext-red-\d+/.test(c) && !/\bdark:text-red-/.test(c));

  it("keeps the card errors readable in the dark theme", () => {
    renderPanel();
    fromHost({ type: "theme", theme: "dark" });
    fromHost({
      type: "study",
      study: { ...STUDY, status: "failed", error: "No model." },
    });
    fromHost({
      type: "image",
      image: { ...IMAGE, status: "failed", stage: null, error: "Broken." },
    });
    expect(screen.getByTestId("study-progress").className).toContain(
      "dark:text-red-400",
    );
    expect(screen.getByTestId("image-progress").className).toContain(
      "dark:text-red-400",
    );
    expect(redWithoutDark()).toEqual([]);
  });

  it("announces the image stages, not every sampling step", () => {
    renderPanel();
    fromHost({ type: "image", image: IMAGE });
    const status = screen.getByRole("status");
    expect(status.textContent).toBe("Making the image…");
    fromHost({ type: "image", image: { ...IMAGE, step: 6 } });
    fromHost({ type: "image", image: { ...IMAGE, step: 7 } });
    expect(screen.getByRole("status").textContent).toBe("Making the image…");
    // Le pas reste visible, hors de la zone annoncée.
    expect(screen.getByTestId("image-progress").textContent).toBe(
      "Step 7 of 20",
    );
    expect(
      screen.getByTestId("image-progress").getAttribute("role"),
    ).toBeNull();
    fromHost({
      type: "image",
      image: { ...IMAGE, status: "ready", stage: null },
    });
    expect(screen.getByRole("status").textContent).toBe(
      "Not saved yet. Save it to site/img/hero.png?",
    );
  });
});

describe("lot 4a cards", () => {
  it("has every text translated", () => {
    usePseudoLanguage();
    const allow = [
      "qwen3:0.6b",
      "How the work project works",
      "README.md, src/index.ts",
      "src/index.ts, line 2",
      ".env",
      "site/img/hero.png",
      "A red kitten",
    ];
    renderPanel();
    const studies: Partial<StudyView>[] = [
      {},
      { status: "creating" },
      { status: "ready", levelsReady: 1, levelsTotal: 3 },
      { status: "ready", levelsTotal: 0 },
      { status: "failed", error: null, files: [] },
    ];
    for (const study of studies) {
      fromHost({ type: "study", study: { ...STUDY, ...study } });
      expect(
        untranslatedTexts(document.body, allow),
        JSON.stringify(study),
      ).toEqual([]);
    }
    const images: Partial<ImageView>[] = [
      {},
      { stage: "starting" },
      { stage: "queued" },
      { stage: "loading" },
      { status: "ready", stage: null },
      { status: "ready", stage: null, exists: true },
      { status: "saving" },
      { status: "saved" },
      { status: "cancelled" },
      { status: "failed", error: null },
    ];
    for (const image of images) {
      fromHost({ type: "image", image: { ...IMAGE, ...image } });
      expect(
        untranslatedTexts(document.body, allow),
        JSON.stringify(image),
      ).toEqual([]);
    }
  });
});
