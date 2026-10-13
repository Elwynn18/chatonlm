// Onglet « Keyboard shortcuts » des réglages face au simulateur : raccourcis
// de l'app (liste exacte, depuis le catalogue), raccourcis du système dans
// chacun de leurs états (coupé, attente du bureau, lié et « Change… »,
// refus, échec, sans portail : commande à lier), interrupteur de la
// correction, évènement « shortcuts-status », actions partagées et échec
// effacé quand le statut change ; textes traduits (pseudo-langue, français,
// espagnol). Sous Windows (`chooser` « app ») : saisie de la combinaison dans
// l'app (acceptée, refusée, tenue par une autre application, annulée,
// rétablie), raccourcis de l'app au repos pendant la saisie.

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
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { setLanguage } from "@/i18n";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { DICTATE_SHORTCUT_COMMAND, PROOFREAD_COMMAND } from "@/test/proofread";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(prepare: (backend: FakeBackend) => void = () => {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  const app = renderApp("/settings?section=shortcuts");
  return { simulator, backend, fake: backend.proofread, app };
}

/** Lignes d'un groupe : action et touches (« Ctrl+N », « Ctrl+Y or … »). */
function group(name: string) {
  return [
    ...screen.getByTestId(`shortcut-group-${name}`).querySelectorAll("dt"),
  ].map((term) => [term.textContent, term.nextElementSibling!.textContent]);
}

const proofreadSwitch = () =>
  screen.getByRole<HTMLButtonElement>("switch", {
    name: "Correct selected text",
  });

describe("Keyboard shortcuts tab", () => {
  it("is the last tab and lists every app shortcut, by group", async () => {
    setup();
    await screen.findByTestId("shortcuts");
    const tab = screen.getByTestId("settings-tab-shortcuts");
    expect(tab.textContent).toBe("Keyboard shortcuts");
    expect(tab.getAttribute("aria-current")).toBe("page");
    expect(
      screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent),
    ).toEqual(["General", "Chat", "Voice mode", "Proofread", "ModelHub"]);
    expect(group("general")).toEqual([
      ["New chat", "Ctrl+N"],
      ["Settings", "Ctrl+,"],
      ["Quit", "Ctrl+Q"],
      ["Zoom in", "Ctrl++"],
      ["Zoom out", "Ctrl+-"],
      ["Actual size", "Ctrl+0"],
      ["Undo in a text field", "Ctrl+Z"],
      ["Redo in a text field", "Ctrl+Shift+Z or Ctrl+Y"],
    ]);
    expect(group("chat")).toEqual([
      ["Send", "Enter or Ctrl+Enter"],
      ["New line", "Shift+Enter"],
      ["Stop the response", "Esc"],
      ["Cancel editing a message", "Esc"],
      ["Paste text, images or files", "Ctrl+V"],
    ]);
    expect(group("voiceMode")).toEqual([
      ["Start or end voice mode", "Ctrl+Shift+Space"],
      [
        "I'm done, or interrupt the reply (push to talk: hold to speak)",
        "Space",
      ],
      ["End voice mode", "Esc"],
    ]);
    expect(group("proofread")).toEqual([
      ["Correct", "Ctrl+Enter"],
      ["Rephrase", "Ctrl+Shift+Enter"],
      ["Stop", "Esc"],
      ["Undo the correction", "Ctrl+Z"],
      ["Redo the correction", "Ctrl+Shift+Z or Ctrl+Y"],
      ["Paste at the end of the corrected text", "Ctrl+V"],
    ]);
    expect(group("hub")).toEqual([["Search models", "/"]]);
    // Chaque touche dans sa pastille.
    const redo = screen.getByText("Redo the correction").nextElementSibling!;
    expect(
      [...redo.querySelectorAll("kbd")].map((key) => key.textContent),
    ).toEqual(["Ctrl", "Shift", "Z", "Ctrl", "Y"]);
  });

  it("dictation off: says so, with a link to the Dictation tab", async () => {
    const user = userEvent.setup();
    const { app } = setup();
    const line = await screen.findByTestId("shortcut-dictation");
    expect(line.textContent).toBe(
      "Turn on dictation to use this shortcut. Dictation settings",
    );
    await user.click(
      within(line).getByRole("link", { name: "Dictation settings" }),
    );
    await screen.findByTestId("dictation");
    expect(app.router.state.location.search).toEqual({ section: "dictation" });
  });

  it("dictation bound: its keys and Change…, which opens the desktop dialog", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((backend) => {
      backend.proofread.shortcuts.dictation = {
        state: "bound",
        trigger: "Ctrl+Alt+D",
      };
    });
    const line = await screen.findByTestId("shortcut-dictation");
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Alt+D"),
    );
    await user.click(within(line).getByRole("button", { name: "Change…" }));
    expect(simulator.callsTo("shortcuts_configure")).toHaveLength(1);
  });

  it("turning on the correction shortcut binds it: waiting, then its keys", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup();
    const line = await screen.findByTestId("shortcut-proofread");
    expect(proofreadSwitch().getAttribute("aria-checked")).toBe("false");
    expect(line.textContent).toBe("Turn on the shortcut to choose its keys.");
    await user.click(proofreadSwitch());
    expect(
      simulator
        .callsTo("update_settings")
        .map((args) => args.patch as SettingsPatch),
    ).toEqual([{ proofreadShortcut: true }]);
    await waitFor(() =>
      expect(line.textContent).toBe("Waiting for your desktop…"),
    );
    expect(proofreadSwitch().getAttribute("aria-checked")).toBe("true");
    expect(await screen.findByText("Saved")).toBeTruthy();

    // Le bureau a lié les touches : l'évènement met la ligne à jour.
    await fake.bound();
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Alt+C"),
    );
    await user.click(within(line).getByRole("button", { name: "Change…" }));
    expect(simulator.callsTo("shortcuts_configure")).toHaveLength(1);

    await user.click(proofreadSwitch());
    await waitFor(() =>
      expect(line.textContent).toBe("Turn on the shortcut to choose its keys."),
    );
  });

  it("refused by the desktop: Not set, and Set shortcut tries again", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((backend) => {
      backend.settings.proofreadShortcut = true;
      backend.proofread.shortcuts.proofread = {
        state: "unavailable",
        problem: "refused",
        reason: "The desktop refused the shortcut",
      };
    });
    const line = await screen.findByTestId("shortcut-proofread");
    await waitFor(() => expect(line.textContent).toContain("Not set"));
    await user.click(
      within(line).getByRole("button", { name: "Set shortcut" }),
    );
    expect(simulator.callsTo("shortcuts_bind")).toHaveLength(1);
    await waitFor(() =>
      expect(line.textContent).toContain("Waiting for your desktop…"),
    );
    // Commande de repli montrée aussi.
    expect(screen.queryByText(PROOFREAD_COMMAND)).toBeNull();
  });

  it("failure: the reason of the engine and Try again; its error until the status changes", async () => {
    const user = userEvent.setup();
    const { fake } = setup((backend) => {
      backend.settings.proofreadShortcut = true;
      backend.proofread.shortcuts.proofread = {
        state: "unavailable",
        problem: "failed",
        reason: "The shortcut service failed",
      };
      backend.proofread.failures.set("shortcuts_bind", {
        kind: "internal",
        message: "portal timed out",
      });
    });
    const line = await screen.findByTestId("shortcut-proofread");
    await waitFor(() =>
      expect(within(line).getByText("The shortcut service failed")),
    );
    expect(within(line).getByText(PROOFREAD_COMMAND)).toBeTruthy();
    await user.click(within(line).getByRole("button", { name: "Try again" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "portal timed out",
    );
    // Relu après l'échec, statut inchangé : l'erreur reste.
    await waitFor(() =>
      expect(
        within(line).getByRole("button", { name: "Try again" }),
      ).toHaveProperty("disabled", false),
    );
    expect(screen.getByRole("alert").textContent).toBe("portal timed out");
    fake.failures.clear();
    // Le bureau lie ensuite le raccourci : l'erreur ne le concerne plus.
    await fake.bound();
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Alt+C"),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("the actions are shared by both lines: one error, shown once", async () => {
    const user = userEvent.setup();
    const { fake } = setup((backend) => {
      backend.settings.proofreadShortcut = true;
      backend.proofread.shortcuts.dictation = {
        state: "unavailable",
        problem: "failed",
        reason: "The shortcut service failed",
      };
      backend.proofread.shortcuts.proofread = {
        state: "unavailable",
        problem: "failed",
        reason: "The shortcut service failed",
      };
    });
    const dictation = await screen.findByTestId("shortcut-dictation");
    const proofread = screen.getByTestId("shortcut-proofread");
    const tryAgain = (line: HTMLElement) =>
      within(line).getByRole<HTMLButtonElement>("button", {
        name: "Try again",
      });
    await waitFor(() => expect(tryAgain(dictation)).toBeTruthy());
    fake.failures.set("shortcuts_bind", {
      kind: "internal",
      message: "portal timed out",
    });
    await user.click(tryAgain(proofread));
    // Une seule erreur, montrée une fois pour les deux lignes.
    await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(1));
    expect(within(dictation).queryByRole("alert")).toBeNull();
    expect(within(proofread).queryByRole("alert")).toBeNull();
    fake.failures.delete("shortcuts_bind");
  });

  it("without the portal: the command to bind, no Try again", async () => {
    setup((backend) => {
      backend.settings.proofreadShortcut = true;
      backend.proofread.shortcuts.proofread = {
        state: "unavailable",
        problem: "noPortal",
        reason: "Your desktop has no global shortcut service",
      };
    });
    const line = await screen.findByTestId("shortcut-proofread");
    await waitFor(() =>
      expect(
        within(line).getByText("Your desktop has no global shortcut service"),
      ),
    );
    expect(
      within(line).getByText(
        "Bind this command to a shortcut in your system settings:",
      ),
    );
    expect(within(line).getByText(PROOFREAD_COMMAND)).toBeTruthy();
    expect(
      within(line).queryByRole("button", { name: "Try again" }),
    ).toBeNull();
  });

  it("a failed save of the switch is shown under the line", async () => {
    const user = userEvent.setup();
    setup((backend) => {
      backend.settingsErrors.update = {
        kind: "internal",
        message: "disk full",
      };
    });
    await screen.findByTestId("shortcut-proofread");
    await user.click(proofreadSwitch());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: disk full",
    );
    expect(proofreadSwitch().getAttribute("aria-checked")).toBe("false");
  });
});

