// Carte « Dictation » des réglages face au simulateur : activation (pas
// sans modèle), raccourci dans chacun de ses états, mode, modèle de
// transcription, langue, nettoyage, consignes, dictionnaire, notifications,
// essai et dernier résultat, erreurs, focus en arrivant (section=dictation),
// textes traduits dans chacun de ces états (pseudo-langue), en français et
// en espagnol.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { DictationSettings } from "@/bindings/DictationSettings";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import {
  DEFAULT_POLISH_PROMPT,
  DICTATE_COMMAND,
  type FakeDictation,
  PARAKEET,
  WHISPER_TINY,
} from "@/test/dictation";
import { renderApp } from "@/test/renderApp";
import { installSimulator, type Simulator } from "@/test/simulator";
import type { ModelInfo } from "@/bindings/ModelInfo";
import { setLanguage } from "@/i18n";
import { modelInfo } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

/** Réglages affichés, après `prepare` du faux backend. */
function setup(
  prepare: (setup: {
    backend: FakeBackend;
    dictation: FakeDictation;
  }) => void = () => {},
  path = "/settings?section=dictation",
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare({ backend, dictation: backend.dictation });
  const app = renderApp(path);
  return { simulator, backend, dictation: backend.dictation, app };
}

/** Dictée activée avec Parakeet et Whisper Tiny installés. */
function ready(settings: Partial<DictationSettings> = {}) {
  return ({ dictation }: { dictation: FakeDictation }) => {
    dictation.install(PARAKEET, WHISPER_TINY);
    Object.assign(dictation.settings, {
      enabled: true,
      speechModel: PARAKEET,
      ...settings,
    });
    dictation.status.shortcut = { state: "bound", trigger: "Ctrl+Alt+D" };
  };
}

/** Carte affichée, réglages et modèles lus. */
async function card() {
  const element = await screen.findByTestId("dictation");
  await within(element).findByRole("switch", { name: "Enable dictation" });
  // Catalogue lu : la liste des modèles de transcription n'attend plus.
  await waitFor(() =>
    expect(
      optionTexts(within(element).getByLabelText("Speech model")),
    ).not.toContain("Loading…"),
  );
  return element;
}

const patches = (simulator: Simulator) =>
  simulator
    .callsTo("update_dictation_settings")
    .map((args) => args.patch as DictationPatch);

const enableSwitch = () =>
  screen.getByRole<HTMLButtonElement>("switch", { name: "Enable dictation" });

/** Modèle que ce processeur ne peut pas faire tourner (sans AVX2). */
function unsupported(dictation: FakeDictation, id: string) {
  const model = dictation.models.find((entry) => entry.id === id);
  if (!model) throw new Error(`unknown speech model: ${id}`);
  model.missingCpuFeatures = ["AVX2"];
}

const optionTexts = (select: HTMLElement) =>
  [...(select as HTMLSelectElement).options].map((option) => option.text);

