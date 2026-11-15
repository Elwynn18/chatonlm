// Accueil de l'outil Transcription face au simulateur : carte de la page
// Tools, fichier choisi (sélecteur natif, fiche lue, refus d'avance),
// modèle et langue avec les conseils (Whisper lent, Parakeet et deux
// langues, langue inconnue de Parakeet), aucun modèle (ModelHub), adresse
// (accord avant le téléchargement de yt-dlp et Deno, refus, installation
// suivie puis transcription lancée d'elle-même, outils déjà là), une seule
// à la fois, historique (ouvrir, supprimer après confirmation), aucun
// bouton pour enregistrer le son ; textes traduits.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { setLanguage } from "@/i18n";
import { setSidebarOpen } from "@/lib/sidebar";
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
  WHISPER,
} from "@/test/transcription";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(
  prepare: (fake: FakeTranscription) => void = () => {},
  path = "/transcription",
) {
  const simulator = installSimulator();
  fakeBackend(simulator);
  const fake = fakeTranscription(simulator);
  prepare(fake);
  act(() => setSidebarOpen(true));
  const app = renderApp(path);
  return { simulator, fake, app };
}

const form = () => screen.getByTestId("transcription-form");
const address = () =>
  screen.getByRole<HTMLInputElement>("textbox", { name: "Video address" });
const startButton = () =>
  screen.getByRole<HTMLButtonElement>("button", { name: "Transcribe" });
const advice = () =>
  [...document.querySelectorAll("[data-advice]")].map((item) =>
    item.getAttribute("data-advice"),
  );

