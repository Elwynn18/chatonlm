// Onglet « Voice mode » des réglages face au simulateur (faux moteur de
// src/test/voiceMode.ts) : onglet de la page, choix de la prise de parole et
// de la pause, interrupteurs (enregistrés aussitôt, erreur sous la ligne),
// consigne vocale (vide : celle par défaut en indication ; enregistrée en
// quittant le champ ; remise à zéro), liens vers la dictée et les voix,
// « Reset to defaults » sans effet sur lui ; textes en pseudo-langue et en
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
import { DEFAULT_VOICE_INSTRUCTIONS } from "@/test/voiceMode";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(
  prepare?: (backend: FakeBackend) => void,
  path = "/settings?section=voiceMode",
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare?.(backend);
  const app = renderApp(path);
  return { simulator, backend, voice: backend.voiceMode, app };
}

const patches = (simulator: ReturnType<typeof installSimulator>) =>
  simulator.callsTo("update_voice_mode_settings").map((args) => args.patch);

const instructions = () =>
  screen.findByLabelText<HTMLTextAreaElement>("Voice instructions");

describe("Voice mode settings", () => {
  it("is a tab of the settings, before Easy Link, VS Code and Keyboard shortcuts", async () => {
    setup();
    await screen.findByTestId("voice-mode-settings");
    const tab = screen.getByTestId("settings-tab-voiceMode");
    expect(tab.textContent).toBe("Voice mode");
    expect(tab.getAttribute("aria-current")).toBe("page");
    expect(tab.getAttribute("href")).toBe("/settings?section=voiceMode");
    const tabs = [
      ...screen.getByTestId("settings-tabs").querySelectorAll("a"),
    ].map((link) => link.textContent);
    expect(tabs.slice(-4)).toEqual([
      "Voice mode",
      "Easy Link",
      "VS Code",
      "Keyboard shortcuts",
    ]);
  });

  it("taking turns and the pause: chosen, saved at once", async () => {
    const { simulator, voice } = setup();
    const turns = await screen.findByRole("radiogroup", {
      name: "Taking turns",
    });
    expect(
      within(turns).getByRole<HTMLInputElement>("radio", { name: "Hands-free" })
        .checked,
    ).toBe(true);
    await userEvent.click(
      within(turns).getByRole("radio", { name: "Push to talk" }),
    );
    await screen.findByText("Saved");
    const pause = screen.getByRole("radiogroup", {
      name: "Pause before sending",
    });
    expect(
      within(pause).getByRole<HTMLInputElement>("radio", { name: "Normal" })
        .checked,
    ).toBe(true);
    await userEvent.click(within(pause).getByRole("radio", { name: "Long" }));
    await waitFor(() =>
      expect(patches(simulator)).toEqual([
        { turnMode: "pushToTalk" },
        { pause: "long" },
      ]),
    );
    expect(voice.settings).toMatchObject({
      turnMode: "pushToTalk",
      pause: "long",
    });
  });

  it("interrupting by speaking (on by default) and thinking (off by default)", async () => {
    const { simulator } = setup();
    const bargeIn = await screen.findByRole("switch", {
      name: "Interrupt by speaking",
    });
    expect(bargeIn.getAttribute("aria-checked")).toBe("true");
    // La phrase sur l'annulation d'écho et le casque.
    expect(
      document.getElementById(bargeIn.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toContain("echo cancellation");
    const think = screen.getByRole("switch", { name: "Let models think" });
    expect(think.getAttribute("aria-checked")).toBe("false");
    await userEvent.click(bargeIn);
    await userEvent.click(think);
    await waitFor(() =>
      expect(patches(simulator)).toEqual([
        { bargeIn: false },
        { keepThink: true },
      ]),
    );
    await waitFor(() => {
      expect(bargeIn.getAttribute("aria-checked")).toBe("false");
      expect(think.getAttribute("aria-checked")).toBe("true");
    });
  });

  it("a refused change: the error under its row", async () => {
    setup((backend) => {
      backend.voiceMode.failures.set("update_voice_mode_settings", {
        kind: "internal",
        message: "database is locked",
      });
    });
    await userEvent.click(
      await screen.findByRole("switch", { name: "Let models think" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: database is locked",
    );
  });

  it("instructions: empty shows the default as a hint, saved on leaving the field, reset", async () => {
    const { simulator, voice } = setup();
    const field = await instructions();
    expect(field.value).toBe("");
    await waitFor(() =>
      expect(field.placeholder).toBe(DEFAULT_VOICE_INSTRUCTIONS),
    );
    const reset = screen.getByRole<HTMLButtonElement>("button", {
      name: "Reset to default",
    });
    expect(reset.disabled).toBe(true);
    await userEvent.type(field, "Answer like a pirate.");
    // Rien avant de quitter le champ.
    expect(patches(simulator)).toEqual([]);
    await userEvent.tab();
    await waitFor(() =>
      expect(patches(simulator)).toEqual([
        { instructions: "Answer like a pirate." },
      ]),
    );
    expect(voice.settings.instructions).toBe("Answer like a pirate.");
    await waitFor(() => expect(reset.disabled).toBe(false));
    await userEvent.click(reset);
    await waitFor(() =>
      expect(patches(simulator).at(-1)).toEqual({ instructions: "" }),
    );
    await waitFor(() => expect(field.value).toBe(""));
  });

  it("instructions typed as the default or as spaces are saved empty; kept when leaving the page", async () => {
    const { simulator, app } = setup();
    const field = await instructions();
    await waitFor(() => expect(field.placeholder).not.toBe(""));
    // Texte par défaut recopié, ou fait d'espaces : rien à enregistrer.
    await userEvent.type(field, DEFAULT_VOICE_INSTRUCTIONS);
    await userEvent.tab();
    expect(patches(simulator)).toEqual([]);
    await userEvent.clear(field);
    await userEvent.type(field, "   ");
    await userEvent.tab();
    expect(patches(simulator)).toEqual([]);
    await userEvent.clear(field);
    await userEvent.type(field, "Be brief.");
    // Page quittée sans quitter le champ : enregistré quand même.
    await act(() =>
      app.router.navigate({ to: "/c/$chatId", params: { chatId: "new" } }),
    );
    await waitFor(() =>
      expect(patches(simulator)).toEqual([{ instructions: "Be brief." }]),
    );
  });

  it("links to the dictation settings and to the voices of the ModelHub", async () => {
    const { app } = setup();
    await screen.findByTestId("voice-mode-settings");
    expect(
      screen
        .getByRole("link", { name: "Dictation settings" })
        .getAttribute("href"),
    ).toBe("/settings?section=dictation");
    await userEvent.click(
      screen.getByRole("link", { name: "Manage voices in the ModelHub" }),
    );
    await waitFor(() => expect(app.path()).toBe("/hub/voices"));
  });

  it("“Reset to defaults” of General leaves it alone", async () => {
    const { simulator } = setup(undefined, "/settings");
    await userEvent.click(
      await screen.findByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    expect(simulator.callsTo("update_voice_mode_settings")).toEqual([]);
  });

  it("is all translated", async () => {
    setup();
    usePseudoLanguage();
    const page = await screen.findByTestId("voice-mode-settings");
    await waitFor(() =>
      expect(page.querySelector("textarea")?.placeholder).not.toBe(""),
    );
    expect(untranslatedTexts(page, [DEFAULT_VOICE_INSTRUCTIONS])).toEqual([]);
  });

  it("in French", async () => {
    setup();
    await screen.findByTestId("voice-mode-settings");
    act(() => setLanguage("fr"));
    expect(
      screen.getByRole("switch", { name: "Interrompre en parlant" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("radio", { name: "Appuyer pour parler" }),
    ).toBeTruthy();
    expect(screen.getByTestId("settings-tab-voiceMode").textContent).toBe(
      "Mode vocal",
    );
  });
});
