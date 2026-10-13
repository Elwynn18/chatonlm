// Onglet « Read aloud » des réglages face au simulateur : interrupteur de la
// lecture automatique (enregistré aussitôt, erreur sous la ligne), état des
// voix (installée, en téléchargement, absente) et lien vers le ModelHub,
// ONNX Runtime absent, voix illisibles ; textes en pseudo-langue et en
// français.

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
import { fakeBackend, type FakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import { VOICE_EN, VOICE_FR } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MB = 1024 ** 2;

function setup(prepare?: (backend: FakeBackend) => void) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare?.(backend);
  const app = renderApp("/settings?section=readAloud");
  return { simulator, backend, tts: backend.tts, app };
}

const toggle = () =>
  screen.findByRole("switch", { name: "Read responses aloud automatically" });

/** Lignes des voix : [titre, état]. */
function voiceRows(): string[][] {
  return screen.getAllByTestId("read-aloud-voice").map((row) => {
    const [title, state] = [...row.children].map(
      (child) => child.textContent ?? "",
    );
    return [title, state];
  });
}

describe("Read aloud settings", () => {
  it("is a tab of the settings, current at its address", async () => {
    setup();
    await toggle();
    const tab = screen.getByTestId("settings-tab-readAloud");
    expect(tab.textContent).toBe("Read aloud");
    expect(tab.getAttribute("aria-current")).toBe("page");
    expect(tab.getAttribute("href")).toBe("/settings?section=readAloud");
    // Huit onglets : à la ligne dans une fenêtre étroite ou au zoom maximal,
    // sans déborder de la page.
    expect(screen.getByTestId("settings-tabs").className.split(" ")).toEqual(
      expect.arrayContaining(["flex-wrap", "max-w-full"]),
    );
  });

  it("the switch is off by default, saved at once when turned on, then off", async () => {
    const { simulator, backend } = setup();
    const control = await toggle();
    expect(control.getAttribute("aria-checked")).toBe("false");
    expect(
      document.getElementById(control.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe(
      "Each new response is read aloud while it's being written, if the voice of its language is downloaded. Older responses aren't read when you open a chat.",
    );
    await userEvent.click(control);
    expect(
      simulator.callsTo("update_settings").map((args) => args.patch),
    ).toEqual([{ autoRead: true }]);
    await screen.findByText("Saved");
    expect(control.getAttribute("aria-checked")).toBe("true");
    expect(backend.settings.autoRead).toBe(true);

    await userEvent.click(control);
    await waitFor(() =>
      expect(control.getAttribute("aria-checked")).toBe("false"),
    );
    expect(
      simulator.callsTo("update_settings").map((args) => args.patch),
    ).toEqual([{ autoRead: true }, { autoRead: false }]);
  });

  it("a refused save says why under the row", async () => {
    const { backend } = setup();
    backend.settingsErrors.update = {
      kind: "internal",
      message: "database is locked",
    };
    await userEvent.click(await toggle());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: database is locked",
    );
    expect((await toggle()).getAttribute("aria-checked")).toBe("false");
  });

  it("the state of each voice, and a link to the Voices tab of the ModelHub", async () => {
    const { tts } = setup((backend) => {
      backend.tts.install(VOICE_FR);
      backend.tts.downloads.push({
        id: VOICE_EN,
        name: "Pocket TTS English",
        state: "downloading",
        completedBytes: 70 * MB,
        totalBytes: 140 * MB,
        error: null,
      });
    });
    await waitFor(() => expect(voiceRows()).toHaveLength(3));
    expect(voiceRows()).toEqual([
      ["English · Alba", "Downloading… 50%"],
      ["Spanish · Lola", "Not downloaded"],
      ["French · Estelle", "Installed"],
    ]);
    const card = screen.getByTestId("read-aloud-voices");
    expect(within(card).getByRole("heading").textContent).toBe("Voices");
    expect(
      within(card)
        .getByRole("link", { name: "Manage voices in the ModelHub" })
        .getAttribute("href"),
    ).toBe("/hub/voices");

    // Fin du téléchargement : voix relues.
    await tts.finish(VOICE_EN);
    await waitFor(() =>
      expect(voiceRows()[0]).toEqual(["English · Alba", "Installed"]),
    );
  });

  it("without ONNX Runtime: said once for all the voices", async () => {
    setup((backend) => {
      for (const voice of backend.tts.voices) {
        voice.runtimeProblem = {
          kind: "missing",
          detail: "/usr/lib/chatonlm/libonnxruntime.so.1",
        };
      }
    });
    await waitFor(() => expect(voiceRows()).toHaveLength(3));
    const note =
      "Not available: ONNX Runtime, which reading aloud needs, is missing (/usr/lib/chatonlm/libonnxruntime.so.1). Reinstall ChatonLM.";
    const card = screen.getByTestId("read-aloud-voices");
    expect(card.textContent?.split(note)).toHaveLength(2);
  });

  it("voices that can't be read: the error, then Try again", async () => {
    const { tts } = setup((backend) =>
      backend.tts.failures.set("voice_models", {
        kind: "internal",
        message: "permission denied",
      }),
    );
    const alert = await within(
      await screen.findByTestId("read-aloud-voices"),
    ).findByRole("alert");
    expect(alert.textContent).toBe(
      "Couldn't read the voices: permission denied",
    );
    tts.failures.clear();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(voiceRows()).toHaveLength(3));
  });
});

describe("Read aloud settings in other languages", () => {
  it("pseudo-language: no text written in the code", async () => {
    usePseudoLanguage();
    setup((backend) => {
      backend.tts.install(VOICE_FR);
      backend.tts.voices[1].runtimeProblem = {
        kind: "loadFailed",
        detail: "GLIBC_2.38",
      };
      backend.tts.downloads.push({
        id: VOICE_EN,
        name: "Pocket TTS English",
        state: "downloading",
        completedBytes: 1,
        totalBytes: 2,
        error: null,
      });
    });
    await waitFor(() => expect(voiceRows()).toHaveLength(3));
    expect(
      untranslatedTexts(screen.getByTestId("read-aloud-settings"), [
        "GLIBC_2.38",
        "/hub/voices",
      ]),
    ).toEqual([]);
    // Une phrase, une clé : le pourcentage est dans le texte traduit.
    expect(voiceRows()[0][1]).toBe("⟦Downloading… 50%⟧");
  });

  it("in French and in Spanish", async () => {
    setup((backend) => backend.tts.install(VOICE_FR));
    await waitFor(() => expect(voiceRows()).toHaveLength(3));
    act(() => setLanguage("fr"));
    expect(
      screen.getByRole("switch", {
        name: "Lire les réponses à voix haute automatiquement",
      }),
    ).toBeTruthy();
    expect(voiceRows()).toEqual([
      ["Anglais · Alba", "Non téléchargée"],
      ["Espagnol · Lola", "Non téléchargée"],
      ["Français · Estelle", "Installée"],
    ]);
    act(() => setLanguage("es"));
    expect(voiceRows()[2]).toEqual(["Francés · Estelle", "Instalada"]);
    expect(screen.getByTestId("settings-tab-readAloud").textContent).toBe(
      "Lectura en voz alta",
    );
  });
});