describe("Dictation settings", () => {
  it("without a speech model: off, cannot be turned on, link to the ModelHub", async () => {
    setup();
    const element = await card();
    const toggle = enableSwitch();
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(toggle.disabled).toBe(true);
    // Décrit par le texte de la carte.
    const description = document.getElementById(
      toggle.getAttribute("aria-describedby")!,
    );
    expect(description?.textContent).toBe(
      "Press a shortcut anywhere to dictate. Your speech is transcribed on this computer, cleaned up by a local model, and copied to the clipboard.",
    );
    expect(
      within(element)
        .getByRole("link", { name: "Download a speech model" })
        .getAttribute("href"),
    ).toBe("/hub/transcription");
    expect(within(element).getByRole("heading", { name: "Dictation" }));
    expect(
      within(element).getByText("Turn on dictation to use the shortcut."),
    ).toBeTruthy();
    const speechModel = within(element).getByLabelText("Speech model");
    expect(optionTexts(speechModel)).toEqual(["No speech model installed"]);
    expect((speechModel as HTMLSelectElement).disabled).toBe(true);
    expect(
      within(element).getByRole<HTMLButtonElement>("button", {
        name: "Try dictation",
      }).disabled,
    ).toBe(true);
    expect(within(element).getByText("Turn on dictation to try it."));
  });

  it("turning it on picks the recommended installed model, binds the shortcut and shows Saved", async () => {
    const { simulator, dictation } = setup(({ dictation }) =>
      dictation.install(WHISPER_TINY, PARAKEET),
    );
    const element = await card();
    expect(
      within(element).queryByRole("link", { name: "Download a speech model" }),
    ).toBeNull();
    await userEvent.click(enableSwitch());
    expect(patches(simulator)).toEqual([
      { enabled: true, speechModel: PARAKEET },
    ]);
    await screen.findByText("Saved");
    expect(enableSwitch().getAttribute("aria-checked")).toBe("true");
    // Statut relu : liaison en cours, puis raccourci lié par le bureau.
    await within(element).findByText("Waiting for your desktop…");
    await dictation.shortcutBound();
    expect((await within(element).findByText("Ctrl+Alt+D")).tagName).toBe(
      "KBD",
    );
    expect(
      (within(element).getByLabelText("Speech model") as HTMLSelectElement)
        .value,
    ).toBe(PARAKEET);
  });

  it("turning it on picks the recommended model even when it is not listed first", async () => {
    const { simulator } = setup(({ dictation }) => {
      for (const model of dictation.models) {
        model.recommended = model.id === "whisper-small";
      }
      dictation.install(WHISPER_TINY, "whisper-small");
    });
    await card();
    await userEvent.click(enableSwitch());
    expect(patches(simulator)).toEqual([
      { enabled: true, speechModel: "whisper-small" },
    ]);
  });

  it("turning it on keeps an installed model already chosen", async () => {
    const { simulator } = setup(({ dictation }) => {
      dictation.install(WHISPER_TINY, PARAKEET);
      dictation.settings.speechModel = WHISPER_TINY;
    });
    await card();
    await userEvent.click(enableSwitch());
    expect(patches(simulator)).toEqual([{ enabled: true }]);
  });

  it("a chosen model deleted since: shown as not installed, dictation can still be turned off", async () => {
    const { simulator } = setup(({ dictation }) => {
      dictation.settings.enabled = true;
      dictation.settings.speechModel = PARAKEET;
    });
    const element = await card();
    expect(optionTexts(within(element).getByLabelText("Speech model"))).toEqual(
      ["Parakeet TDT 0.6B v3 (not installed)"],
    );
    expect(enableSwitch().disabled).toBe(false);
    await userEvent.click(enableSwitch());
    expect(patches(simulator)).toEqual([{ enabled: false }]);
    await waitFor(() =>
      expect(enableSwitch().getAttribute("aria-checked")).toBe("false"),
    );
    expect(enableSwitch().disabled).toBe(true);
  });

  it("an error saving a setting shows under its row, without Saved; the switch goes back", async () => {
    const { dictation } = setup(ready());
    const element = await card();
    dictation.failures.set("update_dictation_settings", {
      kind: "database",
      message: "disk full",
    });
    const notify = within(element).getByRole("switch", {
      name: "Show the text in notifications",
    });
    expect(notify.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(notify);
    expect((await within(element).findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: disk full",
    );
    await waitFor(() =>
      expect(notify.getAttribute("aria-checked")).toBe("true"),
    );
    expect(screen.queryByText("Saved")).toBeNull();
  });

  it("the notifications switch is saved", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const notify = within(element).getByRole("switch", {
      name: "Show the text in notifications",
    });
    await userEvent.click(notify);
    expect(patches(simulator)).toEqual([{ notifyText: false }]);
    await waitFor(() =>
      expect(notify.getAttribute("aria-checked")).toBe("false"),
    );
  });

  it("Reset to defaults (General tab) leaves dictation alone", async () => {
    const { simulator } = setup(ready(), "/settings");
    await screen.findByText("Server address");
    expect(screen.queryByTestId("dictation")).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    expect(simulator.callsTo("update_settings")).toHaveLength(1);
    expect(patches(simulator)).toEqual([]);
  });

  it("could not be read: a message and Try again", async () => {
    const { dictation } = setup(({ dictation }) => {
      dictation.failures.set("get_dictation_settings", {
        kind: "database",
        message: "locked",
      });
    });
    const element = await screen.findByTestId("dictation");
    await within(element).findByText("Couldn't load the dictation settings.");
    dictation.failures.delete("get_dictation_settings");
    await userEvent.click(
      within(element).getByRole("button", { name: "Try again" }),
    );
    await card();
  });
});

describe("Dictation shortcut", () => {
  it("on Windows, Change… records the keys in the app; no command to bind", async () => {
    const { simulator, backend } = setup((setup) => {
      ready()(setup);
      setup.backend.proofread.shortcuts = {
        ...setup.backend.proofread.shortcuts,
        chooser: "app",
        dictation: { state: "bound", trigger: "Ctrl+Alt+D" },
      };
    });
    const element = await card();
    await within(element).findByText("Ctrl+Alt+D");
    await userEvent.click(
      await within(element).findByRole("button", { name: "Change…" }),
    );
    const zone = await within(element).findByTestId("shortcut-recorder-zone");
    expect(simulator.callsTo("dictation_configure_shortcut")).toHaveLength(0);
    expect(
      within(element).getByRole("group", { name: "New shortcut: Dictation" }),
    ).toBeTruthy();
    fireEvent.keyDown(zone, {
      key: "j",
      keyCode: 74,
      ctrlKey: true,
      shiftKey: true,
    });
    expect((await within(element).findByText("Ctrl+Shift+J")).tagName).toBe(
      "KBD",
    );
    expect(simulator.callsTo("shortcuts_set_trigger")).toEqual([
      {
        id: "dictation",
        keys: { ctrl: true, alt: false, shift: true, meta: false, keyCode: 74 },
      },
    ]);
    expect(within(element).queryByTestId("shortcut-recorder-zone")).toBeNull();

    // Combinaison tenue par une autre application : la raison, « Change… »
    // et « Try again », jamais la commande à lier.
    await backend.proofread.setShortcuts({
      dictation: {
        state: "unavailable",
        problem: "taken",
        reason: "Ctrl+Shift+J is already used by another app",
      },
    });
    await within(element).findByText(
      "Ctrl+Shift+J is already used by another app",
    );
    expect(within(element).queryByText(DICTATE_COMMAND)).toBeNull();
    expect(element.textContent).not.toContain("Bind this command");
    await userEvent.click(
      within(element).getByRole("button", { name: "Try again" }),
    );
    expect(simulator.callsTo("dictation_bind_shortcut")).toHaveLength(1);
  });

  it("bound: the keys, and Change… opens the desktop dialog", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    expect((await within(element).findByText("Ctrl+Alt+D")).tagName).toBe(
      "KBD",
    );
    await userEvent.click(
      within(element).getByRole("button", { name: "Change…" }),
    );
    expect(simulator.callsTo("dictation_configure_shortcut")).toHaveLength(1);
  });

  it("bound without Change… when the desktop cannot configure it", async () => {
    setup((setup) => {
      ready()(setup);
      setup.dictation.status.canConfigureShortcut = false;
    });
    const element = await card();
    await within(element).findByText("Ctrl+Alt+D");
    expect(
      within(element).queryByRole("button", { name: "Change…" }),
    ).toBeNull();
  });

  it("bound without keys: Not set, Set shortcut opens the dialog (or binds again)", async () => {
    const { simulator, dictation } = setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = { state: "bound", trigger: "" };
    });
    const element = await card();
    await within(element).findByText("Not set");
    await userEvent.click(
      within(element).getByRole("button", { name: "Set shortcut" }),
    );
    expect(simulator.callsTo("dictation_configure_shortcut")).toHaveLength(1);
    expect(simulator.callsTo("dictation_bind_shortcut")).toHaveLength(0);

    await dictation.setStatus({ canConfigureShortcut: false });
    await userEvent.click(
      within(element).getByRole("button", { name: "Set shortcut" }),
    );
    expect(simulator.callsTo("dictation_bind_shortcut")).toHaveLength(1);
  });

  it("refused: Not set, Set shortcut binds again; the command to bind oneself", async () => {
    const { simulator } = setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = {
        state: "unavailable",
        problem: "refused",
        reason: "The shortcut was not set",
      };
    });
    const element = await card();
    await within(element).findByText("Not set");
    expect(
      within(element).getByText(
        "Bind this command to a shortcut in your system settings:",
      ),
    ).toBeTruthy();
    expect(within(element).getByText(DICTATE_COMMAND).tagName).toBe("CODE");
    await userEvent.click(
      within(element).getByRole("button", { name: "Set shortcut" }),
    );
    expect(simulator.callsTo("dictation_bind_shortcut")).toHaveLength(1);
    // Statut relu : la boîte du bureau est ouverte.
    await within(element).findByText("Waiting for your desktop…");
  });

  it("no portal: the reason, the command, and the push to talk note", async () => {
    setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = {
        state: "unavailable",
        problem: "noPortal",
        reason: "Global shortcuts are not available on this desktop",
      };
    });
    const element = await card();
    await within(element).findByText(
      "Global shortcuts are not available on this desktop",
    );
    expect(within(element).getByText(DICTATE_COMMAND)).toBeTruthy();
    expect(
      within(element).queryByRole("button", { name: "Set shortcut" }),
    ).toBeNull();
    const mode = within(element).getByRole("radiogroup", { name: "Mode" });
    expect(
      document.getElementById(mode.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toContain("Push to talk needs the system shortcut.");
  });

  it("follows the status events, and shows a failure to bind", async () => {
    const { dictation } = setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = {
        state: "unavailable",
        problem: "refused",
        reason: "The shortcut was not set",
      };
      setup.dictation.failures.set("dictation_bind_shortcut", {
        kind: "internal",
        message: "portal gone",
      });
    });
    const element = await card();
    await userEvent.click(
      await within(element).findByRole("button", { name: "Set shortcut" }),
    );
    expect((await within(element).findByRole("alert")).textContent).toBe(
      "portal gone",
    );
    await dictation.setStatus({ shortcut: { state: "binding" } });
    await within(element).findByText("Waiting for your desktop…");
    await dictation.setStatus({
      shortcut: { state: "bound", trigger: "Meta+H" },
    });
    expect((await within(element).findByText("Meta+H")).tagName).toBe("KBD");
    // Mode : plus de note sans raccourci.
    expect(
      within(element).queryByText("Push to talk needs the system shortcut."),
    ).toBeNull();
  });
});

