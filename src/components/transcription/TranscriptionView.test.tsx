// Page d'une transcription face au simulateur : étapes (fiche de la vidéo,
// octets, décodage, transcription avec vitesse et temps restant), texte au
// fil de l'eau (passages en delta, état relu s'il en manque), « Cancel »,
// rechargement (l'état revient), fin (paragraphes, horodatage, copie,
// .txt/.md/.srt enregistrés), échecs d'après leur code (ModelHub, mise à
// jour de yt-dlp, nouvel essai), texte partiel, renommer, supprimer ;
// document de lecture (Synthétiser, flux, Stop, erreur, enregistrement
// .md/.docx, PDF refusé avec le .docx proposé, resynthèse) ; textes
// traduits.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { TranscriptionRequest } from "@/bindings/TranscriptionRequest";
import { setLanguage } from "@/i18n";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import {
  type FakeTranscription,
  fakeTranscription,
  installedTools,
  PARAKEET,
  segment,
} from "@/test/transcription";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(prepare: (fake: FakeTranscription) => void = () => {}) {
  const simulator = installSimulator();
  fakeBackend(simulator);
  const fake = fakeTranscription(simulator);
  fake.tools = installedTools();
  prepare(fake);
  return { simulator, fake };
}

/** Transcription d'une adresse lancée depuis l'accueil ; sa page ouverte. */
async function startFromAddress(
  fake: FakeTranscription,
  url = "https://video.test/ok",
) {
  const app = renderApp("/transcription");
  const user = userEvent.setup();
  await user.type(
    await screen.findByRole("textbox", { name: "Video address" }),
    url,
  );
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await screen.findByTestId("transcription-view");
  return { app, user, id: fake.job!.id };
}

/** Transcription finie, ouverte directement. */
function openDone(fake: FakeTranscription) {
  const detail = fake.add({ title: "Lecture 3", durationS: 125 }, [
    segment(0, 4, "Welcome to the lecture."),
    segment(4.5, 8, "Today, copyright."),
    segment(12, 15, "First point."),
  ]);
  renderApp(`/transcription/${detail.item.id}`);
  return detail;
}

const steps = () =>
  [...document.querySelectorAll("[data-step]")].map(
    (step) =>
      `${step.getAttribute("data-step")}:${step.getAttribute("data-state")}`,
  );
const detailText = () =>
  screen.getByTestId("transcription-step-detail").textContent;
const paragraphs = () =>
  screen.queryAllByTestId("transcription-paragraph").map((p) => p.textContent);