describe("Transcription home", () => {
  it("opens from its card on the Tools page, Tools staying highlighted", async () => {
    const user = userEvent.setup();
    const { app } = setup(() => {}, "/tools");
    await user.click(await screen.findByTestId("tool-card-transcription"));
    await screen.findByTestId("transcription-form");
    expect(app.path()).toBe("/transcription");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Transcription",
    );
    expect(
      document.querySelector('nav a[href="/tools"]')!.className.split(" "),
    ).toContain("bg-accent");
    expect(await screen.findByTestId("transcription-empty")).toBeTruthy();
    // Une transcription, pas un téléchargeur : aucun bouton pour garder le
    // son ou la vidéo, et aucun site mis en avant.
    const page = screen.getByTestId("transcription-home");
    expect(page.textContent).not.toMatch(
      /youtube|download the video|save the audio/i,
    );
    expect(
      within(page).queryByRole("button", { name: /download|save/i }),
    ).toBeNull();
  });

  it("transcribes a chosen file with the chosen model and language, then opens its page", async () => {
    const user = userEvent.setup();
    const { simulator, fake, app } = setup((fake) => {
      fake.picked = "/home/me/talk.mkv";
      fake.probes["/home/me/talk.mkv"] = {
        name: "talk.mkv",
        durationS: 3600,
        error: null,
      };
    });
    await screen.findByTestId("transcription-form");
    expect(startButton().disabled).toBe(true);
    await user.click(screen.getByRole("button", { name: "Choose a file…" }));
    const chip = await screen.findByTestId("transcription-file");
    expect(chip.textContent).toContain("talk.mkv");
    expect(chip.textContent).toContain("1:00:00");
    expect(simulator.callsTo("transcription_probe")).toEqual([
      { path: "/home/me/talk.mkv" },
    ]);
    // Parakeet d'office : il ne garde qu'une langue par passage.
    const model = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Model",
    });
    expect(model.value).toBe(PARAKEET);
    // Seuls les modèles installés sont proposés.
    expect([...model.options].map((o) => o.value)).toEqual([PARAKEET, WHISPER]);
    expect(advice()).toEqual(["twoLanguages"]);
    // Langue que Parakeet ne connaît pas : Whisper conseillé.
    const language = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Language",
    });
    await user.selectOptions(language, "ja");
    expect(advice()).toEqual(["parakeetLanguage", "twoLanguages"]);
    expect(form().textContent).toContain(
      "Parakeet doesn't know Japanese: choose a Whisper model.",
    );
    // Whisper sur une heure : lent, le temps est annoncé.
    await user.selectOptions(model, WHISPER);
    expect(advice()).toEqual(["whisperSlowFile"]);
    expect(form().textContent).toContain(
      "Whisper is slow on a processor: allow about 55 minutes for this file.",
    );

    await user.click(startButton());
    await screen.findByTestId("transcription-view");
    expect(simulator.callsTo("transcription_start")).toEqual([
      {
        request: {
          source: { kind: "file", path: "/home/me/talk.mkv" },
          model: WHISPER,
          language: "ja",
        },
      },
    ]);
    expect(app.path()).toBe(`/transcription/${fake.job!.id}`);
  });

  it("refuses a file the engine can't read, with the reason", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((fake) => {
      fake.picked = "/home/me/film.avi";
      fake.probes["/home/me/film.avi"] = {
        name: "film.avi",
        durationS: null,
        error: {
          code: "unsupportedFormat",
          fetch: null,
          message: "AVI files can't be read. Formats read: MP4, MKV…",
        },
      };
    });
    await screen.findByTestId("transcription-form");
    await user.click(screen.getByRole("button", { name: "Choose a file…" }));
    const error = await screen.findByTestId("transcription-error");
    expect(error.getAttribute("data-code")).toBe("unsupportedFormat");
    expect(error.textContent).toContain("This file format can't be read");
    expect(error.textContent).toContain("AVI files can't be read.");
    expect(startButton().disabled).toBe(true);
    // Retiré : plus d'erreur.
    await user.click(screen.getByRole("button", { name: "Remove film.avi" }));
    expect(screen.queryByTestId("transcription-error")).toBeNull();
    expect(simulator.callsTo("transcription_start")).toEqual([]);
  });

  it("offers the ModelHub when no transcription model is installed", async () => {
    setup((fake) => {
      fake.options.models = fake.options.models.map((m) => ({
        ...m,
        installed: false,
      }));
      fake.options.defaultModel = null;
    });
    const note = await screen.findByTestId("transcription-no-model");
    expect(note.textContent).toContain("No transcription model is installed.");
    expect(
      within(note)
        .getByRole("link", { name: "Get one in the ModelHub" })
        .getAttribute("href"),
    ).toBe("/hub/transcription");
    await userEvent.setup().type(address(), "https://video.test/ok");
    expect(startButton().disabled).toBe(true);
  });

  it("asks for consent before downloading yt-dlp and Deno, then starts once they are installed", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup();
    await screen.findByTestId("transcription-form");
    await user.type(address(), "https://video.test/ok");
    // Outils absents : proposés sous le champ, avec leur taille.
    const tools = await screen.findByTestId("transcription-tools");
    expect(tools.textContent).toContain("yt-dlp and Deno");
    expect(tools.textContent).toContain("81 MB");
    // Parakeet sur une adresse : conseil des deux langues seulement.
    expect(advice()).toEqual(["twoLanguages"]);

    // Transcrire : rien ne part avant l'accord.
    await user.click(startButton());
    const dialog = await screen.findByRole("alertdialog", {
      name: "Download yt-dlp and Deno?",
    });
    const text = dialog.textContent ?? "";
    expect(text).toContain(
      "ChatonLM will download yt-dlp (Unlicense) and Deno (MIT) from GitHub (81 MB).",
    );
    expect(text).toContain("some of them under the GPL or the LGPL");
    expect(text).toContain(
      "only use this feature for content you have the right to copy for your personal use",
    );
    expect(text).toContain("deleted once transcribed");
    expect(simulator.callsTo("tools_install")).toEqual([]);
    // Licences ouvertes dans le navigateur.
    await user.click(
      within(dialog).getByRole("button", { name: "yt-dlp license" }),
    );
    await user.click(
      within(dialog).getByRole("button", {
        name: "Licenses of yt-dlp's components",
      }),
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Deno license" }),
    );
    expect(simulator.callsTo("open_external")).toEqual([
      { url: "https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE" },
      {
        url: "https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt",
      },
      { url: "https://github.com/denoland/deno/blob/main/LICENSE.md" },
    ]);

    await user.click(
      within(dialog).getByRole("button", { name: "Accept and download" }),
    );
    expect(simulator.callsTo("tools_install")).toEqual([
      { acceptLicenses: true },
    ]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(
      (await screen.findByTestId("transcription-waiting-tools")).textContent,
    ).toContain("once yt-dlp and Deno are installed");
    expect(startButton().disabled).toBe(true);

    // Progression suivie par « tools-status ».
    await fake.toolsEvent({}, [
      {
        phase: "downloading",
        completedBytes: 9 * 2 ** 20,
        totalBytes: 36 * 2 ** 20,
      },
      { phase: "fetching" },
    ]);
    const progress = await screen.findByTestId("transcription-tools-progress");
    expect(progress.textContent).toContain(
      "Downloading yt-dlp: 9.0 MB of 36 MB",
    );
    expect(
      within(progress).getByRole("progressbar").getAttribute("aria-valuenow"),
    ).toBe("25");
    expect(simulator.callsTo("transcription_start")).toEqual([]);

    // Prêts : la transcription part d'elle-même, vers sa page.
    await fake.toolsEvent({ ready: true }, [
      { phase: "installed", version: "2026.10.26" },
      { phase: "installed", version: "2.9.8" },
    ]);
    await screen.findByTestId("transcription-view");
    expect(simulator.callsTo("transcription_start")).toEqual([
      {
        request: {
          source: { kind: "url", url: "https://video.test/ok" },
          model: PARAKEET,
          language: "auto",
        },
      },
    ]);
  });

  it("downloads nothing when consent is refused", async () => {
    const user = userEvent.setup();
    const { simulator } = setup();
    await screen.findByTestId("transcription-form");
    await user.type(address(), "https://video.test/ok");
    await user.click(startButton());
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // Plus tard, des outils prêts ne lancent rien.
    expect(simulator.callsTo("tools_install")).toEqual([]);
    expect(screen.queryByTestId("transcription-waiting-tools")).toBeNull();
    expect(startButton().disabled).toBe(false);
  });

  it("starts at once when the tools are installed; refuses an address that isn't one", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((fake) => {
      fake.tools = installedTools();
    });
    await screen.findByTestId("transcription-form");
    await user.type(address(), "video.test/ok");
    expect(
      (await screen.findByTestId("transcription-tools-installed")).textContent,
    ).toBe("yt-dlp 2026.10.26 and Deno 2.9.8 are installed.");
    await user.click(startButton());
    expect(screen.getByRole("alert").textContent).toBe(
      "Enter the full address of the page, starting with https://.",
    );
    expect(simulator.callsTo("transcription_start")).toEqual([]);
    await user.clear(address());
    await user.type(address(), "https://video.test/ok");
    await user.click(startButton());
    await screen.findByTestId("transcription-view");
    expect(simulator.callsTo("tools_install")).toEqual([]);
    expect(simulator.callsTo("transcription_start")).toHaveLength(1);
  });

  it("offers the update of yt-dlp, and says when the system can't fetch videos", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup((fake) => {
      fake.tools = installedTools();
      fake.tools.tools[0].phase = "updateAvailable";
      fake.tools.tools[0].latest = "2026.11.10";
    });
    await screen.findByTestId("transcription-form");
    await user.type(address(), "https://video.test/ok");
    const tools = await screen.findByTestId("transcription-tools");
    expect(tools.textContent).toContain(
      "A newer version of yt-dlp is available (2026.11.10).",
    );
    await user.click(within(tools).getByRole("button", { name: "Update" }));
    expect(simulator.callsTo("tools_update")).toHaveLength(1);
    await fake.toolsEvent({ supported: false });
    await waitFor(() =>
      expect(screen.getByTestId("transcription-tools").textContent).toBe(
        "Videos from the web can't be transcribed on this system. You can still transcribe a file.",
      ),
    );
  });

  it("shows the transcription in progress and greys out a second one", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup((fake) => {
      fake.tools = installedTools();
    });
    await screen.findByTestId("transcription-form");
    await user.type(address(), "https://video.test/ok");
    await user.click(startButton());
    await screen.findByTestId("transcription-view");
    await fake.progress(
      { stage: "transcribing", durationS: 100, doneS: 40, title: "A lecture" },
      [segment(0, 3, "Hello.")],
    );
    await user.click(screen.getByRole("link", { name: "All transcriptions" }));
    const running = await screen.findByTestId("transcription-running");
    expect(running.textContent).toContain(
      "Transcription in progress · A lecture",
    );
    expect(running.textContent).toContain("0:40 of 1:40");
    await user.type(address(), "https://video.test/other");
    expect(startButton().disabled).toBe(true);
    expect(form().textContent).toContain(
      "Another transcription is in progress",
    );
    // Annuler depuis l'accueil.
    await user.click(within(running).getByRole("button", { name: "Cancel" }));
    expect(simulator.callsTo("transcription_cancel")).toHaveLength(1);
    await fake.finish("cancelled");
    await waitFor(() =>
      expect(screen.queryByTestId("transcription-running")).toBeNull(),
    );
    expect(startButton().disabled).toBe(false);
  });

  it("lists the history, opens one and deletes another after confirmation", async () => {
    const user = userEvent.setup();
    const { simulator, app } = setup((fake) => {
      fake.add({ title: "Old interview", source: "url", durationS: 754 }, [
        segment(0, 2, "Old text."),
      ]);
      fake.add({ title: "Lecture", state: "interrupted", hasSummary: true }, [
        segment(0, 2, "Lecture text."),
      ]);
    });
    const rows = await screen.findAllByTestId("transcription-item");
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("Lecture"),
      expect.stringContaining("Old interview"),
    ]);
    expect(rows[0].textContent).toContain("Interrupted");
    expect(rows[0].textContent).toContain("Document ready");
    expect(rows[0].textContent).toContain("Lecture text.");
    expect(rows[1].textContent).toContain("Video from the web");
    expect(rows[1].textContent).toContain("12:34");
    expect(rows[1].textContent).not.toContain("Interrupted");

    await user.click(
      within(rows[1]).getByRole("button", { name: "Delete Old interview" }),
    );
    const dialog = screen.getByRole("alertdialog", {
      name: "Delete “Old interview”?",
    });
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() =>
      expect(screen.getAllByTestId("transcription-item")).toHaveLength(1),
    );
    expect(simulator.callsTo("transcription_delete")).toEqual([{ id: "t-1" }]);

    await user.click(screen.getByRole("link", { name: /Lecture/ }));
    await screen.findByTestId("transcription-view");
    expect(app.path()).toBe("/transcription/t-2");
  });

  it("is translated: Spanish, French, no text outside the catalogs", async () => {
    const user = userEvent.setup();
    setup((fake) => {
      fake.picked = "/home/me/talk.mkv";
      fake.add({ title: "Lecture", state: "failed", source: "url" }, [
        segment(0, 2, "Lecture text."),
      ]);
    });
    await screen.findByTestId("transcription-form");
    await user.click(screen.getByRole("button", { name: "Choose a file…" }));
    await screen.findByTestId("transcription-file");
    act(() => setLanguage("es"));
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
        "Transcripción",
      ),
    );
    expect(form().textContent).toContain("Dirección de un vídeo");
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(form().textContent).toContain("Adresse d'une vidéo"),
    );
    expect(form().textContent).toContain("Transcrire");
    act(() => setLanguage("en"));

    usePseudoLanguage();
    // Adresse : outils et accord.
    await user.click(screen.getByRole("button", { name: /Remove/ }));
    await user.type(
      screen.getByRole("textbox", { name: /Video address/ }),
      "https://video.test/ok",
    );
    await screen.findByTestId("transcription-tools");
    const allow = [
      "talk.mkv",
      "Lecture",
      "Lecture text.",
      "Parakeet TDT 0.6B v3",
      "Whisper Large v3 Turbo",
      "yt-dlp",
      "Deno",
    ];
    expect(
      untranslatedTexts(screen.getByTestId("transcription-home"), allow),
    ).toEqual([]);
    await user.click(screen.getByTestId("transcription-start"));
    const dialog = await screen.findByRole("alertdialog");
    expect(untranslatedTexts(dialog, allow)).toEqual([]);
  });
});