describe("Dictation choices", () => {
  it("mode: Push to talk is saved", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const toggle = within(element).getByRole<HTMLInputElement>("radio", {
      name: "Toggle",
    });
    expect(toggle.checked).toBe(true);
    await userEvent.click(
      within(element).getByRole("radio", { name: "Push to talk" }),
    );
    expect(patches(simulator)).toEqual([{ mode: "hold" }]);
    await screen.findByText("Saved");
    expect(
      within(element).getByRole<HTMLInputElement>("radio", {
        name: "Push to talk",
      }).checked,
    ).toBe(true);
  });

  it("speech model: the installed models only, saved on change", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const select =
      within(element).getByLabelText<HTMLSelectElement>("Speech model");
    expect(optionTexts(select)).toEqual([
      "Parakeet TDT 0.6B v3",
      "Whisper Tiny",
    ]);
    expect(select.value).toBe(PARAKEET);
    await userEvent.selectOptions(select, "Whisper Tiny");
    expect(patches(simulator)).toEqual([{ speechModel: WHISPER_TINY }]);
    expect(
      within(element)
        .getByRole("link", { name: "Manage speech models" })
        .getAttribute("href"),
    ).toBe("/hub/transcription");
  });

  it("speech model: Choose a model while none is chosen", async () => {
    setup(({ dictation }) => dictation.install(WHISPER_TINY));
    const element = await card();
    const select =
      within(element).getByLabelText<HTMLSelectElement>("Speech model");
    expect(optionTexts(select)).toEqual(["Choose a model", "Whisper Tiny"]);
    expect(select.options[0].disabled).toBe(true);
  });

  it("speech model: one this processor cannot run is greyed out, and explained once chosen", async () => {
    const { simulator } = setup(({ dictation }) => {
      dictation.install(PARAKEET, WHISPER_TINY);
      unsupported(dictation, WHISPER_TINY);
      dictation.settings.speechModel = WHISPER_TINY;
    });
    const element = await card();
    const select =
      within(element).getByLabelText<HTMLSelectElement>("Speech model");
    expect(optionTexts(select)).toEqual([
      "Parakeet TDT 0.6B v3",
      "Whisper Tiny (not supported by this processor)",
    ]);
    expect(select.options[0].disabled).toBe(false);
    expect(select.options[1].disabled).toBe(true);
    expect(
      within(element).getByText(
        "Whisper Tiny can't run on this processor (missing: AVX2). Choose a Parakeet model.",
      ),
    ).toBeTruthy();
    // Activer la dictée propose Parakeet, pas le modèle choisi.
    await userEvent.click(enableSwitch());
    expect(patches(simulator)).toEqual([
      { enabled: true, speechModel: PARAKEET },
    ]);
    // Plus d'explication une fois Parakeet choisi.
    await waitFor(() =>
      expect(
        within(element).queryByText(/can't run on this processor/),
      ).toBeNull(),
    );
  });

  it("speech model: none usable on this processor, dictation cannot be turned on", async () => {
    setup(({ dictation }) => {
      dictation.install(WHISPER_TINY);
      unsupported(dictation, WHISPER_TINY);
    });
    const element = await card();
    expect(enableSwitch().disabled).toBe(true);
    expect(
      within(element).getByRole("link", { name: "Download a speech model" }),
    ).toBeTruthy();
  });

  it("speech model: Parakeet without ONNX Runtime is greyed out, explained once chosen, never proposed", async () => {
    const { simulator } = setup(({ dictation }) => {
      dictation.install(PARAKEET, WHISPER_TINY);
      const parakeet = dictation.models.find((model) => model.id === PARAKEET);
      if (parakeet) {
        parakeet.runtimeProblem = {
          kind: "incompatible",
          detail: "expected version >= '1.27.x', but got '1.16.3'",
        };
      }
      dictation.settings.speechModel = PARAKEET;
    });
    const element = await card();
    const select =
      within(element).getByLabelText<HTMLSelectElement>("Speech model");
    expect(optionTexts(select)).toEqual([
      "Parakeet TDT 0.6B v3 (unavailable)",
      "Whisper Tiny",
    ]);
    expect(select.options[0].disabled).toBe(true);
    expect(select.options[1].disabled).toBe(false);
    expect(
      within(element).getByText(
        "Parakeet TDT 0.6B v3 can't run: the ONNX Runtime found is not compatible (expected version >= '1.27.x', but got '1.16.3'). Reinstall ChatonLM, or choose a Whisper model.",
      ),
    ).toBeTruthy();
    // Activer la dictée propose Whisper Tiny, pas Parakeet (recommandé).
    await userEvent.click(enableSwitch());
    expect(patches(simulator)).toEqual([
      { enabled: true, speechModel: WHISPER_TINY },
    ]);
  });

  it("language: Parakeet keeps its languages only, with a note; Whisper takes them all", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const select =
      within(element).getByLabelText<HTMLSelectElement>("Language");
    expect(select.value).toBe("auto");
    const option = (name: string) =>
      [...select.options].find((entry) => entry.text === name)!;
    expect(option("Auto-detect")).toBeTruthy();
    expect(option("Japanese").disabled).toBe(true);
    expect(option("French").disabled).toBe(false);
    const describedBy = () =>
      document.getElementById(select.getAttribute("aria-describedby")!)
        ?.textContent;
    expect(describedBy()).toContain(
      "Parakeet detects the language automatically.",
    );
    await userEvent.selectOptions(select, "French");
    expect(patches(simulator)).toEqual([{ language: "fr" }]);

    await userEvent.selectOptions(
      within(element).getByLabelText("Speech model"),
      "Whisper Tiny",
    );
    await waitFor(() => expect(option("Japanese").disabled).toBe(false));
    expect(describedBy()).not.toContain("Parakeet");
  });

  it("polish model: Off and the models of Ollama, saved on change", async () => {
    const { simulator } = setup(ready({ polishModel: "gemma3:4b" }));
    const element = await card();
    const select =
      within(element).getByLabelText<HTMLSelectElement>("Polish model");
    await waitFor(() =>
      expect(optionTexts(select)).toEqual([
        "Off",
        "gemma3:4b (not installed)",
        "qwen3:0.6b",
        "qwen3.5:0.8b",
      ]),
    );
    expect(select.value).toBe("gemma3:4b");
    await userEvent.selectOptions(select, "qwen3:0.6b");
    expect(patches(simulator)).toEqual([{ polishModel: "qwen3:0.6b" }]);
    await userEvent.selectOptions(select, "Off");
    expect(patches(simulator).at(-1)).toEqual({ polishModel: "" });
    expect(
      within(element)
        .getByRole("link", { name: "Recommended models" })
        .getAttribute("href"),
    ).toBe("/hub/polish");
  });
});

