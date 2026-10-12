// Onglet Voices du ModelHub face au simulateur (faux moteur de
// src/test/tts.ts) : cartes, licences et attribution (CC BY), téléchargement
// (progression, Cancel, échec), suppression confirmée, ONNX Runtime absent ;
// section « Voices » de l'onglet Downloads et badge ; textes en
// pseudo-langue et en français.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { setLanguage } from "@/i18n";
import { initials } from "@/lib/hub";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderHub } from "@/test/renderHub";
import { VOICE_EN, VOICE_ES, VOICE_FR } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 ** 2;

const cards = () => screen.getAllByTestId("voice-model");
const card = (name: string) =>
  screen.getByRole("listitem", { name }) as HTMLElement;

function download(
  id: string,
  name: string,
  fields: Partial<SpeechDownload>,
): SpeechDownload {
  return {
    id,
    name,
    state: "downloading",
    completedBytes: 0,
    totalBytes: 140 * MIB,
    error: null,
    ...fields,
  };
}

describe("Voices tab", () => {
  it("one card per voice: size, licenses of the model and of the voice, attribution", async () => {
    renderHub("/hub/voices");
    await screen.findAllByTestId("voice-model");
    expect(
      screen.getByText(
        "Voices read responses aloud. They run inside ChatonLM, not in Ollama.",
        { exact: false },
      ),
    ).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Read aloud settings" })
        .getAttribute("href"),
    ).toBe("/settings?section=readAloud");
    expect(
      cards().map(
        (element) => within(element).getByRole("heading").textContent,
      ),
    ).toEqual(["English · Alba", "Spanish · Lola", "French · Estelle"]);

    const french = card("French · Estelle");
    expect(french.textContent).toContain("175 MB");
    expect(
      within(french)
        .getByRole("link", { name: "CC BY 4.0" })
        .getAttribute("href"),
    ).toBe("https://creativecommons.org/licenses/by/4.0/");
    // Crédit de la voix, lié à son enregistrement ; sa licence, à son texte.
    expect(french.textContent).toContain("Voice: Kyutai, CC0");
    expect(
      within(french).getByRole("link", { name: "Kyutai" }).getAttribute("href"),
    ).toBe(
      "https://huggingface.co/kyutai/tts-voices/tree/main/unmute-prod-website",
    );
    expect(
      within(french).getByRole("link", { name: "CC0" }).getAttribute("href"),
    ).toBe("https://creativecommons.org/publicdomain/zero/1.0/legalcode");
    // CC BY : œuvre d'origine, auteurs et changements dits.
    expect(french.textContent).toContain(
      "Based on kyutai/pocket-tts-without-voice-cloning by Kyutai, licensed under CC BY 4.0. Exported to ONNX and quantized by ChatonLM.",
    );
    expect(
      within(french)
        .getByRole("link", { name: "kyutai/pocket-tts-without-voice-cloning" })
        .getAttribute("href"),
    ).toBe("https://huggingface.co/kyutai/pocket-tts-without-voice-cloning");
    // Voix sous CC BY (Alba) : son autrice nommée et liée, sa licence liée
    // à son texte.
    const english = card("English · Alba");
    expect(english.textContent).toContain("Voice: Alba MacKenna, CC BY 4.0");
    expect(
      within(english)
        .getByRole("link", { name: "Alba MacKenna" })
        .getAttribute("href"),
    ).toBe("https://huggingface.co/kyutai/tts-voices/tree/main/alba-mackenna");
    expect(
      within(english)
        .getAllByRole("link", { name: "CC BY 4.0" })
        .map((link) => link.getAttribute("href")),
    ).toEqual([
      "https://creativecommons.org/licenses/by/4.0/",
      "https://creativecommons.org/licenses/by/4.0/legalcode",
    ]);
    expect(card("Spanish · Lola").textContent).toContain(
      "Voice: Mozilla Common Voice, CC0",
    );
    expect(
      cards().map((element) =>
        within(element)
          .getAllByRole("button")
          .map((button) => button.textContent),
      ),
    ).toEqual([["Download"], ["Download"], ["Download"]]);
  });

  it("Download: progress, Cancel, then Delete once it is installed", async () => {
    const { simulator, backend } = renderHub("/hub/voices");
    const tts = backend.tts;
    await screen.findAllByTestId("voice-model");
    await userEvent.click(
      within(card("Spanish · Lola")).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("voice_download")).toEqual([{ id: VOICE_ES }]);
    await within(card("Spanish · Lola")).findByText("Waiting…");
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    await tts.progress(VOICE_ES, 70 * MIB, 140 * MIB);
    const bar = await within(card("Spanish · Lola")).findByRole("progressbar", {
      name: "Downloading Spanish · Lola",
    });
    await waitFor(() => expect(bar.getAttribute("aria-valuenow")).toBe("50"));
    expect(bar.getAttribute("aria-valuetext")).toBe("70 MB of 140 MB");
    await userEvent.click(
      within(card("Spanish · Lola")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("voice_cancel_download")).toEqual([
      { id: VOICE_ES },
    ]);
    await within(card("Spanish · Lola")).findByRole("button", {
      name: "Download",
    });

    await userEvent.click(
      within(card("Spanish · Lola")).getByRole("button", { name: "Download" }),
    );
    await within(card("Spanish · Lola")).findByText("Waiting…");
    await tts.finish(VOICE_ES);
    // Voix relues à la fin : installée.
    await within(card("Spanish · Lola")).findByRole("button", {
      name: "Delete Spanish · Lola",
    });
    expect(screen.queryByTestId("active-downloads")).toBeNull();
  });

  it("a failed download says why, and can be started again", async () => {
    const { simulator, backend } = renderHub("/hub/voices");
    await screen.findAllByTestId("voice-model");
    await userEvent.click(
      within(card("English · Alba")).getByRole("button", { name: "Download" }),
    );
    await within(card("English · Alba")).findByText("Waiting…");
    await backend.tts.fail(
      VOICE_EN,
      "The file did not match its checksum. Try again.",
    );
    expect(
      (await within(card("English · Alba")).findByRole("alert")).textContent,
    ).toBe("The file did not match its checksum. Try again.");
    await userEvent.click(
      within(card("English · Alba")).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("voice_download")).toHaveLength(2);
  });

  it("a command that fails shows its error on the card", async () => {
    const { backend } = renderHub("/hub/voices");
    backend.tts.failures.set("voice_download", {
      kind: "internal",
      message: "no space left on device",
    });
    await screen.findAllByTestId("voice-model");
    await userEvent.click(
      within(card("French · Estelle")).getByRole("button", {
        name: "Download",
      }),
    );
    expect(
      (await within(card("French · Estelle")).findByRole("alert")).textContent,
    ).toBe("no space left on device");
  });

  it("Delete asks first, removes the voice and puts the focus on the card title", async () => {
    const { simulator } = renderHub("/hub/voices", ({ backend }) => {
      backend.tts.install(VOICE_FR);
    });
    await screen.findAllByTestId("voice-model");
    const french = card("French · Estelle");
    await userEvent.click(
      within(french).getByRole("button", { name: "Delete French · Estelle" }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete French · Estelle?",
    });
    expect(dialog.textContent).toContain(
      "This removes the voice from this computer. You can download it again later.",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("voice_delete_model")).toEqual([]);

    await userEvent.click(
      within(french).getByRole("button", { name: "Delete French · Estelle" }),
    );
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(simulator.callsTo("voice_delete_model")).toEqual([{ id: VOICE_FR }]);
    expect(document.activeElement).toBe(
      within(french).getByRole("heading", { name: "French · Estelle" }),
    );
    await within(french).findByRole("button", { name: "Download" });
  });

  it("a refused delete shows why", async () => {
    const { backend } = renderHub("/hub/voices", ({ backend }) => {
      backend.tts.install(VOICE_EN);
    });
    backend.tts.failures.set("voice_delete_model", {
      kind: "system",
      message: "permission denied",
    });
    await screen.findAllByTestId("voice-model");
    await userEvent.click(
      within(card("English · Alba")).getByRole("button", {
        name: "Delete English · Alba",
      }),
    );
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(
      (await within(card("English · Alba")).findByRole("alert")).textContent,
    ).toBe("permission denied");
  });

  it("without ONNX Runtime: said on each card, without Download; an installed voice can still be deleted", async () => {
    renderHub("/hub/voices", ({ backend }) => {
      backend.tts.install(VOICE_FR);
      for (const voice of backend.tts.voices) {
        voice.runtimeProblem = { kind: "incompatible", detail: "1.16.0" };
      }
    });
    await screen.findAllByTestId("voice-model");
    const note =
      "Not available: the ONNX Runtime found can't run reading aloud (1.16.0). Reinstall ChatonLM.";
    for (const element of cards()) {
      expect(element.textContent).toContain(note);
      expect(
        within(element).queryByRole("button", { name: "Download" }),
      ).toBeNull();
    }
    expect(
      within(card("French · Estelle")).getByRole("button", {
        name: "Delete French · Estelle",
      }),
    ).toBeTruthy();
  });
});