describe("Transcription page", () => {
  it("follows a video from its page to its text, passage by passage", async () => {
    const { fake, simulator } = setup();
    const { user } = await startFromAddress(fake);
    expect(screen.getByTestId("transcription-progress").textContent).toContain(
      "Preparing…",
    );

    await fake.progress({ stage: "fetching" });
    await waitFor(() => expect(steps()[0]).toBe("fetching:current"));
    expect(steps()).toEqual([
      "fetching:current",
      "downloading:pending",
      "decoding:pending",
      "transcribing:pending",
    ]);
    // Fiche connue : titre, chaîne, durée.
    await fake.progress({
      stage: "downloading",
      title: "CopyrightX: Lecture 3.2",
      channel: "Berkman Klein Center",
      durationS: 721,
      url: "https://video.test/ok",
      receivedBytes: 2 * 2 ** 20,
      totalBytes: 8 * 2 ** 20,
    });
    await waitFor(() => expect(steps()[1]).toBe("downloading:current"));
    expect(screen.getByTestId("transcription-title").textContent).toBe(
      "CopyrightX: Lecture 3.2",
    );
    const meta = screen.getByTestId("transcription-meta").textContent;
    expect(meta).toContain("by Berkman Klein Center");
    expect(meta).toContain("12:01");
    expect(detailText()).toBe("2.0 MB of 8.0 MB");
    expect(
      screen
        .getByRole("progressbar", { name: "Progress" })
        .getAttribute("aria-valuenow"),
    ).toBe("25");

    await fake.progress({ stage: "decoding", doneS: 360 });
    await waitFor(() => expect(steps()[2]).toBe("decoding:current"));
    expect(steps()).toEqual([
      "fetching:done",
      "downloading:done",
      "decoding:current",
      "transcribing:pending",
    ]);
    expect(detailText()).toBe("6:00 of 12:01");
    expect(screen.getByTestId("transcription-live").textContent).toContain(
      "The text appears here as it is transcribed.",
    );

    await fake.progress({ stage: "transcribing", doneS: 40, speed: 10.7 }, [
      segment(0, 3, "Hello and welcome."),
    ]);
    await waitFor(() =>
      expect(detailText()).toBe(
        "0:40 of 12:01 · 10.7× real time · about 2 minutes left",
      ),
    );
    await fake.progress({ doneS: 80 }, [
      segment(3.5, 6, "Today we talk."),
      segment(9, 11, "New idea."),
    ]);
    await waitFor(() =>
      expect(paragraphs()).toEqual([
        "Hello and welcome. Today we talk.",
        "New idea.",
      ]),
    );

    // Un passage perdu : l'état complet est relu.
    const reads = simulator.callsTo("transcription_status").length;
    fake.job!.segments.push(segment(12, 13, "Lost."));
    await fake.progress({ doneS: 90 }, [segment(14, 15, "After.")]);
    await waitFor(() =>
      expect(simulator.callsTo("transcription_status").length).toBe(reads + 1),
    );
    await waitFor(() =>
      expect(paragraphs()).toEqual([
        "Hello and welcome. Today we talk.",
        "New idea. Lost. After.",
      ]),
    );

    // Annuler.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(simulator.callsTo("transcription_cancel")).toHaveLength(1);

    // Fin : le texte enregistré, ses outils.
    await fake.finish("done");
    await screen.findByTestId("transcription-transcript");
    expect(screen.queryByTestId("transcription-progress")).toBeNull();
    await waitFor(() =>
      expect(paragraphs()).toEqual([
        "Hello and welcome. Today we talk.",
        "New idea. Lost. After.",
      ]),
    );
    expect(
      screen.getByRole("button", { name: "Open the video page" }),
    ).toBeTruthy();
  });

  it("comes back after a reload, with every passage", async () => {
    setup((fake) => {
      fake.job = {
        id: "t-9",
        source: "file",
        origin: "/home/me/talk.mp3",
        stage: "transcribing",
        model: PARAKEET,
        engine: "parakeet",
        language: null,
        title: "talk.mp3",
        channel: null,
        url: null,
        site: null,
        durationS: 300,
        receivedBytes: null,
        totalBytes: null,
        doneS: 120,
        speed: 12,
        segments: [
          segment(0, 2, "Already here."),
          segment(2.5, 4, "And this."),
        ],
        segmentsFrom: 0,
        error: null,
      };
      fake.add({ id: "t-9", state: "running", title: "talk.mp3" });
    });
    renderApp("/transcription/t-9");
    await screen.findByTestId("transcription-progress");
    expect(steps()).toEqual(["decoding:done", "transcribing:current"]);
    expect(paragraphs()).toEqual(["Already here. And this."]);
    // Pas de suppression pendant le travail.
    expect(screen.queryByTestId("transcription-delete")).toBeNull();
  });

  it("shows the text by paragraphs, with timestamps, copy and the three text formats", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    openDone(fake);
    await screen.findByTestId("transcription-transcript");
    expect(paragraphs()).toEqual([
      "Welcome to the lecture. Today, copyright.",
      "First point.",
    ]);
    expect(screen.queryByTestId("transcription-stamp")).toBeNull();
    await user.click(screen.getByRole("checkbox", { name: "Timestamps" }));
    expect(paragraphs()).toEqual([
      "0:00Welcome to the lecture. Today, copyright.",
      "0:12First point.",
    ]);

    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(screen.getByRole("button", { name: /Copy/ }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        "[0:00] Welcome to the lecture. Today, copyright.\n\n[0:12] First point.",
      ),
    );

    for (const [name, kind] of [
      ["Text (.txt)", "txt"],
      ["Markdown (.md)", "md"],
      ["Subtitles (.srt)", "srt"],
    ]) {
      await user.click(screen.getByRole("button", { name }));
      await waitFor(() =>
        expect(screen.getByTestId("transcription-saved").textContent).toBe(
          `Saved: /home/me/saved.${kind}`,
        ),
      );
    }
    // Texte du moteur (horodatage demandé), nommé d'après le titre.
    expect(
      fake.saves.map(({ kind, name, text }) => ({ kind, name, text })),
    ).toEqual(
      ["txt", "md", "srt"].map((kind) => ({
        kind,
        name: "Lecture 3",
        text: `${kind}:[0:00] Welcome to the lecture. Today, copyright. First point.`,
      })),
    );

    // Boîte annulée : rien n'est dit ; échec : la raison.
    fake.savePath = null;
    await user.click(screen.getByRole("button", { name: "Text (.txt)" }));
    await waitFor(() => expect(fake.saves).toHaveLength(4));
    fake.failures.set("save_document", {
      kind: "system",
      message: "Disk full",
    });
    await user.click(screen.getByRole("button", { name: "Text (.txt)" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The file couldn't be saved: Disk full",
    );
  });

  it("says why a transcription failed, from its code, with what can be done", async () => {
    const { fake, simulator } = setup();
    const { user } = await startFromAddress(
      fake,
      "https://video.test/outdated",
    );
    await fake.progress({ stage: "fetching" });
    await fake.finish("failed", {
      code: "fetch",
      fetch: "outdated",
      message: "The site changed: update yt-dlp, then try again.",
    });
    const error = await screen.findByTestId("transcription-error");
    expect(error.getAttribute("data-fetch")).toBe("outdated");
    expect(error.textContent).toContain("The video couldn't be retrieved");
    expect(error.textContent).toContain(
      "The site changed: update yt-dlp, then try again.",
    );
    await user.click(
      within(error).getByRole("button", { name: "Update yt-dlp" }),
    );
    expect(simulator.callsTo("tools_update")).toHaveLength(1);
    // Nouvel essai : même source, même modèle.
    await user.click(within(error).getByRole("button", { name: "Try again" }));
    await waitFor(() =>
      expect(simulator.callsTo("transcription_start")).toHaveLength(2),
    );
    const [first, second] = simulator
      .callsTo("transcription_start")
      .map((args) => args.request as TranscriptionRequest);
    expect(second.source).toEqual(first.source);
    expect(second.model).toBe(PARAKEET);
  });

  it("points to the ModelHub when the model is missing, and keeps a private video's reason", async () => {
    const { fake } = setup();
    const { user } = await startFromAddress(fake, "https://video.test/private");
    await fake.finish("failed", {
      code: "noModel",
      fetch: null,
      message: "Install a transcription model first.",
    });
    const error = await screen.findByTestId("transcription-error");
    expect(error.textContent).toContain("No transcription model is installed");
    expect(
      within(error)
        .getByRole("link", { name: "Open the ModelHub" })
        .getAttribute("href"),
    ).toBe("/hub/transcription");
    expect(
      within(error).queryByRole("button", { name: "Update yt-dlp" }),
    ).toBeNull();

    await user.click(within(error).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(fake.job?.stage).toBe("preparing"));
    await fake.finish("failed", {
      code: "fetch",
      fetch: "private",
      message: "This video is private.",
    });
    const next = await screen.findByTestId("transcription-error");
    expect(next.getAttribute("data-fetch")).toBe("private");
    expect(next.textContent).toContain("This video is private.");
    expect(within(next).queryByRole("link")).toBeNull();
  });

  it("keeps what was transcribed before a failure, and says so", async () => {
    const { fake } = setup((fake) => {
      fake.add({ title: "Cut short", state: "interrupted" }, [
        segment(0, 2, "Part one."),
      ]);
    });
    renderApp("/transcription/t-1");
    expect(
      (await screen.findByTestId("transcription-partial")).textContent,
    ).toBe(
      "ChatonLM was closed during the transcription: here is what was transcribed.",
    );
    expect(paragraphs()).toEqual(["Part one."]);
    expect(fake.items).toHaveLength(1);
  });

  it("renames and deletes a transcription", async () => {
    const user = userEvent.setup();
    const { fake, simulator } = setup();
    openDone(fake);
    await screen.findByTestId("transcription-transcript");
    await user.click(screen.getByRole("button", { name: "Rename" }));
    const input = screen.getByRole<HTMLInputElement>("textbox", {
      name: "Title",
    });
    expect(document.activeElement).toBe(input);
    await user.clear(input);
    await user.type(input, "Copyright, lecture 3{Enter}");
    await waitFor(() =>
      expect(screen.getByTestId("transcription-title").textContent).toBe(
        "Copyright, lecture 3",
      ),
    );
    expect(simulator.callsTo("transcription_rename")).toEqual([
      { id: "t-1", title: "Copyright, lecture 3" },
    ]);

    await user.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = screen.getByRole("alertdialog", {
      name: "Delete “Copyright, lecture 3”?",
    });
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await screen.findByTestId("transcription-home");
    expect(simulator.callsTo("transcription_delete")).toEqual([{ id: "t-1" }]);
  });

  it("writes the reading document in a stream, stops it, then saves it", async () => {
    const user = userEvent.setup();
    const { fake, simulator } = setup();
    const detail = openDone(fake);
    const panel = await screen.findByTestId("transcription-synthesis");
    expect(panel.textContent).toContain(
      "A language model rewrites the transcription as a document to read",
    );
    // « Auto » par défaut : aucun modèle envoyé.
    const model = within(panel).getByRole<HTMLSelectElement>("combobox", {
      name: "Model",
    });
    expect(model.value).toBe("");
    await user.click(within(panel).getByRole("button", { name: "Summarize" }));
    expect(simulator.callsTo("transcription_synthesize")).toEqual([
      { id: detail.item.id, model: null },
    ]);
    const running = {
      id: detail.item.id,
      stage: "running" as const,
      markdown: "# Lecture 3\n\nCopyright is",
      model: "qwen3:0.6b",
      error: null,
    };
    // Notes d'abord : rien à montrer, la page attend.
    await fake.synthesisEvent({ ...running, markdown: "" });
    await waitFor(() =>
      expect(
        screen.getByTestId("transcription-synthesis-phase").textContent,
      ).toBe("Reading the transcription…"),
    );
    expect(screen.queryByTestId("transcription-document")).toBeNull();
    await fake.synthesisEvent(running);
    await waitFor(() =>
      expect(
        screen.getByTestId("transcription-synthesis-phase").textContent,
      ).toBe("Writing the document…"),
    );
    expect(panel.textContent).toContain("Writing the document…");
    expect(
      within(panel).getByRole("heading", { name: "Lecture 3" }),
    ).toBeTruthy();
    await fake.synthesisEvent({
      ...running,
      markdown: "# Lecture 3\n\nCopyright is **law**.",
    });
    await waitFor(() =>
      expect(
        panel.querySelector(
          "[data-testid=transcription-document] span.font-semibold",
        )?.textContent,
      ).toBe("law"),
    );

    await user.click(within(panel).getByRole("button", { name: "Stop" }));
    expect(simulator.callsTo("transcription_synthesis_cancel")).toEqual([
      { id: detail.item.id },
    ]);
    await fake.synthesisEvent({ ...running, stage: "cancelled" });
    await waitFor(() =>
      expect(panel.textContent).toContain(
        "The document was stopped before the end.",
      ),
    );

    // Un modèle choisi, puis le document fini.
    await user.selectOptions(model, "qwen3.5:0.8b");
    await user.click(within(panel).getByRole("button", { name: "Summarize" }));
    expect(simulator.callsTo("transcription_synthesize")[1]).toEqual({
      id: detail.item.id,
      model: "qwen3.5:0.8b",
    });
    await fake.synthesisEvent({
      ...running,
      stage: "done",
      model: "qwen3.5:0.8b",
      markdown: "# Lecture 3\n\n## Key points\n\n- Copyright",
    });
    await waitFor(() =>
      expect(
        within(panel).getByRole("button", { name: "Summarize again" }),
      ).toBeTruthy(),
    );
    expect(panel.textContent).toContain("Written by qwen3.5:0.8b");
    expect(
      within(panel).getByRole("heading", { name: "Key points" }),
    ).toBeTruthy();

    await user.click(
      within(panel).getByRole("button", { name: "Markdown (.md)" }),
    );
    await waitFor(() => expect(fake.saves).toHaveLength(1));
    expect(fake.saves[0]).toMatchObject({
      kind: "md",
      name: "Lecture 3",
      text: "# Lecture 3\n\n## Key points\n\n- Copyright",
    });
    await user.click(
      within(panel).getByRole("button", { name: "Word (.docx)" }),
    );
    await waitFor(() => expect(fake.saves).toHaveLength(2));
    expect(fake.saves[1].kind).toBe("docx");
    expect(String.fromCharCode(...fake.saves[1].bytes.slice(0, 2))).toBe("PK");
  });

  it("offers Word when the PDF refuses characters, and shows a synthesis error", async () => {
    const user = userEvent.setup();
    const { fake } = setup((fake) => {
      fake.synthesis = {
        id: "t-1",
        stage: "error",
        markdown: "",
        model: "qwen3:0.6b",
        error: {
          code: "engineFailed",
          message: "The model stopped answering.",
        },
      } as FakeTranscription["synthesis"];
    });
    const detail = openDone(fake);
    detail.summary = {
      markdown: "# Лекция\n\nТекст.",
      model: "qwen3:0.6b",
      at: Date.UTC(2026, 10, 14),
    };
    detail.item.hasSummary = true;
    const panel = await screen.findByTestId("transcription-synthesis");
    const failure = await within(panel).findByTestId(
      "transcription-synthesis-error",
    );
    expect(failure.getAttribute("data-code")).toBe("engineFailed");
    expect(failure.textContent).toBe(
      "The model stopped workingThe model stopped answering.",
    );
    await user.click(within(panel).getByRole("button", { name: "PDF (.pdf)" }));
    // pdfmake chargé au premier PDF : lent sous jsdom.
    const alert = await within(panel).findByText(
      /characters the PDF font can't show/,
      {},
      { timeout: 20_000 },
    );
    expect(alert.textContent).toContain("Export it as .docx instead.");
    expect(fake.saves).toHaveLength(0);
    await user.click(
      within(panel).getByRole("button", { name: "Save as Word (.docx)" }),
    );
    await waitFor(() => expect(fake.saves).toHaveLength(1));
    expect(fake.saves[0].kind).toBe("docx");
  });

  it("is translated: Spanish, French, no text outside the catalogs", async () => {
    const { fake } = setup();
    const { user } = await startFromAddress(fake);
    await fake.progress(
      {
        stage: "transcribing",
        durationS: 100,
        doneS: 10,
        speed: 5,
        title: "Talk",
      },
      [segment(0, 2, "Words.")],
    );
    await waitFor(() => expect(detailText()).toContain("real time"));
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(
        screen.getByTestId("transcription-progress").textContent,
      ).toContain("Transcription"),
    );
    expect(detailText()).toContain("5,0 × le temps réel");
    act(() => setLanguage("es"));
    await waitFor(() => expect(detailText()).toContain("tiempo real"));
    act(() => setLanguage("en"));
    usePseudoLanguage();
    const allow = ["Talk", "Words.", PARAKEET];
    expect(
      untranslatedTexts(screen.getByTestId("transcription-view"), allow),
    ).toEqual([]);
    await fake.finish("failed", {
      code: "noSpeech",
      fetch: null,
      message: "No speech.",
    });
    await screen.findByTestId("transcription-transcript");
    expect(
      untranslatedTexts(screen.getByTestId("transcription-view"), [
        ...allow,
        "No speech.",
      ]),
    ).toEqual([]);
    void user;
  });
});