describe("Dictation instructions and dictionary", () => {
  it("instructions: the default text, saved on leaving the field, reset to default", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const field =
      within(element).getByLabelText<HTMLTextAreaElement>("Instructions");
    await waitFor(() => expect(field.value).toBe(DEFAULT_POLISH_PROMPT));
    const reset = within(element).getByRole<HTMLButtonElement>("button", {
      name: "Reset to default",
    });
    expect(reset.disabled).toBe(true);
    // Entrer puis sortir sans rien changer : rien d'enregistré.
    await userEvent.click(field);
    await userEvent.tab();
    // Modifié puis rendu tel quel : rien non plus.
    await userEvent.type(field, "x{Backspace}");
    await userEvent.tab();
    expect(patches(simulator)).toEqual([]);

    await userEvent.type(field, " Keep it short.");
    expect(patches(simulator)).toEqual([]);
    await userEvent.tab();
    expect(patches(simulator)).toEqual([
      { polishPrompt: `${DEFAULT_POLISH_PROMPT} Keep it short.` },
    ]);
    await screen.findByText("Saved");
    await waitFor(() => expect(reset.disabled).toBe(false));

    await userEvent.click(reset);
    expect(patches(simulator).at(-1)).toEqual({ polishPrompt: "" });
    await waitFor(() => expect(field.value).toBe(DEFAULT_POLISH_PROMPT));
    expect(reset.disabled).toBe(true);
  });

  it("instructions: a draft that is not saved stays in the field", async () => {
    const { dictation } = setup(ready());
    const element = await card();
    const field =
      within(element).getByLabelText<HTMLTextAreaElement>("Instructions");
    await waitFor(() => expect(field.value).toBe(DEFAULT_POLISH_PROMPT));
    dictation.failures.set("update_dictation_settings", {
      kind: "invalid",
      message: "the instructions hold at most 4000 characters",
    });
    await userEvent.clear(field);
    await userEvent.type(field, "Fix the punctuation.");
    await userEvent.tab();
    expect((await within(element).findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: the instructions hold at most 4000 characters",
    );
    // Réglages relus après l'échec : le brouillon reste.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(field.value).toBe("Fix the punctuation.");
  });

  it("dictionary: Enter adds a word, a word already there is not added twice, × removes", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const input =
      within(element).getByLabelText<HTMLInputElement>("Dictionary");
    await userEvent.type(input, "  ChatonLM {Enter}");
    expect(patches(simulator)).toEqual([{ dictionary: ["ChatonLM"] }]);
    const list = await within(element).findByRole("list", {
      name: "Dictionary words",
    });
    expect(within(list).getByRole("listitem").textContent).toBe("ChatonLM");
    await waitFor(() => expect(input.value).toBe(""));

    await userEvent.type(input, "chatonlm{Enter}");
    await userEvent.type(input, "   {Enter}");
    expect(patches(simulator)).toHaveLength(1);
    expect(input.value).toBe("   ");

    await userEvent.clear(input);
    await userEvent.type(input, "Ollama{Enter}");
    expect(patches(simulator).at(-1)).toEqual({
      dictionary: ["ChatonLM", "Ollama"],
    });
    await within(list).findByText("Ollama");

    await userEvent.click(
      within(list).getByRole("button", { name: "Remove ChatonLM" }),
    );
    expect(patches(simulator).at(-1)).toEqual({ dictionary: ["Ollama"] });
    expect(document.activeElement).toBe(input);
    await waitFor(() =>
      expect(within(list).queryByText("ChatonLM")).toBeNull(),
    );
  });
});