describe("Replace the selection", () => {
  const pasteSwitch = () =>
    screen.getByRole<HTMLButtonElement>("switch", {
      name: "Replace the selection",
    });

  it("is on by default, says what it does, and saves the setting", async () => {
    const user = userEvent.setup();
    const { simulator } = setup();
    await screen.findByTestId("shortcut-proofread");
    expect(pasteSwitch().getAttribute("aria-checked")).toBe("true");
    const described = document.getElementById(
      pasteSwitch().getAttribute("aria-describedby")!,
    )!;
    // Linux : le bureau demandera une fois l'autorisation.
    expect(described.textContent).toBe(
      "ChatonLM pastes the corrected text over the selection. Turned off, the corrected text is only copied: press Ctrl+V to paste it. The first time, your desktop asks to let ChatonLM send the paste keystroke.",
    );
    // Le raccourci lui-même ne promet plus un Ctrl+V à faire.
    expect(
      document.getElementById(
        proofreadSwitch().getAttribute("aria-describedby")!,
      )!.textContent,
    ).toBe("Corrects the text selected in any app.");

    await user.click(pasteSwitch());
    await waitFor(() =>
      expect(pasteSwitch().getAttribute("aria-checked")).toBe("false"),
    );
    await user.click(pasteSwitch());
    await waitFor(() =>
      expect(pasteSwitch().getAttribute("aria-checked")).toBe("true"),
    );
    expect(
      simulator
        .callsTo("update_settings")
        .map((call) => call.patch as SettingsPatch),
    ).toEqual([{ proofreadPaste: false }, { proofreadPaste: true }]);
    // Sans effet sur le raccourci lui-même (aucune liaison demandée).
    expect(proofreadSwitch().getAttribute("aria-checked")).toBe("false");
  });

  it("a failed save is shown under its own line, the switch unchanged", async () => {
    const user = userEvent.setup();
    setup((backend) => {
      backend.settingsErrors.update = {
        kind: "internal",
        message: "disk full",
      };
    });
    await screen.findByTestId("shortcut-proofread");
    await user.click(pasteSwitch());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("Couldn't save this setting: disk full");
    expect(pasteSwitch().getAttribute("aria-checked")).toBe("true");
    // Sous sa ligne, pas sous celle du raccourci.
    expect(
      alert.compareDocumentPosition(screen.getByTestId("shortcut-proofread")) &
        Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy();
  });
});

describe("Keyboard shortcuts in other languages", () => {
  it("has no hard-coded text, in each state", async () => {
    const { fake } = setup((backend) => {
      backend.settings.proofreadShortcut = true;
      backend.proofread.shortcuts = {
        ...backend.proofread.shortcuts,
        dictation: { state: "disabled" },
        proofread: { state: "binding" },
      };
    });
    await screen.findByTestId("shortcut-proofread");
    usePseudoLanguage();
    const allow = [
      "Ctrl+Alt+D",
      "Ctrl+Alt+C",
      "The shortcut service failed",
      "No portal",
      PROOFREAD_COMMAND,
      DICTATE_SHORTCUT_COMMAND,
    ];
    const check = () =>
      expect(untranslatedTexts(screen.getByTestId("shortcuts"), allow)).toEqual(
        [],
      );
    await waitFor(() =>
      expect(screen.getByTestId("shortcut-proofread").textContent).toContain(
        "⟦Waiting",
      ),
    );
    check();
    const states = [
      {
        dictation: { state: "bound", trigger: "Ctrl+Alt+D" },
        proofread: {
          state: "unavailable",
          problem: "refused",
          reason: "The desktop refused",
        },
      },
      {
        dictation: { state: "bound", trigger: "" },
        proofread: {
          state: "unavailable",
          problem: "failed",
          reason: "The shortcut service failed",
        },
      },
      {
        dictation: {
          state: "unavailable",
          problem: "noPortal",
          reason: "No portal",
        },
        proofread: { state: "bound", trigger: "Ctrl+Alt+C" },
      },
    ] as const;
    for (const state of states) {
      await fake.setShortcuts(state);
      await waitFor(() =>
        expect(screen.queryByText("⟦Waiting", { exact: false })).toBeNull(),
      );
      check();
    }
  });

  it("names the keys as the keyboard does, in French and Spanish", async () => {
    setup();
    await screen.findByTestId("shortcuts");
    act(() => setLanguage("fr"));
    expect(screen.getByTestId("settings-tab-shortcuts").textContent).toBe(
      "Raccourcis clavier",
    );
    expect(group("proofread")).toEqual([
      ["Corriger", "Ctrl+Entrée"],
      ["Reformuler", "Ctrl+Maj+Entrée"],
      ["Arrêter", "Échap"],
      ["Annuler la correction", "Ctrl+Z"],
      ["Rétablir la correction", "Ctrl+Maj+Z ou Ctrl+Y"],
      ["Coller à la fin du texte corrigé", "Ctrl+V"],
    ]);
    act(() => setLanguage("es"));
    expect(group("chat")[0]).toEqual(["Enviar", "Enter o Ctrl+Enter"]);
    expect(group("chat")[1]).toEqual(["Nueva línea", "Shift+Enter"]);
    expect(screen.getByTestId("settings-tab-shortcuts").textContent).toBe(
      "Atajos de teclado",
    );
  });
});

/** Onglet sous Windows : ChatonLM fait choisir les touches lui-même. */
function setupWindows(prepare: (backend: FakeBackend) => void = () => {}) {
  return setup((backend) => {
    backend.proofread.shortcuts = {
      ...backend.proofread.shortcuts,
      chooser: "app",
      dictation: { state: "bound", trigger: "Ctrl+Alt+D" },
      dictateCommand: '"C:\\Program Files\\ChatonLM\\chatonlm.exe" --dictate',
    };
    prepare(backend);
  });
}

/** Ouvre la saisie du raccourci de la dictée ; rend sa zone. */
async function openRecorder(user: ReturnType<typeof userEvent.setup>) {
  const line = await screen.findByTestId("shortcut-dictation");
  await user.click(
    await within(line).findByRole("button", { name: "Change…" }),
  );
  const zone = await within(line).findByTestId("shortcut-recorder-zone");
  return { line, zone };
}

describe("Global shortcuts chosen in the app (Windows)", () => {
  it("says the keys are chosen here, and never shows a command to bind", async () => {
    setupWindows((backend) => {
      backend.settings.proofreadShortcut = true;
      backend.proofread.takenTriggers.add("Ctrl+Alt+C");
      backend.proofread.shortcuts.proofread = {
        state: "unavailable",
        problem: "taken",
        reason: "Ctrl+Alt+C is already used by another app",
      };
    });
    const line = await screen.findByTestId("shortcut-proofread");
    await waitFor(() =>
      expect(line.textContent).toContain(
        "Ctrl+Alt+C is already used by another app",
      ),
    );
    expect(
      screen.getByText("They work in any app. Choose their keys here."),
    ).toBeTruthy();
    expect(
      within(line)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Change…", "Try again"]);
    expect(screen.getByTestId("shortcuts").textContent).not.toContain(
      "Bind this command",
    );
    expect(screen.getByTestId("shortcuts").textContent).not.toContain(
      "--dictate",
    );
  });

  it("Change… records the keys pressed: the engine binds them, focus returns", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setupWindows();
    const { line, zone } = await openRecorder(user);
    // Raccourcis globaux suspendus, la zone a le focus et dit quoi faire.
    expect(fake.recordings).toEqual([true]);
    expect(document.activeElement).toBe(zone);
    expect(zone.textContent).toBe("Press the new shortcut…");
    expect(
      within(line).getByRole("group", { name: "New shortcut: Dictation" }),
    ).toBeTruthy();
    expect(simulator.callsTo("shortcuts_configure")).toHaveLength(0);

    // Modificateurs seuls, répétition : la saisie attend la touche.
    fireEvent.keyDown(zone, { key: "Control", keyCode: 17, ctrlKey: true });
    fireEvent.keyDown(zone, {
      key: "Alt",
      keyCode: 18,
      ctrlKey: true,
      altKey: true,
    });
    expect(simulator.callsTo("shortcuts_set_trigger")).toHaveLength(0);

    fireEvent.keyDown(zone, {
      key: "k",
      code: "KeyK",
      keyCode: 75,
      ctrlKey: true,
      altKey: true,
    });
    expect(simulator.callsTo("shortcuts_set_trigger")).toEqual([
      {
        id: "dictation",
        keys: { ctrl: true, alt: true, shift: false, meta: false, keyCode: 75 },
      },
    ]);
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Alt+K"),
    );
    expect(within(line).queryByTestId("shortcut-recorder-zone")).toBeNull();
    await waitFor(() => expect(fake.recordings).toEqual([true, false]));
    expect(document.activeElement).toBe(
      within(line).getByRole("button", { name: "Change…" }),
    );
  });

  it("a refused combination: the reason of the engine, the zone stays open", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setupWindows((backend) => {
      backend.proofread.failures.set("shortcuts_set_trigger", {
        kind: "invalid",
        message: "Add Ctrl, Alt or the Windows key",
      });
    });
    const { line, zone } = await openRecorder(user);
    fireEvent.keyDown(zone, { key: "k", keyCode: 75 });
    expect((await within(line).findByRole("alert")).textContent).toBe(
      "Add Ctrl, Alt or the Windows key",
    );
    expect(document.activeElement).toBe(zone);
    expect(fake.recordings).toEqual([true]);

    // Nouvel essai, accepté : l'erreur s'efface avec la saisie.
    fake.failures.delete("shortcuts_set_trigger");
    fireEvent.keyDown(zone, { key: "F9", keyCode: 120 });
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("F9"),
    );
    expect(within(line).queryByRole("alert")).toBeNull();
    expect(simulator.callsTo("shortcuts_set_trigger")).toHaveLength(2);
  });

  it("a combination held by another app is kept and said, with Try again", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setupWindows((backend) => {
      backend.proofread.takenTriggers.add("Ctrl+Alt+K");
    });
    const { line, zone } = await openRecorder(user);
    fireEvent.keyDown(zone, {
      key: "k",
      keyCode: 75,
      ctrlKey: true,
      altKey: true,
    });
    await waitFor(() =>
      expect(line.textContent).toBe(
        "Ctrl+Alt+K is already used by another appChange…Try again",
      ),
    );
    await waitFor(() => expect(fake.recordings).toEqual([true, false]));
    await user.click(within(line).getByRole("button", { name: "Try again" }));
    expect(simulator.callsTo("shortcuts_bind")).toHaveLength(1);
  });

  it("Esc, Cancel and losing focus close without changing anything", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setupWindows();
    const { line, zone: first } = await openRecorder(user);
    fireEvent.keyDown(first, { key: "Escape", keyCode: 27 });
    await waitFor(() =>
      expect(within(line).queryByTestId("shortcut-recorder-zone")).toBeNull(),
    );
    expect(fake.recordings).toEqual([true, false]);

    await openRecorder(user);
    await user.click(within(line).getByRole("button", { name: "Cancel" }));
    expect(within(line).queryByTestId("shortcut-recorder-zone")).toBeNull();
    expect(document.activeElement).toBe(
      within(line).getByRole("button", { name: "Change…" }),
    );

    // Tab reste au navigateur ; le focus sorti de la saisie la ferme.
    const { zone } = await openRecorder(user);
    expect(fireEvent.keyDown(zone, { key: "Tab", keyCode: 9 })).toBe(true);
    expect(within(line).queryByTestId("shortcut-recorder-zone")).not.toBeNull();
    await user.click(proofreadSwitch());
    expect(within(line).queryByTestId("shortcut-recorder-zone")).toBeNull();

    expect(simulator.callsTo("shortcuts_set_trigger")).toHaveLength(0);
    expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Alt+D");
    expect(fake.recordings.at(-1)).toBe(false);
  });

  it("Reset to default gives the default keys back", async () => {
    const user = userEvent.setup();
    const { simulator } = setupWindows((backend) => {
      backend.proofread.shortcuts.dictation = {
        state: "bound",
        trigger: "Ctrl+Alt+K",
      };
    });
    const { line } = await openRecorder(user);
    await user.click(
      within(line).getByRole("button", { name: "Reset to default" }),
    );
    expect(simulator.callsTo("shortcuts_reset_trigger")).toEqual([
      { id: "dictation" },
    ]);
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Alt+D"),
    );
    expect(within(line).queryByTestId("shortcut-recorder-zone")).toBeNull();
  });

  it("keeps the app shortcuts quiet while recording (Ctrl+Q is a combination, not Quit)", async () => {
    const user = userEvent.setup();
    const { simulator, app } = setupWindows();
    const { line, zone } = await openRecorder(user);
    fireEvent.keyDown(zone, { key: "q", keyCode: 81, ctrlKey: true });
    await waitFor(() =>
      expect(line.querySelector("kbd")?.textContent).toBe("Ctrl+Q"),
    );
    expect(simulator.callsTo("quit_app")).toHaveLength(0);

    // Saisie fermée : Ctrl+N agit de nouveau.
    fireEvent.keyDown(document.body, { key: "n", keyCode: 78, ctrlKey: true });
    await waitFor(() =>
      expect(app.router.state.location.pathname).toBe("/c/new"),
    );
  });

  it("has no hard-coded text while recording", async () => {
    const user = userEvent.setup();
    setupWindows((backend) => {
      backend.proofread.failures.set("shortcuts_set_trigger", {
        kind: "invalid",
        message: "Add Ctrl, Alt or the Windows key",
      });
    });
    const { line, zone } = await openRecorder(user);
    fireEvent.keyDown(zone, { key: "k", keyCode: 75 });
    await within(line).findByRole("alert");
    usePseudoLanguage();
    expect(
      untranslatedTexts(screen.getByTestId("shortcuts"), [
        "Ctrl+Alt+D",
        "Add Ctrl, Alt or the Windows key",
      ]),
    ).toEqual([]);
    expect(zone.textContent).toBe("⟦Press the new shortcut…⟧");
    act(() => setLanguage("fr"));
    expect(
      screen.getByText(
        "⟦Ils fonctionnent dans toute application. Choisissez leurs touches ici.⟧",
      ),
    ).toBeTruthy();
  });
});