describe("Voices in the Downloads tab", () => {
  it("a section of their own, titled like their cards, with the same actions", async () => {
    const { simulator } = renderHub("/hub/downloads", ({ backend }) => {
      backend.tts.install(VOICE_EN);
      backend.tts.downloads.push(
        download(VOICE_FR, "Pocket TTS French", {
          completedBytes: 35 * MIB,
        }),
        download(VOICE_ES, "Pocket TTS Spanish", {
          state: "failed",
          error: "Network error",
        }),
        // Fini, puis supprimé (catalogue relu sans lui).
        download(VOICE_EN, "Pocket TTS English", {
          state: "done",
          completedBytes: 140 * MIB,
        }),
      );
      backend.tts.voices[0].installed = false;
    });
    const section = await screen.findByRole("region", { name: "Voices" });
    const rows = () => within(section).getAllByTestId("voice-download");
    await waitFor(() =>
      expect(
        rows().map((row) => within(row).getByRole("link").textContent),
      ).toEqual(["French · Estelle", "Spanish · Lola", "English · Alba"]),
    );
    expect(within(rows()[0]).getByRole("link").getAttribute("href")).toBe(
      "/hub/voices",
    );
    expect(
      within(rows()[0]).getByRole("progressbar", {
        name: "Downloading French · Estelle",
      }),
    ).toBeTruthy();
    expect(within(rows()[1]).getByRole("alert").textContent).toBe(
      "Network error",
    );
    await waitFor(() =>
      expect(rows()[2].textContent).toContain("No longer installed"),
    );
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    // Par son titre : les actifs passent avant les finis.
    const row = (name: string) =>
      rows().find(
        (element) => within(element).getByRole("link").textContent === name,
      )!;
    await userEvent.click(
      within(row("French · Estelle")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("voice_cancel_download")).toEqual([
      { id: VOICE_FR },
    ]);
    await userEvent.click(
      within(row("Spanish · Lola")).getByRole("button", { name: "Try again" }),
    );
    await userEvent.click(
      within(row("English · Alba")).getByRole("button", {
        name: "Download again",
      }),
    );
    expect(simulator.callsTo("voice_download")).toEqual([
      { id: VOICE_ES },
      { id: VOICE_EN },
    ]);
    await waitFor(() =>
      expect(row("French · Estelle").textContent).toContain("Cancelled"),
    );
    // Actifs d'abord (dans l'ordre de demande), puis les finis.
    expect(
      rows().map((element) => within(element).getByRole("link").textContent),
    ).toEqual(["Spanish · Lola", "English · Alba", "French · Estelle"]);
    await userEvent.click(
      within(row("French · Estelle")).getByRole("button", { name: "Dismiss" }),
    );
    expect(simulator.callsTo("voice_dismiss_download")).toEqual([
      { id: VOICE_FR },
    ]);
  });

  it("only voices: no empty state, nothing else listed", async () => {
    renderHub("/hub/downloads", ({ backend }) => {
      backend.tts.downloads.push(
        download(VOICE_FR, "Pocket TTS French", { state: "queued" }),
      );
    });
    const section = await screen.findByRole("region", { name: "Voices" });
    expect(within(section).getAllByTestId("voice-download")).toHaveLength(1);
    expect(screen.queryByText("No downloads yet.")).toBeNull();
  });

  it("the badge counts the voices being downloaded", async () => {
    const { backend } = renderHub("/hub/voices", ({ backend }) => {
      backend.tts.downloads.push(
        download(VOICE_FR, "Pocket TTS French", { state: "verifying" }),
      );
    });
    await waitFor(() =>
      expect(screen.getByTestId("active-downloads").textContent).toBe("1"),
    );
    await backend.tts.finish(VOICE_FR);
    await waitFor(() =>
      expect(screen.queryByTestId("active-downloads")).toBeNull(),
    );
  });
});

describe("Voices in other languages", () => {
  /** Données du catalogue des voix (noms, initiales, licences). */
  const DATA = [
    "Alba",
    "Lola",
    "Estelle",
    initials("Alba"),
    initials("Lola"),
    initials("Estelle"),
    "CC BY 4.0",
    "CC0",
    "Kyutai",
    "Alba MacKenna",
    "Mozilla Common Voice",
    "kyutai/pocket-tts-without-voice-cloning",
    "1.16.0",
    "Network error",
  ];

  it("pseudo-language: Voices tab and Downloads, no text written in the code", async () => {
    usePseudoLanguage();
    const { app } = renderHub("/hub/voices", ({ backend }) => {
      backend.tts.install(VOICE_FR);
      backend.tts.voices[1].runtimeProblem = {
        kind: "missing",
        detail: "1.16.0",
      };
      backend.tts.downloads.push(
        download(VOICE_EN, "Pocket TTS English", {
          completedBytes: 10 * MIB,
        }),
        download(VOICE_ES, "Pocket TTS Spanish", {
          state: "failed",
          error: "Network error",
        }),
      );
    });
    await screen.findAllByTestId("voice-model");
    const root = () => screen.getByTestId("hub-scroll").parentElement!;
    const links = () =>
      [...root().querySelectorAll("a")].map(
        (link) => link.getAttribute("href") ?? "",
      );
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);

    await act(() => app.router.navigate({ to: "/hub/downloads" }));
    await screen.findAllByTestId("voice-download");
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);
  });

  it("in French: titles, licenses and attribution", async () => {
    renderHub("/hub/voices");
    await screen.findAllByTestId("voice-model");
    act(() => setLanguage("fr"));
    expect(
      cards().map(
        (element) => within(element).getByRole("heading").textContent,
      ),
    ).toEqual(["Anglais · Alba", "Espagnol · Lola", "Français · Estelle"]);
    const french = card("Français · Estelle");
    expect(french.textContent).toContain("175 Mo");
    expect(french.textContent).toContain(
      "Dérivé de kyutai/pocket-tts-without-voice-cloning de Kyutai, sous licence CC BY 4.0. Exporté en ONNX et quantifié par ChatonLM.",
    );
    expect(french.textContent).toContain("Voix\u00a0: Kyutai, CC0");
    expect(
      within(french).getByRole("button", { name: "Télécharger" }),
    ).toBeTruthy();
  });
});