describe("Try dictation", () => {
  it("Try dictation, Stop and Cancel follow the phase; the focus stays on the button", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const button = within(element).getByRole("button", {
      name: "Try dictation",
    });
    await userEvent.click(button);
    expect(simulator.callsTo("dictation_toggle")).toHaveLength(1);
    await waitFor(() => expect(button.textContent).toBe("Stop"));
    expect(document.activeElement).toBe(button);
    expect(within(element).getByText("Listening…")).toBeTruthy();
    expect(
      within(element).getByText("Listening…").getAttribute("aria-live"),
    ).toBe("polite");

    await userEvent.click(button);
    expect(simulator.callsTo("dictation_toggle")).toHaveLength(2);
    await waitFor(() => expect(button.textContent).toBe("Cancel"));
    expect(within(element).getByText("Transcribing…")).toBeTruthy();
    await userEvent.click(button);
    expect(simulator.callsTo("dictation_cancel")).toHaveLength(1);
    await waitFor(() => expect(button.textContent).toBe("Try dictation"));
  });

  it("Cancel while listening throws the recording away", async () => {
    const { simulator, dictation } = setup(ready());
    const element = await card();
    await dictation.setStatus({ phase: "recording" });
    await userEvent.click(
      await within(element).findByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("dictation_cancel")).toHaveLength(1);
    expect(simulator.callsTo("dictation_toggle")).toHaveLength(0);
    await within(element).findByRole("button", { name: "Try dictation" });
  });

  it("needs an installed model chosen", async () => {
    setup(({ dictation }) => {
      dictation.install(WHISPER_TINY);
      dictation.settings.enabled = true;
      dictation.settings.speechModel = PARAKEET;
    });
    const element = await card();
    expect(
      within(element).getByRole<HTMLButtonElement>("button", {
        name: "Try dictation",
      }).disabled,
    ).toBe(true);
    expect(
      within(element).getByText("Choose an installed speech model to try it."),
    ).toBeTruthy();
  });

  it("shows the last result: copied text, not polished with the reason, or the failure", async () => {
    const { dictation } = setup(ready({ polishModel: "gemma3:4b" }));
    const element = await card();
    expect(within(element).queryByTestId("last-dictation")).toBeNull();

    await dictation.complete({
      text: "Hello world.",
      raw: "hello world",
      polished: true,
    });
    const last = await within(element).findByTestId("last-dictation");
    expect(within(last).getByText("Hello world.")).toBeTruthy();
    expect(within(last).getByText("just now")).toBeTruthy();
    expect(within(last).getByText("Copied to the clipboard")).toBeTruthy();
    expect(within(last).queryByText("not polished")).toBeNull();

    await dictation.complete({
      text: "hello world",
      raw: "hello world",
      polished: false,
      error: "Polish skipped: the model's answer did not look like your text",
    });
    await within(last).findByText("not polished");
    expect(last.textContent).toContain(
      "Polish skipped: the model's answer did not look like your text",
    );

    await dictation.complete({ text: "", error: "No speech detected" });
    await within(last).findByText("No speech detected");
    expect(within(last).queryByText("not polished")).toBeNull();
    expect(within(last).queryByText("Copied to the clipboard")).toBeNull();
  });

  it("a copy that failed is clearly shown, with the reason, polished or not", async () => {
    const { dictation } = setup(ready({ polishModel: "gemma3:4b" }));
    const element = await card();
    await dictation.complete({
      text: "Hello world.",
      raw: "hello world",
      polished: true,
      copied: false,
      error:
        "Could not copy the text to the clipboard: no clipboard on this desktop",
      copyFailed: true,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("Not copied: select the text to copy it");
    expect(within(last).queryByText("Copied to the clipboard")).toBeNull();
    // Nettoyé : pas de « not polished » pour un échec de la copie.
    expect(within(last).queryByText("not polished")).toBeNull();
    expect(last.textContent).toContain(
      "Could not copy the text to the clipboard: no clipboard on this desktop",
    );
    expect(within(last).getByText("Hello world.")).toBeTruthy();
  });

  it("without a polish model: no not polished badge, but any reason is shown", async () => {
    const { dictation } = setup(ready());
    const element = await card();
    await dictation.complete({
      text: "hello world",
      raw: "hello world",
      polished: false,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("hello world");
    expect(within(last).queryByText("not polished")).toBeNull();
    await dictation.complete({
      text: "hello again",
      raw: "hello again",
      polished: true,
      error: "Something the engine wants you to know",
    });
    await within(last).findByText("Something the engine wants you to know");
  });
});

describe("Opened for dictation (section=dictation)", () => {
  it("the title of the card takes the focus", async () => {
    setup(ready(), "/settings?section=dictation");
    const element = await card();
    const title = within(element).getByRole("heading", { name: "Dictation" });
    await waitFor(() => expect(document.activeElement).toBe(title));
  });

  it("opened from its tab: the focus stays on the tab", async () => {
    setup(ready(), "/settings");
    const tab = await screen.findByTestId("settings-tab-dictation");
    await userEvent.click(tab);
    await card();
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(document.activeElement).toBe(tab);
  });

  it("the user already elsewhere: the focus is left alone", async () => {
    const { simulator, dictation } = setup(
      ready(),
      "/settings?section=dictation",
    );
    let answer!: () => void;
    simulator.handle(
      "get_dictation_settings",
      () =>
        new Promise((resolve) => {
          answer = () => resolve(dictation.settings);
        }),
    );
    const elsewhere = await screen.findByTestId("settings-tab-general");
    act(() => elsewhere.focus());
    await waitFor(() =>
      expect(simulator.callsTo("get_dictation_settings")).toHaveLength(1),
    );
    await act(async () => answer());
    await card();
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(document.activeElement).toBe(elsewhere);
  });
});

describe("Dictation without the lists (review 43, F2)", () => {
  it("speech models not read yet: Loading…, never not installed", async () => {
    const { simulator, dictation } = setup(ready());
    let answer!: () => void;
    simulator.handle(
      "speech_models",
      () =>
        new Promise((resolve) => (answer = () => resolve(dictation.models))),
    );
    const element = await screen.findByTestId("dictation");
    await within(element).findByRole("switch", { name: "Enable dictation" });
    const select = within(element).getByLabelText("Speech model");
    expect(optionTexts(select)).toEqual(["Loading…"]);
    expect(element.textContent).not.toContain("not installed");
    expect(
      within(element).queryByRole("link", { name: "Download a speech model" }),
    ).toBeNull();
    await waitFor(() => expect(answer).toBeDefined());
    await act(async () => answer());
    await waitFor(() =>
      expect(optionTexts(select)).toEqual([
        "Parakeet TDT 0.6B v3",
        "Whisper Tiny",
      ]),
    );
  });

  it("speech models that cannot be read: why, Try again, never not installed", async () => {
    const { dictation } = setup((setup) => {
      ready()(setup);
      setup.dictation.failures.set("speech_models", {
        kind: "system",
        message: "permission denied",
      });
    });
    const element = await screen.findByTestId("dictation");
    const alert = await within(element).findByText(
      "Couldn't read the speech models: permission denied",
      { exact: false },
    );
    expect(optionTexts(within(element).getByLabelText("Speech model"))).toEqual(
      [PARAKEET],
    );
    expect(element.textContent).not.toContain("not installed");
    dictation.failures.delete("speech_models");
    await userEvent.click(
      within(alert).getByRole("button", { name: "Try again" }),
    );
    await card();
  });

  it("Ollama not answering yet: Loading models…, the polish model never not installed", async () => {
    const { simulator } = setup(ready({ polishModel: "gemma3:4b" }));
    simulator.handle("list_models", () => new Promise(() => {}));
    const element = await card();
    const select = within(element).getByLabelText("Polish model");
    expect(optionTexts(select)).toEqual(["Off", "gemma3:4b"]);
    expect(within(element).getByText("Loading models…")).toBeTruthy();
  });

  it("Ollama unreachable: said so, the polish model never not installed", async () => {
    const { simulator } = setup(ready({ polishModel: "gemma3:4b" }));
    simulator.handle("list_models", () => {
      throw { kind: "ollama", message: "connection refused" };
    });
    const element = await card();
    await within(element).findByText(
      "Ollama is unreachable. Its models will be listed once it answers.",
    );
    expect(optionTexts(within(element).getByLabelText("Polish model"))).toEqual(
      ["Off", "gemma3:4b"],
    );
    expect(within(element).queryByText("Loading models…")).toBeNull();
  });
});

describe("Dictation and privacy (review 43, D3)", () => {
  it("cloud models are never offered for polish", async () => {
    const { simulator } = setup(ready({ polishModel: "gpt-oss:120b-cloud" }));
    simulator.handle("list_models", (): ModelInfo[] =>
      ["qwen3:0.6b", "gpt-oss:120b-cloud", "kimi-k2:cloud"].map(modelInfo),
    );
    const element = await card();
    const select = within(element).getByLabelText("Polish model");
    await waitFor(() =>
      expect(optionTexts(select)).toEqual([
        "Off",
        "gpt-oss:120b-cloud (cloud models are not used)",
        "qwen3:0.6b",
      ]),
    );
  });

  it("a remote Ollama server: the card says the transcript goes there", async () => {
    setup(({ backend, dictation }) => {
      ready()({ dictation });
      backend.settings.serverUrl = "http://192.168.1.20:11434";
      backend.settings.effectiveServerUrl = "http://192.168.1.20:11434";
    });
    const element = await card();
    const description = document.getElementById(
      enableSwitch().getAttribute("aria-describedby")!,
    );
    expect(description?.textContent).toBe(
      "Press a shortcut anywhere to dictate. Your speech is transcribed on this computer, cleaned up, and copied to the clipboard. The transcript is sent to your Ollama server at http://192.168.1.20:11434 for clean-up.",
    );
    expect(element.textContent).not.toContain("local model");
  });
});

describe("Dictation shortcut retries (review 43, F3)", () => {
  it.each([
    ["failed", "The desktop could not set the shortcut: timeout"],
    ["lost", "The desktop stopped the shortcut"],
  ] as const)("%s: Try again binds again", async (problem, reason) => {
    const { simulator } = setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = {
        state: "unavailable",
        problem,
        reason,
      };
    });
    const element = await card();
    await within(element).findByText(reason);
    expect(within(element).queryByText("Not set")).toBeNull();
    await userEvent.click(
      within(element).getByRole("button", { name: "Try again" }),
    );
    expect(simulator.callsTo("dictation_bind_shortcut")).toHaveLength(1);
    await within(element).findByText("Waiting for your desktop…");
  });
});

describe("Dictation shown on each request (review 43, F4)", () => {
  it("asked again from the system tray: scrolled and focused again, even from elsewhere", async () => {
    const { simulator } = setup(ready(), "/settings?section=dictation");
    const element = await card();
    const title = within(element).getByRole("heading", { name: "Dictation" });
    await waitFor(() => expect(document.activeElement).toBe(title));
    const elsewhere = screen.getByTestId("settings-tab-account");
    act(() => elsewhere.focus());
    await waitFor(() => expect(simulator.listening("navigate")).toBe(1));
    await simulator.emitEvent("navigate", "/settings?section=dictation");
    await waitFor(() => expect(document.activeElement).toBe(title));
  });

  it("asked from the General tab, typing an address: the Dictation tab, its card focused", async () => {
    const { simulator, app } = setup(ready(), "/settings");
    const address = await screen.findByLabelText("Server address");
    act(() => address.focus());
    await waitFor(() => expect(simulator.listening("navigate")).toBe(1));
    await simulator.emitEvent("navigate", "/settings?section=dictation");
    const element = await card();
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(element).getByRole("heading", { name: "Dictation" }),
      ),
    );
    expect(app.router.state.location.search).toEqual({
      section: "dictation",
    });
  });
});

describe("Dictation instructions kept (review 43, F5)", () => {
  async function typeDraft() {
    const setupResult = setup(ready());
    const element = await card();
    const field =
      within(element).getByLabelText<HTMLTextAreaElement>("Instructions");
    await waitFor(() => expect(field.value).toBe(DEFAULT_POLISH_PROMPT));
    await userEvent.clear(field);
    await userEvent.type(field, "Fix the punctuation.");
    expect(patches(setupResult.simulator)).toEqual([]);
    return { ...setupResult, field };
  }

  it("saved when the page is left without leaving the field", async () => {
    const { simulator, app } = await typeDraft();
    await act(() => app.router.navigate({ to: "/hub/installed" }));
    await waitFor(() =>
      expect(patches(simulator)).toEqual([
        { polishPrompt: "Fix the punctuation." },
      ]),
    );
  });

  it("saved before Ctrl+Q quits", async () => {
    const { simulator, field } = await typeDraft();
    act(() => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "q",
          ctrlKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    await waitFor(() => expect(simulator.callsTo("quit_app")).toHaveLength(1));
    const order = simulator.calls
      .map((call) => call.command)
      .filter(
        (command) =>
          command === "update_dictation_settings" || command === "quit_app",
      );
    expect(order).toEqual(["update_dictation_settings", "quit_app"]);
    expect(patches(simulator)).toEqual([
      { polishPrompt: "Fix the punctuation." },
    ]);
  });
});

describe("Dictation dictionary and input methods (review 43, F7)", () => {
  it("the Enter that ends a composition does not add the word", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const input = within(element).getByLabelText("Dictionary");
    fireEvent.change(input, { target: { value: "にほ" } });
    fireEvent.compositionStart(input);
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.change(input, { target: { value: "日本" } });
    fireEvent.compositionEnd(input);
    // Entrée de fin de composition, juste après compositionend.
    fireEvent.keyDown(input, { key: "Enter" });
    expect(patches(simulator)).toEqual([]);
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(patches(simulator)).toEqual([{ dictionary: ["日本"] }]),
    );
  });
});

describe("Dictation without automatic copy", () => {
  it("Copy to clipboard automatically: on by default, saved when turned off", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const toggle = within(element).getByRole("switch", {
      name: "Copy to clipboard automatically",
    });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(
      document.getElementById(toggle.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe(
      "When off, copy the text from the notification or from the last result below.",
    );
    await userEvent.click(toggle);
    expect(patches(simulator)).toEqual([{ autoCopy: false }]);
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("false"),
    );
  });

  it("Paste where the cursor is: on by default, saved, greyed out without automatic copy", async () => {
    const { simulator } = setup(ready());
    const element = await card();
    const toggle = within(element).getByRole("switch", {
      name: "Paste where the cursor is",
    });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(toggle.hasAttribute("disabled")).toBe(false);
    expect(
      document.getElementById(toggle.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe(
      "ChatonLM pastes the text into the app you are typing in, right after copying it. Needs automatic copy. The first time, your desktop asks to let ChatonLM send the paste keystroke.",
    );
    await userEvent.click(toggle);
    expect(patches(simulator)).toEqual([{ autoPaste: false }]);
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("false"),
    );
    await userEvent.click(toggle);
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("true"),
    );
    // Sans la copie automatique : grisé, rien d'enregistré au clic.
    await userEvent.click(
      within(element).getByRole("switch", {
        name: "Copy to clipboard automatically",
      }),
    );
    await waitFor(() => expect(toggle.hasAttribute("disabled")).toBe(true));
    await userEvent.click(toggle);
    expect(patches(simulator)).toEqual([
      { autoPaste: false },
      { autoPaste: true },
      { autoCopy: false },
    ]);
  });

  it("a pasted dictation says so; a failed paste says why", async () => {
    const { dictation } = setup(ready());
    const element = await card();
    await dictation.complete({
      text: "Hello world.",
      raw: "hello world",
      polished: true,
      pasted: true,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("Pasted where the cursor was");
    expect(within(last).queryByText("Copied to the clipboard")).toBeNull();

    await dictation.complete({
      text: "Second text.",
      raw: "second text",
      polished: true,
      pasteError: "Pasting was not allowed: press Ctrl+V.",
    });
    await within(last).findByText("Second text.");
    within(last).getByText("Copied to the clipboard");
    within(last).getByText("Pasting was not allowed: press Ctrl+V.");
    expect(within(last).queryByText("Pasted where the cursor was")).toBeNull();
  });

  it("a text left to copy: no error, a Copy button that copies it", async () => {
    const { simulator, dictation } = setup(ready({ autoCopy: false }));
    const element = await card();
    await dictation.complete({
      text: "Hello world.",
      raw: "hello world",
      polished: true,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("Hello world.");
    expect(dictation.status.lastResult?.copied).toBe(false);
    expect(within(last).queryByText(/Not copied/)).toBeNull();
    expect(within(last).queryByText("Copied to the clipboard")).toBeNull();
    expect(within(last).queryByRole("alert")).toBeNull();
    const copy = within(last).getByRole("button", { name: "Copy" });
    await userEvent.click(copy);
    expect(simulator.callsTo("dictation_copy_last")).toEqual([{}]);
    await waitFor(() => expect(copy.textContent).toBe("Copied"));
    await within(last).findByText("Copied to the clipboard");
  });

  it("a copy that really failed: still shown, and Copy tries again", async () => {
    const { dictation } = setup(ready());
    const element = await card();
    await dictation.complete({
      text: "Hello world.",
      raw: "hello world",
      polished: true,
      copied: false,
      error: "Could not copy the text to the clipboard: no clipboard",
      copyFailed: true,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("Not copied: select the text to copy it");
    await userEvent.click(within(last).getByRole("button", { name: "Copy" }));
    await within(last).findByText("Copied to the clipboard");
    expect(within(last).queryByText(/Not copied/)).toBeNull();
    expect(last.textContent).not.toContain("Could not copy");
  });

  it("Copy that fails says why", async () => {
    const { dictation } = setup(ready({ autoCopy: false }));
    const element = await card();
    dictation.failures.set("dictation_copy_last", {
      kind: "system",
      message: "no clipboard on this desktop",
    });
    await dictation.complete({ text: "Hello world.", raw: "hello world" });
    const last = await within(element).findByTestId("last-dictation");
    await userEvent.click(
      await within(last).findByRole("button", { name: "Copy" }),
    );
    expect((await within(last).findByRole("alert")).textContent).toBe(
      "Couldn't copy: no clipboard on this desktop",
    );
    expect(within(last).getByRole("button", { name: "Copy" })).toBeTruthy();
  });
});

describe("Dictation texts translated (pseudo-language)", () => {
  /** Données des essais, jamais traduites : modèles, touches, commande, consignes. */
  const DATA = [
    "Parakeet TDT 0.6B v3",
    "Whisper Tiny",
    "qwen3:0.6b",
    "qwen3.5:0.8b",
    "Ctrl+Alt+D",
    DICTATE_COMMAND,
    DEFAULT_POLISH_PROMPT,
  ];
  const untranslated = (element: Element, allow: string[] = []) =>
    untranslatedTexts(element, [...DATA, ...allow]);

  /**
   * Carte affichée, réglages et modèles lus, textes marqués « ⟦ ⟧ » (la
   * pseudo-langue est mise avant le rendu : noms cherchés en partie).
   */
  async function pseudoCard() {
    const element = await screen.findByTestId("dictation");
    await within(element).findByRole("switch", { name: /Enable dictation/ });
    await waitFor(() =>
      expect(
        optionTexts(within(element).getByLabelText(/Speech model/)).join(" "),
      ).not.toMatch(/Loading/),
    );
    return element;
  }

  it("off, without a speech model", async () => {
    usePseudoLanguage();
    setup();
    const element = await pseudoCard();
    await within(element).findByText(/Turn on dictation to use the shortcut/);
    expect(untranslated(element)).toEqual([]);
  });

  it("ready: shortcut bound, polish model not installed, remote server, dictionary", async () => {
    usePseudoLanguage();
    setup(({ backend, dictation }) => {
      ready({ polishModel: "gemma3:4b", dictionary: ["ChatonLM", "Ollama"] })({
        dictation,
      });
      backend.settings.serverUrl = "http://192.168.1.20:11434";
      backend.settings.effectiveServerUrl = "http://192.168.1.20:11434";
    });
    const element = await pseudoCard();
    await within(element).findByRole("button", { name: /Change…/ });
    await within(element).findByText(/192\.168\.1\.20/);
    await waitFor(() =>
      expect(
        optionTexts(within(element).getByLabelText(/Polish model/)).join(" "),
      ).toMatch(/not installed/),
    );
    // Noms des langues parlées : écrits par Intl, marqués eux aussi.
    expect(untranslated(element, ["ChatonLM", "Ollama"])).toEqual([]);
  });

  it("shortcut refused: Not set, Set shortcut, the command", async () => {
    usePseudoLanguage();
    setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = {
        state: "unavailable",
        problem: "refused",
        reason: "The shortcut was not set",
      };
    });
    const element = await pseudoCard();
    await within(element).findByText(/Not set/);
    // La raison du moteur n'est pas montrée pour un refus.
    expect(untranslated(element)).toEqual([]);
  });

  it("no portal: the reason of the engine as is, the push to talk note", async () => {
    const reason = "Global shortcuts are not available on this desktop";
    usePseudoLanguage();
    setup((setup) => {
      ready()(setup);
      setup.dictation.status.shortcut = {
        state: "unavailable",
        problem: "noPortal",
        reason,
      };
    });
    const element = await pseudoCard();
    await within(element).findByText(reason);
    expect(untranslated(element, [reason])).toEqual([]);
  });

  it("settings that cannot be read", async () => {
    usePseudoLanguage();
    setup(({ dictation }) => {
      dictation.failures.set("get_dictation_settings", {
        kind: "database",
        message: "locked",
      });
    });
    const element = await screen.findByTestId("dictation");
    await within(element).findByText(/Couldn't load the dictation settings/);
    expect(untranslated(element)).toEqual([]);
  });

  it("speech models that cannot be read, Ollama unreachable", async () => {
    usePseudoLanguage();
    const { simulator } = setup((setup) => {
      ready({ polishModel: "gemma3:4b" })(setup);
      setup.dictation.failures.set("speech_models", {
        kind: "system",
        message: "permission denied",
      });
    });
    simulator.handle("list_models", () => {
      throw { kind: "ollama", message: "connection refused" };
    });
    const element = await screen.findByTestId("dictation");
    await within(element).findByText(/Ollama is unreachable/);
    await within(element).findByText(/Couldn't read the speech models/);
    // Modèles pas lus : les valeurs enregistrées, telles quelles.
    expect(
      untranslated(element, [PARAKEET, "gemma3:4b", "permission denied"]),
    ).toEqual([]);
  });

  it("a cloud model chosen before: said not used", async () => {
    usePseudoLanguage();
    const { simulator } = setup(ready({ polishModel: "gpt-oss:120b-cloud" }));
    simulator.handle("list_models", (): ModelInfo[] =>
      ["qwen3:0.6b", "gpt-oss:120b-cloud"].map(modelInfo),
    );
    const element = await pseudoCard();
    await waitFor(() =>
      expect(
        optionTexts(within(element).getByLabelText(/Polish model/)).join(" "),
      ).toMatch(/cloud models are not used/),
    );
    expect(untranslated(element)).toEqual([]);
  });

  it("recording, transcribing, polishing", async () => {
    usePseudoLanguage();
    const { dictation } = setup(ready());
    const element = await pseudoCard();
    for (const phase of ["recording", "transcribing", "polishing"] as const) {
      await dictation.setStatus({ phase });
      await waitFor(() =>
        expect(element.querySelector("[aria-live]")?.textContent).toMatch(/⟧$/),
      );
      expect(untranslated(element), phase).toEqual([]);
    }
  });

  it("last results: polished, not polished, copy failed, failure, copy refused", async () => {
    usePseudoLanguage();
    const { dictation } = setup(ready({ polishModel: "gemma3:4b" }));
    const element = await pseudoCard();
    const reasons = [
      "Polish skipped: the model's answer did not look like your text",
      "Could not copy the text to the clipboard: no clipboard",
      "No speech detected",
      "no clipboard on this desktop",
    ];
    const check = async (text: string) => {
      const last = await within(element).findByTestId("last-dictation");
      await within(last).findByText(text);
      expect(untranslated(element, ["Hello world.", ...reasons])).toEqual([]);
    };

    await dictation.complete({ text: "Hello world.", polished: true });
    await check("Hello world.");
    await dictation.complete({
      text: "Hello world.",
      polished: false,
      error: reasons[0],
    });
    await check(reasons[0]);
    await dictation.complete({
      text: "Hello world.",
      polished: true,
      copied: false,
      error: reasons[1],
      copyFailed: true,
    });
    await check(reasons[1]);
    await dictation.complete({ text: "", error: reasons[2] });
    await check(reasons[2]);
    // Échec sans raison : le texte de ChatonLM.
    await dictation.complete({ text: "", error: null });
    const last = await within(element).findByTestId("last-dictation");
    await waitFor(() =>
      expect(within(last).queryByText(reasons[2])).toBeNull(),
    );
    expect(untranslated(element, ["Hello world.", ...reasons])).toEqual([]);

    dictation.failures.set("dictation_copy_last", {
      kind: "system",
      message: reasons[3],
    });
    await dictation.complete({ text: "Hello world.", copied: false });
    await userEvent.click(
      await within(last).findByRole("button", { name: /Copy/ }),
    );
    await within(last).findByRole("alert");
    expect(untranslated(element, ["Hello world.", ...reasons])).toEqual([]);
  });
});

describe("Dictation in French and Spanish", () => {
  /** Carte affichée dans la langue des réglages, modèles lus. */
  async function cardIn(enable: string, speechModel: string, loading: string) {
    const element = await screen.findByTestId("dictation");
    await within(element).findByRole("switch", { name: enable });
    await waitFor(() =>
      expect(
        optionTexts(within(element).getByLabelText(speechModel)),
      ).not.toContain(loading),
    );
    return element;
  }

  /** Réglages de l'app dans `lang` (langue lue au démarrage, comme l'app). */
  const inLanguage =
    (lang: "fr" | "es", settings: Partial<DictationSettings> = {}) =>
    (setup: { backend: FakeBackend; dictation: FakeDictation }) => {
      ready(settings)(setup);
      setup.backend.settings.language = lang;
      setup.backend.settings.effectiveLanguage = lang;
    };

  it("French: texts, spoken languages named and sorted in French, relative dates", async () => {
    const { simulator, dictation } = setup(
      inLanguage("fr", { polishModel: "gemma3:4b" }),
    );
    const element = await cardIn(
      "Activer la dictée",
      "Modèle de transcription",
      "Chargement…",
    );
    expect(within(element).getByRole("heading", { name: "Dictée" }));
    expect(
      within(element).getByRole("radio", { name: "Appuyer pour parler" }),
    ).toBeTruthy();
    const language =
      within(element).getByLabelText<HTMLSelectElement>("Langue");
    expect(optionTexts(language)).toEqual([
      "Détection automatique",
      "Allemand",
      "Anglais",
      "Arabe",
      "Chinois",
      "Espagnol",
      "Français",
      "Japonais",
      "Néerlandais",
    ]);
    // Parakeet : le japonais reste écarté, sous son nom français.
    const japanese = [...language.options].find(
      (option) => option.text === "Japonais",
    )!;
    expect(japanese.disabled).toBe(true);
    await userEvent.selectOptions(language, "Allemand");
    expect(patches(simulator)).toEqual([{ language: "de" }]);

    await dictation.complete({
      text: "Bonjour.",
      polished: false,
      at: Date.now() - 3 * 60_000,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("il y a 3 minutes");
    expect(within(last).getByText("Dernière dictée")).toBeTruthy();
    expect(within(last).getByText("sans mise au propre")).toBeTruthy();
    expect(within(last).getByText("Copié dans le presse-papiers")).toBeTruthy();
    await dictation.complete({
      text: "Bonjour.",
      polished: true,
      at: Date.now() - 60_000,
    });
    await within(last).findByText("il y a 1 minute");
  });

  it("Spanish: texts, spoken languages named and sorted in Spanish, phase, relative date", async () => {
    const { dictation } = setup(inLanguage("es"));
    const element = await cardIn(
      "Activar el dictado",
      "Modelo de transcripción",
      "Cargando…",
    );
    expect(within(element).getByRole("heading", { name: "Dictado" }));
    expect(
      optionTexts(within(element).getByLabelText("Idioma")).slice(0, 4),
    ).toEqual(["Detección automática", "Alemán", "Árabe", "Chino"]);
    expect(
      within(element).getByPlaceholderText("Añadir una palabra"),
    ).toBeTruthy();
    await dictation.setStatus({ phase: "recording" });
    await within(element).findByText("Escuchando…");
    expect(
      within(element).getByRole("button", { name: "Detener" }),
    ).toBeTruthy();
    await dictation.complete({
      text: "Hola.",
      at: Date.now() - 2 * 3600_000,
    });
    const last = await within(element).findByTestId("last-dictation");
    await within(last).findByText("hace 2 horas");
    expect(within(last).getByText("Copiado al portapapeles")).toBeTruthy();
  });

  it("changing the language keeps what is being typed", async () => {
    setup(ready());
    const element = await card();
    const input =
      within(element).getByLabelText<HTMLInputElement>("Dictionary");
    await userEvent.type(input, "Ollam");
    act(() => setLanguage("es"));
    const same =
      within(element).getByLabelText<HTMLInputElement>("Diccionario");
    expect(same).toBe(input);
    expect(same.value).toBe("Ollam");
    expect(same.placeholder).toBe("Añadir una palabra");
  });
});
